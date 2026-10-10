use std::{
    collections::HashSet,
    path::PathBuf,
    time::{Duration, Instant},
};

use git2::{
    BlameOptions, Commit, Delta, Diff, DiffFindOptions, DiffOptions, ErrorCode, ObjectType, Oid,
    Repository, Tree,
};

use crate::{
    model::*,
    working_tree::{file_key, key_bytes},
    Result, MAX_FILE_SIZE, MAX_PATCH_SIZE,
};

const MAX_LINES: usize = 2_000;
const MAX_TEXT_BYTES: usize = 256 * 1024;
const MAX_HISTORY_SCAN: usize = 256;
const HISTORY_PAGE_SIZE: usize = 40;
const MAX_FILE_KEY_LENGTH: usize = 32 * 1024;
const MAX_DIFF_FILES: usize = 5_000;
const MAX_RENAME_CANDIDATES: usize = 200;
const MAX_BLAME_BYTES: usize = 32 * 1024 * 1024;
const SHALLOW_LIMITATION: &str = "Frontière shallow atteinte ; les révisions plus anciennes peuvent être absentes. Aucun fetch automatique.";

pub(crate) fn path_bytes(key: &str) -> Result<Vec<u8>> {
    if key.len() > MAX_FILE_KEY_LENGTH {
        return Err("Chemin de fichier trop long.".into());
    }
    let path = key_bytes(key)?;
    if path.contains(&0)
        || path
            .split(|byte| *byte == b'/')
            .any(|part| part.is_empty() || part == b"." || part == b"..")
    {
        return Err("Chemin Git invalide.".into());
    }
    Ok(path)
}

pub(crate) fn object_error(error: git2::Error) -> String {
    if error.code() == ErrorCode::NotFound {
        "Objet Git absent : dépôt partiel, shallow ou objet indisponible. Aucun fetch automatique."
            .into()
    } else {
        error.to_string()
    }
}

fn commit<'a>(repository: &'a Repository, sha: &str) -> Result<Commit<'a>> {
    let id = Oid::from_str(sha).map_err(|_| "Identifiant de commit invalide.")?;
    repository.find_commit(id).map_err(object_error)
}

// Walk raw tree names so lossy display labels never identify a blob, even on Windows.
fn entry(repository: &Repository, tree: &Tree<'_>, path: &[u8]) -> Result<Option<(Oid, i32)>> {
    let mut current = tree.clone();
    let mut parts = path.split(|byte| *byte == b'/').peekable();
    while let Some(part) = parts.next() {
        let Some(found) = current.get_name_bytes(part) else {
            return Ok(None);
        };
        let id = found.id();
        let mode = found.filemode();
        let kind = found.kind();
        drop(found);
        if parts.peek().is_none() {
            return Ok(Some((id, mode)));
        }
        if kind != Some(ObjectType::Tree) {
            return Ok(None);
        }
        current = repository.find_tree(id).map_err(object_error)?;
    }
    Ok(None)
}

pub(crate) fn content(
    repository: &Repository,
    sha: &str,
    key: &str,
    limit: PreviewLimit,
) -> Result<FileContent> {
    let path = path_bytes(key)?;
    let revision = commit(repository, sha)?;
    let tree = revision.tree().map_err(object_error)?;
    let mut result = FileContent {
        sha: revision.id().to_string(),
        path: String::from_utf8_lossy(&path).into_owned(),
        file_key: key.into(),
        kind: "deleted".into(),
        size: 0,
        lines: vec![],
        truncated: false,
        ends_with_newline: false,
        parent_sha: revision.parent_ids().next().map(|id| id.to_string()),
        oversized: None,
    };
    let Some((id, mode)) = entry(repository, &tree, &path)? else {
        return Ok(result);
    };
    if mode == 0o160000 {
        result.kind = "submodule".into();
        return Ok(result);
    }
    let (size, kind) = repository
        .odb()
        .map_err(object_error)?
        .read_header(id)
        .map_err(object_error)?;
    result.size = size;
    if kind != ObjectType::Blob {
        result.kind = "unsupported".into();
        return Ok(result);
    }
    if let Some(oversized) = limit.exceeded_by(size as u64) {
        result.kind = "large".into();
        result.truncated = true;
        result.oversized = Some(oversized);
        return Ok(result);
    }
    let blob = repository.find_blob(id).map_err(object_error)?;
    let bytes = blob.content();
    let Ok(text) = std::str::from_utf8(bytes) else {
        result.kind = "binary".into();
        return Ok(result);
    };
    if blob.is_binary() {
        result.kind = "binary".into();
        return Ok(result);
    }
    result.kind = "text".into();
    result.ends_with_newline = bytes.ends_with(b"\n");
    let mut displayed = 0;
    for line in text.split_inclusive('\n') {
        if result.lines.len() == MAX_LINES || displayed + line.len() > MAX_TEXT_BYTES {
            result.truncated = true;
            break;
        }
        displayed += line.len();
        let line = match line.strip_suffix('\n') {
            Some(line) => line.strip_suffix('\r').unwrap_or(line),
            None => line,
        };
        result.lines.push(line.into());
    }
    Ok(result)
}

#[derive(Default)]
struct BlobBudget {
    checked: HashSet<Oid>,
    bytes: usize,
}

fn blob_size(repository: &Repository, id: Oid, mode: i32) -> Result<Option<usize>> {
    if !matches!(mode, 0o100644 | 0o100664 | 0o100755 | 0o120000) {
        return Ok(None);
    }
    let (size, kind) = repository
        .odb()
        .map_err(object_error)?
        .read_header(id)
        .map_err(object_error)?;
    if kind != ObjectType::Blob {
        return Err("Objet de fichier Git non pris en charge.".into());
    }
    Ok(Some(size))
}

fn literal_blame_path(path: &[u8]) -> Result<()> {
    // libgit2's blame uses raw names as pathspecs without a literal-path option.
    // Patterns can attribute another file's change and escape our blob preflight.
    if path
        .iter()
        .any(|byte| matches!(byte, b'*' | b'?' | b'[' | b'\\'))
    {
        return Err("Blame indisponible pour ce chemin : libgit2 interprète les caractères de motif (*, ?, [, \\) au lieu de garantir une attribution littérale.".into());
    }
    Ok(())
}

impl BlobBudget {
    fn check(&mut self, repository: &Repository, id: Oid, mode: i32) -> Result<()> {
        if self.checked.contains(&id) {
            return Ok(());
        }
        let Some(size) = blob_size(repository, id, mode)? else {
            return Ok(());
        };
        if size > MAX_FILE_SIZE as usize || size > MAX_BLAME_BYTES - self.bytes {
            return Err("Limite de lecture atteinte : blob de plus de 2 Mio ou budget de 32 Mio pour les objets historiques.".into());
        }
        self.checked.insert(id);
        self.bytes += size;
        Ok(())
    }
}

fn metadata_diff<'a>(
    repository: &'a Repository,
    parent: Option<&Tree<'_>>,
    tree: &Tree<'_>,
) -> Result<Diff<'a>> {
    let mut options = DiffOptions::new();
    options
        .include_typechange(true)
        .skip_binary_check(true)
        .max_size(MAX_PATCH_SIZE);
    let diff = repository
        .diff_tree_to_tree(parent, Some(tree), Some(&mut options))
        .map_err(object_error)?;
    if diff.deltas().len() > MAX_DIFF_FILES {
        return Err(
            "Limite de lecture atteinte : plus de 5 000 fichiers modifiés dans une révision."
                .into(),
        );
    }
    Ok(diff)
}

// Similarity detection loads whole candidate blobs regardless of DiffOptions::max_size.
// Check their object headers before either our rename search or libgit2's blame search.
fn check_diff_blobs(
    repository: &Repository,
    diff: &Diff<'_>,
    budget: &mut BlobBudget,
) -> Result<()> {
    let mut candidates = 0;
    for delta in diff.deltas() {
        let count = match delta.status() {
            Delta::Added | Delta::Deleted => 1,
            // Blame's internal diff splits type changes into an addition and a deletion.
            Delta::Typechange => 2,
            _ => continue,
        };
        candidates += count;
        if candidates > MAX_RENAME_CANDIDATES {
            return Err("Suivi des renommages borné à 200 fichiers candidats par révision.".into());
        }
        for file in [delta.old_file(), delta.new_file()] {
            budget.check(repository, file.id(), file.mode().into())?;
        }
    }
    Ok(())
}

fn find_renames(
    repository: &Repository,
    diff: &mut Diff<'_>,
    budget: &mut BlobBudget,
) -> Result<()> {
    if !diff
        .deltas()
        .any(|delta| matches!(delta.status(), Delta::Added | Delta::Typechange))
        || !diff
            .deltas()
            .any(|delta| matches!(delta.status(), Delta::Deleted | Delta::Typechange))
    {
        return Ok(());
    }
    check_diff_blobs(repository, diff, budget)?;
    let mut options = DiffFindOptions::new();
    options.renames(true).rename_limit(MAX_RENAME_CANDIDATES);
    diff.find_similar(Some(&mut options)).map_err(object_error)
}

pub(crate) fn revision_diff(
    repository: &Repository,
    sha: &str,
    key: &str,
    limit: PreviewLimit,
) -> Result<FileDiff> {
    let path = path_bytes(key)?;
    let revision = commit(repository, sha)?;
    let tree = revision.tree().map_err(object_error)?;
    let parent = if revision.parent_count() > 0 {
        Some(
            revision
                .parent(0)
                .map_err(object_error)?
                .tree()
                .map_err(object_error)?,
        )
    } else {
        None
    };
    let mut diff = metadata_diff(repository, parent.as_ref(), &tree)?;
    find_renames(repository, &mut diff, &mut BlobBudget::default())?;
    match diff.deltas().position(|delta| {
        delta.new_file().path_bytes() == Some(path.as_slice())
            || delta.old_file().path_bytes() == Some(path.as_slice())
    }) {
        Some(index) => crate::patch::render_bounded(repository, &diff, index, limit),
        None => Ok(FileDiff::default()),
    }
}

pub(crate) fn images(
    repository: &Repository,
    sha: &str,
    key: &str,
    comparison: bool,
) -> Result<FileImages> {
    let path = path_bytes(key)?;
    let revision = commit(repository, sha)?;
    let tree = revision.tree().map_err(object_error)?;
    if !comparison {
        let after = match entry(repository, &tree, &path)? {
            Some((id, mode)) => crate::images::blob(repository, id, mode)?,
            None => FileImageData::Absent,
        };
        return Ok(FileImages {
            before: None,
            after,
        });
    }
    let parent = revision
        .parent_ids()
        .next()
        .map(|id| repository.find_commit(id).and_then(|commit| commit.tree()))
        .transpose()
        .map_err(object_error)?;
    // Match the comparison already shown in commit details. The historical
    // blame budget includes unrelated rename candidates and must not prevent
    // reading the two bounded image blobs selected by this diff.
    let diff = crate::tree_diff(repository, parent.as_ref(), &tree)?;
    if let Some(delta) = diff.deltas().find(|delta| {
        delta.new_file().path_bytes() == Some(path.as_slice())
            || delta.old_file().path_bytes() == Some(path.as_slice())
    }) {
        return crate::images::delta(repository, &delta);
    }
    let before = match parent
        .as_ref()
        .map(|tree| entry(repository, tree, &path))
        .transpose()?
        .flatten()
    {
        Some((id, mode)) => crate::images::blob(repository, id, mode)?,
        None => FileImageData::Absent,
    };
    let after = match entry(repository, &tree, &path)? {
        Some((id, mode)) => crate::images::blob(repository, id, mode)?,
        None => FileImageData::Absent,
    };
    Ok(FileImages {
        before: Some(before),
        after,
    })
}

struct HistoryStep {
    entry: Option<FileHistoryEntry>,
    next: Option<Oid>,
    reached_root: bool,
}

fn history_step(
    repository: &Repository,
    id: Oid,
    path: &mut Vec<u8>,
    budget: &mut BlobBudget,
) -> Result<HistoryStep> {
    let revision = repository.find_commit(id).map_err(object_error)?;
    let tree = revision.tree().map_err(object_error)?;
    let parent = if revision.parent_count() > 0 {
        Some(revision.parent(0).map_err(object_error)?)
    } else {
        None
    };
    let parent_tree = parent
        .as_ref()
        .map(|parent| parent.tree().map_err(object_error))
        .transpose()?;
    // A tree at this address is not the previous or current file identity.
    let current = entry(repository, &tree, path)?.filter(|(_, mode)| *mode != 0o040000);
    let previous = parent_tree
        .as_ref()
        .map(|tree| entry(repository, tree, path))
        .transpose()?
        .flatten()
        .filter(|(_, mode)| *mode != 0o040000);
    let mut step = HistoryStep {
        entry: None,
        next: parent.as_ref().map(|parent| parent.id()),
        reached_root: parent.is_none(),
    };
    if current == previous {
        return Ok(step);
    }
    let mut status = if current.is_none() {
        "deleted"
    } else if previous.is_none() {
        "added"
    } else {
        "modified"
    };
    let mut old_path = None;
    let selected_path = path.clone();
    if current.is_some() && previous.is_none() && parent.is_some() {
        let mut diff = metadata_diff(repository, parent_tree.as_ref(), &tree)?;
        find_renames(repository, &mut diff, budget)?;
        if let Some(delta) = diff.deltas().find(|delta| {
            delta.status() == Delta::Renamed
                && delta.new_file().path_bytes() == Some(path.as_slice())
        }) {
            if let Some(old) = delta.old_file().path_bytes() {
                old_path = Some(String::from_utf8_lossy(old).into_owned());
                *path = old.to_vec();
                status = "renamed";
            }
        }
    }
    let author = revision.author();
    step.entry = Some(FileHistoryEntry {
        sha: id.to_string(),
        path: String::from_utf8_lossy(&selected_path).into_owned(),
        file_key: file_key(&selected_path),
        old_path,
        status: status.into(),
        summary: String::from_utf8_lossy(revision.summary_bytes().unwrap_or_default()).into_owned(),
        author: String::from_utf8_lossy(author.name_bytes()).into_owned(),
        time: author.when().seconds(),
        merge: revision.parent_count() > 1,
    });
    if status == "added" {
        step.next = None;
    }
    Ok(step)
}

pub(crate) fn history(
    repository: &Repository,
    sha: &str,
    key: &str,
    cursor: Option<&str>,
) -> Result<FileHistoryPage> {
    let mut path = path_bytes(key)?;
    let root = commit(repository, sha)?.id();
    let mut next = Some(root);
    if let Some(cursor) = cursor {
        // The cursor retains both the original identity and a potentially renamed identity.
        if cursor.len() > 2 * MAX_FILE_KEY_LENGTH + 2 * 40 + 3 {
            return Err("Curseur d’historique invalide.".into());
        }
        let fields: Vec<_> = cursor.split(':').collect();
        if fields.len() != 4 || fields[0] != sha || fields[1] != key {
            return Err("Ce curseur appartient à une autre sélection.".into());
        }
        next = Some(Oid::from_str(fields[2]).map_err(|_| "Curseur d’historique invalide.")?);
        path = path_bytes(fields[3])?;
    }
    let mut result = FileHistoryPage {
        entries: vec![],
        next_cursor: None,
        scanned: 0,
        limitation: None,
    };
    let started = Instant::now();
    let mut budget = BlobBudget::default();
    while let Some(id) = next {
        result.scanned += 1;
        let step = match history_step(repository, id, &mut path, &mut budget) {
            Ok(step) => step,
            Err(error) => {
                // Keep the already-read page when a later ancestor or rename object is absent.
                result.limitation = Some(error);
                break;
            }
        };
        if let Some(entry) = step.entry {
            result.entries.push(entry);
        }
        next = step.next;
        if step.reached_root && repository.is_shallow() {
            result.limitation = Some(SHALLOW_LIMITATION.into());
        }
        if result.scanned >= MAX_HISTORY_SCAN
            || result.entries.len() >= HISTORY_PAGE_SIZE
            || started.elapsed() >= Duration::from_secs(2)
        {
            break;
        }
    }
    if result.limitation.is_none() {
        result.next_cursor = next.map(|id| format!("{sha}:{key}:{id}:{}", file_key(&path)));
    }
    Ok(result)
}

fn blame_boundary(repository: &Repository, sha: &str, key: &str) -> Result<(Oid, Option<String>)> {
    let mut oldest = commit(repository, sha)?;
    let mut budget = BlobBudget::default();
    let mut paths = HashSet::from([path_bytes(key)?]);
    let started = Instant::now();
    for depth in 0..MAX_HISTORY_SCAN {
        if oldest.parent_count() == 0 {
            return Ok((
                oldest.id(),
                repository.is_shallow().then(|| SHALLOW_LIMITATION.into()),
            ));
        }
        if depth == MAX_HISTORY_SCAN - 1 || started.elapsed() >= Duration::from_secs(2) {
            return Ok((oldest.id(), Some("Blame borné à 256 commits et deux secondes de préparation sur le premier parent ; les groupes de frontière peuvent avoir un auteur plus ancien. Aucun fetch automatique.".into())));
        }
        let step = (|| {
            let parent = oldest.parent(0).map_err(object_error)?;
            let tree = oldest.tree().map_err(object_error)?;
            let parent_tree = parent.tree().map_err(object_error)?;
            let mut changed = false;
            let mut present_in_parent = false;
            for path in &paths {
                let current = entry(repository, &tree, path)?;
                let previous = entry(repository, &parent_tree, path)?;
                changed |= current != previous;
                present_in_parent |= previous.is_some();
                for (id, mode) in [current, previous].into_iter().flatten() {
                    budget.check(repository, id, mode)?;
                }
            }
            if changed {
                // libgit2's blame finds renames in a full diff whenever any tracked path changes.
                // Check its candidate blobs before inflating them, and retain each old identity.
                let mut diff = metadata_diff(repository, Some(&parent_tree), &tree)?;
                find_renames(repository, &mut diff, &mut budget)?;
                let old_paths: Vec<_> = diff
                    .deltas()
                    .filter_map(|delta| {
                        let new_path = delta.new_file().path_bytes()?;
                        if delta.status() == Delta::Renamed && paths.contains(new_path) {
                            delta.old_file().path_bytes().map(|path| path.to_vec())
                        } else {
                            None
                        }
                    })
                    .collect();
                for path in &old_paths {
                    literal_blame_path(path)?;
                }
                present_in_parent |= !old_paths.is_empty();
                paths.extend(old_paths);
            }
            Ok::<_, String>((parent, present_in_parent))
        })();
        match step {
            Ok((parent, true)) => oldest = parent,
            // No tracked identity precedes this creation, so older unrelated commits need not be scanned.
            Ok((_, false)) => return Ok((oldest.id(), None)),
            Err(error) => {
                return Ok((
                    oldest.id(),
                    Some(format!("Blame interrompu à une frontière : {error}")),
                ));
            }
        }
    }
    unreachable!("the final ancestry iteration always returns a boundary")
}

pub(crate) fn blame(repository: &Repository, sha: &str, key: &str) -> Result<FileBlame> {
    // Blame keeps the background bound whatever the preview showed.
    let content = content(repository, sha, key, PreviewLimit::MIN)?;
    if content.oversized.is_some() {
        return Err("Blame disponible uniquement pour un fichier de 2 Mio au plus.".into());
    }
    if content.kind != "text" {
        return Err(
            "Blame disponible uniquement pour un fichier texte présent à cette révision.".into(),
        );
    }
    if content.lines.is_empty() {
        return Ok(FileBlame {
            groups: vec![],
            limitation: None,
        });
    }
    let path = path_bytes(key)?;
    literal_blame_path(&path)?;
    #[cfg(unix)]
    let native_path = {
        use std::os::unix::ffi::OsStringExt;
        PathBuf::from(std::ffi::OsString::from_vec(path))
    };
    #[cfg(not(unix))]
    let native_path = PathBuf::from(
        String::from_utf8(path)
            .map_err(|_| "Blame des chemins non UTF-8 indisponible sur cet OS.")?,
    );
    let (oldest, limitation) = blame_boundary(repository, sha, key)?;
    let mut options = BlameOptions::new();
    options
        .newest_commit(Oid::from_str(sha).map_err(|_| "Identifiant de commit invalide.")?)
        .oldest_commit(oldest)
        .first_parent(true)
        .min_line(1)
        .max_line(content.lines.len());
    let blame = repository
        .blame_file(&native_path, Some(&mut options))
        .map_err(object_error)?;
    let groups: Vec<_> = blame
        .iter()
        .filter_map(|hunk| {
            let start = hunk.final_start_line();
            if start > content.lines.len() {
                return None;
            }
            let author = hunk.final_signature();
            Some(BlameGroup {
                start_line: start,
                lines: hunk.lines_in_hunk().min(content.lines.len() + 1 - start),
                sha: hunk.final_commit_id().to_string(),
                author: author
                    .as_ref()
                    .map(|author| String::from_utf8_lossy(author.name_bytes()).into_owned())
                    .unwrap_or_default(),
                time: author
                    .as_ref()
                    .map(|author| author.when().seconds())
                    .unwrap_or_default(),
                boundary: hunk.is_boundary(),
            })
        })
        .collect();
    let reached_boundary = groups
        .iter()
        .any(|group| group.boundary && group.sha == oldest.to_string());
    Ok(FileBlame {
        groups,
        limitation: limitation.filter(|_| reached_boundary),
    })
}
