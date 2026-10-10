use std::{collections::HashMap, fs, path::Path};

use git2::{Oid, Repository, Signature, Time};
use mangrove_core::{
    model::{BranchTracking, TrackingList, UpstreamGone, UpstreamState},
    Workspace,
};
use tempfile::TempDir;

fn commit(repository: &Repository, parents: &[Oid], message: &str) -> Oid {
    let mut builder = repository.treebuilder(None).unwrap();
    let blob = repository.blob(message.as_bytes()).unwrap();
    builder.insert("file.txt", blob, 0o100644).unwrap();
    let tree = repository.find_tree(builder.write().unwrap()).unwrap();
    let signature =
        Signature::new("Test", "test@example.test", &Time::new(1_700_000_000, 0)).unwrap();
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
