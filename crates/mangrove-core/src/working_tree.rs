use std::{
    collections::HashMap,
    io::Read,
    path::{Component, Path, PathBuf},
};

use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt, OpenOptionsSyncExt};
use cap_std::{
    ambient_authority,
    fs::{Dir, OpenOptions},
};
use git2::{Delta, Diff, DiffDelta, DiffOptions, ErrorCode, Repository};

use crate::{
    changed_file,
    model::{FileDiff, FileImageData, FileImages, WorkingTreeFile, WorkingTreeStatus},
    render_patch, Result, MAX_CHANGED_FILES, MAX_FILE_SIZE,
};

const MAX_IDENTITY_BYTES: usize = 2 * 1024 * 1024;

struct FileIdentity {
    old_path: Option<Vec<u8>>,
    new_path: Option<Vec<u8>>,
    status: Delta,
}

impl FileIdentity {
    fn matches(&self, delta: &DiffDelta<'_>) -> bool {
        self.status == delta.status()
            && self.old_path.as_deref() == delta.old_file().path_bytes()
            && self.new_path.as_deref() == delta.new_file().path_bytes()
    }
}

/// Only identities are retained, never libgit2's workdir diff or file contents.
/// A selected patch rechecks the index/workdir for these literal paths.
#[derive(Default)]
pub(crate) struct WorkingTreeCache {
    staged: HashMap<Vec<u8>, FileIdentity>,
    unstaged: HashMap<Vec<u8>, FileIdentity>,
}

impl WorkingTreeCache {
    fn area(&self, area: &str) -> Result<&HashMap<Vec<u8>, FileIdentity>> {
        match area {
            "staged" => Ok(&self.staged),
            "unstaged" => Ok(&self.unstaged),
            _ => Err("Zone de modifications invalide : staged ou unstaged attendue.".into()),
        }
    }

    fn remember(&mut self, area: &str, diff: &Diff<'_>, flags: &IndexFlags) {
        let mut identities = HashMap::new();
        let mut bytes = 0usize;
        for delta in diff
            .deltas()
            .filter(|delta| eligible(flags, area, delta))
            .take(MAX_CHANGED_FILES)
        {
            let old_path = delta.old_file().path_bytes().map(<[u8]>::to_vec);
            let new_path = delta.new_file().path_bytes().map(<[u8]>::to_vec);
            let Some(path) = new_path.as_ref().or(old_path.as_ref()) else {
                continue;
            };
            bytes += path.len()
                + old_path.as_ref().map_or(0, Vec::len)
                + new_path.as_ref().map_or(0, Vec::len);
            if bytes > MAX_IDENTITY_BYTES {
                break;
            }
            identities.insert(
                path.clone(),
                FileIdentity {
                    old_path,
                    new_path,
                    status: delta.status(),
                },
            );
        }
        match area {
            "staged" => self.staged = identities,
            "unstaged" => self.unstaged = identities,
            _ => unreachable!(),
        }
    }
}

pub(crate) fn status(
    repository: &Repository,
    cache: &mut WorkingTreeCache,
) -> Result<WorkingTreeStatus> {
    if repository.is_bare() {
        return Ok(WorkingTreeStatus::default());
    }
    let index = read_index(repository)?;
    let flags = index_flags(&index);
    let (staged, staged_limit) = area_diff(repository, "staged", &index, None)?;
    let (unstaged, unstaged_limit) = area_diff(repository, "unstaged", &index, None)?;
    cache.remember("staged", &staged, &flags);
    cache.remember("unstaged", &unstaged, &flags);
    Ok(WorkingTreeStatus {
        available: true,
        similarity_limited: match (staged_limit, unstaged_limit) {
            (None, None) => None,
            (Some(limit), None) | (None, Some(limit)) => Some(limit),
            (Some(staged), Some(unstaged)) if staged == unstaged => Some(staged),
            (Some(staged), Some(unstaged)) => Some(format!(
                "Index : {staged} Modifications locales : {unstaged}"
            )),
        },
        staged_count: staged
            .deltas()
            .filter(|delta| eligible(&flags, "staged", delta))
            .count(),
        unstaged_count: unstaged
            .deltas()
            .filter(|delta| eligible(&flags, "unstaged", delta))
            .count(),
        staged: staged
            .deltas()
            .enumerate()
            .filter(|(_, delta)| eligible(&flags, "staged", delta))
            .take(MAX_CHANGED_FILES)
            .map(|(index, delta)| working_file(index, &delta))
            .collect(),
        unstaged: unstaged
            .deltas()
            .enumerate()
            .filter(|(_, delta)| eligible(&flags, "unstaged", delta))
            .take(MAX_CHANGED_FILES)
            .map(|(index, delta)| {
                let mut file = working_file(index, &delta);
                if delta_flags(&flags, &delta).is_intent_to_add() {
                    file.file.status = "added".into();
                }
                file
            })
            .collect(),
    })
}

pub(crate) fn file_key(path: &[u8]) -> String {
    const HEX: &[u8] = b"0123456789abcdef";
    let mut key = String::with_capacity(path.len() * 2);
    for byte in path {
        key.push(HEX[(byte >> 4) as usize] as char);
        key.push(HEX[(byte & 0xf) as usize] as char);
    }
    key
}

pub(crate) fn key_bytes(key: &str) -> Result<Vec<u8>> {
    if key.is_empty()
        || !key.len().is_multiple_of(2)
        || !key.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err("Identité de fichier invalide.".into());
    }
    let digit = |byte: u8| {
        if byte.is_ascii_digit() {
            byte - b'0'
        } else {
            byte.to_ascii_lowercase() - b'a' + 10
        }
    };
    Ok(key
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| (digit(pair[0]) << 4) | digit(pair[1]))
        .collect())
}

fn working_file(index: usize, delta: &DiffDelta<'_>) -> WorkingTreeFile {
    let file = changed_file(index, delta);
    WorkingTreeFile {
        key: file.file_key.clone(),
        file,
    }
}

pub(crate) fn diff(
    repository: &Repository,
    area: &str,
    key: &str,
    cache: &mut WorkingTreeCache,
) -> Result<FileDiff> {
    let bytes = key_bytes(key)?;
    if repository.is_bare() {
        return Err("Ce dépôt bare ne possède pas de fichiers non commités.".into());
    }
    let (diff, index) = selected_diff(repository, area, &bytes, cache)?;
    let delta = diff.get_delta(index).ok_or("Fichier introuvable.")?;
    let path_bytes = delta
        .new_file()
        .path_bytes()
        .or_else(|| delta.old_file().path_bytes())
        .ok_or("Chemin de fichier introuvable.")?;
    let path = relative_path(path_bytes)?;
    validate_relative_path(&path)?;
    if matches!(
        delta.status(),
        git2::Delta::Conflicted | git2::Delta::Unreadable
    ) {
        return Err("Le patch de ce fichier en conflit ou illisible n'est pas disponible.".into());
    }
    if area == "unstaged" {
        return worktree_patch(repository, &delta, &path);
    }
    crate::commit_diff::render_bounded(repository, &diff, index)
}

pub(crate) fn images(
    repository: &Repository,
    area: &str,
    key: &str,
    cache: &mut WorkingTreeCache,
) -> Result<FileImages> {
    let bytes = crate::file_browser::path_bytes(key)?;
    if repository.is_bare() {
        return Err("Ce dépôt bare ne possède pas de fichiers non commités.".into());
    }
    let (diff, selected) = selected_diff(repository, area, &bytes, cache)?;
    let delta = diff.get_delta(selected).ok_or("Fichier introuvable.")?;
    if area == "staged" {
        return crate::images::delta(repository, &delta);
    }
    let before = crate::images::blob(
        repository,
        delta.old_file().id(),
        delta.old_file().mode().into(),
    )?;
    let after = if delta.status() == git2::Delta::Deleted {
        FileImageData::Absent
    } else if matches!(
        delta.new_file().mode(),
        git2::FileMode::Link | git2::FileMode::Commit
    ) {
        FileImageData::Unavailable(
            "Les liens symboliques et les sous-modules n’ont pas d’aperçu image.".into(),
        )
    } else {
        let path = relative_path(&bytes)?;
        match worktree_content(repository, &path)? {
            WorktreeContent::Bytes(bytes) => FileImageData::Bytes(bytes),
            WorktreeContent::Missing => {
                return Err("Ce fichier a changé ou n’est plus présent dans cette liste.".into())
            }
            WorktreeContent::Oversized => crate::images::oversized(),
        }
    };
    Ok(FileImages {
        before: Some(before),
        after,
    })
}

fn selected_diff<'a>(
    repository: &'a Repository,
    area: &str,
    bytes: &[u8],
    cache: &mut WorkingTreeCache,
) -> Result<(Diff<'a>, usize)> {
    let index = read_index(repository)?;
    let flags = index_flags(&index);
    if let Some(identity) = cache.area(area)?.get(bytes) {
        let (filtered, _) = area_diff(repository, area, &index, Some(identity))?;
        if let Some(selected) = filtered.deltas().position(|delta| {
            eligible(&flags, area, &delta)
                && delta
                    .new_file()
                    .path_bytes()
                    .or_else(|| delta.old_file().path_bytes())
                    == Some(bytes)
                && (identity.status != Delta::Renamed || identity.matches(&delta))
        }) {
            return Ok((filtered, selected));
        }
        // Renames can become additions/deletions or reuse paths after the
        // last status read. Resolve a new full snapshot only on this miss.
    }
    let (diff, _) = area_diff(repository, area, &index, None)?;
    cache.remember(area, &diff, &flags);
    let selected = diff
        .deltas()
        .position(|delta| {
            eligible(&flags, area, &delta)
                && delta
                    .new_file()
                    .path_bytes()
                    .or_else(|| delta.old_file().path_bytes())
                    == Some(bytes)
        })
        .ok_or("Ce fichier a changé ou n'est plus présent dans cette liste.")?;
    Ok((diff, selected))
}

fn area_diff<'a>(
    repository: &'a Repository,
    area: &str,
    index: &git2::Index,
    selected: Option<&FileIdentity>,
) -> Result<(Diff<'a>, Option<String>)> {
    let mut options = DiffOptions::new();
    // Never persist the stat-cache refresh performed during a worktree diff.
    options
        .update_index(false)
        .include_typechange(true)
        .include_unreadable(true)
        .skip_binary_check(true)
        .max_size(MAX_FILE_SIZE);
    if let Some(identity) = selected {
        options.disable_pathspec_match(true);
        for path in [&identity.old_path, &identity.new_path]
            .into_iter()
            .flatten()
        {
            options.pathspec(path.as_slice());
        }
    }
    let mut diff = match area {
        "staged" => {
            let head = match repository.head() {
                Ok(head) => Some(head.peel_to_tree().map_err(|e| e.to_string())?),
                Err(error)
                    if matches!(error.code(), ErrorCode::UnbornBranch | ErrorCode::NotFound) =>
                {
                    None
                }
                Err(error) => return Err(error.to_string()),
            };
            repository.diff_tree_to_index(head.as_ref(), Some(index), Some(&mut options))
        }
        "unstaged" => {
            options
                .include_untracked(true)
                .recurse_untracked_dirs(true)
                .show_untracked_content(true)
                .ignore_submodules(true);
            repository.diff_index_to_workdir(Some(index), Some(&mut options))
        }
        _ => return Err("Zone de modifications invalide : staged ou unstaged attendue.".into()),
    }
    .map_err(|e| e.to_string())?;
    let limitation = if selected.is_none_or(|identity| identity.status == Delta::Renamed) {
        crate::similarity::find_renames(repository, &mut diff, area == "unstaged")?
    } else {
        None
    };
    Ok((diff, limitation))
}

pub(crate) fn relative_path(bytes: &[u8]) -> Result<PathBuf> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        Ok(std::ffi::OsString::from_vec(bytes.to_vec()).into())
    }
    #[cfg(not(unix))]
    {
        std::str::from_utf8(bytes)
            .map(PathBuf::from)
            .map_err(|_| "Ce chemin Git non UTF-8 n'est pas représentable sur cet OS.".into())
    }
}

fn read_index(repository: &Repository) -> Result<git2::Index> {
    repository.index().map_err(|error| {
        if error.message().contains("sdir") {
            "L'index sparse n'est pas encore pris en charge. Utilisez un index complet pour consulter les modifications locales.".into()
        } else { error.to_string() }
    })
}
type IndexFlags = std::collections::HashMap<Vec<u8>, git2::IndexEntryExtendedFlag>;
fn index_flags(index: &git2::Index) -> IndexFlags {
    index
        .iter()
        .filter(|entry| entry.flags_extended != 0)
        .map(|entry| {
            (
                entry.path,
                git2::IndexEntryExtendedFlag::from_bits_truncate(entry.flags_extended),
            )
        })
        .collect()
}
fn delta_flags(flags: &IndexFlags, delta: &DiffDelta<'_>) -> git2::IndexEntryExtendedFlag {
    delta
        .new_file()
        .path_bytes()
        .or_else(|| delta.old_file().path_bytes())
        .and_then(|path| flags.get(path))
        .copied()
        .unwrap_or_else(git2::IndexEntryExtendedFlag::empty)
}
fn eligible(flags: &IndexFlags, area: &str, delta: &DiffDelta<'_>) -> bool {
    let flags = delta_flags(flags, delta);
    !(area == "staged" && flags.is_intent_to_add()
        || area == "unstaged" && delta.status() == git2::Delta::Deleted && flags.is_skip_worktree())
}

fn validate_relative_path(path: &Path) -> Result<()> {
    if path.as_os_str().is_empty()
        || path.as_os_str().as_encoded_bytes().contains(&0)
        || !path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err("Le chemin doit désigner un fichier relatif au dépôt.".into());
    }
    Ok(())
}

enum WorktreeContent {
    Bytes(Vec<u8>),
    Missing,
    Oversized,
}

pub(crate) fn open_worktree(repository: &Repository) -> Result<Dir> {
    let workdir = repository.workdir().ok_or("Worktree introuvable.")?;
    let root_parent = Dir::open_ambient_dir(
        workdir.parent().ok_or("Parent du worktree introuvable.")?,
        ambient_authority(),
    )
    .map_err(|error| error.to_string())?;
    root_parent
        .open_dir_nofollow(workdir.file_name().ok_or("Worktree invalide.")?)
        .map_err(|error| error.to_string())
}

fn worktree_content(repository: &Repository, path: &Path) -> Result<WorktreeContent> {
    validate_relative_path(path)?;
    let mut directory = open_worktree(repository)?;
    let mut components = path.components().peekable();
    while let Some(component) = components.next() {
        let name = component.as_os_str();
        if components.peek().is_some() {
            directory = match directory.open_dir_nofollow(name) {
                Ok(directory) => directory,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    return Ok(WorktreeContent::Missing)
                }
                Err(error) => {
                    return Err(format!(
                        "Un parent du fichier est un lien ou n'est pas un dossier : {error}"
                    ))
                }
            };
            continue;
        }
        let metadata = match directory.symlink_metadata(name) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(WorktreeContent::Missing)
            }
            Err(error) => return Err(error.to_string()),
        };
        if metadata.file_type().is_symlink() {
            // readlinkat reads the link itself, never its target, even if the
            // directory entry is replaced concurrently. The parent is pinned.
            let target = directory
                .read_link_contents(name)
                .map_err(|error| error.to_string())?;
            return Ok(WorktreeContent::Bytes(
                target.as_os_str().as_encoded_bytes().to_vec(),
            ));
        }
        if !metadata.is_file() {
            return Err("Les dossiers, sous-modules et fichiers spéciaux ne sont pas lus comme des patches.".into());
        }
        let mut options = OpenOptions::new();
        options.read(true).follow(FollowSymlinks::No).nonblock(true);
        let file = directory
            .open_with(name, &options)
            .map_err(|error| error.to_string())?;
        // Check the opened descriptor, not the earlier directory entry. A
        // regular-file→symlink race fails the nofollow open; a special-file
        // race cannot block open and is rejected before any content read.
        let metadata = file.metadata().map_err(|error| error.to_string())?;
        if !metadata.is_file() {
            return Err("Les fichiers spéciaux ne sont pas lus comme des patches.".into());
        }
        if metadata.len() > MAX_FILE_SIZE as u64 {
            return Ok(WorktreeContent::Oversized);
        }
        let mut bytes = Vec::with_capacity(metadata.len() as usize);
        file.take(MAX_FILE_SIZE as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| error.to_string())?;
        return Ok(if bytes.len() > MAX_FILE_SIZE as usize {
            WorktreeContent::Oversized
        } else {
            WorktreeContent::Bytes(bytes)
        });
    }
    Err("Chemin de fichier introuvable.".into())
}

fn truncated_diff() -> FileDiff {
    FileDiff {
        binary: false,
        truncated: true,
        additions: 0,
        deletions: 0,
        hunks: vec![],
    }
}

fn worktree_patch(repository: &Repository, delta: &DiffDelta<'_>, path: &Path) -> Result<FileDiff> {
    let diff_attribute = repository
        .get_attr_bytes(path, "diff", git2::AttrCheckFlags::empty())
        .map_err(|error| error.to_string())?;
    let force_binary = matches!(
        git2::AttrValue::from_bytes(diff_attribute),
        git2::AttrValue::False
    );
    if force_binary {
        return Ok(FileDiff {
            binary: true,
            truncated: false,
            additions: 0,
            deletions: 0,
            hunks: vec![],
        });
    }
    let force_text = matches!(
        git2::AttrValue::from_bytes(diff_attribute),
        git2::AttrValue::True
    );
    let old_id = delta.old_file().id();
    let old = if old_id.is_zero() {
        None
    } else {
        let (size, _) = repository
            .odb()
            .and_then(|odb| odb.read_header(old_id))
            .map_err(|error| error.to_string())?;
        if size > MAX_FILE_SIZE as usize {
            return Ok(truncated_diff());
        }
        Some(
            repository
                .find_blob(old_id)
                .map_err(|error| error.to_string())?,
        )
    };
    let content = if delta.status() == git2::Delta::Deleted {
        // This snapshot is a deletion. Its patch only needs the index blob;
        // do not open a guessed replacement at the old name (which can even
        // be an index-only path the host filesystem cannot represent).
        WorktreeContent::Missing
    } else {
        worktree_content(repository, path)?
    };
    let new = match content {
        WorktreeContent::Bytes(bytes) => bytes,
        WorktreeContent::Missing if delta.status() == git2::Delta::Deleted => vec![],
        WorktreeContent::Missing => {
            return Err("Ce fichier a changé ou n'est plus présent dans cette liste.".into())
        }
        WorktreeContent::Oversized => return Ok(truncated_diff()),
    };
    let mut options = DiffOptions::new();
    options.max_size(MAX_FILE_SIZE).force_text(force_text);
    let new = if delta.new_file().mode() == git2::FileMode::Link {
        new
    } else {
        crate::worktree_filter::clean(repository, path.as_os_str().as_encoded_bytes(), &new)?
    };
    let old_path = delta
        .old_file()
        .path_bytes()
        .map(relative_path)
        .transpose()?;
    let patch = git2::Patch::from_buffers(
        old.as_ref().map_or(&[], |blob| blob.content()),
        old_path.as_deref(),
        &new,
        Some(path),
        Some(&mut options),
    )
    .map_err(|error| error.to_string())?;
    render_patch(patch)
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn worktree_root_replaced_by_a_symlink_is_not_followed() {
        let fixture = tempfile::tempdir().unwrap();
        let workdir = fixture.path().join("worktree");
        let repository = Repository::init(&workdir).unwrap();
        assert!(open_worktree(&repository).is_ok());
        let relocated = fixture.path().join("relocated");
        std::fs::rename(&workdir, &relocated).unwrap();
        std::os::unix::fs::symlink(&relocated, &workdir).unwrap();
        assert!(open_worktree(&repository).is_err());
    }
}
