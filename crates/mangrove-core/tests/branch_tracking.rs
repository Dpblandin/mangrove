use std::{
    collections::HashMap,
    fs,
    path::Path,
    process::Command,
    sync::atomic::{AtomicI64, Ordering},
    time::{Duration, Instant},
};

use git2::{Oid, Repository, Signature, Time};
use mangrove_core::{
    model::{AheadBehind, BranchTracking, TrackingList, UpstreamGone, UpstreamState},
    Workspace, MAX_AHEAD_BEHIND_PAIRS,
};
use tempfile::TempDir;

/// Commit dates increase, as Git's graph walks expect.
fn commit(repository: &Repository, parents: &[Oid], message: &str) -> Oid {
    static TIMESTAMP: AtomicI64 = AtomicI64::new(1_700_000_000);
    let mut builder = repository.treebuilder(None).unwrap();
    let blob = repository.blob(message.as_bytes()).unwrap();
    builder.insert("file.txt", blob, 0o100644).unwrap();
    let tree = repository.find_tree(builder.write().unwrap()).unwrap();
    let time = Time::new(TIMESTAMP.fetch_add(60, Ordering::Relaxed), 0);
    let signature = Signature::new("Test", "test@example.test", &time).unwrap();
    let parents: Vec<_> = parents
        .iter()
        .map(|id| repository.find_commit(*id).unwrap())
        .collect();
    repository
        .commit(
            None,
            &signature,
            &signature,
            message,
            &tree,
            &parents.iter().collect::<Vec<_>>(),
        )
        .unwrap()
}

fn reference(repository: &Repository, name: &str, target: Oid) {
    repository.reference(name, target, true, "test").unwrap();
}

/// Writes `branch.<name>.remote` and `branch.<name>.merge`, unless empty.
fn track(repository: &Repository, branch: &str, remote: &str, merge: &str) {
    let mut config = repository.config().unwrap();
    for (variable, value) in [("remote", remote), ("merge", merge)] {
        if !value.is_empty() {
            config
                .set_str(&format!("branch.{branch}.{variable}"), value)
                .unwrap();
        }
    }
}

fn add_remote(repository: &Repository, name: &str, fetch: &[&str]) {
    let mut config = repository.config().unwrap();
    config
        .set_str(&format!("remote.{name}.url"), &format!("../{name}.git"))
        .unwrap();
    for refspec in fetch {
        config
            .set_multivar(&format!("remote.{name}.fetch"), "^$", refspec)
            .unwrap();
    }
}

fn tracking(workspace: &Workspace, path: &Path) -> (String, TrackingList) {
    let info = workspace.open_repository(path.to_str().unwrap()).unwrap();
    let list = workspace.branch_tracking(&info.id).unwrap();
    (info.id, list)
}

fn by_branch(list: &TrackingList) -> HashMap<&str, &BranchTracking> {
    list.branches
        .iter()
        .map(|branch| (branch.branch.as_str(), branch))
        .collect()
}

fn gone(reason: UpstreamGone) -> UpstreamState {
    UpstreamState::Gone { reason }
}

struct Fixture {
    root: TempDir,
    source: std::path::PathBuf,
    repository: Repository,
    tip: Oid,
}

/// Points libgit2's global, XDG and system levels at one isolated global
/// file, before any test of this binary reads a configuration: the user's
/// own never applies, and both global sections below do.
fn isolate_configuration() {
    static GLOBAL: std::sync::OnceLock<TempDir> = std::sync::OnceLock::new();
    GLOBAL.get_or_init(|| {
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join(".gitconfig"),
            "[branch \"global-only\"]\n\tremote = origin\n\tmerge = refs/heads/main\n\
             [branch \"overridden\"]\n\tremote = origin\n\tmerge = refs/heads/old\n",
        )
        .unwrap();
        for level in [
            git2::ConfigLevel::Global,
            git2::ConfigLevel::XDG,
            git2::ConfigLevel::System,
        ] {
            // SAFETY: every test calls this first; `get_or_init` makes the
            // others wait, so no libgit2 call runs while the paths change.
            unsafe { git2::opts::set_search_path(level, directory.path()) }.unwrap();
        }
        directory
    });
}

fn fixture() -> Fixture {
    isolate_configuration();
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("repository");
    let repository = Repository::init(&source).unwrap();
    repository.set_head("refs/heads/main").unwrap();
    let tip = commit(&repository, &[], "initial");
    Fixture {
        root,
        source,
        repository,
        tip,
    }
}

#[test]
fn upstreams_match_libgit2_in_every_configuration_state() {
    let Fixture {
        root,
        source,
        repository,
        tip,
    } = fixture();
    let ahead = commit(&repository, &[tip], "ahead");
    add_remote(
        &repository,
        "origin",
        &[
            "+refs/heads/*:refs/remotes/origin/*",
            "^refs/heads/private/*",
        ],
    );
    add_remote(&repository, "mirror", &["+refs/heads/*:refs/mirror/*"]);
    add_remote(&repository, "short", &["+refs/heads/*:remotes/short/*"]);
    add_remote(
        &repository,
        "narrow",
        &["+refs/heads/main:refs/remotes/narrow/main"],
    );
    // libgit2 refuses a glob source with a plain destination.
    add_remote(
        &repository,
        "invalid",
        &["+refs/heads/*:refs/remotes/invalid"],
    );
    for name in [
        "main",
        "feature/x.y",
        "local",
        "gone",
        "removed",
        "private/wip",
        "custom",
        "short-name",
        "narrow",
        "merge-only",
        "remote-only",
        "twice",
        "case",
        "untracked",
        "invalid-refspec",
        "invalid-name",
        "global-only",
        "overridden",
        "included",
        "per-worktree",
    ] {
        reference(&repository, &format!("refs/heads/{name}"), tip);
    }
    reference(&repository, "refs/remotes/origin/main", ahead);
    reference(&repository, "refs/remotes/origin/feature/x.y", tip);
    // Remote branches of the same name do not make an upstream.
    reference(&repository, "refs/remotes/origin/untracked", ahead);
    reference(&repository, "refs/remotes/origin/private/wip", tip);
    reference(&repository, "refs/mirror/custom", ahead);
    reference(&repository, "refs/remotes/short/short-name", ahead);
    for (branch, remote, merge) in [
        ("main", "origin", "refs/heads/main"),
        ("feature/x.y", "origin", "refs/heads/feature/x.y"),
        ("local", ".", "refs/heads/main"),
        ("gone", "origin", "refs/heads/gone"),
        ("removed", "deleted", "refs/heads/main"),
        ("private/wip", "origin", "refs/heads/private/wip"),
        ("custom", "mirror", "refs/heads/custom"),
        ("short-name", "short", "refs/heads/short-name"),
        ("narrow", "narrow", "refs/heads/narrow"),
        ("merge-only", "", "refs/heads/main"),
        ("remote-only", "origin", ""),
        ("twice", "origin", "refs/heads/old"),
        // Branch sections are case sensitive; an unborn branch has no SHA.
        ("CASE", "origin", "refs/heads/main"),
        ("unborn", "origin", "refs/heads/main"),
        ("invalid-refspec", "invalid", "refs/heads/main"),
        ("invalid-name", "not a remote", "refs/heads/main"),
        // The local level overrides the global one.
        ("overridden", "", "refs/heads/main"),
        ("per-worktree", "origin", "refs/heads/old"),
    ] {
        track(&repository, branch, remote, merge);
    }
    repository
        .config()
        .unwrap()
        .set_multivar("branch.twice.merge", "^$", "refs/heads/main")
        .unwrap();
    // An included file, and a worktree level above the local one.
    fs::write(
        repository.path().join("included.config"),
        "[branch \"included\"]\n\tremote = origin\n\tmerge = refs/heads/feature/x.y\n",
    )
    .unwrap();
    fs::write(
        repository.path().join("config.worktree"),
        "[branch \"per-worktree\"]\n\tmerge = refs/heads/main\n",
    )
    .unwrap();
    let mut local = git2::Config::open(&repository.path().join("config")).unwrap();
    local.set_str("include.path", "included.config").unwrap();
    local.set_i32("core.repositoryformatversion", 1).unwrap();
    local.set_bool("extensions.worktreeConfig", true).unwrap();

    let workspace = Workspace::new(root.path().join("cache")).unwrap();
    let info = workspace.open_repository(source.to_str().unwrap()).unwrap();
    let list = workspace.branch_tracking(&info.id).unwrap();
    let branches = by_branch(&list);
    // A new handle loads the worktree level enabled after the first one.
    let repository = Repository::open(&source).unwrap();
    for reference in info
        .references
        .iter()
        .filter(|reference| reference.kind == "local")
    {
        let expected = repository
            .branch_upstream_name(&reference.name)
            .ok()
            .map(|name| name.as_str().unwrap().to_owned());
        assert_eq!(
            branches
                .get(reference.name.as_str())
                .and_then(|branch| branch.upstream.clone()),
            expected,
            "{}",
            reference.name
        );
    }
    let state = |branch: &str| {
        let branch = branches[format!("refs/heads/{branch}").as_str()];
        (branch.label.as_str(), branch.state.clone())
    };
    let tracking = |sha: Oid| UpstreamState::Tracking {
        sha: sha.to_string(),
    };
    assert_eq!(state("main"), ("origin/main", tracking(ahead)));
    assert_eq!(state("feature/x.y"), ("origin/feature/x.y", tracking(tip)));
    assert_eq!(state("local"), ("main", tracking(tip)));
    assert_eq!(
        state("gone"),
        ("origin/gone", gone(UpstreamGone::MissingRef))
    );
    assert_eq!(
        state("removed"),
        ("deleted/main", gone(UpstreamGone::MissingRemote))
    );
    assert_eq!(
        state("private/wip"),
        ("origin/private/wip", gone(UpstreamGone::NoMatchingRefspec))
    );
    assert_eq!(state("custom"), ("mirror/custom", tracking(ahead)));
    assert_eq!(state("short-name"), ("short/short-name", tracking(ahead)));
    assert_eq!(
        state("narrow"),
        ("narrow/narrow", gone(UpstreamGone::NoMatchingRefspec))
    );
    assert_eq!(state("twice"), ("origin/main", tracking(ahead)));
    for branch in ["global-only", "overridden", "per-worktree"] {
        assert_eq!(state(branch), ("origin/main", tracking(ahead)), "{branch}");
    }
    assert_eq!(state("included"), ("origin/feature/x.y", tracking(tip)));
    for (branch, label) in [
        ("invalid-refspec", "invalid/main"),
        ("invalid-name", "not a remote/main"),
    ] {
        let (shown, state) = state(branch);
        assert_eq!(shown, label);
        assert!(
            matches!(state, UpstreamState::Invalid { ref error } if !error.is_empty()),
            "{branch}: {state:?}"
        );
    }
    let main = branches["refs/heads/main"];
    assert_eq!(main.sha, tip.to_string());
    assert_eq!(main.remote, "origin");
    assert_eq!(branches["refs/heads/local"].remote, ".");
    assert_eq!(list.branches.len(), 16);
    for absent in ["merge-only", "remote-only", "case", "untracked", "unborn"] {
        assert!(!branches.contains_key(format!("refs/heads/{absent}").as_str()));
    }
    // The list follows the sidebar order of the references.
    let names: Vec<_> = list.branches.iter().map(|branch| &branch.branch).collect();
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted);
}

#[test]
fn changing_an_upstream_without_moving_a_ref_keeps_the_session() {
    let Fixture {
        root,
        source,
        repository,
        tip,
    } = fixture();
    add_remote(
        &repository,
        "origin",
        &["+refs/heads/*:refs/remotes/origin/*"],
    );
    reference(&repository, "refs/heads/main", tip);
    reference(&repository, "refs/remotes/origin/main", tip);
    reference(&repository, "refs/remotes/origin/next", tip);
    let workspace = Workspace::new(root.path().join("cache")).unwrap();
    let (id, list) = tracking(&workspace, &source);
    assert!(list.branches.is_empty());

    track(&repository, "main", "origin", "refs/heads/main");
    let (same, list) = tracking(&workspace, &source);
    assert_eq!(same, id);
    assert_eq!(list.branches[0].label, "origin/main");

    track(&repository, "main", "", "refs/heads/next");
    let (same, list) = tracking(&workspace, &source);
    assert_eq!(same, id);
    assert_eq!(list.branches[0].label, "origin/next");
    assert!(workspace.branch_tracking("closed-session").is_err());
}

#[test]
fn linked_worktrees_bare_repositories_and_detached_heads_share_the_configuration() {
    let Fixture {
        root,
        source,
        repository,
        tip,
    } = fixture();
    reference(&repository, "refs/heads/main", tip);
    reference(&repository, "refs/heads/feature", tip);
    reference(&repository, "refs/remotes/origin/feature", tip);
    add_remote(
        &repository,
        "origin",
        &["+refs/heads/*:refs/remotes/origin/*"],
    );
    track(&repository, "feature", "origin", "refs/heads/feature");
    // A name that is not UTF-8 is only a `git-bytes:` key: it has no section.
    let mut packed = format!("{tip} ").into_bytes();
    packed.extend_from_slice(b"refs/heads/caf\xe9\n");
    fs::write(repository.path().join("packed-refs"), packed).unwrap();
    let linked = root.path().join("linked");
    repository.worktree("linked", &linked, None).unwrap();
    repository.set_head_detached(tip).unwrap();
    let workspace = Workspace::new(root.path().join("cache")).unwrap();
    for path in [&source, &linked] {
        let (_, list) = tracking(&workspace, path);
        assert_eq!(list.branches.len(), 1, "{}", path.display());
        assert_eq!(list.branches[0].branch, "refs/heads/feature");
        assert_eq!(list.branches[0].label, "origin/feature");
    }

    let bare_path = root.path().join("bare.git");
    let bare = Repository::init_bare(&bare_path).unwrap();
    let bare_tip = commit(&bare, &[], "bare");
    reference(&bare, "refs/heads/main", bare_tip);
    track(&bare, "main", ".", "refs/heads/main");
    let (_, list) = tracking(&workspace, &bare_path);
    assert_eq!(
        list.branches[0].state,
        UpstreamState::Tracking {
            sha: bare_tip.to_string()
        }
    );
}

fn pair(local: Oid, upstream: Oid) -> (String, String) {
    (local.to_string(), upstream.to_string())
}

fn counts(ahead: usize, behind: usize) -> Option<AheadBehind> {
    Some(AheadBehind::Counts { ahead, behind })
}

fn far() -> Instant {
    Instant::now() + Duration::from_secs(60)
}

/// `git rev-list --left-right --count local...upstream`.
fn rev_list(path: &Path, local: Oid, upstream: Oid) -> (usize, usize) {
    let output = Command::new("git")
        .current_dir(path)
        .args(["rev-list", "--left-right", "--count"])
        .arg(format!("{local}...{upstream}"))
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_COMMON_DIR")
        .env(
            "GIT_CONFIG_GLOBAL",
            if cfg!(windows) { "NUL" } else { "/dev/null" },
        )
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let text = String::from_utf8(output.stdout).unwrap();
    let (ahead, behind) = text.trim().split_once('\t').unwrap();
    (ahead.parse().unwrap(), behind.parse().unwrap())
}

/// A five-commit main line with a branch from its second commit, a merge of
/// that branch and a history without common ancestor.
struct Graph {
    fixture: Fixture,
    main: Vec<Oid>,
    feature: Oid,
    merge: Oid,
    orphan: Oid,
}

fn graph() -> Graph {
    let fixture = fixture();
    let mut main = vec![fixture.tip];
    for index in 1..6 {
        main.push(commit(&fixture.repository, &[main[index - 1]], "main"));
    }
    let first = commit(&fixture.repository, &[main[2]], "feature");
    let feature = commit(&fixture.repository, &[first], "feature");
    let merge = commit(&fixture.repository, &[main[5], feature], "merge");
    let root = commit(&fixture.repository, &[], "orphan");
    let orphan = commit(&fixture.repository, &[root], "orphan");
    reference(&fixture.repository, "refs/heads/main", main[5]);
    Graph {
        fixture,
        main,
        feature,
        merge,
        orphan,
    }
}

#[test]
fn counts_match_libgit2_and_git_rev_list() {
    let Graph {
        fixture,
        main,
        feature,
        merge,
        orphan,
    } = graph();
    let cases = [
        ((feature, main[5]), (2, 3)),
        ((merge, main[5]), (3, 0)),
        ((main[1], main[5]), (0, 4)),
        ((main[3], main[3]), (0, 0)),
        ((orphan, main[5]), (2, 6)),
    ];
    let workspace = Workspace::new(fixture.root.path().join("cache")).unwrap();
    let info = workspace
        .open_repository(fixture.source.to_str().unwrap())
        .unwrap();
    let pairs: Vec<_> = cases.iter().map(|((l, u), _)| pair(*l, *u)).collect();
    let result = workspace
        .ahead_behind(&info.id, &pairs, far(), &|| false)
        .unwrap();
    for (((local, upstream), expected), count) in cases.into_iter().zip(result) {
        assert_eq!(count, counts(expected.0, expected.1));
        assert_eq!(
            fixture
                .repository
                .graph_ahead_behind(local, upstream)
                .unwrap(),
            expected
        );
        assert_eq!(rev_list(&fixture.source, local, upstream), expected);
    }
}

#[test]
fn counting_stops_between_pairs_at_the_deadline_or_when_cancelled() {
    let Graph {
        fixture,
        main,
        feature,
        merge,
        orphan,
    } = graph();
    let workspace = Workspace::new(fixture.root.path().join("cache")).unwrap();
    let info = workspace
        .open_repository(fixture.source.to_str().unwrap())
        .unwrap();
    let pairs = [
        pair(feature, main[5]),
        pair(main[3], main[3]),
        pair(merge, main[5]),
        pair(orphan, main[5]),
    ];
    let count = |deadline, cancelled: bool| {
        workspace
            .ahead_behind(&info.id, &pairs, deadline, &|| cancelled)
            .unwrap()
    };
    // Equal commits need no walk.
    assert_eq!(count(far(), true), [None, counts(0, 0), None, None]);
    // A past deadline still computes one pair; computed pairs are cached.
    let past = Instant::now();
    assert_eq!(count(past, false), [counts(2, 3), counts(0, 0), None, None]);
    assert_eq!(
        count(past, false),
        [counts(2, 3), counts(0, 0), counts(3, 0), None]
    );
    assert_eq!(
        count(far(), false),
        [counts(2, 3), counts(0, 0), counts(3, 0), counts(2, 6)]
    );

    let missing = "1".repeat(40);
    let result = workspace
        .ahead_behind(&info.id, &[(missing, main[5].to_string())], far(), &|| {
            false
        })
        .unwrap();
    assert!(matches!(&result[0], Some(AheadBehind::Unavailable { .. })));
    assert!(workspace
        .ahead_behind(&info.id, &[("main".into(), "HEAD".into())], far(), &|| {
            false
        })
        .is_err());
    let too_many = vec![pair(feature, main[5]); MAX_AHEAD_BEHIND_PAIRS + 1];
    assert!(workspace
        .ahead_behind(&info.id, &too_many, far(), &|| false)
        .is_err());
}

#[test]
fn counts_outlive_refreshes_and_serve_linked_worktrees_until_shallow_boundaries_or_grafts_change() {
    let Graph {
        fixture,
        main,
        feature,
        merge,
        ..
    } = graph();
    let workspace = Workspace::new(fixture.root.path().join("cache")).unwrap();
    let pairs = [pair(feature, main[5]), pair(merge, main[5])];
    let cached = |id: &str| workspace.ahead_behind(id, &pairs, far(), &|| true).unwrap();
    let (id, list) = tracking(&workspace, &fixture.source);
    assert!(!list.shallow);
    assert_eq!(cached(&id), [None, None]);
    workspace
        .ahead_behind(&id, &pairs, far(), &|| false)
        .unwrap();

    // A fetch renews the session when it moves a ref.
    reference(&fixture.repository, "refs/remotes/origin/main", main[5]);
    let (refreshed, refreshed_list) = tracking(&workspace, &fixture.source);
    assert_ne!(refreshed, id);
    assert_eq!(refreshed_list.counts_key, list.counts_key);
    assert!(workspace
        .ahead_behind(&id, &pairs, far(), &|| false)
        .is_err());
    assert_eq!(cached(&refreshed), [counts(2, 3), counts(3, 0)]);
    let linked = fixture.root.path().join("linked");
    fixture
        .repository
        .worktree("linked", &linked, None)
        .unwrap();
    let (worktree, worktree_list) = tracking(&workspace, &linked);
    assert_eq!(worktree_list.counts_key, list.counts_key);
    assert_eq!(cached(&worktree), [counts(2, 3), counts(3, 0)]);

    // A graft renews the session, even without a ref or shallow change.
    let info = fixture.repository.path().join("info");
    fs::create_dir_all(&info).unwrap();
    fs::write(info.join("grafts"), format!("{}\n", main[3])).unwrap();
    let (grafted, grafted_list) = tracking(&workspace, &fixture.source);
    assert_ne!(grafted, refreshed);
    assert_ne!(grafted_list.counts_key, list.counts_key);
    assert_eq!(cached(&grafted), [None, None]);
    fs::remove_file(info.join("grafts")).unwrap();

    fs::write(
        fixture.repository.path().join("shallow"),
        format!("{}\n", main[1]),
    )
    .unwrap();
    let (shallow, shallow_list) = tracking(&workspace, &fixture.source);
    assert!(shallow_list.shallow);
    assert_ne!(shallow_list.counts_key, list.counts_key);
    assert_eq!(cached(&shallow), [None, None]);
}
