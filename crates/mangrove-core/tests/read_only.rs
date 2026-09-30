use std::{
    collections::{BTreeMap, HashMap},
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicI64, Ordering},
    thread,
    time::{Duration, Instant, SystemTime},
};

use git2::{Oid, Repository, Signature, Time};
use mangrove_core::{
    model::{Reference, RepositoryInfo},
    Workspace,
};
use rusqlite::Connection;
use sha2::{Digest, Sha256};
use tempfile::TempDir;

struct Fixture {
    root: TempDir,
    source: PathBuf,
    cache: PathBuf,
    repository: Repository,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("repository");
        let cache = root.path().join("cache");
        let repository = Repository::init(&source).unwrap();
        repository.set_head("refs/heads/main").unwrap();
        Self {
            root,
            source,
            cache,
            repository,
        }
    }

    fn workspace(&self) -> Workspace {
        Workspace::new(&self.cache).unwrap()
    }

    fn open(&self, workspace: &Workspace) -> RepositoryInfo {
        workspace
            .open_repository(self.source.to_str().unwrap())
            .unwrap()
    }
}

fn commit(
    repository: &Repository,
    message: &str,
    parents: &[Oid],
    files: &[(&str, &[u8])],
    reference: Option<&str>,
) -> Oid {
    static TIMESTAMP: AtomicI64 = AtomicI64::new(1_700_000_000);
    let mut builder = repository.treebuilder(None).unwrap();
    for (path, bytes) in files {
        let blob = repository.blob(bytes).unwrap();
        builder.insert(*path, blob, 0o100644).unwrap();
    }
    let tree = repository.find_tree(builder.write().unwrap()).unwrap();
    let signature = Signature::new(
        "Élodie Müller",
        "elodie+dev@example.org",
        &Time::new(TIMESTAMP.fetch_add(1, Ordering::Relaxed), 0),
    )
    .unwrap();
    let parents: Vec<_> = parents
        .iter()
        .map(|id| repository.find_commit(*id).unwrap())
        .collect();
    repository
        .commit(
            reference,
            &signature,
            &signature,
            message,
            &tree,
            &parents.iter().collect::<Vec<_>>(),
        )
        .unwrap()
}

fn finish(workspace: &Workspace, id: &str) {
    let started = Instant::now();
    loop {
        let status = workspace.index_status(id).unwrap();
        assert!(status.error.is_none(), "Index failed: {:?}", status.error);
        if status.complete {
            return;
        }
        assert!(
            started.elapsed() < Duration::from_secs(15),
            "Index timed out"
        );
        thread::sleep(Duration::from_millis(5));
    }
}

#[derive(Debug, PartialEq, Eq)]
struct FileState {
    modified: SystemTime,
    contents: Vec<u8>,
    kind: &'static str,
}

// Deliberately include directory mtimes and every Git administrative file, not
// just refs and index. Read access times are excluded because reads may update them.
fn snapshot(root: &Path) -> BTreeMap<PathBuf, FileState> {
    fn visit(root: &Path, path: &Path, entries: &mut BTreeMap<PathBuf, FileState>) {
        let metadata = fs::symlink_metadata(path).unwrap();
        let (kind, contents) = if metadata.file_type().is_symlink() {
            (
                "symlink",
                fs::read_link(path)
                    .unwrap()
                    .as_os_str()
                    .as_encoded_bytes()
                    .to_vec(),
            )
        } else if metadata.is_dir() {
            ("directory", vec![])
        } else if metadata.is_file() {
            ("file", fs::read(path).unwrap())
        } else {
            ("special", vec![])
        };
        entries.insert(
            path.strip_prefix(root).unwrap().to_path_buf(),
            FileState {
                modified: metadata.modified().unwrap(),
                contents,
                kind,
            },
        );
        if metadata.is_dir() {
            for entry in fs::read_dir(path).unwrap() {
                visit(root, &entry.unwrap().path(), entries);
            }
        }
    }
    let mut result = BTreeMap::new();
    visit(root, root, &mut result);
    result
}

fn cache_database(cache: &Path) -> PathBuf {
    fs::read_dir(cache)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.extension()
                .is_some_and(|extension| extension == "sqlite")
        })
        .unwrap()
}

#[test]
fn inspecting_commits_search_and_diffs_preserves_all_source_bytes_and_mtimes() {
    let fixture = Fixture::new();
    let first = commit(
        &fixture.repository,
        "initialisation",
        &[],
        &[("README.md", b"Bonjour\n")],
        Some("HEAD"),
    );
    let second = commit(
        &fixture.repository,
        "fix(#123): améliore le café",
        &[first],
        &[
            ("README.md", b"Bonjour Mangrove\n"),
            ("binary.dat", b"\0\x01\x02"),
        ],
        Some("HEAD"),
    );
    fs::write(
        fixture.source.join("README.md"),
        "modification locale à préserver\n",
    )
    .unwrap();
    fs::write(fixture.source.join("untracked.txt"), "non suivi\n").unwrap();
    let mut index = fixture.repository.index().unwrap();
    index.add_path(Path::new("README.md")).unwrap();
    index.write().unwrap();
    fixture
        .repository
        .remote("origin", "git@github.com:example/private.git")
        .unwrap();
    let before = snapshot(&fixture.source);
    let workspace = fixture.workspace();
    let info = fixture.open(&workspace);
    finish(&workspace, &info.id);
    assert_eq!(info.remotes, ["git@github.com:example/private.git"]);
    let page = workspace.list_commits(&info.id, 0, 256, "").unwrap();
    assert_eq!(page.total, 2);
    assert_eq!(page.commits[0].sha, second.to_string());
    for row in page.commits {
        assert_eq!(
            workspace.commit_position(&info.id, &row.sha).unwrap(),
            Some(row.position)
        );
        let details = workspace.commit_details(&info.id, &row.sha).unwrap();
        for file in &details.files {
            let expected = if row.sha == second.to_string() && file.path == "README.md" {
                "modified"
            } else {
                "added"
            };
            assert_eq!(file.status, expected);
        }
        for file in details.files {
            workspace.file_diff(&info.id, &row.sha, file.index).unwrap();
        }
    }
    assert_eq!(
        workspace
            .list_commits(&info.id, 0, 10, "café")
            .unwrap()
            .total,
        1
    );
    assert_eq!(
        workspace
            .list_commits(&info.id, 0, 10, "main")
            .unwrap()
            .total,
        1
    );
    assert_eq!(before, snapshot(&fixture.source));
}

#[test]
fn search_handles_unicode_punctuation_sha_and_conflicting_reference_labels() {
    let fixture = Fixture::new();
    let first = commit(
        &fixture.repository,
        "Initialisation",
        &[],
        &[],
        Some("HEAD"),
    );
    let second = commit(
        &fixture.repository,
        "fix(#123): améliore l'index\n\nRecherche café et naïveté.",
        &[first],
        &[],
        Some("HEAD"),
    );
    fixture
        .repository
        .reference("refs/heads/feature/café_100%", second, false, "test")
        .unwrap();
    fixture
        .repository
        .reference("refs/tags/main", first, false, "test")
        .unwrap();
    let workspace = fixture.workspace();
    let info = fixture.open(&workspace);
    finish(&workspace, &info.id);
    for query in [
        "améliore",
        "ameliore",
        "café",
        "\"café\"",
        "fix(#123):",
        "naive",
        "feature/café_100%",
        "refs/heads/feature/café_100%",
    ] {
        let page = workspace.list_commits(&info.id, 0, 10, query).unwrap();
        assert_eq!(page.total, 1, "query {query:?}");
        assert_eq!(page.commits[0].sha, second.to_string(), "query {query:?}");
    }
    for query in ["Élodie", "Muller", "elodie+dev@example.org", "main"] {
        assert_eq!(
            workspace
                .list_commits(&info.id, 0, 10, query)
                .unwrap()
                .total,
            2,
            "query {query:?}"
        );
    }
    let sha = second.to_string();
    assert_eq!(
        workspace
            .list_commits(&info.id, 0, 10, &sha[..8].to_ascii_uppercase())
            .unwrap()
            .commits[0]
            .sha,
        sha
    );
    for query in [
        "!!!",
        "\"",
        "AND",
        "(x)",
        "%",
        "_",
        "\\",
        "'; DROP TABLE commits; --",
    ] {
        assert_eq!(
            workspace
                .list_commits(&info.id, 0, 10, query)
                .unwrap()
                .total,
            0,
            "query {query:?}"
        );
    }
    assert_eq!(
        workspace.list_commits(&info.id, 0, 10, "").unwrap().total,
        2
    );
}

#[test]
fn history_is_topological_and_merge_edges_preserve_git_parent_order() {
    let fixture = Fixture::new();
    let root = commit(&fixture.repository, "root", &[], &[], Some("HEAD"));
    let left = commit(&fixture.repository, "left", &[root], &[], None);
    let right = commit(&fixture.repository, "right", &[root], &[], None);
    fixture
        .repository
        .reference("refs/heads/main", left, true, "test")
        .unwrap();
    let merge = commit(
        &fixture.repository,
        "merge",
        &[left, right],
        &[],
        Some("HEAD"),
    );
    fixture
        .repository
        .reference("refs/heads/left", left, false, "test")
        .unwrap();
    fixture
        .repository
        .reference("refs/heads/right", right, false, "test")
        .unwrap();
    let workspace = fixture.workspace();
    let info = fixture.open(&workspace);
    finish(&workspace, &info.id);
    let page = workspace.list_commits(&info.id, 0, 256, "").unwrap();
    assert_eq!(page.total, 4);
    let positions: HashMap<_, _> = page
        .commits
        .iter()
        .map(|row| (row.sha.clone(), row.position))
        .collect();
    for row in &page.commits {
        for parent in &row.parents {
            assert!(row.position < positions[parent]);
        }
        assert_eq!(
            row.graph
                .edges
                .iter()
                .filter(|edge| edge.kind == "parent")
                .count(),
            row.parents.len()
        );
        assert!(row.graph.lane < row.graph.width);
        assert!(row
            .graph
            .edges
            .iter()
            .all(|edge| edge.from < row.graph.width && edge.to < row.graph.width));
    }
    let row = &page.commits[0];
    assert_eq!(row.sha, merge.to_string());
    assert_eq!(row.parents, [left.to_string(), right.to_string()]);
    assert_eq!(
        workspace
            .commit_details(&info.id, &row.sha)
            .unwrap()
            .parents,
        row.parents
    );
}

#[test]
fn complete_cache_is_reused_and_new_commits_and_refs_invalidate_only_the_walk() {
    let fixture = Fixture::new();
    let first = commit(&fixture.repository, "first", &[], &[], Some("HEAD"));
    let second = commit(&fixture.repository, "second", &[first], &[], Some("HEAD"));
    let workspace = fixture.workspace();
    let initial = fixture.open(&workspace);
    finish(&workspace, &initial.id);
    let database = cache_database(&fixture.cache);
    let connection = Connection::open(&database).unwrap();
    let run: String = connection
        .query_row(
            "SELECT value FROM meta WHERE key = 'index_run'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let old_id: i64 = connection
        .query_row(
            "SELECT id FROM commits WHERE sha = ?1",
            [first.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    let same = fixture.open(&workspace);
    assert_eq!(same.id, initial.id);
    assert!(same.index.complete);
    drop(workspace);
    let workspace = fixture.workspace();
    let warm = fixture.open(&workspace);
    assert!(warm.index.complete);
    assert_eq!(warm.index.indexed, 2);
    assert_eq!(
        run,
        connection
            .query_row::<String, _, _>(
                "SELECT value FROM meta WHERE key = 'index_run'",
                [],
                |row| row.get(0)
            )
            .unwrap()
    );
    let third = commit(&fixture.repository, "third", &[second], &[], Some("HEAD"));
    let updated = fixture.open(&workspace);
    assert_ne!(warm.id, updated.id);
    assert!(workspace.list_commits(&warm.id, 0, 10, "").is_err());
    finish(&workspace, &updated.id);
    assert_eq!(
        workspace
            .list_commits(&updated.id, 0, 10, "")
            .unwrap()
            .total,
        3
    );
    assert_eq!(
        old_id,
        connection
            .query_row::<i64, _, _>(
                "SELECT id FROM commits WHERE sha = ?1",
                [first.to_string()],
                |row| row.get(0)
            )
            .unwrap()
    );
    fixture
        .repository
        .reference("refs/heads/new-label", third, false, "test")
        .unwrap();
    let reference_update = fixture.open(&workspace);
    assert_ne!(updated.id, reference_update.id);
    finish(&workspace, &reference_update.id);
    assert_eq!(
        workspace
            .list_commits(&reference_update.id, 0, 10, "new-label")
            .unwrap()
            .commits[0]
            .sha,
        third.to_string()
    );
    let orphan = commit(
        &fixture.repository,
        "orphan-only-search",
        &[],
        &[],
        Some("refs/heads/temporary"),
    );
    let with_orphan = fixture.open(&workspace);
    finish(&workspace, &with_orphan.id);
    assert!(workspace
        .commit_position(&with_orphan.id, &orphan.to_string())
        .unwrap()
        .is_some());
    fixture
        .repository
        .find_reference("refs/heads/temporary")
        .unwrap()
        .delete()
        .unwrap();
    let without_orphan = fixture.open(&workspace);
    finish(&workspace, &without_orphan.id);
    assert_eq!(
        workspace
            .list_commits(&without_orphan.id, 0, 10, "orphan-only-search")
            .unwrap()
            .total,
        0
    );
    assert!(workspace
        .commit_position(&without_orphan.id, &orphan.to_string())
        .unwrap()
        .is_none());
}

#[test]
fn unborn_detached_bare_and_linked_worktree_repositories_can_be_read() {
    let fixture = Fixture::new();
    let workspace = fixture.workspace();
    let empty = fixture.open(&workspace);
    finish(&workspace, &empty.id);
    assert_eq!(empty.head, None);
    assert_eq!(empty.head_name, "main");
    assert_eq!(
        workspace.list_commits(&empty.id, 0, 100, "").unwrap().total,
        0
    );
    let first = commit(
        &fixture.repository,
        "first",
        &[],
        &[("file.txt", b"text\n")],
        Some("HEAD"),
    );
    fixture.repository.set_head_detached(first).unwrap();
    let detached = fixture.open(&workspace);
    finish(&workspace, &detached.id);
    assert_eq!(detached.head_name, "HEAD détachée");
    assert_eq!(detached.head, Some(first.to_string()));
    fixture.repository.set_head("refs/heads/main").unwrap();
    let linked_path = fixture.root.path().join("linked");
    fixture
        .repository
        .worktree("linked", &linked_path, None)
        .unwrap();
    let before_source = snapshot(&fixture.source);
    let before_linked = snapshot(&linked_path);
    let linked = workspace
        .open_repository(linked_path.to_str().unwrap())
        .unwrap();
    finish(&workspace, &linked.id);
    assert_eq!(
        linked.path,
        linked_path.canonicalize().unwrap().to_string_lossy()
    );
    assert_eq!(
        workspace
            .list_commits(&linked.id, 0, 100, "")
            .unwrap()
            .total,
        1
    );
    workspace
        .file_diff(&linked.id, &first.to_string(), 0)
        .unwrap();
    assert_eq!(before_source, snapshot(&fixture.source));
    assert_eq!(before_linked, snapshot(&linked_path));
    let bare_path = fixture.root.path().join("bare.git");
    let bare_repository = Repository::init_bare(&bare_path).unwrap();
    commit(&bare_repository, "bare", &[], &[], Some("HEAD"));
    let before_bare = snapshot(&bare_path);
    let bare = workspace
        .open_repository(bare_path.to_str().unwrap())
        .unwrap();
    finish(&workspace, &bare.id);
    assert_eq!(
        workspace.list_commits(&bare.id, 0, 100, "").unwrap().total,
        1
    );
    assert_eq!(before_bare, snapshot(&bare_path));
}

#[test]
fn binary_empty_and_large_diffs_have_bounded_output() {
    let fixture = Fixture::new();
    let oversized = vec![b'x'; 2 * 1024 * 1024 + 1];
    let many_lines = (0..3_000)
        .map(|index| format!("line {index}\n"))
        .collect::<String>();
    let sha = commit(
        &fixture.repository,
        "files",
        &[],
        &[
            ("binary.dat", b"\0\x01\x02"),
            ("empty.txt", b""),
            ("large.txt", many_lines.as_bytes()),
            ("oversized.txt", &oversized),
        ],
        Some("HEAD"),
    );
    let workspace = fixture.workspace();
    let info = fixture.open(&workspace);
    finish(&workspace, &info.id);
    let details = workspace
        .commit_details(&info.id, &sha.to_string())
        .unwrap();
    for file in details.files {
        let diff = workspace
            .file_diff(&info.id, &sha.to_string(), file.index)
            .unwrap();
        match file.path.as_str() {
            "binary.dat" => {
                assert!(diff.binary);
                assert!(diff.hunks.is_empty());
            }
            "empty.txt" => {
                assert!(!diff.binary);
                assert!(!diff.truncated);
                assert!(diff.hunks.is_empty());
            }
            "large.txt" => {
                assert!(!diff.binary);
                assert!(diff.truncated);
                assert_eq!(diff.additions, 3_000);
                assert_eq!(
                    diff.hunks
                        .iter()
                        .map(|hunk| hunk.lines.len())
                        .sum::<usize>(),
                    2_000
                );
            }
            "oversized.txt" => {
                assert!(!diff.binary);
                assert!(diff.truncated);
                assert!(diff.hunks.is_empty());
            }
            _ => unreachable!(),
        }
    }
    assert!(workspace
        .file_diff(&info.id, &sha.to_string(), usize::MAX)
        .is_err());
    assert!(workspace.commit_details(&info.id, "invalid").is_err());
}

#[test]
fn cache_inside_the_source_is_rejected_before_any_directory_is_created() {
    let fixture = Fixture::new();
    for cache in [
        fixture.source.join("generated-cache"),
        fixture.source.join(".git/generated-cache"),
    ] {
        let before = snapshot(&fixture.source);
        let workspace = Workspace::new(&cache).unwrap();
        assert!(fixture.open_error(&workspace).contains("en dehors"));
        assert!(!cache.exists());
        assert_eq!(before, snapshot(&fixture.source));
    }
}

impl Fixture {
    fn open_error(&self, workspace: &Workspace) -> String {
        workspace
            .open_repository(self.source.to_str().unwrap())
            .unwrap_err()
    }
}

#[cfg(unix)]
#[test]
fn cache_symlinks_into_the_source_are_rejected() {
    let fixture = Fixture::new();
    let alias = fixture.root.path().join("source-alias");
    std::os::unix::fs::symlink(&fixture.source, &alias).unwrap();
    let cache = alias.join("generated-cache");
    let before = snapshot(&fixture.source);
    let workspace = Workspace::new(cache).unwrap();
    assert!(fixture.open_error(&workspace).contains("en dehors"));
    assert_eq!(before, snapshot(&fixture.source));
}

#[test]
fn linked_worktree_cannot_put_its_cache_in_the_main_working_tree() {
    let fixture = Fixture::new();
    commit(&fixture.repository, "first", &[], &[], Some("HEAD"));
    let linked = fixture.root.path().join("linked");
    fixture
        .repository
        .worktree("linked", &linked, None)
        .unwrap();
    let before = snapshot(&fixture.source);
    let workspace = Workspace::new(fixture.source.join("generated-cache")).unwrap();
    let error = workspace
        .open_repository(linked.to_str().unwrap())
        .unwrap_err();
    assert!(error.contains("en dehors"));
    assert_eq!(before, snapshot(&fixture.source));
}

#[test]
fn repeated_refresh_eviction_and_pagination_keep_only_the_current_generation() {
    let fixture = Fixture::new();
    let first = commit(&fixture.repository, "batch 0000", &[], &[], Some("HEAD"));
    let mut tip = first;
    for index in 1..600 {
        tip = commit(
            &fixture.repository,
            &format!("batch {index:04}"),
            &[tip],
            &[],
            Some("HEAD"),
        );
    }
    let before = snapshot(&fixture.source);
    let workspace = fixture.workspace();
    let initial = fixture.open(&workspace);
    for index in 0..20 {
        fixture
            .repository
            .reference(
                "refs/heads/moving",
                if index % 2 == 0 { first } else { tip },
                true,
                "test",
            )
            .unwrap();
        fixture.open(&workspace);
    }
    // Opening more than eight repos evicts the first session while its worker
    // may still be processing a batch; reopening must reuse the same write lock.
    for index in 0..9 {
        let path = fixture.root.path().join(format!("extra-{index}.git"));
        let repository = Repository::init_bare(&path).unwrap();
        commit(&repository, "extra", &[], &[], Some("HEAD"));
        workspace.open_repository(path.to_str().unwrap()).unwrap();
    }
    let current = fixture.open(&workspace);
    finish(&workspace, &current.id);
    assert!(workspace.index_status(&initial.id).is_err());
    let first_page = workspace
        .list_commits(&current.id, 0, usize::MAX, "")
        .unwrap();
    assert_eq!(first_page.total, 600);
    assert_eq!(first_page.index.indexed, 600);
    assert!(first_page.index.complete);
    assert_eq!(first_page.commits.len(), 256);
    assert_eq!(first_page.commits[0].sha, tip.to_string());
    let second_page = workspace
        .list_commits(&current.id, 256, 256, "batch")
        .unwrap();
    assert_eq!(second_page.total, 600);
    assert_eq!(second_page.commits.len(), 256);
    assert_eq!(second_page.commits[0].position, 256);
    assert!(workspace
        .list_commits(&current.id, 600, 256, "")
        .unwrap()
        .commits
        .is_empty());
    // Only the reference changes from the test fixture are allowed; processing
    // must still preserve the object database, working files, and Git config.
    let after = snapshot(&fixture.source);
    for (path, state) in before {
        if path.starts_with(".git/refs")
            || path.starts_with(".git/logs/refs")
            || path == Path::new(".git/logs")
        {
            continue;
        }
        assert_eq!(
            after.get(&path),
            Some(&state),
            "source changed: {}",
            path.display()
        );
    }
}

#[test]
fn another_workspace_cannot_make_an_old_session_read_the_new_cache_generation() {
    let fixture = Fixture::new();
    let first = commit(&fixture.repository, "first", &[], &[], Some("HEAD"));
    let first_workspace = fixture.workspace();
    let old = fixture.open(&first_workspace);
    finish(&first_workspace, &old.id);
    commit(&fixture.repository, "second", &[first], &[], Some("HEAD"));
    let second_workspace = fixture.workspace();
    let new = fixture.open(&second_workspace);
    finish(&second_workspace, &new.id);
    assert!(first_workspace.list_commits(&old.id, 0, 100, "").is_err());
    assert!(first_workspace
        .commit_position(&old.id, &first.to_string())
        .is_err());
    assert_eq!(
        second_workspace
            .list_commits(&new.id, 0, 100, "")
            .unwrap()
            .total,
        2
    );
    // Return to the first fingerprint: equality with the in-memory session must
    // not make it reusable after another Workspace replaced its persistent lease.
    fixture
        .repository
        .find_reference("refs/heads/main")
        .unwrap()
        .set_target(first, "restore first reference")
        .unwrap();
    let reopened = fixture.open(&first_workspace);
    assert_ne!(reopened.id, old.id);
    finish(&first_workspace, &reopened.id);
    let page = first_workspace
        .list_commits(&reopened.id, 0, 100, "")
        .unwrap();
    assert_eq!(page.total, 1);
    assert_eq!(page.commits[0].sha, first.to_string());
}

#[cfg(unix)]
fn assert_cache_file_aliases_preserve_source(create_alias: impl Fn(&Path, &Path)) {
    for suffix in ["", "-wal", "-shm", "-journal"] {
        let fixture = Fixture::new();
        let source_database = fixture.source.join("data.sqlite");
        {
            let connection = Connection::open(&source_database).unwrap();
            connection
                .execute_batch(
                    "CREATE TABLE valuable (value TEXT); INSERT INTO valuable VALUES ('original');",
                )
                .unwrap();
        }
        let bytes = fs::read(&source_database).unwrap();
        commit(
            &fixture.repository,
            "source database",
            &[],
            &[("data.sqlite", &bytes)],
            Some("HEAD"),
        );
        {
            let workspace = fixture.workspace();
            let info = fixture.open(&workspace);
            finish(&workspace, &info.id);
        }
        let database = cache_database(&fixture.cache);
        let mut alias = database.as_os_str().to_owned();
        alias.push(suffix);
        let alias = PathBuf::from(alias);
        if alias.exists() {
            fs::remove_file(&alias).unwrap();
        }
        create_alias(&source_database, &alias);
        let before = snapshot(&fixture.source);
        let workspace = fixture.workspace();
        assert!(fixture.open_error(&workspace).contains("lien"), "{suffix}");
        assert_eq!(before, snapshot(&fixture.source), "{suffix}");
    }
}

#[cfg(unix)]
#[test]
fn sqlite_and_sidecar_symlinks_cannot_redirect_cache_writes_into_the_source() {
    assert_cache_file_aliases_preserve_source(|source, alias| {
        std::os::unix::fs::symlink(source, alias).unwrap();
    });
}

#[cfg(unix)]
#[test]
fn sqlite_and_sidecar_hardlinks_cannot_redirect_cache_writes_into_the_source() {
    assert_cache_file_aliases_preserve_source(|source, alias| {
        fs::hard_link(source, alias).unwrap();
    });
}

#[test]
fn removing_shallow_boundaries_rebuilds_history_and_effective_parents_without_ref_changes() {
    let fixture = Fixture::new();
    let root = commit(&fixture.repository, "root", &[], &[], Some("HEAD"));
    let middle = commit(&fixture.repository, "middle", &[root], &[], Some("HEAD"));
    let tip = commit(&fixture.repository, "tip", &[middle], &[], Some("HEAD"));
    let shallow_file = fixture.repository.path().join("shallow");
    // Keeping all objects locally makes this test independent of Git/network,
    // while exercising libgit2's same boundary semantics as a depth=1 clone.
    fs::write(&shallow_file, format!("{tip}\n")).unwrap();
    let before = snapshot(&fixture.source);
    let workspace = fixture.workspace();
    let shallow = fixture.open(&workspace);
    finish(&workspace, &shallow.id);
    let page = workspace.list_commits(&shallow.id, 0, 100, "").unwrap();
    assert_eq!(page.total, 1);
    assert!(page.commits[0].parents.is_empty());
    assert_eq!(before, snapshot(&fixture.source));

    fs::remove_file(&shallow_file).unwrap();
    let before = snapshot(&fixture.source);
    let deep = fixture.open(&workspace);
    assert_ne!(deep.id, shallow.id);
    assert_eq!(deep.head, shallow.head);
    assert_eq!(
        serde_json::to_value(&deep.references).unwrap(),
        serde_json::to_value(&shallow.references).unwrap()
    );
    finish(&workspace, &deep.id);
    let page = workspace.list_commits(&deep.id, 0, 100, "").unwrap();
    assert_eq!(page.total, 3);
    assert_eq!(page.commits[0].parents, [middle.to_string()]);
    assert_eq!(page.commits[1].parents, [root.to_string()]);
    assert!(page.commits[2].parents.is_empty());
    assert_eq!(
        page.commits[0]
            .graph
            .edges
            .iter()
            .filter(|edge| edge.kind == "parent")
            .count(),
        1
    );
    assert_eq!(before, snapshot(&fixture.source));

    drop(workspace);
    let workspace = fixture.workspace();
    let warm = fixture.open(&workspace);
    assert!(warm.index.complete);
    assert_eq!(warm.index.indexed, 3);
    assert_eq!(
        workspace
            .list_commits(&warm.id, 0, 100, "")
            .unwrap()
            .commits[0]
            .parents,
        [middle.to_string()]
    );
}

#[test]
fn t3_checkpoint_refs_do_not_enter_history_search_or_trigger_refreshes() {
    let fixture = Fixture::new();
    let root = commit(&fixture.repository, "root", &[], &[], Some("HEAD"));
    let visible = commit(
        &fixture.repository,
        "T3 checkpoint visible on a real branch",
        &[root],
        &[],
        Some("HEAD"),
    );
    let hidden = commit(
        &fixture.repository,
        "technical-only-snapshot",
        &[root],
        &[],
        Some("refs/t3/checkpoints/session/turn/1"),
    );
    fixture
        .repository
        .reference("refs/t3/checkpoints/session/shared", visible, false, "test")
        .unwrap();
    fixture
        .repository
        .reference("refs/heads/t3/checkpoints-feature", visible, false, "test")
        .unwrap();
    let backup = commit(
        &fixture.repository,
        "neighboring backup commit",
        &[root],
        &[],
        Some("refs/t3/checkpoints-backup/session"),
    );
    let before = snapshot(&fixture.source);
    let workspace = fixture.workspace();
    let info = fixture.open(&workspace);
    finish(&workspace, &info.id);
    assert!(info
        .references
        .iter()
        .all(|reference| !reference.name.starts_with("refs/t3/checkpoints/")));
    assert!(info
        .references
        .iter()
        .any(|reference| reference.name == "refs/heads/t3/checkpoints-feature"));
    assert!(info
        .references
        .iter()
        .any(|reference| reference.name == "refs/t3/checkpoints-backup/session"));
    let page = workspace.list_commits(&info.id, 0, 100, "").unwrap();
    assert_eq!(page.total, 3);
    assert!(page
        .commits
        .iter()
        .any(|row| row.sha == visible.to_string()));
    assert!(page.commits.iter().any(|row| row.sha == backup.to_string()));
    assert!(page.commits.iter().all(|row| row.graph.width <= 2));
    for query in [
        "neighboring backup commit",
        "refs/t3/checkpoints-backup/session",
        &backup.to_string(),
    ] {
        let result = workspace.list_commits(&info.id, 0, 100, query).unwrap();
        assert_eq!(result.total, 1, "{query}");
        assert_eq!(result.commits[0].sha, backup.to_string(), "{query}");
    }
    for query in [
        "technical-only-snapshot",
        "refs/t3/checkpoints/session/turn/1",
        &hidden.to_string(),
    ] {
        assert_eq!(
            workspace
                .list_commits(&info.id, 0, 100, query)
                .unwrap()
                .total,
            0,
            "{query}"
        );
    }
    assert_eq!(
        workspace
            .list_commits(&info.id, 0, 100, "T3 checkpoint visible")
            .unwrap()
            .total,
        1
    );
    assert_eq!(
        workspace
            .commit_position(&info.id, &hidden.to_string())
            .unwrap(),
        None
    );
    assert_eq!(before, snapshot(&fixture.source));
    let database = cache_database(&fixture.cache);
    let connection = Connection::open(&database).unwrap();
    let run: String = connection
        .query_row(
            "SELECT value FROM meta WHERE key = 'index_run'",
            [],
            |row| row.get(0),
        )
        .unwrap();

    commit(
        &fixture.repository,
        "technical-only-new-snapshot",
        &[hidden],
        &[],
        Some("refs/t3/checkpoints/session/turn/2"),
    );
    fixture
        .repository
        .find_reference("refs/t3/checkpoints/session/turn/1")
        .unwrap()
        .delete()
        .unwrap();
    let before = snapshot(&fixture.source);
    let same = fixture.open(&workspace);
    assert_eq!(same.id, info.id);
    assert!(same.index.complete);
    assert_eq!(same.index.indexed, 3);
    assert_eq!(
        run,
        connection
            .query_row::<String, _, _>(
                "SELECT value FROM meta WHERE key = 'index_run'",
                [],
                |row| row.get(0)
            )
            .unwrap()
    );
    assert_eq!(before, snapshot(&fixture.source));
}

#[test]
fn legacy_checkpoint_cache_is_rebuilt_without_exposing_retained_fts_metadata() {
    let fixture = Fixture::new();
    let root = commit(&fixture.repository, "root", &[], &[], Some("HEAD"));
    let main = commit(
        &fixture.repository,
        "main change",
        &[root],
        &[],
        Some("HEAD"),
    );
    let checkpoint = commit(
        &fixture.repository,
        "legacy-only-checkpoint",
        &[root],
        &[],
        Some("refs/heads/legacy-checkpoint-cache"),
    );
    let legacy = {
        // Load the same three commits the old engine would have reached from a
        // technical ref, exercising its real persistence and graph creation.
        let workspace = fixture.workspace();
        let info = fixture.open(&workspace);
        finish(&workspace, &info.id);
        assert_eq!(
            workspace.list_commits(&info.id, 0, 100, "").unwrap().total,
            3
        );
        info
    };
    fixture
        .repository
        .find_reference("refs/heads/legacy-checkpoint-cache")
        .unwrap()
        .delete()
        .unwrap();
    let technical = fixture
        .repository
        .reference(
            "refs/t3/checkpoints/session/turn/1",
            checkpoint,
            false,
            "test",
        )
        .unwrap();
    let mut references: Vec<_> = legacy
        .references
        .iter()
        .filter(|reference| reference.name != "refs/heads/legacy-checkpoint-cache")
        .cloned()
        .collect();
    let technical = Reference {
        name: technical.name().unwrap().into(),
        label: technical.shorthand().unwrap().into(),
        kind: "other".into(),
        sha: checkpoint.to_string(),
    };
    references.push(technical.clone());
    references.sort_by(|left, right| left.name.cmp(&right.name));
    let fingerprint = format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_vec(&(
                3u32,
                &legacy.head,
                &legacy.head_name,
                &references,
                &legacy.remotes,
                vec![Vec::<u8>::new()]
            ))
            .unwrap()
        )
    );
    let database = cache_database(&fixture.cache);
    let connection = Connection::open(&database).unwrap();
    connection
        .execute(
            "UPDATE meta SET value = ?1 WHERE key = 'fingerprint'",
            [&fingerprint],
        )
        .unwrap();
    connection
        .execute(
            "DELETE FROM refs WHERE name = 'refs/heads/legacy-checkpoint-cache'",
            [],
        )
        .unwrap();
    connection
        .execute(
            "INSERT INTO refs(name, label, sha) VALUES(?1, ?2, ?3)",
            rusqlite::params![technical.name, technical.label, technical.sha],
        )
        .unwrap();
    assert_eq!(
        connection
            .query_row::<i64, _, _>("SELECT COUNT(*) FROM walk", [], |row| row.get(0))
            .unwrap(),
        3
    );
    assert_eq!(
        connection
            .query_row::<i64, _, _>(
                "SELECT COUNT(*) FROM search WHERE search MATCH 'legacy'",
                [],
                |row| row.get(0)
            )
            .unwrap(),
        1
    );

    let before = snapshot(&fixture.source);
    let workspace = fixture.workspace();
    let info = fixture.open(&workspace);
    assert!(!info.index.complete);
    finish(&workspace, &info.id);
    let page = workspace.list_commits(&info.id, 0, 100, "").unwrap();
    assert_eq!(page.total, 2);
    assert_eq!(page.commits[0].sha, main.to_string());
    assert!(page.commits.iter().all(|row| row.graph.width == 1));
    assert!(info
        .references
        .iter()
        .all(|reference| !reference.name.starts_with("refs/t3/checkpoints/")));
    for query in [
        "legacy-only-checkpoint",
        "refs/t3/checkpoints/session/turn/1",
        &checkpoint.to_string(),
    ] {
        assert_eq!(
            workspace
                .list_commits(&info.id, 0, 100, query)
                .unwrap()
                .total,
            0,
            "{query}"
        );
    }
    assert_eq!(
        workspace
            .commit_position(&info.id, &checkpoint.to_string())
            .unwrap(),
        None
    );
    assert_eq!(
        connection
            .query_row::<i64, _, _>(
                "SELECT COUNT(*) FROM search WHERE search MATCH 'legacy'",
                [],
                |row| row.get(0)
            )
            .unwrap(),
        1
    );
    assert_eq!(
        connection
            .query_row::<i64, _, _>(
                "SELECT COUNT(*) FROM refs WHERE name LIKE 'refs/t3/checkpoints/%'",
                [],
                |row| row.get(0)
            )
            .unwrap(),
        0
    );
    assert_eq!(before, snapshot(&fixture.source));
}

fn working_key(path: &str) -> String {
    path.as_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn checkout_head(fixture: &Fixture) {
    let mut options = git2::build::CheckoutBuilder::new();
    options.force();
    fixture
        .repository
        .checkout_head(Some(&mut options))
        .unwrap();
}

fn patch_text(diff: &mangrove_core::model::FileDiff) -> String {
    diff.hunks
        .iter()
        .flat_map(|hunk| &hunk.lines)
        .map(|line| format!("{}:{}", line.kind, line.content))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn working_tree_separates_staged_unstaged_untracked_renames_and_preserves_the_source() {
    let fixture = Fixture::new();
    commit(
        &fixture.repository,
        "baseline",
        &[],
        &[
            ("both.txt", b"initial\n"),
            ("deleted.txt", b"deleted baseline\n"),
            ("staged-old.txt", b"staged rename baseline\n"),
            ("unstaged-old.txt", b"unstaged rename baseline\n"),
            ("binary.dat", b"\0\x01"),
            (".gitignore", b"ignored.txt\n"),
        ],
        Some("HEAD"),
    );
    checkout_head(&fixture);
    fs::write(fixture.source.join("both.txt"), b"staged\n").unwrap();
    fs::rename(
        fixture.source.join("staged-old.txt"),
        fixture.source.join("staged-new.txt"),
    )
    .unwrap();
    let mut index = fixture.repository.index().unwrap();
    index.add_path(Path::new("both.txt")).unwrap();
    index.remove_path(Path::new("staged-old.txt")).unwrap();
    index.add_path(Path::new("staged-new.txt")).unwrap();
    index.write().unwrap();
    fs::write(fixture.source.join("both.txt"), b"worktree\n").unwrap();
    fs::write(fixture.source.join("binary.dat"), b"\0\x02\x03").unwrap();
    fs::write(
        fixture.source.join("untracked.txt"),
        b"untracked addition\n",
    )
    .unwrap();
    fs::write(fixture.source.join("ignored.txt"), b"ignored content\n").unwrap();
    fs::remove_file(fixture.source.join("deleted.txt")).unwrap();
    fs::rename(
        fixture.source.join("unstaged-old.txt"),
        fixture.source.join("unstaged-new.txt"),
    )
    .unwrap();
    let before = snapshot(&fixture.source);
    let workspace = fixture.workspace();
    let info = fixture.open(&workspace);
    finish(&workspace, &info.id);
    let status = workspace.working_tree_status(&info.id).unwrap();
    assert!(status.available);
    assert_eq!(status.staged_count, 2);
    assert_eq!(status.unstaged_count, 5);
    for (files, path, expected, old) in [
        (&status.staged, "both.txt", "modified", None),
        (
            &status.staged,
            "staged-new.txt",
            "renamed",
            Some("staged-old.txt"),
        ),
        (&status.unstaged, "both.txt", "modified", None),
        (&status.unstaged, "deleted.txt", "deleted", None),
        (
            &status.unstaged,
            "unstaged-new.txt",
            "renamed",
            Some("unstaged-old.txt"),
        ),
        (&status.unstaged, "untracked.txt", "added", None),
    ] {
        let file = files.iter().find(|file| file.file.path == path).unwrap();
        assert_eq!(file.file.status, expected, "{path}");
        assert_eq!(file.file.old_path.as_deref(), old, "{path}");
    }
    assert!(status
        .unstaged
        .iter()
        .all(|file| file.file.path != "ignored.txt"));
    let staged = workspace
        .working_tree_diff(&info.id, "staged", &working_key("both.txt"))
        .unwrap();
    let unstaged = workspace
        .working_tree_diff(&info.id, "unstaged", &working_key("both.txt"))
        .unwrap();
    assert!(patch_text(&staged).contains("deletion:initial"));
    assert!(patch_text(&staged).contains("addition:staged"));
    assert!(patch_text(&unstaged).contains("deletion:staged"));
    assert!(patch_text(&unstaged).contains("addition:worktree"));
    assert!(
        workspace
            .working_tree_diff(&info.id, "unstaged", &working_key("binary.dat"))
            .unwrap()
            .binary
    );
    assert!(patch_text(
        &workspace
            .working_tree_diff(&info.id, "unstaged", &working_key("deleted.txt"))
            .unwrap()
    )
    .contains("deletion:deleted baseline"));
    assert!(patch_text(
        &workspace
            .working_tree_diff(&info.id, "unstaged", &working_key("untracked.txt"))
            .unwrap()
    )
    .contains("addition:untracked addition"));
    for (area, files) in [("staged", &status.staged), ("unstaged", &status.unstaged)] {
        for file in files {
            workspace
                .working_tree_diff(&info.id, area, &file.key)
                .unwrap();
        }
    }
    assert_eq!(before, snapshot(&fixture.source));

    // Adding an earlier-sorted entry must not redirect a selection by index.
    fs::write(fixture.source.join("aaa-new.txt"), b"different file\n").unwrap();
    let before = snapshot(&fixture.source);
    assert!(patch_text(
        &workspace
            .working_tree_diff(&info.id, "unstaged", &working_key("untracked.txt"))
            .unwrap()
    )
    .contains("addition:untracked addition"));
    assert_eq!(before, snapshot(&fixture.source));
}

#[test]
fn working_tree_supports_unborn_branches_and_reports_bare_repositories_unavailable() {
    let fixture = Fixture::new();
    fs::write(fixture.source.join("first.txt"), b"staged first file\n").unwrap();
    let mut index = fixture.repository.index().unwrap();
    index.add_path(Path::new("first.txt")).unwrap();
    index.write().unwrap();
    fs::write(fixture.source.join("first.txt"), b"unstaged first file\n").unwrap();
    fs::write(fixture.source.join("new.txt"), b"untracked first file\n").unwrap();
    let before = snapshot(&fixture.source);
    let workspace = fixture.workspace();
    let info = fixture.open(&workspace);
    let status = workspace.working_tree_status(&info.id).unwrap();
    assert!(status.available);
    assert_eq!(status.staged_count, 1);
    assert_eq!(status.unstaged_count, 2);
    assert_eq!(status.staged[0].file.status, "added");
    assert!(patch_text(
        &workspace
            .working_tree_diff(&info.id, "staged", &working_key("first.txt"))
            .unwrap()
    )
    .contains("addition:staged first file"));
    assert!(patch_text(
        &workspace
            .working_tree_diff(&info.id, "unstaged", &working_key("first.txt"))
            .unwrap()
    )
    .contains("addition:unstaged first file"));
    assert_eq!(before, snapshot(&fixture.source));
    let bare_path = fixture.root.path().join("bare.git");
    let bare = Repository::init_bare(&bare_path).unwrap();
    commit(&bare, "bare", &[], &[], Some("HEAD"));
    let before = snapshot(&bare_path);
    let info = workspace
        .open_repository(bare_path.to_str().unwrap())
        .unwrap();
    let status = workspace.working_tree_status(&info.id).unwrap();
    assert!(!status.available);
    assert!(status.staged.is_empty());
    assert!(status.unstaged.is_empty());
    assert_eq!((status.staged_count, status.unstaged_count), (0, 0));
    assert!(workspace
        .working_tree_diff(&info.id, "staged", &working_key("first.txt"))
        .is_err());
    assert_eq!(before, snapshot(&bare_path));
}

#[test]
fn working_tree_patches_bound_large_content_and_reject_invalid_areas_and_paths() {
    let fixture = Fixture::new();
    let workspace = fixture.workspace();
    let info = fixture.open(&workspace);
    let oversized = vec![b'x'; 2 * 1024 * 1024 + 1];
    fs::write(fixture.source.join("large.txt"), &oversized).unwrap();
    fs::write(
        fixture.source.join("many.txt"),
        (0..3000)
            .map(|number| format!("line {number}\n"))
            .collect::<String>(),
    )
    .unwrap();
    let mut index = fixture.repository.index().unwrap();
    index.add_path(Path::new("large.txt")).unwrap();
    index.write().unwrap();
    fs::write(
        fixture.source.join("large.txt"),
        vec![b'y'; oversized.len()],
    )
    .unwrap();
    let before = snapshot(&fixture.source);
    for area in ["staged", "unstaged"] {
        let diff = workspace
            .working_tree_diff(&info.id, area, &working_key("large.txt"))
            .unwrap();
        assert!(diff.truncated);
        assert!(!diff.binary);
        assert!(diff.hunks.is_empty());
    }
    let diff = workspace
        .working_tree_diff(&info.id, "unstaged", &working_key("many.txt"))
        .unwrap();
    assert!(diff.truncated);
    assert_eq!(
        diff.hunks
            .iter()
            .map(|hunk| hunk.lines.len())
            .sum::<usize>(),
        2000
    );
    assert!(workspace
        .working_tree_diff(&info.id, "push", &working_key("large.txt"))
        .is_err());
    for path in [
        "",
        ".",
        "..",
        "../secret",
        "/etc/passwd",
        "directory/../secret",
        "secret\0.txt",
    ] {
        for area in ["staged", "unstaged"] {
            assert!(
                workspace
                    .working_tree_diff(&info.id, area, &working_key(path))
                    .is_err(),
                "{path:?}"
            );
        }
    }
    assert!(workspace
        .working_tree_diff(&info.id, "unstaged", &working_key("absent.txt"))
        .is_err());
    assert_eq!(before, snapshot(&fixture.source));
}

#[cfg(unix)]
#[test]
fn working_tree_symlinks_special_files_and_gitlinks_do_not_expose_external_contents() {
    use std::os::unix::{fs::symlink, net::UnixListener};
    let fixture = Fixture::new();
    let root = commit(
        &fixture.repository,
        "root",
        &[],
        &[("tracked.txt", b"baseline\n")],
        Some("HEAD"),
    );
    checkout_head(&fixture);
    let outside = fixture.root.path().join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(
        outside.join("secret.txt"),
        "EXTERNAL SECRET MUST NOT BE READ\n",
    )
    .unwrap();
    symlink(outside.join("secret.txt"), fixture.source.join("link.txt")).unwrap();
    symlink(&outside, fixture.source.join("escape")).unwrap();
    fs::remove_file(fixture.source.join("tracked.txt")).unwrap();
    symlink(
        outside.join("secret.txt"),
        fixture.source.join("tracked.txt"),
    )
    .unwrap();
    let _socket = UnixListener::bind(fixture.source.join("socket")).unwrap();
    let mut index = fixture.repository.index().unwrap();
    let mut gitlink = index.get_path(Path::new("tracked.txt"), 0).unwrap();
    gitlink.path = b"vendor".to_vec();
    gitlink.mode = 0o160000;
    gitlink.id = root;
    index.add(&gitlink).unwrap();
    index.write().unwrap();
    symlink(&outside, fixture.source.join("vendor")).unwrap();
    fixture
        .repository
        .config()
        .unwrap()
        .set_bool("core.symlinks", false)
        .unwrap();
    let before = snapshot(&fixture.source);
    let before_outside = snapshot(&outside);
    let workspace = fixture.workspace();
    let info = fixture.open(&workspace);
    let status = workspace.working_tree_status(&info.id).unwrap();
    assert!(status
        .unstaged
        .iter()
        .any(|file| file.file.path == "tracked.txt" && file.file.status == "typechange"));
    assert!(status
        .unstaged
        .iter()
        .all(|file| !file.file.path.starts_with("escape/")));
    for path in ["link.txt", "tracked.txt"] {
        let diff = workspace
            .working_tree_diff(&info.id, "unstaged", &working_key(path))
            .unwrap();
        assert!(!patch_text(&diff).contains("EXTERNAL SECRET MUST NOT BE READ"));
        assert!(patch_text(&diff).contains("secret.txt"), "{path} {diff:?}");
    }
    let gitlink = workspace
        .working_tree_diff(&info.id, "staged", &working_key("vendor"))
        .unwrap();
    assert!(patch_text(&gitlink).contains("Subproject commit"));
    assert!(!patch_text(&gitlink).contains("EXTERNAL SECRET MUST NOT BE READ"));
    for path in ["escape/secret.txt", "socket", "vendor/secret.txt"] {
        assert!(
            workspace
                .working_tree_diff(&info.id, "unstaged", &working_key(path))
                .is_err(),
            "{path}"
        );
    }
    assert_eq!(before, snapshot(&fixture.source));
    assert_eq!(before_outside, snapshot(&outside));
}

#[test]
fn working_tree_conflicts_are_reported_without_modifying_the_index_or_worktree() {
    let fixture = Fixture::new();
    commit(
        &fixture.repository,
        "baseline",
        &[],
        &[("conflict.txt", b"ancestor\n")],
        Some("HEAD"),
    );
    checkout_head(&fixture);
    let mut index = fixture.repository.index().unwrap();
    let base = index.get_path(Path::new("conflict.txt"), 0).unwrap();
    index.remove_path(Path::new("conflict.txt")).unwrap();
    for (stage, content) in [
        (1, b"ancestor\n".as_slice()),
        (2, b"ours\n".as_slice()),
        (3, b"theirs\n".as_slice()),
    ] {
        let mut entry = git2::IndexEntry {
            id: fixture.repository.blob(content).unwrap(),
            flags: (base.flags & !0x3000) | (stage << 12),
            path: base.path.clone(),
            ..base
        };
        entry.file_size = content.len() as u32;
        index.add(&entry).unwrap();
    }
    index.write().unwrap();
    assert!(index.has_conflicts());
    fs::write(
        fixture.source.join("conflict.txt"),
        "<<<<<<< ours\nours\n=======\ntheirs\n>>>>>>> theirs\n",
    )
    .unwrap();
    let before = snapshot(&fixture.source);
    let workspace = fixture.workspace();
    let info = fixture.open(&workspace);
    let status = workspace.working_tree_status(&info.id).unwrap();
    assert!(status
        .staged
        .iter()
        .chain(&status.unstaged)
        .any(|file| file.file.path == "conflict.txt"));
    for area in ["staged", "unstaged"] {
        assert!(workspace
            .working_tree_diff(&info.id, area, &working_key("conflict.txt"))
            .unwrap_err()
            .contains("conflit"));
    }
    assert_eq!(before, snapshot(&fixture.source));
}

#[cfg(unix)]
#[test]
fn working_tree_unreadable_files_fail_without_refreshing_index_stats() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new();
    commit(
        &fixture.repository,
        "baseline",
        &[],
        &[("private.txt", b"initial\n")],
        Some("HEAD"),
    );
    checkout_head(&fixture);
    let file = fixture.source.join("private.txt");
    fs::write(&file, b"unreadable changes\n").unwrap();
    let before = snapshot(&fixture.source);
    let permissions = fs::metadata(&file).unwrap().permissions();
    fs::set_permissions(&file, fs::Permissions::from_mode(0o0)).unwrap();
    let workspace = fixture.workspace();
    let info = fixture.open(&workspace);
    let status = workspace.working_tree_status(&info.id).unwrap();
    if fs::read(&file).is_err() {
        assert!(status
            .unstaged
            .iter()
            .any(|entry| entry.file.path == "private.txt"));
        assert!(workspace
            .working_tree_diff(&info.id, "unstaged", &working_key("private.txt"))
            .is_err());
    }
    fs::set_permissions(&file, permissions).unwrap();
    assert_eq!(before, snapshot(&fixture.source));
}

#[test]
fn working_tree_status_reports_full_counts_with_bounded_file_lists() {
    let fixture = Fixture::new();
    for number in 0..5_001 {
        fs::write(
            fixture.source.join(format!("untracked-{number:04}.txt")),
            b"content\n",
        )
        .unwrap();
    }
    let before = snapshot(&fixture.source);
    let workspace = fixture.workspace();
    let info = fixture.open(&workspace);
    let status = workspace.working_tree_status(&info.id).unwrap();
    assert_eq!(status.unstaged_count, 5_001);
    assert_eq!(status.unstaged.len(), 5_000);
    assert_eq!(status.staged_count, 0);
    assert!(status.staged.is_empty());
    let json = serde_json::to_value(status).unwrap();
    assert_eq!(json["unstagedCount"], 5_001);
    assert_eq!(json["stagedCount"], 0);
    assert_eq!(before, snapshot(&fixture.source));
}

#[cfg(unix)]
#[test]
fn working_tree_keys_distinguish_non_utf8_names_and_unicode_display_collisions() {
    use std::{ffi::OsStr, os::unix::ffi::OsStrExt};
    let fixture = Fixture::new();
    let names: [(&[u8], &[u8], &[u8]); 3] = [
        (b"f\xff.txt", b"raw index\n", b"raw worktree\n"),
        (
            "f�.txt".as_bytes(),
            b"replacement index\n",
            b"replacement worktree\n",
        ),
        (
            "文件.txt".as_bytes(),
            b"unicode index\n",
            b"unicode worktree\n",
        ),
    ];
    let mut index = fixture.repository.index().unwrap();
    // Some filesystems (including APFS) reject non-UTF8 names. Git's index
    // still supports them; use an index-only name with a deletion in worktree.
    for (name, staged, _) in &names[1..] {
        let path = Path::new(OsStr::from_bytes(name));
        fs::write(fixture.source.join(path), staged).unwrap();
        index.add_path(path).unwrap();
    }
    let mut raw = index.get_path(Path::new("f�.txt"), 0).unwrap();
    raw.path = names[0].0.to_vec();
    raw.id = fixture.repository.blob(names[0].1).unwrap();
    raw.file_size = names[0].1.len() as u32;
    index.add(&raw).unwrap();
    index.write().unwrap();
    for (name, _, unstaged) in &names[1..] {
        fs::write(fixture.source.join(OsStr::from_bytes(name)), unstaged).unwrap();
    }
    let before = snapshot(&fixture.source);
    let workspace = fixture.workspace();
    let info = fixture.open(&workspace);
    let status = workspace.working_tree_status(&info.id).unwrap();
    assert_eq!((status.staged_count, status.unstaged_count), (3, 3));
    for (area, files) in [("staged", &status.staged), ("unstaged", &status.unstaged)] {
        let raw = files
            .iter()
            .find(|file| file.key == "66ff2e747874")
            .unwrap();
        let replacement = files
            .iter()
            .find(|file| file.key == "66efbfbd2e747874")
            .unwrap();
        assert_eq!(raw.file.path, replacement.file.path);
        assert_ne!(raw.key, replacement.key);
        for (name, staged, unstaged) in names {
            let expected_key: String = name.iter().map(|byte| format!("{byte:02x}")).collect();
            let file = files.iter().find(|file| file.key == expected_key).unwrap();
            let diff = workspace
                .working_tree_diff(&info.id, area, &file.key)
                .unwrap();
            let expected = if area == "staged" || name == b"f\xff.txt" {
                staged
            } else {
                unstaged
            };
            assert!(
                patch_text(&diff).contains(std::str::from_utf8(expected).unwrap().trim()),
                "{area} {file:?}"
            );
            if file.key == raw.key {
                assert!(!patch_text(&diff).contains("replacement"));
            }
        }
    }
    let json = serde_json::to_value(&status).unwrap();
    assert!(json["staged"][0]["key"].is_string());
    assert!(json["staged"][0]["path"].is_string());
    assert!(json["staged"][0].get("file").is_none());
    // Display paths and malformed strings are never accepted as identities.
    for key in ["f�.txt", "66f", "66zz", ""] {
        assert!(workspace
            .working_tree_diff(&info.id, "unstaged", key)
            .is_err());
    }
    assert_eq!(before, snapshot(&fixture.source));
}

#[cfg(unix)]
#[test]
fn working_tree_patch_races_never_follow_replaced_leaves_or_parent_directories() {
    use std::{
        os::unix::fs::symlink,
        sync::{atomic::AtomicBool, Arc, Barrier},
    };
    for replace_parent in [false, true] {
        let fixture = Fixture::new();
        let directory = fixture.source.join("nested");
        fs::create_dir(&directory).unwrap();
        let file = directory.join("target.txt");
        fs::write(&file, b"index baseline\n").unwrap();
        let mut index = fixture.repository.index().unwrap();
        index.add_path(Path::new("nested/target.txt")).unwrap();
        index.write().unwrap();
        fs::write(&file, b"safe current content\n").unwrap();
        fixture
            .repository
            .config()
            .unwrap()
            .set_bool("core.symlinks", false)
            .unwrap();
        let outside = fixture.root.path().join("outside");
        fs::create_dir(&outside).unwrap();
        let secret = outside.join("target.txt");
        fs::write(&secret, b"EXTERNAL RACE SECRET MUST NEVER APPEAR\n").unwrap();
        let outside_before = snapshot(&outside);
        let index_before = fs::read(fixture.repository.path().join("index")).unwrap();
        let workspace = fixture.workspace();
        let info = fixture.open(&workspace);
        let status = workspace.working_tree_status(&info.id).unwrap();
        let key = status
            .unstaged
            .iter()
            .find(|entry| entry.file.path == "nested/target.txt")
            .unwrap()
            .key
            .clone();
        let stop = Arc::new(AtomicBool::new(false));
        let barrier = Arc::new(Barrier::new(2));
        let worker_stop = Arc::clone(&stop);
        let worker_barrier = Arc::clone(&barrier);
        let temporary = fixture.root.path().join("replacement");
        let worker = thread::spawn(move || {
            worker_barrier.wait();
            while !worker_stop.load(Ordering::Relaxed) {
                if replace_parent {
                    fs::rename(&directory, &temporary).unwrap();
                    symlink(&outside, &directory).unwrap();
                    thread::yield_now();
                    fs::remove_file(&directory).unwrap();
                    fs::rename(&temporary, &directory).unwrap();
                } else {
                    symlink(&secret, &temporary).unwrap();
                    fs::rename(&temporary, &file).unwrap();
                    thread::yield_now();
                    fs::write(&temporary, b"safe current content\n").unwrap();
                    fs::rename(&temporary, &file).unwrap();
                }
            }
        });
        barrier.wait();
        let mut exposed_secret = false;
        for _ in 0..200 {
            // Concurrent replacements may fail the scan/read safely. Any patch
            // which succeeds must contain only the pinned file or link text.
            if let Ok(diff) = workspace.working_tree_diff(&info.id, "unstaged", &key) {
                exposed_secret |= patch_text(&diff).contains("EXTERNAL RACE SECRET");
            }
        }
        stop.store(true, Ordering::Relaxed);
        worker.join().unwrap();
        assert!(!exposed_secret, "replace_parent={replace_parent}");
        let diff = workspace
            .working_tree_diff(&info.id, "unstaged", &key)
            .unwrap();
        assert!(patch_text(&diff).contains("safe current content"));
        assert_eq!(
            index_before,
            fs::read(fixture.repository.path().join("index")).unwrap()
        );
        assert_eq!(
            outside_before,
            snapshot(&fixture.root.path().join("outside"))
        );
    }
}

fn git_at(path: &Path, args: &[&str]) {
    let output = std::process::Command::new("git")
        .current_dir(path)
        .args(args)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_COMMON_DIR")
        .env(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        )
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.test")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.test")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn separate_git_directory_keeps_the_selected_worktree_and_cache_guards() {
    let root = tempfile::tempdir().unwrap();
    let worktree = root.path().join("worktree");
    let administration = root.path().join("administration");
    git_at(
        root.path(),
        &[
            "init",
            "--separate-git-dir",
            administration.to_str().unwrap(),
            worktree.to_str().unwrap(),
        ],
    );
    let repository = Repository::open(&worktree).unwrap();
    repository.set_head("refs/heads/main").unwrap();
    commit(
        &repository,
        "initial",
        &[],
        &[("tracked.txt", b"before\n")],
        Some("HEAD"),
    );
    repository
        .checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
        .unwrap();
    fs::write(worktree.join("tracked.txt"), "after\n").unwrap();
    fs::write(root.path().join("outside-secret.txt"), "EXTERNAL SECRET").unwrap();
    let cache = tempfile::tempdir().unwrap();
    let workspace = Workspace::new(cache.path()).unwrap();
    // Search from a subdirectory must still preserve the .git indirection.
    fs::create_dir(worktree.join("nested")).unwrap();
    let before = snapshot(root.path());
    let info = workspace
        .open_repository(worktree.join("nested").to_str().unwrap())
        .unwrap();
    finish(&workspace, &info.id);
    assert_eq!(PathBuf::from(&info.path), worktree.canonicalize().unwrap());
    let status = workspace.working_tree_status(&info.id).unwrap();
    assert_eq!(status.unstaged.len(), 1);
    assert_eq!(status.unstaged[0].file.path, "tracked.txt");
    assert!(patch_text(
        &workspace
            .working_tree_diff(&info.id, "unstaged", &status.unstaged[0].key)
            .unwrap()
    )
    .contains("after"));
    assert!(workspace
        .commit_details(&info.id, info.head.as_ref().unwrap())
        .is_ok());
    assert_eq!(before, snapshot(root.path()));
    assert!(Workspace::new(worktree.join("cache"))
        .unwrap()
        .open_repository(worktree.to_str().unwrap())
        .unwrap_err()
        .contains("en dehors"));
}

#[test]
fn operational_refs_are_not_history_roots_or_refresh_triggers() {
    let fixture = Fixture::new();
    let root = commit(&fixture.repository, "visible", &[], &[], Some("HEAD"));
    for name in [
        "refs/stash",
        "refs/notes/commits",
        "refs/prefetch/remotes/origin/main",
    ] {
        commit(
            &fixture.repository,
            "operational-only",
            &[root],
            &[],
            Some(name),
        );
    }
    let workspace = fixture.workspace();
    let info = fixture.open(&workspace);
    finish(&workspace, &info.id);
    assert_eq!(
        workspace.list_commits(&info.id, 0, 100, "").unwrap().total,
        1
    );
    assert_eq!(
        workspace
            .list_commits(&info.id, 0, 100, "operational-only")
            .unwrap()
            .total,
        0
    );
    let old = fixture
        .repository
        .find_reference("refs/stash")
        .unwrap()
        .target()
        .unwrap();
    commit(
        &fixture.repository,
        "new-stash",
        &[old],
        &[],
        Some("refs/stash"),
    );
    assert_eq!(fixture.open(&workspace).id, info.id);
}

#[test]
fn removing_the_persistent_cache_recovers_on_reopen() {
    let fixture = Fixture::new();
    commit(&fixture.repository, "cached", &[], &[], Some("HEAD"));
    let workspace = fixture.workspace();
    let info = fixture.open(&workspace);
    finish(&workspace, &info.id);
    fs::remove_dir_all(&fixture.cache).unwrap();
    let reopened = fixture.open(&workspace);
    assert_ne!(info.id, reopened.id);
    finish(&workspace, &reopened.id);
    assert_eq!(
        workspace
            .list_commits(&reopened.id, 0, 100, "")
            .unwrap()
            .total,
        1
    );
}

#[test]
fn punctuation_only_search_terms_do_not_hide_matching_commits() {
    let fixture = Fixture::new();
    commit(
        &fixture.repository,
        "Release 1.2 - hotfix",
        &[],
        &[],
        Some("HEAD"),
    );
    let workspace = fixture.workspace();
    let info = fixture.open(&workspace);
    finish(&workspace, &info.id);
    for query in [
        "Release 1.2 - hotfix",
        "1.2 — hotfix",
        "hotfix !",
        "hotfix /",
        "— hotfix |",
    ] {
        assert_eq!(
            workspace
                .list_commits(&info.id, 0, 100, query)
                .unwrap()
                .total,
            1,
            "{query}"
        );
    }
    assert_eq!(
        workspace.list_commits(&info.id, 0, 100, "!").unwrap().total,
        0
    );
}

#[test]
fn non_utf8_references_keep_distinct_identity_and_readable_labels() {
    let fixture = Fixture::new();
    let oid = commit(&fixture.repository, "initial", &[], &[], Some("HEAD"));
    let mut packed = Vec::new();
    for name in [b"refs/heads/caf\xe9".as_slice(), b"refs/heads/caf\xff"] {
        packed.extend_from_slice(format!("{oid} ").as_bytes());
        packed.extend_from_slice(name);
        packed.push(b'\n');
    }
    fs::write(fixture.repository.path().join("packed-refs"), packed).unwrap();
    let workspace = fixture.workspace();
    let info = fixture.open(&workspace);
    finish(&workspace, &info.id);
    assert_eq!(info.references.len(), 3);
    let keys: std::collections::HashSet<_> = info
        .references
        .iter()
        .map(|reference| &reference.name)
        .collect();
    assert_eq!(keys.len(), 3);
    assert!(info
        .references
        .iter()
        .all(|reference| !reference.label.is_empty()));
}

#[test]
fn missing_final_newlines_are_annotations_without_line_numbers() {
    let fixture = Fixture::new();
    let first = commit(
        &fixture.repository,
        "before",
        &[],
        &[("a.ts", b"const a = 1;\nconst b = 2;")],
        Some("HEAD"),
    );
    let second = commit(
        &fixture.repository,
        "after",
        &[first],
        &[("a.ts", b"const a = 1;\nconst b = 3;")],
        Some("HEAD"),
    );
    let workspace = fixture.workspace();
    let info = fixture.open(&workspace);
    let patch = workspace
        .file_diff(&info.id, &second.to_string(), 0)
        .unwrap();
    let annotations: Vec<_> = patch
        .hunks
        .iter()
        .flat_map(|hunk| &hunk.lines)
        .filter(|line| line.kind == "annotation")
        .collect();
    assert_eq!(annotations.len(), 2);
    assert!(annotations.iter().all(|line| line.old_line.is_none()
        && line.new_line.is_none()
        && line.content == "\\ No newline at end of file"));
}

#[test]
fn local_diffs_respect_clean_filters_binary_attributes_and_index_flags() {
    let fixture = Fixture::new();
    commit(
        &fixture.repository,
        "before",
        &[],
        &[
            ("a.txt", b"one\ntwo\nthree\n"),
            ("secret.lock", b"secret before\n"),
            ("excluded.txt", b"sparse\n"),
        ],
        Some("HEAD"),
    );
    checkout_head(&fixture);
    git_at(&fixture.source, &["config", "core.autocrlf", "true"]);
    fs::write(fixture.source.join("a.txt"), "one\r\nTWO\r\nthree\r\n").unwrap();
    fs::write(fixture.source.join(".gitattributes"), "secret.lock -diff\n").unwrap();
    fs::write(fixture.source.join("secret.lock"), "secret after\n").unwrap();
    fs::write(fixture.source.join("intent.txt"), "intent\n").unwrap();
    git_at(&fixture.source, &["add", "-N", "intent.txt"]);
    git_at(
        &fixture.source,
        &["update-index", "--skip-worktree", "excluded.txt"],
    );
    fs::remove_file(fixture.source.join("excluded.txt")).unwrap();
    let before = snapshot(&fixture.source);
    let workspace = fixture.workspace();
    let info = fixture.open(&workspace);
    let status = workspace.working_tree_status(&info.id).unwrap();
    assert!(!status
        .staged
        .iter()
        .any(|file| file.file.path == "intent.txt"));
    assert!(!status
        .unstaged
        .iter()
        .any(|file| file.file.path == "excluded.txt"));
    for path in ["a.txt", "secret.lock", "intent.txt"] {
        assert!(
            status.unstaged.iter().any(|file| file.file.path == path),
            "{path}"
        );
    }
    for file in status.unstaged {
        if file.file.path == "intent.txt" {
            assert_eq!(file.file.status, "added");
        }
        let patch = workspace
            .working_tree_diff(&info.id, "unstaged", &file.key)
            .unwrap();
        if file.file.path == "a.txt" {
            assert_eq!((patch.additions, patch.deletions), (1, 1));
        }
        if file.file.path == "secret.lock" {
            assert!(patch.binary);
            assert!(patch.hunks.is_empty());
        }
    }
    assert_eq!(before, snapshot(&fixture.source));
}

#[test]
fn sparse_index_reports_an_explicit_limitation_without_expanding_or_writing_it() {
    let fixture = Fixture::new();
    for directory in ["keep", "other"] {
        fs::create_dir(fixture.source.join(directory)).unwrap();
        fs::write(fixture.source.join(directory).join("file.txt"), "content\n").unwrap();
    }
    git_at(&fixture.source, &["add", "keep", "other"]);
    git_at(&fixture.source, &["commit", "-m", "sparse tree"]);
    git_at(
        &fixture.source,
        &["sparse-checkout", "init", "--cone", "--sparse-index"],
    );
    git_at(&fixture.source, &["sparse-checkout", "set", "keep"]);
    let before = snapshot(&fixture.source);
    let workspace = fixture.workspace();
    let info = fixture.open(&workspace);
    assert!(workspace
        .working_tree_status(&info.id)
        .unwrap_err()
        .contains("index sparse"));
    assert_eq!(before, snapshot(&fixture.source));
}

#[test]
fn index_only_non_utf8_paths_can_be_listed_without_host_path_conversion() {
    let fixture = Fixture::new();
    commit(
        &fixture.repository,
        "initial",
        &[],
        &[("normal.txt", b"normal\n")],
        Some("HEAD"),
    );
    checkout_head(&fixture);
    let mut index = fixture.repository.index().unwrap();
    let mut entry = index.get(0).unwrap();
    entry.path = b"index-only-\xff.txt".to_vec();
    entry.flags = entry.path.len() as u16;
    entry.flags_extended = 0;
    index.add(&entry).unwrap();
    index.write().unwrap();
    let before = snapshot(&fixture.source);
    let workspace = fixture.workspace();
    let info = fixture.open(&workspace);
    let status = workspace.working_tree_status(&info.id).unwrap();
    assert_eq!(status.staged.len(), 1);
    assert_eq!(status.staged[0].file.status, "added");
    assert!(status.staged[0].file.path.contains('\u{fffd}'));
    assert!(status.staged[0].key.contains("ff"));
    assert_eq!(before, snapshot(&fixture.source));
}
