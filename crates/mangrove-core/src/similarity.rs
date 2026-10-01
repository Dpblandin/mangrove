//! Bound rename detection before libgit2 loads any candidate contents. The
//! normal diff remains complete when fuzzy matching cannot fit the budget.
use std::{collections::HashMap, path::Component};

use cap_fs_ext::DirExt;
use git2::{Delta, Diff, DiffFile, DiffFindOptions, FileMode, Repository};

use crate::{Result, MAX_FILE_SIZE};

const MAX_CANDIDATES: usize = 200;
const MAX_SIMILARITY_BYTES: usize = 16 * 1024 * 1024;

pub(crate) fn find_renames(
    repository: &Repository,
    diff: &mut Diff<'_>,
    worktree: bool,
) -> Result<Option<String>> {
    let mut sources = 0;
    let mut targets = 0;
    for delta in diff.deltas() {
        if matches!(delta.status(), Delta::Deleted | Delta::Typechange)
            && blob_mode(delta.old_file().mode())
        {
            sources += 1;
        }
        if (matches!(delta.status(), Delta::Added | Delta::Typechange)
            || worktree && delta.status() == Delta::Untracked)
            && blob_mode(delta.new_file().mode())
        {
            targets += 1;
        }
    }
    if sources == 0 || targets == 0 {
        return Ok(None);
    }
    if sources + targets > MAX_CANDIDATES {
        return Ok(Some(format!(
            "Détection des renommages limitée : plus de {MAX_CANDIDATES} fichiers candidats. Les ajouts et suppressions restent affichés."
        )));
    }

    let odb = repository.odb().map_err(|error| error.to_string())?;
    let mut sizes = HashMap::new();
    let mut bytes = 0usize;
    let mut limitation = None;
    for delta in diff.deltas() {
        let old = delta.old_file();
        let new = delta.new_file();
        let candidates = [
            (
                matches!(delta.status(), Delta::Deleted | Delta::Typechange),
                old,
            ),
            (
                matches!(delta.status(), Delta::Added | Delta::Typechange)
                    || worktree && delta.status() == Delta::Untracked,
                new,
            ),
        ];
        for (candidate, file) in candidates {
            if !candidate || !blob_mode(file.mode()) {
                continue;
            }
            let size = if file.id().is_zero() {
                // A workdir OID is not computed yet: inspect metadata only.
                // Reading headers or stat information must never hash a blob.
                match worktree_size(repository, &file) {
                    Ok(size) => size,
                    Err(_) => {
                        limitation = Some("un fichier local ne peut pas être inspecté sans suivre un lien ou lire un fichier spécial");
                        break;
                    }
                }
            } else {
                if let Some(&size) = sizes.get(&file.id()) {
                    size
                } else {
                    let size = odb
                        .read_header(file.id())
                        .map_err(|error| error.to_string())?
                        .0;
                    sizes.insert(file.id(), size);
                    size
                }
            };
            if size > MAX_FILE_SIZE as usize {
                limitation = Some("un fichier candidat dépasse 2 Mio");
                break;
            }
            // libgit2 caches signatures per delta, not per OID: repeated
            // candidate paths must consume the budget even for shared blobs.
            bytes = bytes.saturating_add(size);
            if bytes > MAX_SIMILARITY_BYTES {
                limitation = Some("les fichiers candidats dépassent le budget total de 16 Mio");
                break;
            }
        }
        if limitation.is_some() {
            break;
        }
    }

    let mut find = DiffFindOptions::new();
    find.renames(true)
        .copies(false)
        .copies_from_unmodified(false)
        .renames_from_rewrites(false)
        .for_untracked(worktree)
        .rename_limit(MAX_CANDIDATES);
    if let Some(reason) = limitation {
        // Tree/index deltas already have OIDs. Exact matching needs no blob
        // reads and retains identical large-file renames. Workdir exact matching
        // hashes files with missing OIDs, so it must be skipped in this case.
        if !worktree {
            find.exact_match_only(true);
            diff.find_similar(Some(&mut find))
                .map_err(|error| error.to_string())?;
        }
        return Ok(Some(format!(
            "Détection de similarité limitée : {reason}. Les ajouts et suppressions restent affichés."
        )));
    }
    diff.find_similar(Some(&mut find))
        .map_err(|error| error.to_string())?;
    Ok(None)
}

fn blob_mode(mode: FileMode) -> bool {
    // Match libgit2's GIT_MODE_ISBLOB: symlinks and gitlinks are not rename
    // candidates and must not consume a regular-file signature budget.
    matches!(mode, FileMode::Blob | FileMode::BlobExecutable)
}

fn worktree_size(repository: &Repository, file: &DiffFile<'_>) -> Result<usize> {
    let path = crate::working_tree::relative_path(
        file.path_bytes().ok_or("Chemin de fichier introuvable.")?,
    )?;
    let mut directory = crate::working_tree::open_worktree(repository)?;
    let mut components = path.components().peekable();
    while let Some(component) = components.next() {
        let Component::Normal(name) = component else {
            return Err("Chemin de fichier invalide.".into());
        };
        if components.peek().is_some() {
            directory = directory
                .open_dir_nofollow(name)
                .map_err(|error| error.to_string())?;
        } else {
            let metadata = directory
                .symlink_metadata(name)
                .map_err(|error| error.to_string())?;
            if !metadata.is_file() && !metadata.file_type().is_symlink() {
                return Err("Fichier spécial.".into());
            }
            return usize::try_from(metadata.len()).map_err(|error| error.to_string());
        }
    }
    Err("Chemin de fichier vide.".into())
}
