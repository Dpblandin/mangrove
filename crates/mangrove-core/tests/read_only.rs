use std::{
    collections::{BTreeMap, HashMap},
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicI64, Ordering},
    thread,
    time::{Duration, Instant, SystemTime},
};

use git2::{Oid, Repository, Signature, Time};
use mangrove_core::{model::RepositoryInfo, Workspace};
use rusqlite::Connection;
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
        } else {
            ("file", fs::read(path).unwrap())
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
