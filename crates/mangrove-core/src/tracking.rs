//! Upstreams of local branches, resolved from one configuration snapshot
//! without walking the commit graph.
use std::collections::HashMap;

use git2::{Direction, Remote, Repository};

use crate::{model::*, Result};

/// `branch.<name>.remote` and `branch.<name>.merge`; empty when unset.
#[derive(Default)]
struct BranchConfig {
    remote: String,
    merge: String,
}

/// libgit2's `git_branch_upstream_name` copies the whole configuration and
/// looks the remote up again for each branch. Reading every branch section
/// once keeps the cost linear in branches plus configuration entries.
pub(crate) fn branches(repository: &Repository, references: &[Reference]) -> Result<TrackingList> {
    let configs = branch_configs(repository)?;
    let shas: HashMap<_, _> = references
        .iter()
        .map(|reference| (reference.name.as_str(), reference.sha.as_str()))
        .collect();
    let mut remotes: HashMap<&str, std::result::Result<Remote, UpstreamState>> = HashMap::new();
    let mut branches = Vec::new();
    // Names that are not UTF-8 are `git-bytes:` keys and have no section.
    for reference in references
        .iter()
        .filter(|reference| reference.kind == "local")
    {
        let Some(config) = reference
            .name
            .strip_prefix("refs/heads/")
            .and_then(|name| configs.get(name))
            .filter(|config| !config.remote.is_empty() && !config.merge.is_empty())
        else {
            continue;
        };
        let resolved = if config.remote == "." {
            Ok(config.merge.clone())
        } else {
            let remote = remotes.entry(&config.remote).or_insert_with(|| {
                repository
                    .find_remote(&config.remote)
                    .map_err(|error| match error.code() {
                        git2::ErrorCode::NotFound => UpstreamState::Gone {
                            reason: UpstreamGone::MissingRemote,
                        },
                        // An invalid remote name or fetch refspec, for example.
                        _ => UpstreamState::Invalid {
                            error: error.message().to_owned(),
                        },
                    })
            });
            match remote {
                Ok(remote) => tracking_name(remote, &config.merge).ok_or(UpstreamState::Gone {
                    reason: UpstreamGone::NoMatchingRefspec,
                }),
                Err(state) => Err(state.clone()),
            }
        };
        let (upstream, state) = match resolved {
            Ok(name) => {
                // Symbolic and custom upstreams are not in the sidebar's list.
                let sha = shas
                    .get(name.as_str())
                    .map(|sha| (*sha).to_owned())
                    .or_else(|| {
                        repository
                            .find_reference(&name)
                            .and_then(|reference| reference.peel_to_commit())
                            .ok()
                            .map(|commit| commit.id().to_string())
                    });
                let state = match sha {
                    Some(sha) => UpstreamState::Tracking { sha },
                    None => UpstreamState::Gone {
                        reason: UpstreamGone::MissingRef,
                    },
                };
                (Some(name), state)
            }
            Err(state) => (None, state),
        };
        let label = match &upstream {
            Some(name) => shorthand(name).to_owned(),
            None => format!("{}/{}", config.remote, shorthand(&config.merge)),
        };
        branches.push(BranchTracking {
            branch: reference.name.clone(),
            sha: reference.sha.clone(),
            remote: config.remote.clone(),
            upstream,
            label,
            state,
        });
    }
    Ok(TrackingList { branches })
}

fn branch_configs(repository: &Repository) -> Result<HashMap<String, BranchConfig>> {
    let config = repository
        .config()
        .and_then(|mut config| config.snapshot())
        .map_err(|e| e.to_string())?;
    let mut entries = config
        .entries(Some(r"^branch\..+\.(remote|merge)$"))
        .map_err(|e| e.to_string())?;
    let mut configs: HashMap<String, BranchConfig> = HashMap::new();
    // Levels come from the lowest priority to the highest, each in file
    // order: the last value read is the one Git uses.
    while let Some(entry) = entries.next() {
        let entry = entry.map_err(|e| e.to_string())?;
        // Variable names are lowercase; branch names keep their case.
        let Some((branch, variable)) = entry
            .name()
            .ok()
            .and_then(|name| name.strip_prefix("branch.")?.rsplit_once('.'))
        else {
            continue;
        };
        let value = if entry.has_value() {
            entry.value().unwrap_or_default()
        } else {
            ""
        };
        let config = configs.entry(branch.to_owned()).or_default();
        if variable == "remote" {
            config.remote = value.into();
        } else {
            config.merge = value.into();
        }
    }
    Ok(configs)
}

/// The remote-tracking reference of `merge`, like libgit2's matching
/// refspec: the first fetch refspec whose source matches, unless a negative
/// refspec excludes the name.
fn tracking_name(remote: &Remote, merge: &str) -> Option<String> {
    let mut matched = None;
    for refspec in remote
        .refspecs()
        .filter(|refspec| refspec.direction() == Direction::Fetch)
    {
        if refspec.src_bytes().starts_with(b"^") {
            // libgit2 matches the rest of a negative source against the name.
            // The same literal first character on both sides keeps that match.
            if refspec.src_matches(&format!("^{merge}")) {
                return None;
            }
        } else if matched.is_none() && refspec.src_matches(merge) {
            matched = Some(refspec);
        }
    }
    let name = matched?.transform(merge).ok()?;
    let name = name.as_str().ok()?;
    // A refspec that stores nothing leaves no reference to track. A short
    // destination is qualified as libgit2 does when it reads the remote.
    if name.is_empty() {
        None
    } else if name.starts_with("refs/") {
        Some(name.to_owned())
    } else if name.starts_with("remotes/") {
        Some(format!("refs/{name}"))
    } else {
        Some(format!("refs/heads/{name}"))
    }
}

/// Git's short name of a reference: `origin/main` for `refs/remotes/origin/main`.
fn shorthand(name: &str) -> &str {
    ["refs/heads/", "refs/tags/", "refs/remotes/", "refs/"]
        .iter()
        .find_map(|prefix| name.strip_prefix(prefix))
        .unwrap_or(name)
}
