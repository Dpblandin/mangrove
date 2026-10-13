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
    model::{
        AheadBehind, FileImageData, FileImageSource, GraphEdgeKind, PreviewLimit, Reference,
        RepositoryInfo,
    },
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

#[test]
fn commit_details_preserve_author_and_committer_signatures() {
    let fixture = Fixture::new();
    let author = Signature::new(
        "Nicolas DELEPLACE",
        "nicolas@example.org",
        &Time::new(1_790_696_260, 120),
    )
    .unwrap();
    let committer = Signature::new(
        "ci-release-user",
        "release@example.org",
        &Time::new(1_790_885_100, 0),
    )
    .unwrap();
    let tree_id = fixture
        .repository
        .treebuilder(None)
        .unwrap()
        .write()
        .unwrap();
    let tree = fixture.repository.find_tree(tree_id).unwrap();
    let sha = fixture
        .repository
        .commit(
            Some("HEAD"),
            &author,
            &committer,
            "feat: add prescription possibilities\n\nA readable commit description.",
            &tree,
            &[],
        )
        .unwrap();
    let workspace = fixture.workspace();
    let info = fixture.open(&workspace);
    let details = workspace
        .commit_details(&info.id, &sha.to_string())
        .unwrap();

    assert_eq!(details.author, "Nicolas DELEPLACE");
    assert_eq!(details.email, "nicolas@example.org");
    assert_eq!(details.author_time, author.when().seconds());
    assert_eq!(details.committer, "ci-release-user");
    assert_eq!(details.committer_email, "release@example.org");
    assert_eq!(details.time, committer.when().seconds());
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
            workspace
                .file_diff(&info.id, &row.sha, file.index, PreviewLimit::MIN)
                .unwrap();
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
fn reading_upstreams_and_counts_preserves_all_source_bytes_and_mtimes() {
    let fixture = Fixture::new();
    let first = commit(&fixture.repository, "first", &[], &[], Some("HEAD"));
    let second = commit(&fixture.repository, "second", &[first], &[], Some("HEAD"));
    fixture
        .repository
        .remote("origin", "git@github.com:example/private.git")
        .unwrap();
    fixture
        .repository
        .reference("refs/remotes/origin/main", first, false, "test")
        .unwrap();
    let mut config = fixture.repository.config().unwrap();
    config.set_str("branch.main.remote", "origin").unwrap();
    config
        .set_str("branch.main.merge", "refs/heads/main")
        .unwrap();
    let workspace = fixture.workspace();
    let read = |path: &Path| {
        let info = workspace.open_repository(path.to_str().unwrap()).unwrap();
        let tracking = workspace.branch_tracking(&info.id).unwrap();
        assert_eq!(tracking.branches[0].label, "origin/main");
        let counts = workspace
            .ahead_behind(
                &info.id,
                &[(second.to_string(), first.to_string())],
                Instant::now() + Duration::from_secs(60),
                &|| false,
            )
            .unwrap();
        assert_eq!(
            counts,
            [Some(AheadBehind::Counts {
                ahead: 1,
                behind: 0
            })]
        );
        finish(&workspace, &info.id);
    };
    let before = snapshot(&fixture.source);
    read(&fixture.source);
    assert_eq!(before, snapshot(&fixture.source));

    // A packed repository with a commit-graph, read from a linked worktree too.
    git_at(&fixture.source, &["repack", "-adq"]);
    git_at(&fixture.source, &["commit-graph", "write", "--reachable"]);
    let linked = fixture.root.path().join("linked");
    fixture
        .repository
        .worktree("linked", &linked, None)
        .unwrap();
    let before = (snapshot(&fixture.source), snapshot(&linked));
    read(&fixture.source);
    read(&linked);
    assert_eq!(before, (snapshot(&fixture.source), snapshot(&linked)));
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
fn search_offsets_are_the_offsets_of_listed_results() {
    let fixture = Fixture::new();
    let mut tip = commit(&fixture.repository, "base", &[], &[], Some("HEAD"));
    for index in 0..40 {
        let kind = if index % 3 == 0 { "fix" } else { "chore" };
        tip = commit(
            &fixture.repository,
            &format!("{kind} {index}"),
            &[tip],
            &[],
            Some("HEAD"),
        );
    }
    let workspace = fixture.workspace();
    let info = fixture.open(&workspace);
    finish(&workspace, &info.id);
    let history = workspace.list_commits(&info.id, 0, 256, "").unwrap();
    let results = workspace.list_commits(&info.id, 0, 256, "fix").unwrap();
    assert_eq!(results.total, 14);
    // « FIX » is not cached yet: one scan, then its cached snapshot.
    for _ in 0..2 {
        for commit in &history.commits {
            let (offset, index) = workspace
                .search_offset(&info.id, " FIX ", &commit.sha)
                .unwrap();
            let expected = results
                .commits
                .iter()
                .position(|result| result.sha == commit.sha);
            assert_eq!(offset, expected, "{}", commit.summary);
            assert_eq!(index.index_run, results.index.index_run);
            assert_eq!(index.indexed, 41);
            let (position, _) = workspace.search_offset(&info.id, "", &commit.sha).unwrap();
            assert_eq!(position, Some(commit.position));
        }
    }
    let absent = "0".repeat(40);
    assert_eq!(
        workspace.search_offset(&info.id, "fix", &absent).unwrap().0,
        None
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
                .filter(|edge| edge.kind == GraphEdgeKind::Parent)
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
        .file_diff(&linked.id, &first.to_string(), 0, PreviewLimit::MIN)
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
            .file_diff(&info.id, &sha.to_string(), file.index, PreviewLimit::MIN)
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
        .file_diff(&info.id, &sha.to_string(), usize::MAX, PreviewLimit::MIN)
        .is_err());
    assert!(workspace.commit_details(&info.id, "invalid").is_err());
}

#[test]
fn preview_limits_bound_every_diff_and_content_read_but_not_background_totals() {
    assert_eq!(PreviewLimit::from_mib(0), PreviewLimit::MIN);
    assert_eq!(PreviewLimit::from_mib(8).bytes(), 8 * 1024 * 1024);
    assert_eq!(PreviewLimit::from_mib(u64::MAX), PreviewLimit::MAX);
    let lines = |changed: usize| {
        (0..40_000)
            .map(|number| {
                let tag = if number == changed { "edit" } else { "kept" };
                format!("{tag} {number:>70}\n")
            })
            .collect::<String>()
    };
    let (old, new) = (lines(usize::MAX), lines(20_000));
    let size = old.len() as u64;
    assert!(size > PreviewLimit::MIN.bytes() && size < 4 * 1024 * 1024);
    let raised = PreviewLimit::from_mib(4);
    let mut fixture = Fixture::new();
    let parent = commit(
        &fixture.repository,
        "old",
        &[],
        &[("large.txt", old.as_bytes())],
        Some("HEAD"),
    );
    let sha = commit(
        &fixture.repository,
        "new",
        &[parent],
        &[("large.txt", new.as_bytes())],
        Some("HEAD"),
    )
    .to_string();
    let key = working_key("large.txt");
    let skipped = |diff: &mangrove_core::model::FileDiff| {
        assert!(diff.truncated && diff.hunks.is_empty());
        let oversized = diff.oversized.unwrap();
        assert_eq!(oversized.size, size);
        assert_eq!(oversized.limit, PreviewLimit::MIN.bytes());
    };
    let read = |diff: &mangrove_core::model::FileDiff| {
        assert!(diff.oversized.is_none() && !diff.truncated);
        assert_eq!((diff.additions, diff.deletions), (1, 1));
        assert!(patch_text(diff).contains(&format!("addition:edit {:>70}", 20_000)));
    };
    let workspace = fixture.workspace();
    let info = fixture.open(&workspace);
    finish(&workspace, &info.id);
    let before = snapshot(&fixture.source);
    let index = workspace.commit_details(&info.id, &sha).unwrap().files[0].index;
    skipped(
        &workspace
            .file_diff(&info.id, &sha, index, PreviewLimit::MIN)
            .unwrap(),
    );
    read(&workspace.file_diff(&info.id, &sha, index, raised).unwrap());
    skipped(
        &workspace
            .file_revision_diff(&info.id, &sha, &key, PreviewLimit::MIN)
            .unwrap(),
    );
    read(
        &workspace
            .file_revision_diff(&info.id, &sha, &key, raised)
            .unwrap(),
    );
    let content = workspace
        .file_content(&info.id, &sha, &key, PreviewLimit::MIN)
        .unwrap();
    assert_eq!(content.kind, "large");
    assert_eq!(content.oversized.unwrap().size, size);
    let content = workspace
        .file_content(&info.id, &sha, &key, raised)
        .unwrap();
    assert_eq!(content.kind, "text");
    assert!(content.oversized.is_none() && content.lines.len() == 2_000);
    // Blame and the commit's line totals keep the background bound.
    assert!(workspace
        .file_blame(&info.id, &sha, &key)
        .unwrap_err()
        .contains("2 Mio"));
    assert_eq!(
        workspace
            .commit_line_totals(&info.id, &sha, 10, &|| false)
            .unwrap(),
        None
    );
    assert_eq!(before, snapshot(&fixture.source));

    checkout_head(&fixture);
    fs::write(fixture.source.join("large.txt"), &old).unwrap();
    for area in ["unstaged", "staged"] {
        if area == "staged" {
            let mut index = fixture.repository.index().unwrap();
            index.add_path(Path::new("large.txt")).unwrap();
            index.write().unwrap();
        }
        workspace.working_tree_status(&info.id).unwrap();
        let before = snapshot(&fixture.source);
        let diff = workspace
            .working_tree_diff(&info.id, area, &key, PreviewLimit::MIN)
            .unwrap();
        skipped(&diff);
        let diff = workspace
            .working_tree_diff(&info.id, area, &key, raised)
            .unwrap();
        assert_eq!((diff.additions, diff.deletions), (1, 1));
        assert!(patch_text(&diff).contains(&format!("deletion:edit {:>70}", 20_000)));
        assert_eq!(before, snapshot(&fixture.source));
    }

    let signature = Signature::now("Stash Author", "stash@example.test").unwrap();
    let stash = fixture
        .repository
        .stash_save(&signature, "large", None)
        .unwrap()
        .to_string();
    let before = snapshot(&fixture.source);
    let stash_key = workspace
        .stash_details(&info.id, &stash)
        .unwrap()
        .groups
        .into_iter()
        .find(|group| group.area == "index")
        .unwrap()
        .files[0]
        .file_key
        .clone();
    let diff = workspace
        .stash_diff(&info.id, &stash, "index", &stash_key, PreviewLimit::MIN)
        .unwrap();
    skipped(&diff);
    let diff = workspace
        .stash_diff(&info.id, &stash, "index", &stash_key, raised)
        .unwrap();
    assert_eq!((diff.additions, diff.deletions), (1, 1));
    assert_eq!(before, snapshot(&fixture.source));
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
            .filter(|edge| edge.kind == GraphEdgeKind::Parent)
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
    assert_checkpoint_refs_are_hidden("refs/t3/checkpoints");
}

#[test]
fn t3_orchestration_checkpoint_refs_do_not_enter_history_search_or_trigger_refreshes() {
    assert_checkpoint_refs_are_hidden("refs/t3/orchestration-v2/checkpoints");
}

fn assert_checkpoint_refs_are_hidden(namespace: &str) {
    let prefix = format!("{namespace}/");
    let first_ref = format!("{namespace}/session/turn/1");
    let second_ref = format!("{namespace}/session/turn/2");
    let shared_ref = format!("{namespace}/session/shared");
    let backup_ref = format!("{namespace}-backup/session");
    let branch_ref = format!(
        "refs/heads/{}-feature",
        namespace.strip_prefix("refs/").unwrap()
    );
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
        Some(&first_ref),
    );
    fixture
        .repository
        .reference(&shared_ref, visible, false, "test")
        .unwrap();
    fixture
        .repository
        .reference(&branch_ref, visible, false, "test")
        .unwrap();
    let backup = commit(
        &fixture.repository,
        "technical-only-backup",
        &[root],
        &[],
        Some(&backup_ref),
    );
    let before = snapshot(&fixture.source);
    let workspace = fixture.workspace();
    let info = fixture.open(&workspace);
    finish(&workspace, &info.id);
    assert!(info
        .references
        .iter()
        .all(|reference| !reference.name.starts_with(&prefix)));
    assert!(info
        .references
        .iter()
        .any(|reference| reference.name == branch_ref));
    assert_eq!(info.references.len(), 2);
    let page = workspace.list_commits(&info.id, 0, 100, "").unwrap();
    assert_eq!(page.total, 2);
    assert!(page
        .commits
        .iter()
        .any(|row| row.sha == visible.to_string()));
    assert!(page.commits.iter().all(|row| row.graph.width == 1));
    for query in [
        "technical-only-backup",
        &backup_ref,
        &backup.to_string(),
        "technical-only-snapshot",
        &first_ref,
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
        Some(&second_ref),
    );
    fixture
        .repository
        .find_reference(&first_ref)
        .unwrap()
        .delete()
        .unwrap();
    let before = snapshot(&fixture.source);
    let same = fixture.open(&workspace);
    assert_eq!(same.id, info.id);
    assert!(same.index.complete);
    assert_eq!(same.index.indexed, 2);
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
    assert_legacy_checkpoint_cache_is_rebuilt("refs/t3/checkpoints");
}

#[test]
fn legacy_orchestration_checkpoint_cache_is_rebuilt_without_exposing_retained_fts_metadata() {
    assert_legacy_checkpoint_cache_is_rebuilt("refs/t3/orchestration-v2/checkpoints");
}

#[test]
fn legacy_custom_ref_cache_is_rebuilt_without_exposing_retained_fts_metadata() {
    assert_legacy_checkpoint_cache_is_rebuilt("refs/custom/snapshots");
}

fn assert_legacy_checkpoint_cache_is_rebuilt(namespace: &str) {
    let prefix = format!("{namespace}/");
    let checkpoint_ref = format!("{namespace}/session/turn/1");
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
        .reference(&checkpoint_ref, checkpoint, false, "test")
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
                7u32,
                &legacy.head,
                &legacy.head_name,
                &references,
                &legacy.remotes,
                vec![Vec::<u8>::new()],
                Vec::<String>::new()
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
        .all(|reference| !reference.name.starts_with(&prefix)));
    for query in [
        "legacy-only-checkpoint",
        &checkpoint_ref,
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
                "SELECT COUNT(*) FROM refs WHERE name LIKE ?1",
                [format!("{prefix}%")],
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

fn image_bytes(image: &FileImageData) -> &[u8] {
    match image {
        FileImageData::Bytes(bytes) => bytes,
        other => panic!("Expected image bytes, got {other:?}"),
    }
}

#[test]
fn image_revisions_follow_renames_and_deletions_without_reading_the_worktree() {
    let fixture = Fixture::new();
    let first = commit(
        &fixture.repository,
        "image added",
        &[],
        &[("old.png", b"\0old image")],
        Some("HEAD"),
    );
    let unrelated_large_blob = vec![0; 2 * 1024 * 1024 + 1];
    let renamed = commit(
        &fixture.repository,
        "image renamed beside an unrelated large addition",
        &[first],
        &[
            ("new.png", b"\0old image"),
            ("large.bin", &unrelated_large_blob),
        ],
        Some("HEAD"),
    );
    let modified = commit(
        &fixture.repository,
        "image modified",
        &[renamed],
        &[("new.png", b"\0new image")],
        Some("HEAD"),
    );
    let deleted = commit(
        &fixture.repository,
        "image deleted",
        &[modified],
        &[],
        Some("HEAD"),
    );
    fs::write(
        fixture.source.join("new.png"),
        b"unrelated local replacement",
    )
    .unwrap();
    let before = snapshot(&fixture.source);
    let workspace = fixture.workspace();
    let info = fixture.open(&workspace);
    finish(&workspace, &info.id);
    let read = |sha: Oid, path: &str, comparison| {
        workspace
            .file_images(
                &info.id,
                &FileImageSource::Commit {
                    sha: sha.to_string(),
                    file_key: working_key(path),
                    comparison,
                },
            )
            .unwrap()
    };
    let added = read(first, "old.png", true);
    assert!(matches!(added.before, Some(FileImageData::Absent)));
    assert_eq!(image_bytes(&added.after), b"\0old image");
    let renamed = read(renamed, "new.png", true);
    assert_eq!(
        image_bytes(renamed.before.as_ref().unwrap()),
        b"\0old image"
    );
    assert_eq!(image_bytes(&renamed.after), b"\0old image");
    let modified = read(modified, "new.png", true);
    assert_eq!(
        image_bytes(modified.before.as_ref().unwrap()),
        b"\0old image"
    );
    assert_eq!(image_bytes(&modified.after), b"\0new image");
    let deleted_diff = read(deleted, "new.png", true);
    assert_eq!(
        image_bytes(deleted_diff.before.as_ref().unwrap()),
        b"\0new image"
    );
    assert!(matches!(deleted_diff.after, FileImageData::Absent));
    let deleted_content = read(deleted, "new.png", false);
    assert!(deleted_content.before.is_none());
    assert!(matches!(deleted_content.after, FileImageData::Absent));
    assert!(workspace
        .file_images(
            &info.id,
            &FileImageSource::Commit {
                sha: first.to_string(),
                file_key: working_key("../outside.png"),
                comparison: false,
            }
        )
        .is_err());
    assert_eq!(before, snapshot(&fixture.source));
}

#[test]
fn local_image_comparisons_separate_head_index_and_worktree_with_size_limits() {
    let fixture = Fixture::new();
    commit(
        &fixture.repository,
        "base image",
        &[],
        &[("image.png", b"\0base image")],
        Some("HEAD"),
    );
    checkout_head(&fixture);
    fs::write(fixture.source.join("image.png"), b"\0index image").unwrap();
    let mut index = fixture.repository.index().unwrap();
    index.add_path(Path::new("image.png")).unwrap();
    index.write().unwrap();
    drop(index);
    fs::write(fixture.source.join("image.png"), b"\0worktree image").unwrap();
    fs::write(
        fixture.source.join("large.png"),
        vec![0; 2 * 1024 * 1024 + 1],
    )
    .unwrap();
    let before = snapshot(&fixture.source);
    let workspace = fixture.workspace();
    let info = fixture.open(&workspace);
    finish(&workspace, &info.id);
    let read = |area: &str, path: &str| {
        workspace
            .file_images(
                &info.id,
                &FileImageSource::WorkingTree {
                    area: area.into(),
                    file_key: working_key(path),
                },
            )
            .unwrap()
    };
    let staged = read("staged", "image.png");
    assert_eq!(
        image_bytes(staged.before.as_ref().unwrap()),
        b"\0base image"
    );
    assert_eq!(image_bytes(&staged.after), b"\0index image");
    let unstaged = read("unstaged", "image.png");
    assert_eq!(
        image_bytes(unstaged.before.as_ref().unwrap()),
        b"\0index image"
    );
    assert_eq!(image_bytes(&unstaged.after), b"\0worktree image");
    assert!(matches!(
        read("unstaged", "large.png").after,
        FileImageData::Unavailable(_)
    ));
    assert_eq!(before, snapshot(&fixture.source));
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
        .working_tree_diff(
            &info.id,
            "staged",
            &working_key("both.txt"),
            PreviewLimit::MIN,
        )
        .unwrap();
    let unstaged = workspace
        .working_tree_diff(
            &info.id,
            "unstaged",
            &working_key("both.txt"),
            PreviewLimit::MIN,
        )
        .unwrap();
    assert!(patch_text(&staged).contains("deletion:initial"));
    assert!(patch_text(&staged).contains("addition:staged"));
    assert!(patch_text(&unstaged).contains("deletion:staged"));
    assert!(patch_text(&unstaged).contains("addition:worktree"));
    assert!(
        workspace
            .working_tree_diff(
                &info.id,
                "unstaged",
                &working_key("binary.dat"),
                PreviewLimit::MIN
            )
            .unwrap()
            .binary
    );
    assert!(patch_text(
        &workspace
            .working_tree_diff(
                &info.id,
                "unstaged",
                &working_key("deleted.txt"),
                PreviewLimit::MIN
            )
            .unwrap()
    )
    .contains("deletion:deleted baseline"));
    assert!(patch_text(
        &workspace
            .working_tree_diff(
                &info.id,
                "unstaged",
                &working_key("untracked.txt"),
                PreviewLimit::MIN
            )
            .unwrap()
    )
    .contains("addition:untracked addition"));
    for (area, files) in [("staged", &status.staged), ("unstaged", &status.unstaged)] {
        for file in files {
            workspace
                .working_tree_diff(&info.id, area, &file.key, PreviewLimit::MIN)
                .unwrap();
        }
    }
    assert_eq!(before, snapshot(&fixture.source));

    // Adding an earlier-sorted entry must not redirect a selection by index.
    fs::write(fixture.source.join("aaa-new.txt"), b"different file\n").unwrap();
    let before = snapshot(&fixture.source);
    assert!(patch_text(
        &workspace
            .working_tree_diff(
                &info.id,
                "unstaged",
                &working_key("untracked.txt"),
                PreviewLimit::MIN
            )
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
            .working_tree_diff(
                &info.id,
                "staged",
                &working_key("first.txt"),
                PreviewLimit::MIN
            )
            .unwrap()
    )
    .contains("addition:staged first file"));
    assert!(patch_text(
        &workspace
            .working_tree_diff(
                &info.id,
                "unstaged",
                &working_key("first.txt"),
                PreviewLimit::MIN
            )
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
        .working_tree_diff(
            &info.id,
            "staged",
            &working_key("first.txt"),
            PreviewLimit::MIN
        )
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
            .working_tree_diff(&info.id, area, &working_key("large.txt"), PreviewLimit::MIN)
            .unwrap();
        assert!(diff.truncated);
        assert!(!diff.binary);
        assert!(diff.hunks.is_empty());
    }
    let diff = workspace
        .working_tree_diff(
            &info.id,
            "unstaged",
            &working_key("many.txt"),
            PreviewLimit::MIN,
        )
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
        .working_tree_diff(
            &info.id,
            "push",
            &working_key("large.txt"),
            PreviewLimit::MIN
        )
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
                    .working_tree_diff(&info.id, area, &working_key(path), PreviewLimit::MIN)
                    .is_err(),
                "{path:?}"
            );
        }
    }
    assert!(workspace
        .working_tree_diff(
            &info.id,
            "unstaged",
            &working_key("absent.txt"),
            PreviewLimit::MIN
        )
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
            .working_tree_diff(&info.id, "unstaged", &working_key(path), PreviewLimit::MIN)
            .unwrap();
        assert!(!patch_text(&diff).contains("EXTERNAL SECRET MUST NOT BE READ"));
        assert!(patch_text(&diff).contains("secret.txt"), "{path} {diff:?}");
    }
    let gitlink = workspace
        .working_tree_diff(
            &info.id,
            "staged",
            &working_key("vendor"),
            PreviewLimit::MIN,
        )
        .unwrap();
    assert!(patch_text(&gitlink).contains("Subproject commit"));
    assert!(!patch_text(&gitlink).contains("EXTERNAL SECRET MUST NOT BE READ"));
    for path in ["escape/secret.txt", "socket", "vendor/secret.txt"] {
        assert!(
            workspace
                .working_tree_diff(&info.id, "unstaged", &working_key(path), PreviewLimit::MIN)
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
            .working_tree_diff(
                &info.id,
                area,
                &working_key("conflict.txt"),
                PreviewLimit::MIN
            )
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
            .working_tree_diff(
                &info.id,
                "unstaged",
                &working_key("private.txt"),
                PreviewLimit::MIN
            )
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
                .working_tree_diff(&info.id, area, &file.key, PreviewLimit::MIN)
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
            .working_tree_diff(&info.id, "unstaged", key, PreviewLimit::MIN)
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
            if let Ok(diff) =
                workspace.working_tree_diff(&info.id, "unstaged", &key, PreviewLimit::MIN)
            {
                exposed_secret |= patch_text(&diff).contains("EXTERNAL RACE SECRET");
            }
        }
        stop.store(true, Ordering::Relaxed);
        worker.join().unwrap();
        assert!(!exposed_secret, "replace_parent={replace_parent}");
        let diff = workspace
            .working_tree_diff(&info.id, "unstaged", &key, PreviewLimit::MIN)
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
            .working_tree_diff(
                &info.id,
                "unstaged",
                &status.unstaged[0].key,
                PreviewLimit::MIN
            )
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
fn only_branches_and_tags_are_history_roots_or_refresh_triggers() {
    let fixture = Fixture::new();
    let root = commit(&fixture.repository, "visible", &[], &[], Some("HEAD"));
    let operational_refs = [
        "refs/stash",
        "refs/notes/commits",
        "refs/prefetch/remotes/origin/main",
        "refs/codex/snapshots/0bf5219b1ee9b260ad4c836df8d3e80d78f27c26",
        "refs/codex/backups/vite-plus-960cd2127e",
        "refs/t3/checkpoints/session/turn/1",
        "refs/t3/orchestration-v2/checkpoints/session/ordinal/0",
        "refs/t3/checkpoints-backup/session",
        "refs/codex-backup/session",
        "refs/custom/snapshots/session",
        "refs/heads-backup/topic",
        "refs/remotes-backup/origin/main",
        "refs/tags-backup/release",
    ];
    for name in operational_refs {
        commit(
            &fixture.repository,
            "operational-only",
            &[root],
            &[],
            Some(name),
        );
    }
    let visible_refs = [
        "refs/heads/main",
        "refs/heads/codex/snapshots/feature",
        "refs/remotes/origin/codex/backups/feature",
        "refs/tags/codex/snapshots/release",
    ];
    let mut visible_commits = vec![("refs/heads/main", root)];
    for name in &visible_refs[1..] {
        let oid = commit(&fixture.repository, name, &[root], &[], Some(name));
        visible_commits.push((name, oid));
    }
    let before = snapshot(&fixture.source);
    let workspace = fixture.workspace();
    let info = fixture.open(&workspace);
    finish(&workspace, &info.id);
    assert_eq!(info.references.len(), visible_refs.len());
    for name in visible_refs {
        assert!(info
            .references
            .iter()
            .any(|reference| reference.name == name));
    }
    assert_eq!(
        workspace.list_commits(&info.id, 0, 100, "").unwrap().total,
        visible_refs.len()
    );
    for (name, oid) in visible_commits {
        for query in [name, &oid.to_string()] {
            let result = workspace.list_commits(&info.id, 0, 100, query).unwrap();
            assert_eq!(result.total, 1, "{query}");
            assert_eq!(result.commits[0].sha, oid.to_string(), "{query}");
        }
    }
    assert_eq!(
        workspace
            .list_commits(&info.id, 0, 100, "operational-only")
            .unwrap()
            .total,
        0
    );
    assert_eq!(before, snapshot(&fixture.source));
    for name in operational_refs {
        let old = fixture
            .repository
            .find_reference(name)
            .unwrap()
            .target()
            .unwrap();
        for query in [name, &old.to_string()] {
            assert_eq!(
                workspace
                    .list_commits(&info.id, 0, 100, query)
                    .unwrap()
                    .total,
                0,
                "{query}"
            );
        }
        commit(
            &fixture.repository,
            "new-operational-snapshot",
            &[old],
            &[],
            Some(name),
        );
        let before_refresh = snapshot(&fixture.source);
        assert_eq!(fixture.open(&workspace).id, info.id, "{name}");
        assert_eq!(before_refresh, snapshot(&fixture.source));
    }
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
        .file_diff(&info.id, &second.to_string(), 0, PreviewLimit::MIN)
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
            .working_tree_diff(&info.id, "unstaged", &file.key, PreviewLimit::MIN)
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

#[test]
fn file_views_use_revision_objects_and_preserve_every_source_file() {
    let fixture = Fixture::new();
    let initial = commit(
        &fixture.repository,
        "initial",
        &[],
        &[(
            "élève.ts",
            "const prénom = 'Élodie';\nconst value = 1;\n".as_bytes(),
        )],
        Some("HEAD"),
    );
    let modified = commit(
        &fixture.repository,
        "modify",
        &[initial],
        &[(
            "élève.ts",
            "const prénom = 'Élodie';\nconst value = 2;\n".as_bytes(),
        )],
        Some("HEAD"),
    );
    fs::write(fixture.source.join("élève.ts"), "uncommitted worktree text").unwrap();
    let workspace = fixture.workspace();
    let info = fixture.open(&workspace);
    finish(&workspace, &info.id);
    let before = snapshot(&fixture.source);
    let details = workspace
        .commit_details(&info.id, &modified.to_string())
        .unwrap();
    let key = &details.files[0].file_key;
    let content = workspace
        .file_content(&info.id, &initial.to_string(), key, PreviewLimit::MIN)
        .unwrap();
    assert_eq!(
        content.lines,
        ["const prénom = 'Élodie';", "const value = 1;"]
    );
    assert!(content.ends_with_newline);
    let history = workspace
        .file_history(&info.id, &modified.to_string(), key, None)
        .unwrap();
    assert_eq!(
        history
            .entries
            .iter()
            .map(|entry| entry.sha.as_str())
            .collect::<Vec<_>>(),
        [modified.to_string(), initial.to_string()]
    );
    let blame = workspace
        .file_blame(&info.id, &modified.to_string(), key)
        .unwrap();
    assert_eq!(blame.groups.len(), 2);
    assert_eq!(blame.groups[0].sha, initial.to_string());
    assert_eq!(blame.groups[1].sha, modified.to_string());
    assert_eq!(blame.groups[0].author, "Élodie Müller");
    assert_eq!(
        workspace
            .file_revision_diff(&info.id, &modified.to_string(), key, PreviewLimit::MIN)
            .unwrap()
            .additions,
        1
    );
    assert_eq!(snapshot(&fixture.source), before);
}

#[test]
fn tree_files_list_raw_paths_of_a_revision_without_submodules_or_worktree_reads() {
    let fixture = Fixture::new();
    let repository = &fixture.repository;
    let blob = repository.blob(b"text\n").unwrap();
    let mut nested = repository.treebuilder(None).unwrap();
    nested.insert("deep.rs", blob, 0o100644).unwrap();
    nested
        .insert(b"raw-\xff.txt".as_slice(), blob, 0o100644)
        .unwrap();
    let nested = nested.write().unwrap();
    let mut root = repository.treebuilder(None).unwrap();
    root.insert("src", nested, 0o040000).unwrap();
    root.insert("README.md", blob, 0o100644).unwrap();
    root.insert("link", repository.blob(b"README.md").unwrap(), 0o120000)
        .unwrap();
    // A gitlink names a commit of another repository.
    root.insert("vendor", Oid::from_str(&"1".repeat(40)).unwrap(), 0o160000)
        .unwrap();
    let tree = repository.find_tree(root.write().unwrap()).unwrap();
    let signature = Signature::now("Test", "test@example.org").unwrap();
    let id = repository
        .commit(Some("HEAD"), &signature, &signature, "tree", &tree, &[])
        .unwrap();
    let before = snapshot(&fixture.source);
    let workspace = fixture.workspace();
    let info = fixture.open(&workspace);
    let listing = workspace.tree_files(&info.id, &id.to_string()).unwrap();
    assert_eq!(listing.sha, id.to_string());
    assert!(!listing.truncated);
    let paths: Vec<_> = listing
        .files
        .iter()
        .map(|file| file.path.as_str())
        .collect();
    assert_eq!(
        paths,
        ["README.md", "link", "src/deep.rs", "src/raw-\u{fffd}.txt"]
    );
    // The lossy label never identifies the file: its key keeps the raw byte.
    let raw = &listing.files[3];
    let raw_key: String = b"src/raw-\xff.txt"
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    assert_eq!(raw.file_key, raw_key);
    let content = workspace
        .file_content(&info.id, &id.to_string(), &raw.file_key, PreviewLimit::MIN)
        .unwrap();
    assert_eq!(content.lines, ["text"]);
    assert!(workspace.tree_files(&info.id, &"0".repeat(40)).is_err());
    assert_eq!(before, snapshot(&fixture.source));
}

#[test]
fn file_history_follows_a_rename_over_a_former_directory() {
    let fixture = Fixture::new();
    let signature = Signature::now("Author", "author@example.org").unwrap();
    let mut revisions = Vec::new();
    for (message, bytes) in [
        ("create nested file", b"original\n".as_slice()),
        ("modify nested file", b"changed\n".as_slice()),
    ] {
        let mut folder = fixture.repository.treebuilder(None).unwrap();
        folder
            .insert(
                "contents.txt",
                fixture.repository.blob(bytes).unwrap(),
                0o100644,
            )
            .unwrap();
        let mut root = fixture.repository.treebuilder(None).unwrap();
        root.insert("item", folder.write().unwrap(), 0o040000)
            .unwrap();
        let tree = fixture.repository.find_tree(root.write().unwrap()).unwrap();
        let parents: Vec<_> = revisions
            .last()
            .map(|id| fixture.repository.find_commit(*id).unwrap())
            .into_iter()
            .collect();
        revisions.push(
            fixture
                .repository
                .commit(
                    Some("HEAD"),
                    &signature,
                    &signature,
                    message,
                    &tree,
                    &parents.iter().collect::<Vec<_>>(),
                )
                .unwrap(),
        );
    }
    let renamed = commit(
        &fixture.repository,
        "move file over its folder",
        &[revisions[1]],
        &[("item", b"changed\n")],
        Some("HEAD"),
    );
    let workspace = fixture.workspace();
    let info = fixture.open(&workspace);
    finish(&workspace, &info.id);
    let before = snapshot(&fixture.source);
    let details = workspace
        .commit_details(&info.id, &renamed.to_string())
        .unwrap();
    let file = details
        .files
        .iter()
        .find(|file| file.path == "item")
        .unwrap();
    assert_eq!(file.status, "renamed");
    let history = workspace
        .file_history(&info.id, &renamed.to_string(), &file.file_key, None)
        .unwrap();
    assert_eq!(history.entries.len(), 3);
    assert_eq!(history.entries[0].status, "renamed");
    assert_eq!(
        history.entries[0].old_path.as_deref(),
        Some("item/contents.txt")
    );
    for (entry, (sha, text)) in history.entries[1..]
        .iter()
        .zip([(revisions[1], "changed"), (revisions[0], "original")])
    {
        assert_eq!(entry.sha, sha.to_string());
        assert_eq!(entry.path, "item/contents.txt");
        let content = workspace
            .file_content(&info.id, &entry.sha, &entry.file_key, PreviewLimit::MIN)
            .unwrap();
        assert_eq!(content.kind, "text");
        assert_eq!(content.lines, [text]);
    }
    assert!(history.next_cursor.is_none());
    assert!(history.limitation.is_none());
    assert_eq!(snapshot(&fixture.source), before);
}

#[test]
fn file_history_treats_a_replacing_directory_as_a_file_deletion() {
    let fixture = Fixture::new();
    let initial = commit(
        &fixture.repository,
        "create file",
        &[],
        &[("item", b"original file\n")],
        Some("HEAD"),
    );
    let mut folder = fixture.repository.treebuilder(None).unwrap();
    folder
        .insert(
            "contents.txt",
            fixture.repository.blob(b"unrelated nested file\n").unwrap(),
            0o100644,
        )
        .unwrap();
    let mut root = fixture.repository.treebuilder(None).unwrap();
    root.insert("item", folder.write().unwrap(), 0o040000)
        .unwrap();
    let tree = fixture.repository.find_tree(root.write().unwrap()).unwrap();
    let signature = Signature::now("Author", "author@example.org").unwrap();
    let replaced = fixture
        .repository
        .commit(
            Some("HEAD"),
            &signature,
            &signature,
            "replace file with directory",
            &tree,
            &[&fixture.repository.find_commit(initial).unwrap()],
        )
        .unwrap();
    let workspace = fixture.workspace();
    let info = fixture.open(&workspace);
    finish(&workspace, &info.id);
    let before = snapshot(&fixture.source);
    let details = workspace
        .commit_details(&info.id, &replaced.to_string())
        .unwrap();
    let file = details
        .files
        .iter()
        .find(|file| file.path == "item")
        .unwrap();
    assert_eq!(file.status, "deleted");
    let history = workspace
        .file_history(&info.id, &replaced.to_string(), &file.file_key, None)
        .unwrap();
    assert_eq!(history.entries.len(), 2);
    assert_eq!(history.entries[0].sha, replaced.to_string());
    assert_eq!(history.entries[0].status, "deleted");
    assert_eq!(history.entries[1].sha, initial.to_string());
    assert_eq!(history.entries[1].status, "added");
    let content = workspace
        .file_content(
            &info.id,
            &history.entries[1].sha,
            &history.entries[1].file_key,
            PreviewLimit::MIN,
        )
        .unwrap();
    assert_eq!(content.kind, "text");
    assert_eq!(content.lines, ["original file"]);
    assert!(history.next_cursor.is_none());
    assert!(history.limitation.is_none());
    assert_eq!(snapshot(&fixture.source), before);
}

#[test]
fn file_history_follows_renames_deletions_and_first_parent_merges() {
    let fixture = Fixture::new();
    let first = commit(
        &fixture.repository,
        "add",
        &[],
        &[("old.rs", b"let original = 1;\n")],
        Some("HEAD"),
    );
    let renamed = commit(
        &fixture.repository,
        "rename",
        &[first],
        &[("new.rs", b"let original = 1;\n")],
        Some("HEAD"),
    );
    let side = commit(
        &fixture.repository,
        "side",
        &[renamed],
        &[("new.rs", b"let original = 1;\nlet side = 2;\n")],
        None,
    );
    let merge = commit(
        &fixture.repository,
        "merge",
        &[renamed, side],
        &[("new.rs", b"let original = 1;\nlet side = 2;\n")],
        Some("HEAD"),
    );
    let deleted = commit(&fixture.repository, "delete", &[merge], &[], Some("HEAD"));
    let workspace = fixture.workspace();
    let info = fixture.open(&workspace);
    let key = workspace
        .commit_details(&info.id, &deleted.to_string())
        .unwrap()
        .files[0]
        .file_key
        .clone();
    let history = workspace
        .file_history(&info.id, &deleted.to_string(), &key, None)
        .unwrap();
    assert_eq!(
        history
            .entries
            .iter()
            .map(|entry| entry.sha.as_str())
            .collect::<Vec<_>>(),
        [
            deleted.to_string(),
            merge.to_string(),
            renamed.to_string(),
            first.to_string()
        ]
    );
    assert!(history.entries[1].merge);
    assert_eq!(history.entries[2].status, "renamed");
    assert_eq!(history.entries[2].old_path.as_deref(), Some("old.rs"));
    assert_eq!(history.entries[3].path, "old.rs");
    let absent = workspace
        .file_content(&info.id, &deleted.to_string(), &key, PreviewLimit::MIN)
        .unwrap();
    assert_eq!(absent.kind, "deleted");
    assert_eq!(absent.parent_sha, Some(merge.to_string()));
    assert!(workspace
        .file_blame(&info.id, &deleted.to_string(), &key)
        .is_err());
    let blame = workspace
        .file_blame(&info.id, &merge.to_string(), &key)
        .unwrap();
    assert_eq!(blame.groups[0].sha, first.to_string());
    assert_eq!(blame.groups[1].sha, merge.to_string());
}

#[test]
fn file_views_define_binary_empty_non_newline_and_size_limits() {
    let fixture = Fixture::new();
    let many_lines = "é\n".repeat(2_100);
    let oversized = vec![b'x'; 2 * 1024 * 1024 + 1];
    let huge_line = vec![b'x'; 256 * 1024 + 1];
    let id = commit(
        &fixture.repository,
        "edge cases",
        &[],
        &[
            ("binary.bin", b"\0\xff\x01"),
            ("empty.txt", b""),
            ("final.txt", "é sans newline".as_bytes()),
            ("lines.txt", many_lines.as_bytes()),
            ("oversized.txt", &oversized),
            ("long.txt", &huge_line),
        ],
        Some("HEAD"),
    );
    let workspace = fixture.workspace();
    let info = fixture.open(&workspace);
    let details = workspace.commit_details(&info.id, &id.to_string()).unwrap();
    let read = |name: &str| {
        let file = details.files.iter().find(|file| file.path == name).unwrap();
        workspace
            .file_content(&info.id, &id.to_string(), &file.file_key, PreviewLimit::MIN)
            .unwrap()
    };
    assert_eq!(read("binary.bin").kind, "binary");
    let empty = read("empty.txt");
    assert!(empty.lines.is_empty());
    assert_eq!(empty.kind, "text");
    let final_line = read("final.txt");
    assert_eq!(final_line.lines, ["é sans newline"]);
    assert!(!final_line.ends_with_newline);
    let lines = read("lines.txt");
    assert_eq!(lines.lines.len(), 2_000);
    assert!(lines.truncated);
    assert_eq!(read("oversized.txt").kind, "large");
    assert!(read("long.txt").truncated);
    let key = &details
        .files
        .iter()
        .find(|file| file.path == "empty.txt")
        .unwrap()
        .file_key;
    assert!(workspace
        .file_blame(&info.id, &id.to_string(), key)
        .unwrap()
        .groups
        .is_empty());
}

#[test]
fn file_history_pages_continue_across_renames_without_duplicates() {
    let fixture = Fixture::new();
    let mut parent = None;
    let mut ids = Vec::new();
    for index in 0..48 {
        let path = if index < 20 { "old.txt" } else { "new.txt" };
        let text = if index == 20 {
            "line 19\n".to_string()
        } else {
            format!("line {index}\n")
        };
        let id = commit(
            &fixture.repository,
            "change",
            &parent.into_iter().collect::<Vec<_>>(),
            &[(path, text.as_bytes())],
            Some("HEAD"),
        );
        parent = Some(id);
        ids.push(id.to_string());
    }
    let workspace = fixture.workspace();
    let info = fixture.open(&workspace);
    let sha = parent.unwrap().to_string();
    let key = workspace.commit_details(&info.id, &sha).unwrap().files[0]
        .file_key
        .clone();
    let page = workspace.file_history(&info.id, &sha, &key, None).unwrap();
    assert_eq!(page.entries.len(), 40);
    let cursor = page.next_cursor.unwrap();
    let next = workspace
        .file_history(&info.id, &sha, &key, Some(&cursor))
        .unwrap();
    let all = page
        .entries
        .iter()
        .chain(next.entries.iter())
        .map(|entry| entry.sha.clone())
        .collect::<Vec<_>>();
    ids.reverse();
    assert_eq!(all, ids);
    assert!(next.next_cursor.is_none());
    assert!(workspace
        .file_history(&info.id, &sha, "6f74686572", Some(&cursor))
        .is_err());
}

#[test]
fn sparse_file_history_returns_a_cursor_even_for_an_empty_page() {
    let fixture = Fixture::new();
    let mut parent = commit(
        &fixture.repository,
        "add",
        &[],
        &[("rare.txt", b"unchanged\n")],
        Some("HEAD"),
    );
    let initial = parent.to_string();
    for _ in 0..260 {
        parent = commit(
            &fixture.repository,
            "other change",
            &[parent],
            &[("rare.txt", b"unchanged\n")],
            Some("HEAD"),
        );
    }
    let workspace = fixture.workspace();
    let info = fixture.open(&workspace);
    let page = workspace
        .file_history(&info.id, &parent.to_string(), "726172652e747874", None)
        .unwrap();
    assert!(page.entries.is_empty());
    assert_eq!(page.scanned, 256);
    let next = workspace
        .file_history(
            &info.id,
            &parent.to_string(),
            "726172652e747874",
            page.next_cursor.as_deref(),
        )
        .unwrap();
    assert_eq!(next.entries[0].sha, initial);
    let blame = workspace
        .file_blame(&info.id, &parent.to_string(), "726172652e747874")
        .unwrap();
    assert!(blame.limitation.is_some());
    assert!(blame.groups[0].boundary);
}

#[test]
fn missing_file_objects_report_a_partial_repository_without_fetch_or_writes() {
    let fixture = Fixture::new();
    let id = commit(
        &fixture.repository,
        "add",
        &[],
        &[("partial.txt", b"missing blob\n")],
        Some("HEAD"),
    );
    let blob = fixture
        .repository
        .find_commit(id)
        .unwrap()
        .tree()
        .unwrap()
        .get_name("partial.txt")
        .unwrap()
        .id()
        .to_string();
    let workspace = fixture.workspace();
    let info = fixture.open(&workspace);
    finish(&workspace, &info.id);
    let details = workspace.commit_details(&info.id, &id.to_string()).unwrap();
    fs::remove_file(
        fixture
            .repository
            .path()
            .join("objects")
            .join(&blob[..2])
            .join(&blob[2..]),
    )
    .unwrap();
    let before = snapshot(&fixture.source);
    let error = workspace
        .file_content(
            &info.id,
            &id.to_string(),
            &details.files[0].file_key,
            PreviewLimit::MIN,
        )
        .unwrap_err();
    assert!(error.contains("Objet Git absent"), "{error}");
    assert!(error.contains("Aucun fetch"));
    assert_eq!(snapshot(&fixture.source), before);
}

#[test]
fn file_text_preserves_isolated_carriage_returns_and_normalizes_crlf() {
    let fixture = Fixture::new();
    let id = commit(
        &fixture.repository,
        "line endings",
        &[],
        &[("endings.txt", b"first\r\nsecond\rthird\r")],
        Some("HEAD"),
    );
    let workspace = fixture.workspace();
    let info = fixture.open(&workspace);
    let content = workspace
        .file_content(
            &info.id,
            &id.to_string(),
            "656e64696e67732e747874",
            PreviewLimit::MIN,
        )
        .unwrap();
    assert_eq!(content.lines, ["first", "second\rthird\r"]);
    assert!(!content.ends_with_newline);
}

#[cfg(unix)]
#[test]
fn file_views_keep_raw_name_identities_and_never_follow_tree_symlinks() {
    let fixture = Fixture::new();
    let mut tree = fixture.repository.treebuilder(None).unwrap();
    let names: [&[u8]; 2] = [b"same-\xfe.txt", b"same-\xff.txt"];
    for (index, name) in names.iter().enumerate() {
        let blob = fixture
            .repository
            .blob(format!("content {index}\n").as_bytes())
            .unwrap();
        tree.insert(*name, blob, 0o100644).unwrap();
    }
    let target = fixture.root.path().join("external-secret.txt");
    fs::write(&target, "secret outside repository").unwrap();
    let symlink = fixture
        .repository
        .blob(target.as_os_str().as_encoded_bytes())
        .unwrap();
    tree.insert("link.txt", symlink, 0o120000).unwrap();
    let subtree = fixture
        .repository
        .treebuilder(None)
        .unwrap()
        .write()
        .unwrap();
    tree.insert("directory", subtree, 0o040000).unwrap();
    let tree = fixture.repository.find_tree(tree.write().unwrap()).unwrap();
    let signature = Signature::now("Author", "author@example.org").unwrap();
    let id = fixture
        .repository
        .commit(
            Some("HEAD"),
            &signature,
            &signature,
            "raw names",
            &tree,
            &[],
        )
        .unwrap();
    let workspace = fixture.workspace();
    let info = fixture.open(&workspace);
    finish(&workspace, &info.id);
    let before = snapshot(&fixture.source);
    let external_before = snapshot(&target);
    let details = workspace.commit_details(&info.id, &id.to_string()).unwrap();
    let files: Vec<_> = details
        .files
        .iter()
        .filter(|file| file.path.starts_with("same-"))
        .collect();
    assert_eq!(files.len(), 2);
    assert_eq!(files[0].path, files[1].path);
    assert_ne!(files[0].file_key, files[1].file_key);
    for file in files {
        let content = workspace
            .file_content(&info.id, &id.to_string(), &file.file_key, PreviewLimit::MIN)
            .unwrap();
        let expected = if file.file_key.contains("fe") {
            "content 0"
        } else {
            "content 1"
        };
        assert_eq!(content.lines, [expected]);
        assert_eq!(
            workspace
                .file_history(&info.id, &id.to_string(), &file.file_key, None)
                .unwrap()
                .entries[0]
                .file_key,
            file.file_key
        );
        assert_eq!(
            workspace
                .file_blame(&info.id, &id.to_string(), &file.file_key)
                .unwrap()
                .groups[0]
                .sha,
            id.to_string()
        );
    }
    let symlink = workspace
        .file_content(
            &info.id,
            &id.to_string(),
            "6c696e6b2e747874",
            PreviewLimit::MIN,
        )
        .unwrap();
    assert_eq!(symlink.kind, "text");
    assert_eq!(symlink.lines, [target.to_str().unwrap()]);
    assert_eq!(
        workspace
            .file_content(
                &info.id,
                &id.to_string(),
                "6469726563746f7279",
                PreviewLimit::MIN
            )
            .unwrap()
            .kind,
        "unsupported"
    );
    assert_eq!(before, snapshot(&fixture.source));
    assert_eq!(external_before, snapshot(&target));
}

#[test]
fn file_history_keeps_completed_entries_when_an_ancestor_tree_is_missing() {
    let fixture = Fixture::new();
    let initial = commit(
        &fixture.repository,
        "initial",
        &[],
        &[("partial.txt", b"one\n")],
        Some("HEAD"),
    );
    let middle = commit(
        &fixture.repository,
        "middle",
        &[initial],
        &[("partial.txt", b"two\n")],
        Some("HEAD"),
    );
    let newest = commit(
        &fixture.repository,
        "newest",
        &[middle],
        &[("partial.txt", b"three\n")],
        Some("HEAD"),
    );
    let missing_tree = fixture
        .repository
        .find_commit(initial)
        .unwrap()
        .tree_id()
        .to_string();
    let workspace = fixture.workspace();
    let info = fixture.open(&workspace);
    finish(&workspace, &info.id);
    fs::remove_file(
        fixture
            .repository
            .path()
            .join("objects")
            .join(&missing_tree[..2])
            .join(&missing_tree[2..]),
    )
    .unwrap();
    let before = snapshot(&fixture.source);
    let history = workspace
        .file_history(
            &info.id,
            &newest.to_string(),
            "7061727469616c2e747874",
            None,
        )
        .unwrap();
    assert_eq!(history.entries.len(), 1);
    assert_eq!(history.entries[0].sha, newest.to_string());
    assert!(history.next_cursor.is_none());
    let limitation = history.limitation.unwrap();
    assert!(limitation.contains("Objet Git absent"), "{limitation}");
    assert!(limitation.contains("Aucun fetch"));
    let content_error = workspace
        .file_content(
            &info.id,
            &initial.to_string(),
            "7061727469616c2e747874",
            PreviewLimit::MIN,
        )
        .unwrap_err();
    assert!(content_error.contains("Objet Git absent"));
    let absent_commit = workspace
        .file_content(
            &info.id,
            "1234567890123456789012345678901234567890",
            "7061727469616c2e747874",
            PreviewLimit::MIN,
        )
        .unwrap_err();
    assert!(absent_commit.contains("Objet Git absent"));
    assert_eq!(before, snapshot(&fixture.source));
}

#[test]
fn file_history_marks_an_apparent_creation_at_a_shallow_boundary() {
    let fixture = Fixture::new();
    let initial = commit(
        &fixture.repository,
        "initial",
        &[],
        &[("file.txt", b"one\n")],
        Some("HEAD"),
    );
    let boundary = commit(
        &fixture.repository,
        "boundary",
        &[initial],
        &[("file.txt", b"two\n")],
        Some("HEAD"),
    );
    fs::write(
        fixture.repository.path().join("shallow"),
        format!("{boundary}\n"),
    )
    .unwrap();
    let workspace = fixture.workspace();
    let info = fixture.open(&workspace);
    finish(&workspace, &info.id);
    let before = snapshot(&fixture.source);
    let history = workspace
        .file_history(&info.id, &boundary.to_string(), "66696c652e747874", None)
        .unwrap();
    assert_eq!(history.entries.len(), 1);
    assert_eq!(history.entries[0].status, "added");
    assert!(history
        .limitation
        .as_deref()
        .unwrap()
        .contains("Frontière shallow"));
    let blame = workspace
        .file_blame(&info.id, &boundary.to_string(), "66696c652e747874")
        .unwrap();
    assert!(blame
        .limitation
        .as_deref()
        .unwrap()
        .contains("Frontière shallow"));
    assert!(blame.groups[0].boundary);
    assert_eq!(before, snapshot(&fixture.source));
}

#[test]
fn file_blame_stops_before_large_ancestors_and_reports_only_reached_boundaries() {
    let fixture = Fixture::new();
    let huge = vec![b'x'; 2 * 1024 * 1024 + 1];
    let initial = commit(
        &fixture.repository,
        "large ancestor",
        &[],
        &[("file.txt", &huge)],
        Some("HEAD"),
    );
    let reduced = commit(
        &fixture.repository,
        "reduce",
        &[initial],
        &[("file.txt", b"small\n")],
        Some("HEAD"),
    );
    let newest = commit(
        &fixture.repository,
        "edit",
        &[reduced],
        &[("file.txt", b"new\n")],
        Some("HEAD"),
    );
    let workspace = fixture.workspace();
    let info = fixture.open(&workspace);
    finish(&workspace, &info.id);
    let before = snapshot(&fixture.source);
    let bounded = workspace
        .file_blame(&info.id, &reduced.to_string(), "66696c652e747874")
        .unwrap();
    assert_eq!(bounded.groups[0].sha, reduced.to_string());
    assert!(bounded.groups[0].boundary);
    assert!(bounded.limitation.as_deref().unwrap().contains("2 Mio"));
    let resolved = workspace
        .file_blame(&info.id, &newest.to_string(), "66696c652e747874")
        .unwrap();
    assert_eq!(resolved.groups[0].sha, newest.to_string());
    assert!(resolved.limitation.is_none());
    let diff = workspace
        .file_revision_diff(
            &info.id,
            &reduced.to_string(),
            "66696c652e747874",
            PreviewLimit::MIN,
        )
        .unwrap();
    assert!(diff.truncated);
    assert_eq!(before, snapshot(&fixture.source));
}

#[test]
fn file_history_long_raw_path_cursors_continue_after_a_rename() {
    let fixture = Fixture::new();
    let old_path = format!("old-{}.txt", "a".repeat(9_000));
    let new_path = format!("new-{}.txt", "b".repeat(9_000));
    let mut parent = None;
    let mut ids = Vec::new();
    for index in 0..48 {
        let path = if index < 10 { &old_path } else { &new_path };
        let text = format!("line {}\n", if index == 10 { 9 } else { index });
        let id = commit(
            &fixture.repository,
            "long path",
            &parent.into_iter().collect::<Vec<_>>(),
            &[(path, text.as_bytes())],
            Some("HEAD"),
        );
        ids.push(id.to_string());
        parent = Some(id);
    }
    let workspace = fixture.workspace();
    let info = fixture.open(&workspace);
    let sha = parent.unwrap().to_string();
    let key = workspace.commit_details(&info.id, &sha).unwrap().files[0]
        .file_key
        .clone();
    let first = workspace.file_history(&info.id, &sha, &key, None).unwrap();
    assert_eq!(first.entries.len(), 40);
    let cursor = first.next_cursor.unwrap();
    assert!(cursor.len() > 33 * 1024);
    let second = workspace
        .file_history(&info.id, &sha, &key, Some(&cursor))
        .unwrap();
    let all: Vec<_> = first
        .entries
        .into_iter()
        .chain(second.entries)
        .map(|entry| entry.sha)
        .collect();
    ids.reverse();
    assert_eq!(all, ids);
    assert!(second.next_cursor.is_none());
}

#[test]
fn file_blame_ignores_unrelated_large_modifications_for_a_stable_path() {
    let fixture = Fixture::new();
    let initial = commit(
        &fixture.repository,
        "initial",
        &[],
        &[("file.txt", b"stable\n")],
        Some("HEAD"),
    );
    let huge = vec![b'x'; 2 * 1024 * 1024 + 1];
    let changed = commit(
        &fixture.repository,
        "unrelated large file",
        &[initial],
        &[("file.txt", b"stable\n"), ("other.txt", &huge)],
        Some("HEAD"),
    );
    let workspace = fixture.workspace();
    let info = fixture.open(&workspace);
    let blame = workspace
        .file_blame(&info.id, &changed.to_string(), "66696c652e747874")
        .unwrap();
    assert_eq!(blame.groups[0].sha, initial.to_string());
    assert!(blame.limitation.is_none());
}

#[test]
fn file_blame_reports_literal_pathspec_limits_without_confusing_other_files() {
    for (literal, matching) in [
        ("*.txt", "other.txt"),
        ("?.txt", "a.txt"),
        ("[ab].txt", "a.txt"),
        ("a\\ file.txt", "a file.txt"),
    ] {
        let fixture = Fixture::new();
        let initial = commit(
            &fixture.repository,
            "initial",
            &[],
            &[(literal, b"stable\n"), (matching, b"before\n")],
            Some("HEAD"),
        );
        let changed = commit(
            &fixture.repository,
            "other file changed",
            &[initial],
            &[(literal, b"stable\n"), (matching, b"after\n")],
            Some("HEAD"),
        );
        let workspace = fixture.workspace();
        let info = fixture.open(&workspace);
        let details = workspace
            .commit_details(&info.id, &initial.to_string())
            .unwrap();
        let key = &details
            .files
            .iter()
            .find(|file| file.path == literal)
            .unwrap()
            .file_key;
        assert_eq!(
            workspace
                .file_content(&info.id, &changed.to_string(), key, PreviewLimit::MIN)
                .unwrap()
                .lines,
            ["stable"]
        );
        let history = workspace
            .file_history(&info.id, &changed.to_string(), key, None)
            .unwrap();
        assert_eq!(history.entries.len(), 1);
        assert_eq!(history.entries[0].sha, initial.to_string());
        assert!(workspace
            .file_blame(&info.id, &changed.to_string(), key)
            .unwrap_err()
            .contains("attribution littérale"));
    }
}

#[test]
fn file_blame_stops_before_a_rename_to_an_unsafe_older_pathspec() {
    let fixture = Fixture::new();
    let initial = commit(
        &fixture.repository,
        "initial",
        &[],
        &[("*.txt", b"stable\n"), ("other.txt", b"before\n")],
        Some("HEAD"),
    );
    let changed = commit(
        &fixture.repository,
        "other file changed",
        &[initial],
        &[("*.txt", b"stable\n"), ("other.txt", b"after\n")],
        Some("HEAD"),
    );
    let renamed = commit(
        &fixture.repository,
        "rename to literal path",
        &[changed],
        &[("safe.txt", b"stable\n"), ("other.txt", b"after\n")],
        Some("HEAD"),
    );
    let workspace = fixture.workspace();
    let info = fixture.open(&workspace);
    let blame = workspace
        .file_blame(&info.id, &renamed.to_string(), "736166652e747874")
        .unwrap();
    assert_eq!(blame.groups.len(), 1);
    assert_eq!(blame.groups[0].sha, renamed.to_string());
    assert!(blame.groups[0].boundary);
    assert!(blame.limitation.unwrap().contains("attribution littérale"));
}

#[test]
fn solo_walk_uses_selected_roots_all_merge_parents_and_shared_ancestors() {
    let fixture = Fixture::new();
    let root = commit(&fixture.repository, "common root", &[], &[], Some("HEAD"));
    let main = commit(
        &fixture.repository,
        "main exclusive",
        &[root],
        &[],
        Some("HEAD"),
    );
    let left = commit(
        &fixture.repository,
        "feature left",
        &[root],
        &[],
        Some("refs/heads/feature"),
    );
    let right = commit(
        &fixture.repository,
        "merged side",
        &[root],
        &[],
        Some("refs/heads/side"),
    );
    let merge = commit(
        &fixture.repository,
        "feature merge",
        &[left, right],
        &[],
        Some("refs/heads/feature"),
    );
    let remote = commit(
        &fixture.repository,
        "remote exclusive",
        &[root],
        &[],
        Some("refs/remotes/origin/topic"),
    );
    let before = snapshot(&fixture.source);
    let workspace = fixture.workspace();
    let solo = workspace
        .open_repository_with_solo(
            fixture.source.to_str().unwrap(),
            &["refs/heads/feature".into()],
        )
        .unwrap();
    finish(&workspace, &solo.id);
    assert_eq!(solo.head, Some(main.to_string()));
    assert_eq!(solo.solo_refs, ["refs/heads/feature"]);
    let page = workspace.list_commits(&solo.id, 0, 256, "").unwrap();
    let ids: Vec<_> = page
        .commits
        .iter()
        .map(|commit| commit.sha.clone())
        .collect();
    assert_eq!(page.total, 4);
    for id in [root, left, right, merge] {
        assert!(ids.contains(&id.to_string()));
    }
    for id in [main, remote] {
        assert!(!ids.contains(&id.to_string()));
    }
    assert_eq!(page.commits[0].sha, merge.to_string());
    assert_eq!(
        page.commits[0]
            .graph
            .edges
            .iter()
            .filter(|edge| edge.kind == GraphEdgeKind::Parent)
            .count(),
        2
    );
    assert_eq!(page.commits.last().unwrap().graph.width, 1);
    assert_eq!(
        workspace
            .list_commits(&solo.id, 0, 10, "exclusive")
            .unwrap()
            .total,
        0
    );
    assert!(workspace
        .commit_position(&solo.id, &main.to_string())
        .unwrap()
        .is_none());
    let first = workspace.list_commits(&solo.id, 0, 2, "").unwrap();
    let second = workspace.list_commits(&solo.id, 2, 2, "").unwrap();
    assert_eq!(first.commits.len() + second.commits.len(), 4);
    assert_ne!(first.commits[1].sha, second.commits[0].sha);
    let union = workspace
        .open_repository_with_solo(
            fixture.source.to_str().unwrap(),
            &[
                "refs/heads/feature".into(),
                "refs/remotes/origin/topic".into(),
            ],
        )
        .unwrap();
    assert!(workspace.list_commits(&solo.id, 0, 1, "").is_err());
    finish(&workspace, &union.id);
    assert_eq!(
        workspace.list_commits(&union.id, 0, 20, "").unwrap().total,
        5
    );
    assert_eq!(
        workspace
            .list_commits(&union.id, 0, 20, "exclusive")
            .unwrap()
            .total,
        1
    );
    let full = fixture.open(&workspace);
    finish(&workspace, &full.id);
    assert!(full.solo_refs.is_empty());
    assert_eq!(
        workspace.list_commits(&full.id, 0, 20, "").unwrap().total,
        6
    );
    assert_eq!(before, snapshot(&fixture.source));
}

#[test]
fn solo_refresh_tracks_external_branch_moves_and_deletions() {
    let fixture = Fixture::new();
    let root = commit(&fixture.repository, "root", &[], &[], Some("HEAD"));
    fixture
        .repository
        .reference("refs/heads/topic", root, false, "setup")
        .unwrap();
    let workspace = fixture.workspace();
    let first = workspace
        .open_repository_with_solo(
            fixture.source.to_str().unwrap(),
            &["refs/heads/topic".into()],
        )
        .unwrap();
    finish(&workspace, &first.id);
    commit(
        &fixture.repository,
        "external topic update",
        &[root],
        &[],
        Some("refs/heads/topic"),
    );
    let second = workspace
        .open_repository_with_solo(
            fixture.source.to_str().unwrap(),
            &["refs/heads/topic".into()],
        )
        .unwrap();
    finish(&workspace, &second.id);
    assert_ne!(first.id, second.id);
    assert_eq!(
        workspace.list_commits(&second.id, 0, 10, "").unwrap().total,
        2
    );
    fixture
        .repository
        .find_reference("refs/heads/topic")
        .unwrap()
        .delete()
        .unwrap();
    let before = snapshot(&fixture.source);
    let refreshed = workspace
        .open_repository_with_solo(
            fixture.source.to_str().unwrap(),
            &["refs/heads/topic".into()],
        )
        .unwrap();
    finish(&workspace, &refreshed.id);
    assert!(refreshed.solo_refs.is_empty());
    assert_eq!(
        workspace
            .list_commits(&refreshed.id, 0, 10, "")
            .unwrap()
            .total,
        1
    );
    assert_eq!(before, snapshot(&fixture.source));
}

#[test]
fn worktree_listing_from_a_linked_tree_preserves_missing_locked_and_detached_states() {
    let fixture = Fixture::new();
    let head = commit(
        &fixture.repository,
        "root",
        &[],
        &[("file.txt", b"base\n")],
        Some("HEAD"),
    );
    let linked_path = fixture.root.path().join("linked");
    let linked = fixture
        .repository
        .worktree("linked", &linked_path, None)
        .unwrap();
    linked.lock(Some("keep this tree")).unwrap();
    Repository::open(&linked_path)
        .unwrap()
        .set_head_detached(head)
        .unwrap();
    let missing_path = fixture.root.path().join("missing");
    fixture
        .repository
        .worktree("missing", &missing_path, None)
        .unwrap();
    fs::remove_dir_all(&missing_path).unwrap();
    let before = snapshot(&fixture.source);
    let before_linked = snapshot(&linked_path);
    let workspace = fixture.workspace();
    let info = workspace
        .open_repository(linked_path.to_str().unwrap())
        .unwrap();
    finish(&workspace, &info.id);
    let list = workspace.list_worktrees(&info.id).unwrap();
    assert_eq!(list.worktrees.len(), 3);
    let primary = list.worktrees.iter().find(|item| item.primary).unwrap();
    assert_eq!(
        primary.path,
        fixture.source.canonicalize().unwrap().to_string_lossy()
    );
    assert!(!primary.current);
    let linked = list
        .worktrees
        .iter()
        .find(|item| item.name == "linked")
        .unwrap();
    assert!(linked.available && linked.current && linked.detached);
    assert_eq!(linked.locked.as_deref(), Some("keep this tree"));
    let missing = list
        .worktrees
        .iter()
        .find(|item| item.name == "missing")
        .unwrap();
    assert!(!missing.available);
    let missing_directory = Path::new(&missing.path);
    assert!(missing_directory.is_absolute());
    assert_eq!(missing_directory.file_name(), missing_path.file_name());
    // Windows canonicalization adds a verbatim prefix; the missing directory
    // retains the Git registration spelling. Compare its existing parent.
    assert_eq!(
        missing_directory.parent().unwrap().canonicalize().unwrap(),
        missing_path.parent().unwrap().canonicalize().unwrap()
    );
    assert!(missing.reason.is_some());
    let main = workspace.open_repository(&primary.path).unwrap();
    assert_eq!(
        main.path,
        fixture.source.canonicalize().unwrap().to_string_lossy()
    );
    assert_eq!(before, snapshot(&fixture.source));
    assert_eq!(before_linked, snapshot(&linked_path));
}

#[test]
fn stash_zones_use_distinct_saved_trees_and_do_not_touch_the_source() {
    let mut fixture = Fixture::new();
    let base = commit(
        &fixture.repository,
        "baseline",
        &[],
        &[("both.txt", b"base\n")],
        Some("HEAD"),
    );
    fixture
        .repository
        .checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
        .unwrap();
    fs::write(fixture.source.join("both.txt"), b"staged\n").unwrap();
    let mut index = fixture.repository.index().unwrap();
    index.add_path(Path::new("both.txt")).unwrap();
    index.write().unwrap();
    drop(index);
    fs::write(fixture.source.join("both.txt"), b"unstaged\n").unwrap();
    fs::write(fixture.source.join("non suivi.txt"), b"untracked\n").unwrap();
    let signature = Signature::now("Stash Author", "stash@example.test").unwrap();
    let sha = fixture
        .repository
        .stash_save(
            &signature,
            "three zones",
            Some(git2::StashFlags::INCLUDE_UNTRACKED),
        )
        .unwrap();
    let before = snapshot(&fixture.source);
    let workspace = fixture.workspace();
    let info = fixture.open(&workspace);
    finish(&workspace, &info.id);
    let list = workspace.list_stashes(&info.id).unwrap();
    assert_eq!(list.stashes.len(), 1);
    assert_eq!(list.stashes[0].sha, sha.to_string());
    assert_eq!(list.stashes[0].base_sha, base.to_string());
    let details = workspace.stash_details(&info.id, &sha.to_string()).unwrap();
    assert_eq!(details.groups.len(), 3);
    for (area, expected_old, expected_new) in [
        ("index", Some("base"), "staged"),
        ("worktree", Some("staged"), "unstaged"),
        ("untracked", None, "untracked"),
    ] {
        let group = details
            .groups
            .iter()
            .find(|group| group.area == area)
            .unwrap();
        assert_eq!(group.file_count, 1);
        let diff = workspace
            .stash_diff(
                &info.id,
                &sha.to_string(),
                area,
                &group.files[0].file_key,
                PreviewLimit::MIN,
            )
            .unwrap();
        let images = workspace
            .file_images(
                &info.id,
                &FileImageSource::Stash {
                    sha: sha.to_string(),
                    area: area.into(),
                    file_key: group.files[0].file_key.clone(),
                },
            )
            .unwrap();
        assert_eq!(
            image_bytes(&images.after),
            format!("{expected_new}\n").as_bytes()
        );
        if let Some(expected_old) = expected_old {
            assert_eq!(
                image_bytes(images.before.as_ref().unwrap()),
                format!("{expected_old}\n").as_bytes()
            );
        } else {
            assert!(matches!(images.before, Some(FileImageData::Absent)));
        }
        let lines: Vec<_> = diff.hunks.iter().flat_map(|hunk| &hunk.lines).collect();
        assert!(lines
            .iter()
            .any(|line| line.kind == "addition" && line.content == expected_new));
        if let Some(old) = expected_old {
            assert!(lines
                .iter()
                .any(|line| line.kind == "deletion" && line.content == old));
        }
    }
    assert!(workspace
        .stash_diff(
            &info.id,
            &sha.to_string(),
            "invalid",
            "626f74682e747874",
            PreviewLimit::MIN
        )
        .is_err());
    assert!(workspace
        .stash_details(&info.id, &base.to_string())
        .is_err());
    assert_eq!(
        workspace.list_commits(&info.id, 0, 10, "").unwrap().total,
        1
    );
    assert_eq!(before, snapshot(&fixture.source));
    fixture.repository.stash_drop(0).unwrap();
    let after_external_drop = snapshot(&fixture.source);
    assert!(workspace.list_stashes(&info.id).unwrap().stashes.is_empty());
    assert!(workspace.stash_details(&info.id, &sha.to_string()).is_err());
    assert_eq!(after_external_drop, snapshot(&fixture.source));
}

#[test]
fn empty_stashes_and_bare_worktrees_have_explicit_read_only_states() {
    let fixture = Fixture::new();
    let workspace = fixture.workspace();
    let info = fixture.open(&workspace);
    assert!(workspace.list_stashes(&info.id).unwrap().stashes.is_empty());
    let bare_path = fixture.root.path().join("bare.git");
    Repository::init_bare(&bare_path).unwrap();
    let before = snapshot(&bare_path);
    let bare = workspace
        .open_repository(bare_path.to_str().unwrap())
        .unwrap();
    let trees = workspace.list_worktrees(&bare.id).unwrap();
    assert_eq!(trees.worktrees.len(), 1);
    assert!(!trees.worktrees[0].available);
    assert!(trees.worktrees[0].reason.is_some());
    assert_eq!(before, snapshot(&bare_path));
}

#[test]
fn worktree_listing_preserves_separate_git_directory_and_unborn_branch() {
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
    Repository::open(&worktree)
        .unwrap()
        .set_head("refs/heads/main")
        .unwrap();
    let cache = tempfile::tempdir().unwrap();
    let before = snapshot(root.path());
    let workspace = Workspace::new(cache.path()).unwrap();
    let info = workspace
        .open_repository(worktree.to_str().unwrap())
        .unwrap();
    finish(&workspace, &info.id);
    let list = workspace.list_worktrees(&info.id).unwrap();
    assert_eq!(list.worktrees.len(), 1);
    let primary = &list.worktrees[0];
    assert_eq!(primary.path, info.path);
    assert!(primary.primary && primary.available && primary.current);
    assert_eq!(primary.head_name, "main");
    assert!(primary.head.is_none());
    assert_eq!(before, snapshot(root.path()));
}

#[test]
fn worktree_listing_handles_non_utf8_lock_reason_and_missing_head_metadata() {
    let fixture = Fixture::new();
    let head = commit(&fixture.repository, "root", &[], &[], Some("HEAD"));
    let linked_path = fixture.root.path().join("linked");
    fixture
        .repository
        .worktree("linked", &linked_path, None)
        .unwrap();
    let registration = fixture.repository.commondir().join("worktrees/linked");
    fs::write(registration.join("locked"), b"keep \xff\n").unwrap();
    let workspace = fixture.workspace();
    let info = fixture.open(&workspace);
    finish(&workspace, &info.id);
    let before = snapshot(&fixture.source);
    let before_linked = snapshot(&linked_path);
    let list = workspace.list_worktrees(&info.id).unwrap();
    let linked = list
        .worktrees
        .iter()
        .find(|item| item.name == "linked")
        .unwrap();
    assert!(linked.available);
    assert_eq!(linked.locked.as_deref(), Some("keep �"));
    assert_eq!(before, snapshot(&fixture.source));
    assert_eq!(before_linked, snapshot(&linked_path));

    fs::remove_dir_all(&linked_path).unwrap();
    let before = snapshot(&fixture.source);
    let list = workspace.list_worktrees(&info.id).unwrap();
    let linked = list
        .worktrees
        .iter()
        .find(|item| item.name == "linked")
        .unwrap();
    assert!(!linked.available);
    assert_eq!(linked.head.as_deref(), Some(head.to_string().as_str()));
    assert_eq!(linked.head_name, "linked");
    assert!(!linked.detached);
    assert_eq!(linked.locked.as_deref(), Some("keep �"));
    assert_eq!(before, snapshot(&fixture.source));

    fs::write(registration.join("HEAD"), format!("{head}\n")).unwrap();
    let before = snapshot(&fixture.source);
    let list = workspace.list_worktrees(&info.id).unwrap();
    let linked = list
        .worktrees
        .iter()
        .find(|item| item.name == "linked")
        .unwrap();
    assert!(!linked.available && linked.detached);
    assert_eq!(linked.head.as_deref(), Some(head.to_string().as_str()));
    assert_eq!(linked.head_name, "HEAD détachée");
    assert_eq!(before, snapshot(&fixture.source));
}

#[test]
fn two_parent_stashes_preserve_binary_and_large_diff_limits_without_untracked_group() {
    let mut fixture = Fixture::new();
    commit(
        &fixture.repository,
        "baseline",
        &[],
        &[
            ("binary.bin", b"\0before"),
            ("large.txt", b"small\n"),
            ("text.txt", b"before\n"),
        ],
        Some("HEAD"),
    );
    fixture
        .repository
        .checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
        .unwrap();
    fs::write(fixture.source.join("binary.bin"), b"\0after").unwrap();
    fs::write(
        fixture.source.join("large.txt"),
        vec![b'x'; 2 * 1024 * 1024 + 1],
    )
    .unwrap();
    fs::write(fixture.source.join("text.txt"), b"after\n").unwrap();
    fs::write(fixture.source.join("not-stashed.txt"), b"untracked\n").unwrap();
    let signature = Signature::now("Stash Author", "stash@example.test").unwrap();
    let sha = fixture
        .repository
        .stash_save(&signature, "two parents", None)
        .unwrap();
    assert_eq!(
        fixture.repository.find_commit(sha).unwrap().parent_count(),
        2
    );
    let before = snapshot(&fixture.source);
    let workspace = fixture.workspace();
    let info = fixture.open(&workspace);
    finish(&workspace, &info.id);
    let details = workspace.stash_details(&info.id, &sha.to_string()).unwrap();
    assert_eq!(details.groups.len(), 2);
    assert_eq!(
        details
            .groups
            .iter()
            .find(|group| group.area == "index")
            .unwrap()
            .file_count,
        0
    );
    let group = details
        .groups
        .iter()
        .find(|group| group.area == "worktree")
        .unwrap();
    assert_eq!(group.file_count, 3);
    for (path, binary, truncated) in [
        ("binary.bin", true, false),
        ("large.txt", false, true),
        ("text.txt", false, false),
    ] {
        let file = group.files.iter().find(|file| file.path == path).unwrap();
        let diff = workspace
            .stash_diff(
                &info.id,
                &sha.to_string(),
                "worktree",
                &file.file_key,
                PreviewLimit::MIN,
            )
            .unwrap();
        assert_eq!(diff.binary, binary);
        assert_eq!(diff.truncated, truncated);
        if binary || truncated {
            assert!(diff.hunks.is_empty());
        } else {
            assert!(patch_text(&diff).contains("addition:after"));
        }
    }
    assert!(workspace
        .stash_diff(
            &info.id,
            &sha.to_string(),
            "untracked",
            &group.files[0].file_key,
            PreviewLimit::MIN,
        )
        .is_err());
    assert_eq!(before, snapshot(&fixture.source));
}

#[test]
fn linked_repository_names_follow_branch_unborn_and_detached_head_in_read_only_mode() {
    let fixture = Fixture::new();
    let head = commit(&fixture.repository, "root", &[], &[], Some("HEAD"));
    fixture
        .repository
        .reference("refs/heads/t3code/issue-4-readable", head, false, "setup")
        .unwrap();
    let linked_path = fixture.root.path().join("t3code-fab52430");
    fixture
        .repository
        .worktree("readable", &linked_path, None)
        .unwrap();
    let linked = Repository::open(&linked_path).unwrap();
    linked
        .set_head("refs/heads/t3code/issue-4-readable")
        .unwrap();
    let nested_path = linked_path.join("nested");
    fs::create_dir(&nested_path).unwrap();
    let before = snapshot(&fixture.source);
    let before_linked = snapshot(&linked_path);
    let paths = vec![nested_path.to_string_lossy().into_owned()];
    let names = mangrove_core::repository_names(&paths).unwrap();
    assert_eq!(names[0].path, paths[0]);
    assert_eq!(
        names[0].repository_path.as_deref(),
        linked_path.canonicalize().unwrap().to_str()
    );
    assert_eq!(
        names[0].primary_path.as_deref(),
        fixture.source.canonicalize().unwrap().to_str()
    );
    assert_eq!(
        names[0].name.as_deref(),
        Some("repository · t3code/issue-4-readable")
    );
    assert!(!fixture.cache.exists());
    assert_eq!(before, snapshot(&fixture.source));
    assert_eq!(before_linked, snapshot(&linked_path));

    let workspace = fixture.workspace();
    for (reference, expected_name) in [
        (
            Some("refs/heads/t3code/issue-4-readable"),
            "repository · t3code/issue-4-readable".into(),
        ),
        (
            Some("refs/heads/t3code/unborn-readable"),
            "repository · t3code/unborn-readable".into(),
        ),
        (
            None,
            format!("repository · HEAD détachée · {}", &head.to_string()[..7]),
        ),
    ] {
        if let Some(reference) = reference {
            linked.set_head(reference).unwrap();
        } else {
            linked.set_head_detached(head).unwrap();
        }
        let before = snapshot(&fixture.source);
        let before_linked = snapshot(&linked_path);
        let info = workspace.open_repository(&paths[0]).unwrap();
        finish(&workspace, &info.id);
        let before_cache = snapshot(&fixture.cache);
        let names = mangrove_core::repository_names(&paths).unwrap();
        assert_eq!(info.name, expected_name);
        assert_eq!(names[0].name.as_deref(), Some(info.name.as_str()));
        assert_eq!(before_cache, snapshot(&fixture.cache));
        assert_eq!(before, snapshot(&fixture.source));
        assert_eq!(before_linked, snapshot(&linked_path));
    }
}

#[test]
fn repository_names_keep_main_folder_names_original_paths_and_six_entry_limit() {
    let fixture = Fixture::new();
    fixture
        .repository
        .set_head("refs/heads/another-branch")
        .unwrap();
    let bare_path = fixture.root.path().join("bare.git");
    Repository::init_bare(&bare_path).unwrap();
    let missing_path = fixture.root.path().join("missing");
    let empty_path = fixture.root.path().join("not-a-repository");
    fs::create_dir(&empty_path).unwrap();
    let file_path = fixture.root.path().join("plain-file");
    fs::write(&file_path, b"file contents").unwrap();
    let nested_path = fixture.source.join("nested");
    fs::create_dir(&nested_path).unwrap();
    let paths = vec![
        fixture.source.to_string_lossy().into_owned(),
        bare_path.to_string_lossy().into_owned(),
        missing_path.to_string_lossy().into_owned(),
        empty_path.to_string_lossy().into_owned(),
        file_path.to_string_lossy().into_owned(),
        format!("  {}  ", nested_path.to_string_lossy()),
    ];
    let before = snapshot(fixture.root.path());
    let names = mangrove_core::repository_names(&paths).unwrap();
    assert_eq!(names.len(), 6);
    for ((entry, path), expected) in names.iter().zip(&paths).zip([
        Some("repository"),
        Some("bare.git"),
        None,
        None,
        None,
        Some("repository"),
    ]) {
        assert_eq!(&entry.path, path);
        assert_eq!(entry.name.as_deref(), expected);
        assert!(entry.primary_path.is_none());
    }
    assert!(serde_json::to_value(&names[2]).unwrap()["name"].is_null());
    assert!(names[2].repository_path.is_none());
    assert_eq!(
        names[5].repository_path.as_deref(),
        fixture.source.canonicalize().unwrap().to_str()
    );
    assert!(mangrove_core::repository_names(&[]).unwrap().is_empty());
    assert!(mangrove_core::repository_names(&vec![paths[0].clone(); 7]).is_err());
    assert_eq!(before, snapshot(fixture.root.path()));
    let workspace = fixture.workspace();
    let info = fixture.open(&workspace);
    finish(&workspace, &info.id);
    assert_eq!(info.name, "repository");
}

#[cfg(unix)]
#[test]
fn repository_names_return_null_for_inaccessible_repositories() {
    use std::os::unix::fs::PermissionsExt;

    let fixture = Fixture::new();
    let original_permissions = fs::metadata(&fixture.source).unwrap().permissions();
    let before = snapshot(&fixture.source);
    fs::set_permissions(&fixture.source, fs::Permissions::from_mode(0o0)).unwrap();
    let names = mangrove_core::repository_names(&[fixture.source.to_string_lossy().into_owned()]);
    fs::set_permissions(&fixture.source, original_permissions).unwrap();
    let names = names.unwrap();
    assert!(names[0].name.is_none());
    assert_eq!(before, snapshot(&fixture.source));
}

#[test]
fn linked_repository_names_use_main_workdir_or_bare_repository_for_context() {
    let root = tempfile::tempdir().unwrap();
    let primary_path = root.path().join("principal-project");
    let administration = root.path().join("git-administration");
    git_at(
        root.path(),
        &[
            "init",
            "--separate-git-dir",
            administration.to_str().unwrap(),
            primary_path.to_str().unwrap(),
        ],
    );
    let primary = Repository::open(&primary_path).unwrap();
    primary.set_head("refs/heads/main").unwrap();
    primary
        .config()
        .unwrap()
        .set_str("core.worktree", primary_path.to_str().unwrap())
        .unwrap();
    commit(&primary, "root", &[], &[], Some("HEAD"));
    let linked_path = root.path().join("t3code-012345");
    primary.worktree("readable", &linked_path, None).unwrap();

    let bare_path = root.path().join("principal.git");
    let bare = Repository::init_bare(&bare_path).unwrap();
    bare.set_head("refs/heads/main").unwrap();
    commit(&bare, "root", &[], &[], Some("HEAD"));
    let bare_linked_path = root.path().join("t3code-987654");
    bare.worktree("bare-topic", &bare_linked_path, None)
        .unwrap();

    let before = snapshot(root.path());
    let paths = vec![
        linked_path.to_string_lossy().into_owned(),
        bare_linked_path.to_string_lossy().into_owned(),
    ];
    let names = mangrove_core::repository_names(&paths).unwrap();
    assert_eq!(
        names[0].name.as_deref(),
        Some("principal-project · readable")
    );
    assert_eq!(names[1].name.as_deref(), Some("principal.git · bare-topic"));
    assert_eq!(
        names[0].primary_path.as_deref(),
        primary_path.canonicalize().unwrap().to_str()
    );
    assert_eq!(
        names[1].primary_path.as_deref(),
        bare_path.canonicalize().unwrap().to_str()
    );
    let cache = tempfile::tempdir().unwrap();
    let workspace = Workspace::new(cache.path()).unwrap();
    for (path, expected) in paths.iter().zip(&names) {
        let info = workspace.open_repository(path).unwrap();
        finish(&workspace, &info.id);
        assert_eq!(Some(info.name.as_str()), expected.name.as_deref());
    }
    assert_eq!(before, snapshot(root.path()));
}
