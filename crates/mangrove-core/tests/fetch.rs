use std::{fs, path::Path, thread, time::Duration};

use git2::{Oid, Repository, Signature, Time};
use mangrove_core::Workspace;

fn commit(repository: &Repository, parent: Option<Oid>, message: &str) -> Oid {
    let mut builder = repository.treebuilder(None).unwrap();
    let blob = repository.blob(message.as_bytes()).unwrap();
    builder.insert("file.txt", blob, 0o100644).unwrap();
    let tree = repository.find_tree(builder.write().unwrap()).unwrap();
    let signature =
        Signature::new("Test", "test@example.test", &Time::new(1_700_000_000, 0)).unwrap();
    let parent = parent.map(|id| repository.find_commit(id).unwrap());
    repository
        .commit(
            Some("refs/heads/main"),
            &signature,
            &signature,
            message,
            &tree,
            &parent.iter().collect::<Vec<_>>(),
        )
        .unwrap()
}

fn remote(path: &Path) -> Repository {
    let repository = Repository::init_bare(path).unwrap();
    repository.set_head("refs/heads/main").unwrap();
    repository
}

#[test]
fn fetches_all_remotes_without_changing_local_branches_index_or_dirty_files() {
    let root = tempfile::tempdir().unwrap();
    let origin = remote(&root.path().join("origin"));
    let initial = commit(&origin, None, "initial");
    let source = root.path().join("source");
    let local = Repository::clone(origin.path().to_str().unwrap(), &source).unwrap();
    let upstream = remote(&root.path().join("upstream"));
    let upstream_tip = commit(&upstream, None, "upstream");
    local
        .remote("upstream", upstream.path().to_str().unwrap())
        .unwrap();
    let tip = commit(&origin, Some(initial), "new remote commit");
    // A mirror-style refspec must never move a local branch during auto-fetch.
    local
        .config()
        .unwrap()
        .set_str("remote.origin.fetch", "+refs/heads/*:refs/heads/*")
        .unwrap();
    fs::write(source.join("file.txt"), "local edits\n").unwrap();
    fs::write(source.join("untracked.txt"), "untracked\n").unwrap();
    let index = fs::read(local.path().join("index")).unwrap();
    let config = fs::read(local.path().join("config")).unwrap();
    let head = fs::read(local.path().join("HEAD")).unwrap();
    let workspace = Workspace::new(root.path().join("cache")).unwrap();
    let info = workspace.open_repository(source.to_str().unwrap()).unwrap();

    assert!(workspace
        .fetch_repository(&info.id)
        .unwrap()
        .error
        .is_none());
    assert_eq!(
        local
            .find_reference("refs/remotes/origin/main")
            .unwrap()
            .target(),
        Some(tip)
    );
    assert_eq!(
        local
            .find_reference("refs/remotes/upstream/main")
            .unwrap()
            .target(),
        Some(upstream_tip)
    );
    assert_eq!(
        local.find_reference("refs/heads/main").unwrap().target(),
        Some(initial)
    );
    assert_eq!(fs::read(local.path().join("index")).unwrap(), index);
    assert_eq!(fs::read(local.path().join("config")).unwrap(), config);
    assert_eq!(fs::read(local.path().join("HEAD")).unwrap(), head);
    assert_eq!(
        fs::read_to_string(source.join("file.txt")).unwrap(),
        "local edits\n"
    );
    assert_eq!(
        fs::read_to_string(source.join("untracked.txt")).unwrap(),
        "untracked\n"
    );

    let refreshed = workspace.open_repository(source.to_str().unwrap()).unwrap();
    assert_ne!(refreshed.id, info.id);
    for _ in 0..300 {
        if workspace.index_status(&refreshed.id).unwrap().complete {
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    let page = workspace.list_commits(&refreshed.id, 0, 20, "").unwrap();
    assert!(page.commits.iter().any(|row| row.sha == tip.to_string()));
    assert!(page
        .commits
        .iter()
        .any(|row| row.sha == upstream_tip.to_string()));
}

#[test]
fn reports_failed_remotes_and_preserves_successful_fetches() {
    let root = tempfile::tempdir().unwrap();
    let origin = remote(&root.path().join("origin"));
    let initial = commit(&origin, None, "initial");
    let source = root.path().join("source");
    let local = Repository::clone(origin.path().to_str().unwrap(), &source).unwrap();
    local
        .remote("broken", root.path().join("missing").to_str().unwrap())
        .unwrap();
    let tip = commit(&origin, Some(initial), "next");
    let workspace = Workspace::new(root.path().join("cache")).unwrap();
    let info = workspace.open_repository(source.to_str().unwrap()).unwrap();
    let result = workspace.fetch_repository(&info.id).unwrap();
    assert!(result.error.unwrap().contains("broken"));
    assert_eq!(
        local
            .find_reference("refs/remotes/origin/main")
            .unwrap()
            .target(),
        Some(tip)
    );
    assert!(workspace.fetch_repository("unknown-session").is_err());
}

#[test]
fn supports_bare_repositories_and_respects_skip_fetch_all() {
    let root = tempfile::tempdir().unwrap();
    let origin = remote(&root.path().join("origin"));
    let tip = commit(&origin, None, "initial");
    let source = root.path().join("source");
    let local = remote(&source);
    local
        .remote("origin", origin.path().to_str().unwrap())
        .unwrap();
    local
        .remote("ignored", root.path().join("missing").to_str().unwrap())
        .unwrap();
    local
        .config()
        .unwrap()
        .set_bool("remote.ignored.skipFetchAll", true)
        .unwrap();
    let workspace = Workspace::new(root.path().join("cache")).unwrap();
    let info = workspace.open_repository(source.to_str().unwrap()).unwrap();
    assert!(workspace
        .fetch_repository(&info.id)
        .unwrap()
        .error
        .is_none());
    assert_eq!(
        local
            .find_reference("refs/remotes/origin/main")
            .unwrap()
            .target(),
        Some(tip)
    );
}
