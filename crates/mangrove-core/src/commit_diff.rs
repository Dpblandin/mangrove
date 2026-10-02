//! Reuse the resolved file identities of immutable commits. Patch reads then
//! compare only the selected paths instead of repeating whole-commit similarity.
use std::{collections::VecDeque, sync::Arc};

use git2::{Delta, Diff, DiffDelta, DiffOptions, Oid, Repository};

use crate::{render_file_diff, Result, MAX_CHANGED_FILES, MAX_FILE_SIZE};

const MAX_COMMITS: usize = 8;
const MAX_PATH_BYTES: usize = 1024 * 1024;
const MAX_CACHE_BYTES: usize = 8 * 1024 * 1024;

/// libgit2's `max_size` can be overridden by a text diff attribute. Tree deltas
/// also commonly have an unknown size until a patch is built, so inspect the
/// selected object headers before asking libgit2 to materialize any contents.
pub(crate) fn render_bounded(
    repository: &Repository,
    diff: &Diff<'_>,
    index: usize,
) -> Result<crate::model::FileDiff> {
    let delta = diff.get_delta(index).ok_or("Fichier introuvable.")?;
    let odb = repository.odb().map_err(|error| error.to_string())?;
    for file in [delta.old_file(), delta.new_file()] {
        if file.id().is_zero()
            || !matches!(
                file.mode(),
                git2::FileMode::Blob | git2::FileMode::BlobExecutable | git2::FileMode::Link
            )
        {
            continue;
        }
        let (size, _) = odb
            .read_header(file.id())
            .map_err(|error| error.to_string())?;
        if size > MAX_FILE_SIZE as usize {
            return Ok(crate::model::FileDiff {
                binary: false,
                truncated: true,
                additions: 0,
                deletions: 0,
                hunks: vec![],
            });
        }
    }
    render_file_diff(diff, index)
}

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
        oid: Oid,
        index: usize,
    ) -> Result<crate::model::FileDiff> {
        let Some(file) = self.0.get(index) else {
            // Long path names can exhaust the identity budget before the
            // public file-list limit. Such entries still use the normal path.
            let (_, diff, _) = crate::commit_diff(repository, &oid.to_string())?;
            return render_bounded(repository, &diff, index);
        };
        let commit = repository.find_commit(oid).map_err(|e| e.to_string())?;
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
            crate::similarity::find_renames(repository, &mut diff, false)?;
        }
        if let Some(selected) = diff.deltas().position(|delta| file.matches(&delta)) {
            return render_bounded(repository, &diff, selected);
        }
        let (_, diff, _) = crate::commit_diff(repository, &oid.to_string())?;
        render_bounded(repository, &diff, index)
    }
}

struct CachedCommit {
    oid: Oid,
    files: Option<Arc<CommitFiles>>,
    details: Option<Arc<crate::model::CommitDetails>>,
}

impl CachedCommit {
    fn bytes(&self) -> usize {
        let files = self.files.as_ref().map_or(0, |files| {
            std::mem::size_of::<CommitFiles>()
                + files.0.capacity() * std::mem::size_of::<FileIdentity>()
                + files
                    .0
                    .iter()
                    .map(|file| {
                        file.old_path.as_ref().map_or(0, Vec::capacity)
                            + file.new_path.as_ref().map_or(0, Vec::capacity)
                    })
                    .sum::<usize>()
        });
        files
            + self.details.as_ref().map_or(0, |details| {
                std::mem::size_of::<crate::model::CommitDetails>()
                    + details.sha.capacity()
                    + details.message.capacity()
                    + details.author.capacity()
                    + details.email.capacity()
                    + details.committer.capacity()
                    + details.committer_email.capacity()
                    + details.parents.capacity() * std::mem::size_of::<String>()
                    + details.parents.iter().map(String::capacity).sum::<usize>()
                    + details
                        .similarity_limited
                        .as_ref()
                        .map_or(0, String::capacity)
                    + details.files.capacity() * std::mem::size_of::<crate::model::ChangedFile>()
                    + details
                        .files
                        .iter()
                        .map(|file| {
                            file.file_key.capacity()
                                + file.path.capacity()
                                + file.status.capacity()
                                + file.old_path.as_ref().map_or(0, String::capacity)
                        })
                        .sum::<usize>()
            })
    }
}

#[derive(Default)]
pub(crate) struct CommitDiffCache {
    entries: VecDeque<CachedCommit>,
}

impl CommitDiffCache {
    pub(crate) fn get(&mut self, sha: Oid) -> Option<Arc<CommitFiles>> {
        let index = self.entries.iter().position(|entry| entry.oid == sha)?;
        let entry = self.entries.remove(index)?;
        let files = entry.files.clone();
        self.entries.push_back(entry);
        files
    }

    pub(crate) fn get_details(&mut self, sha: Oid) -> Option<Arc<crate::model::CommitDetails>> {
        let index = self.entries.iter().position(|entry| entry.oid == sha)?;
        let entry = self.entries.remove(index)?;
        let details = entry.details.clone();
        self.entries.push_back(entry);
        details
    }

    fn take_entry(&mut self, oid: Oid) -> CachedCommit {
        self.entries
            .iter()
            .position(|entry| entry.oid == oid)
            .and_then(|index| self.entries.remove(index))
            .unwrap_or(CachedCommit {
                oid,
                files: None,
                details: None,
            })
    }

    fn insert(&mut self, entry: CachedCommit) {
        self.entries.push_back(entry);
        while self.entries.len() > MAX_COMMITS
            || self.entries.iter().map(CachedCommit::bytes).sum::<usize>() > MAX_CACHE_BYTES
        {
            self.entries.pop_front();
        }
    }

    pub(crate) fn remember_details(&mut self, sha: Oid, details: Arc<crate::model::CommitDetails>) {
        let mut entry = self.take_entry(sha);
        entry.details = Some(details);
        // An unusually large message may not fit. Retain its bounded file
        // identities so the immediately selected patch never repeats the diff.
        if entry.bytes() > MAX_CACHE_BYTES {
            entry.details = None;
        }
        self.insert(entry);
    }

    pub(crate) fn remember(&mut self, sha: Oid, diff: &Diff<'_>) {
        let mut bytes = 0;
        let mut files = Vec::with_capacity(diff.deltas().len().min(MAX_CHANGED_FILES));
        // Details expose only this prefix. Even a giant commit must retain
        // those identities, otherwise its first patch repeats all similarity.
        for delta in diff.deltas().take(MAX_CHANGED_FILES) {
            let old = delta.old_file();
            let new = delta.new_file();
            bytes +=
                old.path_bytes().map_or(0, <[u8]>::len) + new.path_bytes().map_or(0, <[u8]>::len);
            if bytes > MAX_PATH_BYTES {
                break;
            }
            files.push(FileIdentity {
                old_path: old.path_bytes().map(<[u8]>::to_vec),
                new_path: new.path_bytes().map(<[u8]>::to_vec),
                old_id: old.id(),
                new_id: new.id(),
                status: delta.status(),
            });
        }
        let mut entry = self.take_entry(sha);
        entry.files = Some(Arc::new(CommitFiles(files)));
        self.insert(entry);
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
            let (_, full, _) = crate::commit_diff(&repository, &sha).unwrap();
            cache.remember(id, &full);
            let cached = cache.get(id).unwrap();
            for index in 0..full.deltas().len() {
                let expected = render_file_diff(&full, index).unwrap();
                let actual = cached.file_diff(&repository, id, index).unwrap();
                assert_eq!(
                    serde_json::to_value(actual).unwrap(),
                    serde_json::to_value(expected).unwrap(),
                    "commit {id}, index {index}"
                );
            }
            assert!(cached.file_diff(&repository, id, usize::MAX).is_err());
        }
    }

    #[test]
    fn giant_commits_keep_the_visible_prefix_without_repeating_unrelated_similarity() {
        let directory = tempfile::tempdir().unwrap();
        let repository = Repository::init_bare(directory.path()).unwrap();
        let paths: Vec<_> = (0..MAX_CHANGED_FILES)
            .map(|index| format!("{index:05}.txt").into_bytes())
            .collect();
        let mut old: Vec<(&[u8], &[u8], i32)> = paths
            .iter()
            .map(|path| (path.as_slice(), b"before\n".as_slice(), 0o100644))
            .collect();
        old.push((b"z-old.txt", b"unrelated rename content\n", 0o100644));
        let parent = commit(&repository, &[], &old);
        let mut new: Vec<(&[u8], &[u8], i32)> = paths
            .iter()
            .map(|path| (path.as_slice(), b"after\n".as_slice(), 0o100644))
            .collect();
        new.push((b"z-new.txt", b"unrelated rename content\n", 0o100644));
        let oid = commit(&repository, &[parent], &new);
        let (_, diff, limit) = crate::commit_diff(&repository, &oid.to_string()).unwrap();
        assert!(limit.is_none());
        assert_eq!(diff.deltas().len(), MAX_CHANGED_FILES + 1);
        let expected = render_file_diff(&diff, 0).unwrap();
        let mut cache = CommitDiffCache::default();
        cache.remember(oid, &diff);
        let cached = cache.get(oid).unwrap();
        assert_eq!(cached.0.len(), MAX_CHANGED_FILES);

        // A full comparison would inspect this unrelated rename's object
        // header and fail. The selected patch only needs its two own blobs.
        let unrelated = repository
            .blob(b"unrelated rename content\n")
            .unwrap()
            .to_string();
        std::fs::remove_file(
            repository
                .path()
                .join("objects")
                .join(&unrelated[..2])
                .join(&unrelated[2..]),
        )
        .unwrap();
        let fresh = Repository::open_bare(directory.path()).unwrap();
        assert!(crate::commit_diff(&fresh, &oid.to_string()).is_err());
        let actual = cached.file_diff(&fresh, oid, 0).unwrap();
        assert_eq!(
            serde_json::to_value(actual).unwrap(),
            serde_json::to_value(expected).unwrap()
        );
    }

    #[test]
    fn oversized_details_do_not_evict_their_bounded_file_identities() {
        let directory = tempfile::tempdir().unwrap();
        let repository = Repository::init_bare(directory.path()).unwrap();
        let oid = commit(&repository, &[], &[(b"one.txt", b"one\n", 0o100644)]);
        let (_, diff, _) = crate::commit_diff(&repository, &oid.to_string()).unwrap();
        let mut cache = CommitDiffCache::default();
        cache.remember(oid, &diff);
        let details = Arc::new(crate::model::CommitDetails {
            sha: oid.to_string(),
            message: "x".repeat(MAX_CACHE_BYTES),
            author: String::new(),
            email: String::new(),
            author_time: 0,
            committer: String::new(),
            committer_email: String::new(),
            time: 0,
            parents: vec![],
            files: vec![],
            file_count: 1,
            similarity_limited: None,
        });
        cache.remember_details(oid, details);
        assert!(cache.get_details(oid).is_none());
        assert!(cache.get(oid).is_some());
        assert!(cache.entries.iter().map(CachedCommit::bytes).sum::<usize>() <= MAX_CACHE_BYTES);
    }
}
