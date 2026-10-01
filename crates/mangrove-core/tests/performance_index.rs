use std::{
    fs,
    path::PathBuf,
    thread,
    time::{Duration, Instant},
};

use git2::{Oid, Repository, Signature, Time};
use mangrove_core::Workspace;
use rusqlite::Connection;

struct Fixture {
    _root: tempfile::TempDir,
    source: PathBuf,
    cache: PathBuf,
    repository: Repository,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source");
        let cache = root.path().join("cache");
        let repository = Repository::init(&source).unwrap();
        repository.set_head("refs/heads/main").unwrap();
        Self {
            _root: root,
            source,
            cache,
            repository,
        }
    }

    fn commit(&self, message: &str, author: &str, parent: Option<Oid>, time: i64) -> Oid {
        let tree = self
            .repository
            .find_tree(self.repository.treebuilder(None).unwrap().write().unwrap())
            .unwrap();
        let signature = Signature::new(author, "author@example.org", &Time::new(time, 0)).unwrap();
        let parent = parent.map(|oid| self.repository.find_commit(oid).unwrap());
        self.repository
            .commit(
                Some("HEAD"),
                &signature,
                &signature,
                message,
                &tree,
                &parent.iter().collect::<Vec<_>>(),
            )
            .unwrap()
    }

    fn database(&self) -> PathBuf {
        fs::read_dir(&self.cache)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .find(|path| path.extension().is_some_and(|ext| ext == "sqlite"))
            .unwrap()
    }
}

fn finish(workspace: &Workspace, id: &str) {
    let started = Instant::now();
    loop {
        let progress = workspace.index_status(id).unwrap();
        assert!(progress.error.is_none(), "{:?}", progress.error);
        if progress.complete {
            return;
        }
        assert!(started.elapsed() < Duration::from_secs(120));
        thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn filtered_snapshots_preserve_matches_order_and_invalidate_on_ref_and_solo_changes() {
    let fixture = Fixture::new();
    let mut parent = None;
    let mut matches = Vec::new();
    let mut all = Vec::new();
    for i in 0..520 {
        let message = if i % 3 == 0 {
            "fix café alpha"
        } else {
            "other beta"
        };
        let oid = fixture.commit(
            message,
            if i % 2 == 0 { "Alice" } else { "Bob" },
            parent,
            1_700_000_000 + i,
        );
        parent = Some(oid);
        all.push(oid);
        if i % 3 == 0 {
            matches.push(oid.to_string());
        }
    }
    matches.reverse();
    let workspace = Workspace::new(&fixture.cache).unwrap();
    let info = workspace
        .open_repository(fixture.source.to_str().unwrap())
        .unwrap();
    finish(&workspace, &info.id);
    let mut actual = Vec::new();
    for offset in [0, 128] {
        let page = workspace
            .list_commits(&info.id, offset, 128, "  FIX café  ")
            .unwrap();
        assert_eq!(page.total, matches.len());
        assert_eq!(page.index.indexed, all.len());
        actual.extend(page.commits.into_iter().map(|row| row.sha));
    }
    assert_eq!(actual, matches);
    assert!(workspace
        .list_commits(&info.id, matches.len(), 128, "FIX café")
        .unwrap()
        .commits
        .is_empty());
    assert_eq!(
        workspace
            .list_commits(&info.id, 0, 128, "Alice beta")
            .unwrap()
            .total,
        173
    );
    let chosen = all[7];
    assert_eq!(
        workspace
            .list_commits(
                &info.id,
                0,
                128,
                &chosen.to_string().to_ascii_uppercase()[..12]
            )
            .unwrap()
            .commits[0]
            .sha,
        chosen.to_string()
    );
    assert_eq!(
        workspace
            .list_commits(&info.id, 0, 128, "pin_%")
            .unwrap()
            .total,
        0
    );
    fixture
        .repository
        .reference("refs/heads/pin_%", chosen, false, "test")
        .unwrap();
    let refreshed = workspace
        .open_repository(fixture.source.to_str().unwrap())
        .unwrap();
    assert_eq!(refreshed.index.indexed, all.len());
    assert!(!refreshed.index.complete);
    finish(&workspace, &refreshed.id);
    let pinned = workspace
        .list_commits(&refreshed.id, 0, 128, "pin_%")
        .unwrap();
    assert_eq!(pinned.total, 1);
    assert_eq!(pinned.commits[0].sha, chosen.to_string());
    let solo = workspace
        .open_repository_with_solo(
            fixture.source.to_str().unwrap(),
            &["refs/heads/pin_%".into()],
        )
        .unwrap();
    assert_eq!(solo.index.indexed, all.len());
    finish(&workspace, &solo.id);
    assert_eq!(
        workspace
            .list_commits(&solo.id, 0, 128, "example")
            .unwrap()
            .total,
        8
    );
    assert_eq!(
        workspace
            .list_commits(&solo.id, 0, 128, "Alice beta")
            .unwrap()
            .total,
        2
    );
    assert!(workspace
        .list_commits(&info.id, 0, 128, "FIX café")
        .is_err());
}

#[test]
fn buffered_publication_preserves_old_history_until_new_walk_and_refs_are_ready() {
    let fixture = Fixture::new();
    let root = fixture.commit("root", "Alice", None, 1_700_000_000);
    let mut parent = root;
    for i in 1..1025 {
        parent = fixture.commit("history", "Alice", Some(parent), 1_700_000_000 + i);
    }
    fixture
        .repository
        .reference("refs/heads/root-only", root, false, "test")
        .unwrap();
    let workspace = Workspace::new(&fixture.cache).unwrap();
    let old = workspace
        .open_repository(fixture.source.to_str().unwrap())
        .unwrap();
    finish(&workspace, &old.id);
    let old_page = workspace.list_commits(&old.id, 0, 10, "").unwrap();
    let reader = Connection::open(fixture.database()).unwrap();
    reader.execute_batch("BEGIN").unwrap();
    let old_run: String = reader
        .query_row(
            "SELECT value FROM meta WHERE key = 'index_run'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(
        reader
            .query_row::<i64, _, _>("SELECT COUNT(*) FROM walk", [], |row| row.get(0))
            .unwrap(),
        1025
    );
    let new = workspace
        .open_repository_with_solo(
            fixture.source.to_str().unwrap(),
            &["refs/heads/root-only".into()],
        )
        .unwrap();
    assert_eq!(new.index.indexed, 1025);
    assert_eq!(new.index.index_run, old_run);
    assert!(!new.index.complete);
    let started = Instant::now();
    loop {
        assert!(started.elapsed() < Duration::from_secs(30));
        let (position, progress) = workspace
            .commit_position_with_status(&new.id, &root.to_string())
            .unwrap();
        if progress.index_run == old_run {
            assert_eq!(position, Some(1024));
            assert_eq!(progress.indexed, 1025);
        } else {
            assert_eq!(position, Some(0));
            assert_eq!(progress.indexed, 1);
            assert!(progress.complete);
        }
        let page = workspace.list_commits(&new.id, 0, 10, "").unwrap();
        if page.index.index_run == old_run {
            assert_eq!(page.total, 1025);
            assert_eq!(page.commits[0].sha, old_page.commits[0].sha);
        } else {
            assert!(page.index.complete);
            assert_eq!(page.total, 1);
            assert_eq!(page.commits[0].sha, root.to_string());
            break;
        }
        thread::sleep(Duration::from_millis(1));
    }
    // A read transaction started before publication stays coherent through the
    // table rename, including its traversal run and old commit positions.
    assert_eq!(
        reader
            .query_row::<String, _, _>(
                "SELECT value FROM meta WHERE key = 'index_run'",
                [],
                |row| row.get(0)
            )
            .unwrap(),
        old_run
    );
    assert_eq!(
        reader
            .query_row::<i64, _, _>("SELECT COUNT(*) FROM walk", [], |row| row.get(0))
            .unwrap(),
        1025
    );
    reader.execute_batch("ROLLBACK").unwrap();
    assert_eq!(
        workspace
            .commit_position(&new.id, &root.to_string())
            .unwrap(),
        Some(0)
    );
    assert_eq!(
        workspace
            .commit_position(&new.id, &parent.to_string())
            .unwrap(),
        None
    );
}

#[test]
fn version_two_cache_migration_keeps_commit_metadata_and_published_run() {
    let fixture = Fixture::new();
    let root = fixture.commit("searchable", "Alice", None, 1_700_000_000);
    let (run, commit_id) = {
        let workspace = Workspace::new(&fixture.cache).unwrap();
        let info = workspace
            .open_repository(fixture.source.to_str().unwrap())
            .unwrap();
        finish(&workspace, &info.id);
        let connection = Connection::open(fixture.database()).unwrap();
        let run = workspace.index_status(&info.id).unwrap().index_run;
        let id = connection
            .query_row::<i64, _, _>(
                "SELECT id FROM commits WHERE sha = ?1",
                [root.to_string()],
                |row| row.get(0),
            )
            .unwrap();
        (run, id)
    };
    let connection = Connection::open(fixture.database()).unwrap();
    connection
        .execute_batch(
            "DROP TABLE walk_build; DROP TABLE refs_build; ALTER TABLE walk DROP COLUMN parents;
         DELETE FROM meta WHERE key IN ('lease_run', 'lease_fingerprint', 'indexed');
         PRAGMA user_version=2;",
        )
        .unwrap();
    let workspace = Workspace::new(&fixture.cache).unwrap();
    let info = workspace
        .open_repository(fixture.source.to_str().unwrap())
        .unwrap();
    assert!(info.index.complete);
    assert_eq!(info.index.index_run, run);
    assert_eq!(info.index.indexed, 1);
    let page = workspace
        .list_commits(&info.id, 0, 128, "searchable")
        .unwrap();
    assert_eq!(page.total, 1);
    assert!(page.commits[0].parents.is_empty());
    assert_eq!(
        connection
            .query_row::<i64, _, _>(
                "SELECT id FROM commits WHERE sha = ?1",
                [root.to_string()],
                |row| row.get(0)
            )
            .unwrap(),
        commit_id
    );
}

#[cfg(unix)]
#[test]
fn pooled_reads_recheck_cache_aliases_before_reusing_a_connection() {
    let fixture = Fixture::new();
    fixture.commit("root", "Alice", None, 1_700_000_000);
    let workspace = Workspace::new(&fixture.cache).unwrap();
    let info = workspace
        .open_repository(fixture.source.to_str().unwrap())
        .unwrap();
    finish(&workspace, &info.id);
    workspace.list_commits(&info.id, 0, 128, "").unwrap();
    let original = fixture.source.join("valuable.sqlite");
    fs::write(&original, b"source contents").unwrap();
    let database = fixture.database();
    fs::remove_file(&database).unwrap();
    std::os::unix::fs::symlink(&original, &database).unwrap();
    assert!(workspace.list_commits(&info.id, 0, 128, "").is_err());
    assert_eq!(fs::read(original).unwrap(), b"source contents");
}

#[test]
fn cache_removal_after_pooled_reads_reopens_with_a_new_generation() {
    let fixture = Fixture::new();
    let oid = fixture.commit("cached history", "Alice", None, 1_700_000_000);
    let workspace = Workspace::new(&fixture.cache).unwrap();
    let old = workspace
        .open_repository(fixture.source.to_str().unwrap())
        .unwrap();
    finish(&workspace, &old.id);
    assert_eq!(
        workspace.list_commits(&old.id, 0, 128, "").unwrap().total,
        1
    );
    assert_eq!(
        workspace
            .list_commits(&old.id, 0, 128, "cached")
            .unwrap()
            .total,
        1
    );
    fs::remove_dir_all(&fixture.cache).unwrap();
    let new = workspace
        .open_repository(fixture.source.to_str().unwrap())
        .unwrap();
    assert_ne!(new.id, old.id);
    finish(&workspace, &new.id);
    let page = workspace.list_commits(&new.id, 0, 128, "cached").unwrap();
    assert_eq!(page.total, 1);
    assert_eq!(page.commits[0].sha, oid.to_string());
    assert!(workspace.list_commits(&old.id, 0, 128, "cached").is_err());
}

#[test]
#[ignore = "native release search benchmark; requires MANGROVE_PERF_REPOSITORY"]
fn benchmark_filtered_pages() {
    let repository = std::env::var("MANGROVE_PERF_REPOSITORY").expect("MANGROVE_PERF_REPOSITORY");
    let cache = tempfile::tempdir().unwrap();
    let workspace = Workspace::new(cache.path()).unwrap();
    let info = workspace.open_repository(&repository).unwrap();
    finish(&workspace, &info.id);
    let count = workspace.index_status(&info.id).unwrap().indexed;
    let mut results = Vec::new();
    for query in ["", "example", "Alice"] {
        let start = Instant::now();
        let first = workspace.list_commits(&info.id, 0, 128, query).unwrap();
        let first_ms = start.elapsed().as_secs_f64() * 1000.;
        for offset in [0, first.total / 4, first.total.saturating_sub(128)] {
            let mut samples = Vec::new();
            for _ in 0..10 {
                let started = Instant::now();
                let page = workspace
                    .list_commits(&info.id, offset, 128, query)
                    .unwrap();
                assert_eq!(page.total, first.total);
                std::hint::black_box(&page);
                samples.push(started.elapsed().as_secs_f64() * 1000.);
            }
            samples.sort_by(f64::total_cmp);
            results.push(serde_json::json!({"query":query,"offset":offset,"matches":first.total,"firstPageMs":first_ms,"p50Ms":samples[4],"p95Ms":samples[9],"samples":samples}));
        }
    }
    let report = serde_json::json!({"commits":count,"results":results});
    println!("{report}");
    if let Ok(path) = std::env::var("MANGROVE_PERF_REPORT") {
        fs::write(path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    }
}
