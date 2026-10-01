//! Reuse the resolved file identities of immutable commits. Patch reads then
//! compare only the selected paths instead of repeating whole-commit similarity.
use std::{collections::VecDeque, sync::Arc};

use git2::{Delta, Diff, DiffDelta, DiffFindOptions, DiffOptions, Oid, Repository};

use crate::{render_file_diff, Result, MAX_CHANGED_FILES, MAX_FILE_SIZE};

const MAX_COMMITS: usize = 4;
const MAX_PATH_BYTES: usize = 1024 * 1024;

struct FileIdentity {
    old_path: Option<Vec<u8>>,
    new_path: Option<Vec<u8>>,
    old_id: Oid,
    new_id: Oid,
    status: Delta,
}

impl FileIdentity {
    fn matches(&self, delta: &DiffDelta<'_>) -> bool {
        self.status == delta.status()
            && self.old_path.as_deref() == delta.old_file().path_bytes()
            && self.new_path.as_deref() == delta.new_file().path_bytes()
            && self.old_id == delta.old_file().id()
            && self.new_id == delta.new_file().id()
    }
}

pub(crate) struct CommitFiles(Vec<FileIdentity>);

impl CommitFiles {
    /// Filtering can change rename pairing when paths are reused. Verify the
    /// complete delta identity and fall back to the original comparison if so.
    pub(crate) fn file_diff(
        &self,
        repository: &Repository,
        sha: &str,
        index: usize,
    ) -> Result<crate::model::FileDiff> {
        let file = self
            .0
            .get(index)
            .ok_or("Fichier introuvable dans ce commit.")?;
        let commit = repository
            .find_commit(Oid::from_str(sha).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        let tree = commit.tree().map_err(|e| e.to_string())?;
        let parent = if commit.parent_count() > 0 {
            Some(
                commit
                    .parent(0)
                    .and_then(|parent| parent.tree())
                    .map_err(|e| e.to_string())?,
            )
        } else {
            None
        };
        let mut options = DiffOptions::new();
        options
            .include_typechange(true)
            .max_size(MAX_FILE_SIZE)
            .disable_pathspec_match(true);
        for path in [&file.old_path, &file.new_path].into_iter().flatten() {
            options.pathspec(path.as_slice());
        }
        let mut diff = repository
            .diff_tree_to_tree(parent.as_ref(), Some(&tree), Some(&mut options))
            .map_err(|e| e.to_string())?;
        if file.status == Delta::Renamed {
            let mut find = DiffFindOptions::new();
            find.renames(true).rename_limit(200);
            diff.find_similar(Some(&mut find))
                .map_err(|e| e.to_string())?;
        }
        if let Some(selected) = diff.deltas().position(|delta| file.matches(&delta)) {
            return render_file_diff(&diff, selected);
        }
        let (_, diff) = crate::commit_diff(repository, sha)?;
        render_file_diff(&diff, index)
    }
}

#[derive(Default)]
pub(crate) struct CommitDiffCache {
    entries: VecDeque<(Oid, Arc<CommitFiles>)>,
}

impl CommitDiffCache {
    pub(crate) fn get(&mut self, sha: Oid) -> Option<Arc<CommitFiles>> {
        let index = self.entries.iter().position(|(key, _)| *key == sha)?;
        let entry = self.entries.remove(index)?;
        let files = entry.1.clone();
        self.entries.push_back(entry);
        Some(files)
    }

    pub(crate) fn remember(&mut self, sha: Oid, diff: &Diff<'_>) {
        if diff.deltas().len() > MAX_CHANGED_FILES {
            return;
        }
        let mut bytes = 0;
        let mut files = Vec::with_capacity(diff.deltas().len());
        for delta in diff.deltas() {
            let old = delta.old_file();
            let new = delta.new_file();
            bytes +=
                old.path_bytes().map_or(0, <[u8]>::len) + new.path_bytes().map_or(0, <[u8]>::len);
            if bytes > MAX_PATH_BYTES {
                return;
            }
            files.push(FileIdentity {
                old_path: old.path_bytes().map(<[u8]>::to_vec),
                new_path: new.path_bytes().map(<[u8]>::to_vec),
                old_id: old.id(),
                new_id: new.id(),
                status: delta.status(),
            });
        }
        self.entries.retain(|(key, _)| *key != sha);
        self.entries.push_back((sha, Arc::new(CommitFiles(files))));
        while self.entries.len() > MAX_COMMITS {
            self.entries.pop_front();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit(repository: &Repository, parents: &[Oid], files: &[(&[u8], &[u8], i32)]) -> Oid {
        let mut builder = repository.treebuilder(None).unwrap();
        for (path, content, mode) in files {
            builder
                .insert(*path, repository.blob(content).unwrap(), *mode)
                .unwrap();
        }
        let tree = repository.find_tree(builder.write().unwrap()).unwrap();
        let signature = git2::Signature::now("Test", "test@example.com").unwrap();
        let parents: Vec<_> = parents
            .iter()
            .map(|id| repository.find_commit(*id).unwrap())
            .collect();
        repository
            .commit(
                None,
                &signature,
                &signature,
                "test",
                &tree,
                &parents.iter().collect::<Vec<_>>(),
            )
            .unwrap()
    }

    #[test]
    fn filtered_patches_match_full_comparison_for_roots_merges_and_reused_literal_paths() {
        let directory = tempfile::tempdir().unwrap();
        let repository = Repository::init_bare(directory.path()).unwrap();
        let root = commit(
            &repository,
            &[],
            &[
                (b"old.txt", b"rename me\nsecond line\n", 0o100644),
                (b"reused.txt", b"original contents\n", 0o100644),
                (b"deleted.txt", b"removed\n", 0o100644),
                (b"literal[1]*.txt", b"before\n", 0o100644),
                (b"literal1other.txt", b"untouched\n", 0o100644),
                (b"mode.txt", b"mode\n", 0o100644),
                (b"type.txt", b"contents\n", 0o100644),
                (b"raw-\xff", b"raw before\n", 0o100644),
            ],
        );
        let files: &[(&[u8], &[u8], i32)] = &[
            (b"reused.txt", b"rename me\nsecond line\n", 0o100644),
            (b"moved.txt", b"original contents\n", 0o100644),
            (b"literal[1]*.txt", b"after\n", 0o100644),
            (b"literal1other.txt", b"untouched\n", 0o100644),
            (b"mode.txt", b"mode\n", 0o100755),
            (b"type.txt", b"target.txt", 0o120000),
            (b"raw-\xff", b"raw after\n", 0o100644),
            (b"binary", b"\0binary\xff", 0o100644),
            (b"empty", b"", 0o100644),
        ];
        let second = commit(&repository, &[root], files);
        let merge = commit(&repository, &[root, second], files);
        let mut cache = CommitDiffCache::default();
        for id in [root, second, merge] {
            let sha = id.to_string();
            let (_, full) = crate::commit_diff(&repository, &sha).unwrap();
            cache.remember(id, &full);
            let cached = cache.get(id).unwrap();
            for index in 0..full.deltas().len() {
                let expected = render_file_diff(&full, index).unwrap();
                let actual = cached.file_diff(&repository, &sha, index).unwrap();
                assert_eq!(
                    serde_json::to_value(actual).unwrap(),
                    serde_json::to_value(expected).unwrap(),
                    "commit {id}, index {index}"
                );
            }
            assert!(cached.file_diff(&repository, &sha, usize::MAX).is_err());
        }
    }
}
