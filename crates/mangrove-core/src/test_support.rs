//! Fixtures shared by the engine's unit tests.
use git2::{Oid, Repository};

/// A commit of exactly `files`, as `(path, contents, mode)`.
pub(crate) fn commit(
    repository: &Repository,
    parents: &[Oid],
    files: &[(&[u8], &[u8], i32)],
) -> Oid {
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
