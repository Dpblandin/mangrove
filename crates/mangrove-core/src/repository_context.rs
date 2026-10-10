use std::{fs::File, io::Read, path::Path};

use git2::{Commit, ErrorCode, Oid, Repository, Tree};

use crate::{
    changed_file, model::*, patch::render_bounded, repository_head_name, tree_diff, Result,
    MAX_CHANGED_FILES,
};

const MAX_WORKTREES: usize = 256;
const MAX_STASHES: usize = 128;
const MAX_REGISTRATION_BYTES: u64 = 16_384;

fn registration_text(path: &Path) -> Result<Option<String>> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.to_string()),
    };
    let mut bytes = Vec::new();
    file.take(MAX_REGISTRATION_BYTES)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    Ok(Some(
        String::from_utf8_lossy(&bytes)
            .trim_end_matches(['\r', '\n'])
            .into(),
    ))
}

fn registration_head(
    repository: &Repository,
    registration: &Path,
) -> (Option<String>, String, bool) {
    if let Ok(Some(head)) = registration_text(&registration.join("HEAD")) {
        if let Some(name) = head.strip_prefix("ref: ") {
            let sha = repository
                .find_reference(name)
                .and_then(|reference| reference.peel_to_commit())
                .ok()
                .map(|commit| commit.id().to_string());
            return (
                sha,
                name.strip_prefix("refs/heads/").unwrap_or(name).into(),
                false,
            );
        }
        if head.len() == 40 {
            if let Ok(id) = Oid::from_str(&head) {
                return (Some(id.to_string()), "HEAD détachée".into(), true);
            }
        }
    }
    (None, "HEAD indisponible".into(), false)
}

fn worktree_info(
    repository: Option<&Repository>,
    path: &Path,
    name: String,
    primary: bool,
    current: &Path,
    locked: Option<String>,
    reason: Option<String>,
) -> WorktreeInfo {
    let head = repository.and_then(|repo| repo.head().ok());
    let detached = repository.is_some_and(|repo| repo.head_detached().unwrap_or(false));
    WorktreeInfo {
        name,
        path: path
            .canonicalize()
            .unwrap_or_else(|_| path.to_path_buf())
            .to_string_lossy()
            .into_owned(),
        primary,
        current: path.canonicalize().ok().as_deref() == Some(current),
        available: repository.is_some_and(|repo| !repo.is_bare()) && reason.is_none(),
        head: head
            .as_ref()
            .and_then(|head| head.peel_to_commit().ok())
            .map(|commit| commit.id().to_string()),
        head_name: repository
            .map(repository_head_name)
            .unwrap_or_else(|| "HEAD indisponible".into()),
        detached,
        locked,
        reason,
    }
}

pub(crate) fn worktrees(repository: &Repository) -> Result<WorktreeList> {
    // Reopening a separate Git directory loses its selected workdir. Only linked
    // worktrees need a distinct handle for their primary repository.
    let primary_repository = repository
        .is_worktree()
        .then(|| Repository::open(repository.commondir()))
        .transpose()
        .map_err(|e| e.to_string())?;
    let main = primary_repository.as_ref().unwrap_or(repository);
    let current = repository
        .workdir()
        .unwrap_or(repository.path())
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let main_path = main.workdir().unwrap_or(main.path());
    let mut items = vec![worktree_info(
        Some(main),
        main_path,
        "Principal".into(),
        true,
        &current,
        None,
        main.is_bare()
            .then(|| "Dépôt bare : aucun répertoire de travail.".into()),
    )];
    let names = main.worktrees().map_err(|e| e.to_string())?;
    for name in names.iter().take(MAX_WORKTREES - 1) {
        let name = name
            .map_err(|e| e.to_string())?
            .ok_or("Nom de worktree indisponible.")?;
        let registration = main.commondir().join("worktrees").join(name);
        // git2's is_locked panics on non-UTF-8 reasons. Read display metadata
        // directly and preserve the lock without repairing its registration.
        let locked = registration_text(&registration.join("locked"))?.map(|reason| {
            if reason.is_empty() {
                "Verrouillé".into()
            } else {
                reason
            }
        });
        let mut info = match main.find_worktree(name) {
            Ok(worktree) => {
                let opened = Repository::open(worktree.path());
                let reason = (worktree.validate().is_err() || opened.is_err())
                    .then(|| "Les métadonnées du worktree sont invalides ou inaccessibles.".into());
                worktree_info(
                    opened.as_ref().ok(),
                    worktree.path(),
                    name.into(),
                    false,
                    &current,
                    locked,
                    reason,
                )
            }
            Err(_) => {
                // Missing directories can make libgit2 refuse the Worktree handle.
                // Read its registration for display only; never repair or prune it.
                let path = registration_text(&registration.join("gitdir"))
                    .ok()
                    .flatten()
                    .and_then(|value| Path::new(&value).parent().map(Path::to_path_buf))
                    .unwrap_or_else(|| registration.clone());
                let reason = if path.is_dir() {
                    "Les métadonnées du worktree sont invalides ou inaccessibles."
                } else {
                    "Le chemin du worktree est absent ou inaccessible."
                };
                worktree_info(
                    None,
                    &path,
                    name.into(),
                    false,
                    &current,
                    locked,
                    Some(reason.into()),
                )
            }
        };
        if !info.available && info.head.is_none() {
            (info.head, info.head_name, info.detached) = registration_head(main, &registration);
        }
        items.push(info);
    }
    Ok(WorktreeList {
        worktrees: items,
        truncated: names.len() >= MAX_WORKTREES,
    })
}

fn stash_log(repository: &Repository) -> Result<Option<git2::Reflog>> {
    match repository.reflog("refs/stash") {
        Ok(log) => Ok(Some(log)),
        Err(error) if error.code() == ErrorCode::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

fn stash_commit<'a>(repository: &'a Repository, sha: &str) -> Result<Commit<'a>> {
    let id = Oid::from_str(sha).map_err(|_| "Identifiant de stash invalide.")?;
    let log = stash_log(repository)?.ok_or("Ce stash n'existe plus. Actualisez le dépôt.")?;
    if !(0..log.len().min(MAX_STASHES))
        .any(|index| log.get(index).is_some_and(|entry| entry.id_new() == id))
    {
        return Err(
            "Ce stash n'existe plus dans la liste consultable. Actualisez le dépôt.".into(),
        );
    }
    let commit = repository.find_commit(id).map_err(|e| e.to_string())?;
    if !(2..=3).contains(&commit.parent_count()) {
        return Err("Structure de stash non prise en charge.".into());
    }
    Ok(commit)
}

pub(crate) fn stashes(repository: &Repository) -> Result<StashList> {
    let Some(log) = stash_log(repository)? else {
        return Ok(StashList {
            stashes: vec![],
            truncated: false,
        });
    };
    let mut stashes = Vec::new();
    for index in 0..log.len().min(MAX_STASHES) {
        let entry = log.get(index).ok_or("Entrée de stash indisponible.")?;
        let commit = repository
            .find_commit(entry.id_new())
            .map_err(|e| e.to_string())?;
        stashes.push(StashInfo {
            sha: commit.id().to_string(),
            index,
            message: String::from_utf8_lossy(commit.message_bytes())
                .chars()
                .take(16_384)
                .collect(),
            author: String::from_utf8_lossy(commit.author().name_bytes()).into_owned(),
            time: commit.time().seconds(),
            base_sha: commit.parent_id(0).map_err(|e| e.to_string())?.to_string(),
        });
    }
    Ok(StashList {
        stashes,
        truncated: log.len() > MAX_STASHES,
    })
}

fn trees<'a>(
    stash: &Commit<'a>,
    area: &str,
) -> Result<(Option<Tree<'a>>, Tree<'a>, Option<String>, String)> {
    let (old, new) = match area {
        "index" => (Some(stash.parent(0)), stash.parent(1)),
        "worktree" => (Some(stash.parent(1)), Ok(stash.clone())),
        "untracked" if stash.parent_count() == 3 => (None, stash.parent(2)),
        _ => return Err("Zone de stash invalide ou absente.".into()),
    };
    let old = old.transpose().map_err(|e| e.to_string())?;
    let new = new.map_err(|e| e.to_string())?;
    let old_sha = old.as_ref().map(|commit| commit.id().to_string());
    let new_sha = new.id().to_string();
    Ok((
        old.map(|commit| commit.tree())
            .transpose()
            .map_err(|e| e.to_string())?,
        new.tree().map_err(|e| e.to_string())?,
        old_sha,
        new_sha,
    ))
}

pub(crate) fn stash_details(repository: &Repository, sha: &str) -> Result<StashDetails> {
    let stash = stash_commit(repository, sha)?;
    let mut groups = Vec::new();
    for (area, comparison) in [
        (
            "worktree",
            "Index sauvegardé → répertoire de travail sauvegardé",
        ),
        ("index", "Commit de base → index sauvegardé"),
        ("untracked", "Arbre vide → fichiers non suivis sauvegardés"),
    ] {
        if area == "untracked" && stash.parent_count() != 3 {
            continue;
        }
        let (old, new, old_sha, new_sha) = trees(&stash, area)?;
        let diff = tree_diff(repository, old.as_ref(), &new)?;
        groups.push(StashGroup {
            area: area.into(),
            comparison: comparison.into(),
            old_sha,
            new_sha,
            files: diff
                .deltas()
                .take(MAX_CHANGED_FILES)
                .enumerate()
                .map(|(index, delta)| changed_file(index, &delta))
                .collect(),
            file_count: diff.deltas().len(),
        });
    }
    let author = String::from_utf8_lossy(stash.author().name_bytes()).into_owned();
    Ok(StashDetails {
        sha: stash.id().to_string(),
        message: String::from_utf8_lossy(stash.message_bytes())
            .chars()
            .take(16_384)
            .collect(),
        author,
        time: stash.time().seconds(),
        base_sha: stash.parent_id(0).map_err(|e| e.to_string())?.to_string(),
        groups,
    })
}

pub(crate) fn stash_diff(
    repository: &Repository,
    sha: &str,
    area: &str,
    file_key: &str,
    limit: PreviewLimit,
) -> Result<FileDiff> {
    crate::file_browser::path_bytes(file_key)?;
    let stash = stash_commit(repository, sha)?;
    let (old, new, _, _) = trees(&stash, area)?;
    let diff = tree_diff(repository, old.as_ref(), &new)?;
    let index = diff
        .deltas()
        .take(MAX_CHANGED_FILES)
        .enumerate()
        .find_map(|(index, delta)| {
            (changed_file(index, &delta).file_key == file_key).then_some(index)
        })
        .ok_or("Fichier introuvable dans cette zone du stash.")?;
    render_bounded(repository, &diff, index, limit)
}

pub(crate) fn stash_images(
    repository: &Repository,
    sha: &str,
    area: &str,
    file_key: &str,
) -> Result<FileImages> {
    crate::file_browser::path_bytes(file_key)?;
    let stash = stash_commit(repository, sha)?;
    let (old, new, _, _) = trees(&stash, area)?;
    let diff = tree_diff(repository, old.as_ref(), &new)?;
    let delta = diff
        .deltas()
        .take(MAX_CHANGED_FILES)
        .find(|delta| changed_file(0, delta).file_key == file_key)
        .ok_or("Fichier introuvable dans cette zone du stash.")?;
    crate::images::delta(repository, &delta)
}
