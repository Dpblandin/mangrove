//! Regression fixtures for the work bounded before rename detection. Timings
//! are reported only by the ignored probe; correctness never relies on time.
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant, SystemTime},
};

use git2::{Oid, Repository, Signature};
use mangrove_core::{
    model::{FileDiff, RepositoryInfo},
    Workspace,
};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

struct Fixture {
    _root: TempDir,
    source: PathBuf,
    repository: Repository,
    workspace: Workspace,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source");
        let repository = Repository::init(&source).unwrap();
        repository.set_head("refs/heads/main").unwrap();
        let workspace = Workspace::new(root.path().join("cache")).unwrap();
        Self {
            _root: root,
            source,
            repository,
            workspace,
        }
    }

    fn commit(&self, parent: Option<Oid>, files: &[(&str, &[u8])]) -> Oid {
        let mut builder = self.repository.treebuilder(None).unwrap();
        for (path, content) in files {
            builder
                .insert(*path, self.repository.blob(content).unwrap(), 0o100644)
                .unwrap();
        }
        let tree = self.repository.find_tree(builder.write().unwrap()).unwrap();
        let author = Signature::now("Test", "test@example.com").unwrap();
        let parent = parent.map(|oid| self.repository.find_commit(oid).unwrap());
        self.repository
            .commit(
                Some("HEAD"),
                &author,
                &author,
                "diff fixture",
                &tree,
                &parent.iter().collect::<Vec<_>>(),
            )
            .unwrap()
    }

    fn open(&self) -> RepositoryInfo {
        let info = self
            .workspace
            .open_repository(self.source.to_str().unwrap())
            .unwrap();
        let started = Instant::now();
        while !self.workspace.index_status(&info.id).unwrap().complete {
            assert!(started.elapsed() < Duration::from_secs(10));
            std::thread::sleep(Duration::from_millis(5));
        }
        info
    }
}

fn source_snapshot(root: &Path) -> BTreeMap<PathBuf, (SystemTime, Vec<u8>)> {
    fn visit(root: &Path, path: &Path, output: &mut BTreeMap<PathBuf, (SystemTime, Vec<u8>)>) {
        let metadata = fs::symlink_metadata(path).unwrap();
        let digest = if metadata.is_file() {
            Sha256::digest(fs::read(path).unwrap()).to_vec()
        } else {
            Vec::new()
        };
        output.insert(
            path.strip_prefix(root).unwrap().to_path_buf(),
            (metadata.modified().unwrap(), digest),
        );
        if metadata.is_dir() {
            for entry in fs::read_dir(path).unwrap() {
                visit(root, &entry.unwrap().path(), output);
            }
        }
    }
    let mut output = BTreeMap::new();
    visit(root, root, &mut output);
    output
}

fn patch_text(patch: &FileDiff) -> String {
    patch
        .hunks
        .iter()
        .flat_map(|hunk| &hunk.lines)
        .map(|line| line.content.as_str())
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn oversized_similarity_preserves_all_files_indices_and_identical_renames() {
    for identical in [false, true] {
        let fixture = Fixture::new();
        let old = b"baseline line\n".repeat(250_000);
        let mut new = old.clone();
        if !identical {
            new[0] = b'X';
        }
        let parent = fixture.commit(None, &[("old.txt", &old), ("small.txt", b"before\n")]);
        let oid = fixture.commit(
            Some(parent),
            &[("new.txt", &new), ("small.txt", b"after\n")],
        );
        let info = fixture.open();
        let before = source_snapshot(&fixture.source);
        let details = fixture
            .workspace
            .commit_details_shared(&info.id, &oid.to_string())
            .unwrap();
        assert!(details.similarity_limited.is_some());
        assert_eq!(details.file_count, if identical { 2 } else { 3 });
        for (index, file) in details.files.iter().enumerate() {
            assert_eq!(file.index, index);
            let patch = fixture
                .workspace
                .file_diff(&info.id, &oid.to_string(), index)
                .unwrap();
            if file.path == "small.txt" {
                assert!(patch_text(&patch).contains("after"));
            }
        }
        if identical {
            assert!(details
                .files
                .iter()
                .any(|file| file.path == "new.txt" && file.status == "renamed"));
        } else {
            assert!(details
                .files
                .iter()
                .any(|file| file.path == "new.txt" && file.status == "added"));
            assert!(details
                .files
                .iter()
                .any(|file| file.path == "old.txt" && file.status == "deleted"));
        }
        let cached = fixture
            .workspace
            .commit_details_shared(&info.id, &oid.to_string())
            .unwrap();
        assert!(Arc::ptr_eq(&details, &cached));
        assert_eq!(source_snapshot(&fixture.source), before);
    }
}

#[test]
fn candidate_budget_keeps_the_complete_file_list_and_selected_patches() {
    let fixture = Fixture::new();
    let old_paths: Vec<_> = (0..101)
        .map(|index| format!("old-{index:03}.txt"))
        .collect();
    let new_paths: Vec<_> = (0..101)
        .map(|index| format!("new-{index:03}.txt"))
        .collect();
    let old_bytes: Vec<_> = (0..101)
        .map(|index| format!("unique file {index}\nbefore\n").into_bytes())
        .collect();
    let new_bytes: Vec<_> = (0..101)
        .map(|index| format!("unique file {index}\nafter\n").into_bytes())
        .collect();
    let old_files: Vec<_> = old_paths
        .iter()
        .zip(&old_bytes)
        .map(|(path, bytes)| (path.as_str(), bytes.as_slice()))
        .collect();
    let new_files: Vec<_> = new_paths
        .iter()
        .zip(&new_bytes)
        .map(|(path, bytes)| (path.as_str(), bytes.as_slice()))
        .collect();
    let parent = fixture.commit(None, &old_files);
    let oid = fixture.commit(Some(parent), &new_files);
    let info = fixture.open();
    let details = fixture
        .workspace
        .commit_details_shared(&info.id, &oid.to_string())
        .unwrap();
    assert!(details
        .similarity_limited
        .as_deref()
        .unwrap()
        .contains("200"));
    assert_eq!(details.file_count, 202);
    assert_eq!(details.files.len(), 202);
    for index in [0, 100, 101, 201] {
        let patch = fixture
            .workspace
            .file_diff(&info.id, &oid.to_string(), index)
            .unwrap();
        assert!(!patch.hunks.is_empty());
    }
}

#[test]
fn cumulative_byte_budget_is_checked_even_when_every_candidate_fits_individually() {
    let fixture = Fixture::new();
    let old_paths: Vec<_> = (0..10).map(|index| format!("old-{index:02}.txt")).collect();
    let new_paths: Vec<_> = (0..10).map(|index| format!("new-{index:02}.txt")).collect();
    let old_bytes: Vec<_> = (0..10)
        .map(|_| {
            let mut bytes = b"line contents\n".repeat(80_000);
            // Reused OIDs still create a libgit2 signature for each path.
            bytes[0] = b'A';
            bytes
        })
        .collect();
    let new_bytes: Vec<_> = old_bytes
        .iter()
        .map(|bytes| {
            let mut bytes = bytes.clone();
            bytes[1] = b'X';
            bytes
        })
        .collect();
    let old_files: Vec<_> = old_paths
        .iter()
        .zip(&old_bytes)
        .map(|(path, bytes)| (path.as_str(), bytes.as_slice()))
        .collect();
    let new_files: Vec<_> = new_paths
        .iter()
        .zip(&new_bytes)
        .map(|(path, bytes)| (path.as_str(), bytes.as_slice()))
        .collect();
    let parent = fixture.commit(None, &old_files);
    let oid = fixture.commit(Some(parent), &new_files);
    let info = fixture.open();
    let details = fixture
        .workspace
        .commit_details_shared(&info.id, &oid.to_string())
        .unwrap();
    assert!(details
        .similarity_limited
        .as_deref()
        .unwrap()
        .contains("16 Mio"));
    assert_eq!(details.file_count, 20);
    assert!(details
        .files
        .iter()
        .all(|file| matches!(file.status.as_str(), "added" | "deleted")));
}

#[test]
fn text_attributes_cannot_load_an_oversized_patch_before_the_size_guard() {
    let fixture = Fixture::new();
    let attributes = b"large.txt diff\n";
    let parent = fixture.commit(
        None,
        &[(".gitattributes", attributes), ("large.txt", b"before\n")],
    );
    let large = b"large text line\n".repeat(200_000);
    let oid = fixture.commit(
        Some(parent),
        &[(".gitattributes", attributes), ("large.txt", &large)],
    );
    let info = fixture.open();
    let details = fixture
        .workspace
        .commit_details_shared(&info.id, &oid.to_string())
        .unwrap();
    let file = details
        .files
        .iter()
        .find(|file| file.path == "large.txt")
        .unwrap();
    let tree = fixture.repository.find_commit(oid).unwrap().tree().unwrap();
    let object = tree.get_name("large.txt").unwrap().id();
    let object_name = object.to_string();
    let object_path = fixture
        .repository
        .path()
        .join("objects")
        .join(&object_name[..2])
        .join(&object_name[2..]);
    // Leave the compressed object's header readable, but remove its checksum.
    // A blob-content read now fails, making an early size check observable.
    let mut compressed = fs::read(&object_path).unwrap();
    compressed.truncate(compressed.len() - 4);
    // Git's loose objects are read-only; replace our disposable fixture's
    // object instead of requesting a writable descriptor for the original.
    fs::remove_file(&object_path).unwrap();
    fs::write(&object_path, compressed).unwrap();
    let fresh = Repository::open(&fixture.source).unwrap();
    assert_eq!(
        fresh.odb().unwrap().read_header(object).unwrap().0,
        large.len()
    );
    assert!(fresh.find_blob(object).is_err());
    let before = source_snapshot(&fixture.source);
    let patch = fixture
        .workspace
        .file_diff(&info.id, &oid.to_string(), file.index)
        .unwrap();
    assert!(patch.truncated);
    assert!(!patch.binary);
    assert!(patch.hunks.is_empty());
    assert_eq!(source_snapshot(&fixture.source), before);
}

#[test]
fn normal_renames_and_stale_local_selections_keep_their_content_and_source_unchanged() {
    let fixture = Fixture::new();
    fixture.commit(
        None,
        &[
            ("old.txt", b"rename baseline\nsecond line\n"),
            ("literal[1]*.txt", b"literal before\n"),
        ],
    );
    fixture
        .repository
        .checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
        .unwrap();
    fs::rename(
        fixture.source.join("old.txt"),
        fixture.source.join("new.txt"),
    )
    .unwrap();
    fs::write(fixture.source.join("literal[1]*.txt"), b"literal after\n").unwrap();
    let info = fixture.open();
    let before = source_snapshot(&fixture.source);
    let status = fixture.workspace.working_tree_status(&info.id).unwrap();
    assert!(status.similarity_limited.is_none());
    let renamed = status
        .unstaged
        .iter()
        .find(|file| file.file.path == "new.txt")
        .unwrap();
    assert_eq!(renamed.file.status, "renamed");
    assert!(patch_text(
        &fixture
            .workspace
            .working_tree_diff(&info.id, "unstaged", &renamed.key)
            .unwrap()
    )
    .is_empty());
    let literal = status
        .unstaged
        .iter()
        .find(|file| file.file.path == "literal[1]*.txt")
        .unwrap();
    assert!(patch_text(
        &fixture
            .workspace
            .working_tree_diff(&info.id, "unstaged", &literal.key)
            .unwrap()
    )
    .contains("literal after"));
    assert_eq!(source_snapshot(&fixture.source), before);
    // No status refresh: path identity must survive unrelated insertions and
    // read the selected file's current content, rather than reuse a patch.
    fs::write(fixture.source.join("aaa.txt"), b"other\n").unwrap();
    fs::write(fixture.source.join("literal[1]*.txt"), b"literal newest\n").unwrap();
    let before = source_snapshot(&fixture.source);
    assert!(patch_text(
        &fixture
            .workspace
            .working_tree_diff(&info.id, "unstaged", &literal.key)
            .unwrap()
    )
    .contains("literal newest"));
    fs::remove_file(fixture.source.join("new.txt")).unwrap();
    let deleted_snapshot = source_snapshot(&fixture.source);
    assert!(fixture
        .workspace
        .working_tree_diff(&info.id, "unstaged", &renamed.key)
        .is_err());
    assert_eq!(source_snapshot(&fixture.source), deleted_snapshot);
    assert_ne!(deleted_snapshot, before);
}

#[test]
fn cached_local_selection_does_not_read_unrelated_rename_candidates() {
    let fixture = Fixture::new();
    let renamed_contents = b"unrelated rename baseline\nsecond line\n";
    fixture.commit(
        None,
        &[
            ("old.txt", renamed_contents),
            ("selected.txt", b"selected before\n"),
        ],
    );
    fixture
        .repository
        .checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
        .unwrap();
    fs::rename(
        fixture.source.join("old.txt"),
        fixture.source.join("new.txt"),
    )
    .unwrap();
    fs::write(fixture.source.join("selected.txt"), b"selected after\n").unwrap();
    let info = fixture.open();
    let status = fixture.workspace.working_tree_status(&info.id).unwrap();
    assert!(status
        .unstaged
        .iter()
        .any(|file| file.file.path == "new.txt" && file.file.status == "renamed"));
    let selected = status
        .unstaged
        .iter()
        .find(|file| file.file.path == "selected.txt")
        .unwrap();

    // A fresh area-wide similarity pass would need this unrelated object's
    // header. Its absence must not prevent reading the already selected path.
    let unrelated = fixture
        .repository
        .blob(renamed_contents)
        .unwrap()
        .to_string();
    fs::remove_file(
        fixture
            .repository
            .path()
            .join("objects")
            .join(&unrelated[..2])
            .join(&unrelated[2..]),
    )
    .unwrap();
    assert!(fixture.workspace.working_tree_status(&info.id).is_err());
    let before = source_snapshot(&fixture.source);
    let patch = fixture
        .workspace
        .working_tree_diff(&info.id, "unstaged", &selected.key)
        .unwrap();
    assert!(patch_text(&patch).contains("selected after"));
    assert_eq!(source_snapshot(&fixture.source), before);
}

#[test]
#[ignore = "release timing probe; excludes application/UI latency"]
fn diff_performance_probe() {
    let fixture = Fixture::new();
    let old = b"baseline line\n".repeat(2_400_000);
    let mut new = old.clone();
    new[0] = b'X';
    let parent = fixture.commit(None, &[("old.txt", &old), ("small.txt", b"before\n")]);
    let oid = fixture.commit(
        Some(parent),
        &[("new.txt", &new), ("small.txt", b"after\n")],
    );
    let info = fixture.open();
    let started = Instant::now();
    let details = fixture
        .workspace
        .commit_details_shared(&info.id, &oid.to_string())
        .unwrap();
    println!(
        "rename32MiB first_details_ms={:.3} files={} limited={}",
        started.elapsed().as_secs_f64() * 1000.0,
        details.file_count,
        details.similarity_limited.is_some()
    );
    let small = details
        .files
        .iter()
        .find(|file| file.path == "small.txt")
        .unwrap();
    for label in ["details_cache", "selected_patch"] {
        let mut samples = Vec::new();
        for _ in 0..11 {
            let started = Instant::now();
            if label == "details_cache" {
                std::hint::black_box(
                    fixture
                        .workspace
                        .commit_details_shared(&info.id, &oid.to_string())
                        .unwrap(),
                );
            } else {
                std::hint::black_box(
                    fixture
                        .workspace
                        .file_diff(&info.id, &oid.to_string(), small.index)
                        .unwrap(),
                );
            }
            samples.push(started.elapsed().as_secs_f64() * 1000.0);
        }
        samples.remove(0);
        samples.sort_by(f64::total_cmp);
        println!(
            "rename32MiB {label} p50_ms={:.3} p95_ms={:.3} samples={samples:?}",
            samples[4], samples[9]
        );
    }
}

#[test]
fn commit_line_totals_match_file_patches_and_stay_unknown_past_the_size_guard() {
    let fixture = Fixture::new();
    let parent = fixture.commit(
        None,
        &[("a.txt", b"one\ntwo\nthree\n"), ("b.txt", b"keep\n")],
    );
    let oid = fixture.commit(
        Some(parent),
        &[
            ("a.txt", b"one\n2\nthree\nfour\n"),
            ("c.txt", b"new\nfile\n"),
        ],
    );
    let info = fixture.open();
    let sha = oid.to_string();
    let details = fixture
        .workspace
        .commit_details_shared(&info.id, &sha)
        .unwrap();
    let expected = (0..details.file_count).fold((0, 0), |(added, deleted), index| {
        let diff = fixture.workspace.file_diff(&info.id, &sha, index).unwrap();
        (added + diff.additions, deleted + diff.deletions)
    });
    assert_eq!(expected, (4, 2));
    assert_eq!(
        fixture
            .workspace
            .commit_line_totals(&info.id, &sha, 1_000, &|| false)
            .unwrap(),
        Some(expected)
    );

    let large = b"large text line\n".repeat(200_000);
    let oversized = fixture.commit(Some(oid), &[("a.txt", &large), ("c.txt", b"new\n")]);
    let info = fixture.open();
    assert_eq!(
        fixture
            .workspace
            .commit_line_totals(&info.id, &oversized.to_string(), 1_000, &|| false)
            .unwrap(),
        None
    );
    // More files than allowed, or a read cancelled by a newer selection.
    assert_eq!(
        fixture
            .workspace
            .commit_line_totals(&info.id, &sha, 1, &|| false)
            .unwrap(),
        None
    );
    assert!(fixture
        .workspace
        .commit_line_totals(&info.id, &sha, 1_000, &|| true)
        .is_err());
}
