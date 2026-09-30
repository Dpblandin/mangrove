use std::{
    io::Read,
    path::{Component, Path},
};

use cap_fs_ext::{DirExt, FollowSymlinks, OpenOptionsFollowExt, OpenOptionsSyncExt};
use cap_std::{
    ambient_authority,
    fs::{Dir, OpenOptions},
};
use git2::{Diff, DiffDelta, DiffFindOptions, DiffOptions, ErrorCode, Repository};

use crate::{
    changed_file,
    model::{FileDiff, WorkingTreeFile, WorkingTreeStatus},
    render_file_diff, render_patch, Result, MAX_CHANGED_FILES, MAX_FILE_SIZE,
};

pub(crate) fn status(repository: &Repository) -> Result<WorkingTreeStatus> {
    if repository.is_bare() {
        return Ok(WorkingTreeStatus::default());
    }
    let staged = area_diff(repository, "staged")?;
    let unstaged = area_diff(repository, "unstaged")?;
    Ok(WorkingTreeStatus {
        available: true,
        staged_count: staged.deltas().len(),
        unstaged_count: unstaged.deltas().len(),
        staged: staged
            .deltas()
            .take(MAX_CHANGED_FILES)
            .enumerate()
            .map(|(index, delta)| working_file(index, &delta))
            .collect(),
        unstaged: unstaged
            .deltas()
            .take(MAX_CHANGED_FILES)
            .enumerate()
            .map(|(index, delta)| working_file(index, &delta))
            .collect(),
    })
}

fn file_key(path: &[u8]) -> String {
    const HEX: &[u8] = b"0123456789abcdef";
    let mut key = String::with_capacity(path.len() * 2);
    for byte in path {
        key.push(HEX[(byte >> 4) as usize] as char);
        key.push(HEX[(byte & 0xf) as usize] as char);
    }
    key
}

fn key_bytes(key: &str) -> Result<Vec<u8>> {
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
    WorkingTreeFile {
        key: file_key(
            delta
                .new_file()
                .path_bytes()
                .or_else(|| delta.old_file().path_bytes())
                .unwrap_or_default(),
        ),
        file: changed_file(index, delta),
    }
}

pub(crate) fn diff(repository: &Repository, area: &str, key: &str) -> Result<FileDiff> {
    let bytes = key_bytes(key)?;
    if repository.is_bare() {
        return Err("Ce dépôt bare ne possède pas de fichiers non commités.".into());
    }
    let diff = area_diff(repository, area)?;
    let index = diff
        .deltas()
        .position(|delta| {
            delta
                .new_file()
                .path_bytes()
                .or_else(|| delta.old_file().path_bytes())
                .is_some_and(|path| path == bytes)
        })
        .ok_or("Ce fichier a changé ou n'est plus présent dans cette liste.")?;
    let delta = diff.get_delta(index).ok_or("Fichier introuvable.")?;
    let path = delta
        .new_file()
        .path()
        .or_else(|| delta.old_file().path())
        .ok_or("Chemin de fichier introuvable.")?;
    validate_relative_path(path)?;
    if matches!(
        delta.status(),
        git2::Delta::Conflicted | git2::Delta::Unreadable
    ) {
        return Err("Le patch de ce fichier en conflit ou illisible n'est pas disponible.".into());
    }
    if area == "unstaged" {
        return worktree_patch(repository, &delta, path);
    }
    render_file_diff(&diff, index)
}

fn area_diff<'a>(repository: &'a Repository, area: &str) -> Result<Diff<'a>> {
    let mut options = DiffOptions::new();
    // Never persist the stat-cache refresh performed during a worktree diff.
    options
        .update_index(false)
        .include_typechange(true)
        .include_unreadable(true)
        .max_size(MAX_FILE_SIZE);
    let index = repository.index().map_err(|e| e.to_string())?;
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
            repository.diff_tree_to_index(head.as_ref(), Some(&index), Some(&mut options))
        }
        "unstaged" => {
            options
                .include_untracked(true)
                .recurse_untracked_dirs(true)
                .show_untracked_content(true)
                .ignore_submodules(true);
            repository.diff_index_to_workdir(Some(&index), Some(&mut options))
        }
        _ => return Err("Zone de modifications invalide : staged ou unstaged attendue.".into()),
    }
    .map_err(|e| e.to_string())?;
    let mut find = DiffFindOptions::new();
    find.renames(true)
        .for_untracked(area == "unstaged")
        .rename_limit(200);
    diff.find_similar(Some(&mut find))
        .map_err(|e| e.to_string())?;
    Ok(diff)
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

fn worktree_content(repository: &Repository, path: &Path) -> Result<WorktreeContent> {
    validate_relative_path(path)?;
    let workdir = repository.workdir().ok_or("Worktree introuvable.")?;
    let root_parent = Dir::open_ambient_dir(
        workdir.parent().ok_or("Parent du worktree introuvable.")?,
        ambient_authority(),
    )
    .map_err(|error| error.to_string())?;
    let mut directory = root_parent
        .open_dir_nofollow(workdir.file_name().ok_or("Worktree invalide.")?)
        .map_err(|error| error.to_string())?;
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
    options.max_size(MAX_FILE_SIZE);
    let patch = git2::Patch::from_buffers(
        old.as_ref().map_or(&[], |blob| blob.content()),
        delta.old_file().path(),
        &new,
        Some(path),
        Some(&mut options),
    )
    .map_err(|error| error.to_string())?;
    render_patch(patch)
}
