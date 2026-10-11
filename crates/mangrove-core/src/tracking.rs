//! Upstreams of local branches, resolved from one configuration snapshot
//! without walking the commit graph, and their ahead/behind counts.
use std::{
    collections::HashMap,
    path::Path,
    sync::{Arc, Mutex},
    time::Instant,
};

use git2::{Direction, Oid, Remote, Repository};

use crate::{model::*, Result};

/// About 3 MiB of counts: every pair of 5 000 tracked branches survives
/// several fetches that each move hundreds of upstreams.
const MAX_CACHED_PAIRS: usize = 32_768;

/// `branch.<name>.remote` and `branch.<name>.merge`; empty when unset.
#[derive(Default)]
struct BranchConfig {
    remote: String,
    merge: String,
}

/// libgit2's `git_branch_upstream_name` copies the whole configuration and
/// looks the remote up again for each branch. Reading every branch section
/// once keeps the cost linear in branches plus configuration entries.
pub(crate) fn branches(
    repository: &Repository,
    references: &[Reference],
    shallow: bool,
    counts_key: &str,
) -> Result<TrackingList> {
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
    Ok(TrackingList {
        branches,
        shallow,
        counts_key: counts_key.to_owned(),
    })
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

/// Counts by pair of commits. The linked worktrees of a repository share its
/// objects, so its counts; sessions renewed by a fetch keep them. Shallow
/// boundaries change them, so they get another cache.
#[derive(Default)]
pub(crate) struct CountCache {
    current: HashMap<(Oid, Oid), (usize, usize)>,
    previous: HashMap<(Oid, Oid), (usize, usize)>,
}

impl CountCache {
    /// A pair read again moves to the current generation. Moving keeps the
    /// total and drops nothing, so no other pair of the same read is lost.
    fn get(&mut self, pair: (Oid, Oid)) -> Option<(usize, usize)> {
        if let Some(counts) = self.current.get(&pair) {
            return Some(*counts);
        }
        let counts = self.previous.remove(&pair)?;
        self.current.insert(pair, counts);
        Some(counts)
    }

    /// Only new counts rotate the generations: the pairs not read since the
    /// last rotation leave with the previous one.
    fn insert(&mut self, pair: (Oid, Oid), counts: (usize, usize)) {
        let total = self.current.len() + self.previous.len();
        if self.current.len() >= MAX_CACHED_PAIRS / 2 || total >= MAX_CACHED_PAIRS {
            self.previous = std::mem::take(&mut self.current);
            // Reads alone can fill the current generation.
            if self.previous.len() >= MAX_CACHED_PAIRS {
                self.previous.clear();
            }
        }
        self.current.insert(pair, counts);
    }
}

/// The ahead/behind counts of a session.
pub(crate) struct Counter {
    cache: Arc<Mutex<CountCache>>,
    /// libgit2 keeps the commits it reads in its repository's object cache:
    /// reopening it for each call made the benchmark's cold count seven times
    /// slower. Only a call stopped by its deadline keeps it, for the next one;
    /// a finished, cancelled or empty call releases its files and memory.
    repository: Mutex<Option<Repository>>,
}

impl Counter {
    pub(crate) fn new(cache: Arc<Mutex<CountCache>>) -> Self {
        Self {
            cache,
            repository: Mutex::default(),
        }
    }

    /// Serves cached counts, then computes the others in order until
    /// `deadline`, at least one per call. libgit2 cannot interrupt a count:
    /// `cancelled` and the deadline are checked between pairs.
    pub(crate) fn ahead_behind(
        &self,
        source: &Path,
        pairs: &[(String, String)],
        deadline: Instant,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<Vec<Option<AheadBehind>>> {
        let pairs = pairs
            .iter()
            .map(|(local, upstream)| Ok((Oid::from_str(local)?, Oid::from_str(upstream)?)))
            .collect::<std::result::Result<Vec<_>, git2::Error>>()
            .map_err(|_| "Identifiant de commit invalide.")?;
        let mut counts: Vec<_> = {
            let mut cache = self.cache.lock().unwrap();
            pairs
                .iter()
                .map(|&(local, upstream)| {
                    if local == upstream {
                        Some((0, 0))
                    } else {
                        cache.get((local, upstream))
                    }
                    .map(|(ahead, behind)| AheadBehind::Counts { ahead, behind })
                })
                .collect()
        };
        if counts.iter().all(Option::is_some) || cancelled() {
            // A running call keeps the lock and decides for itself.
            if let Ok(mut slot) = self.repository.try_lock() {
                *slot = None;
            }
            return Ok(counts);
        }
        let mut slot = self.repository.lock().unwrap();
        let repository = match slot.take() {
            Some(repository) => repository,
            None => Repository::open(source).map_err(|e| e.to_string())?,
        };
        let (mut computed, mut unfinished) = (false, false);
        for (count, &(local, upstream)) in counts.iter_mut().zip(&pairs) {
            if count.is_some() {
                continue;
            }
            if computed && cancelled() {
                break;
            }
            if computed && Instant::now() >= deadline {
                unfinished = true;
                break;
            }
            *count = Some(match repository.graph_ahead_behind(local, upstream) {
                Ok((ahead, behind)) => {
                    self.cache
                        .lock()
                        .unwrap()
                        .insert((local, upstream), (ahead, behind));
                    AheadBehind::Counts { ahead, behind }
                }
                Err(error) => AheadBehind::Unavailable {
                    error: error.message().to_owned(),
                },
            });
            computed = true;
        }
        if unfinished {
            *slot = Some(repository);
        }
        Ok(counts)
    }
}

#[cfg(test)]
mod tests {
    use super::{CountCache, Counter, MAX_CACHED_PAIRS};
    use crate::test_support::commit;
    use git2::{Oid, Repository};
    use std::{
        cell::Cell,
        time::{Duration, Instant},
    };

    #[test]
    fn only_a_count_stopped_by_its_deadline_keeps_the_repository_open() {
        let root = tempfile::tempdir().unwrap();
        let repository = Repository::init(root.path()).unwrap();
        let base = commit(&repository, &[], &[(b"file", b"base", 0o100644)]);
        let pairs: Vec<_> = (0..8)
            .map(|index| {
                let content = format!("tip {index}");
                let tip = commit(
                    &repository,
                    &[base],
                    &[(b"file", content.as_bytes(), 0o100644)],
                );
                (tip.to_string(), base.to_string())
            })
            .collect();
        let counter = Counter::new(Default::default());
        let held = || counter.repository.lock().unwrap().is_some();
        let far = || Instant::now() + Duration::from_secs(60);
        let count = |range: std::ops::Range<usize>, deadline, cancelled: &dyn Fn() -> bool| {
            counter
                .ahead_behind(root.path(), &pairs[range], deadline, cancelled)
                .unwrap()
        };
        // A past deadline stops after one pair: the next call goes on with it.
        count(0..2, Instant::now(), &|| false);
        assert!(held());
        count(0..2, far(), &|| false);
        assert!(!held(), "a finished count releases it");

        count(2..4, Instant::now(), &|| false);
        assert!(held());
        count(2..4, far(), &|| true);
        assert!(!held(), "a cancelled call releases it");

        count(4..6, Instant::now(), &|| false);
        assert!(held());
        count(0..0, far(), &|| false);
        assert!(!held(), "an empty call releases it");

        // Cancelled after its first pair.
        let calls = Cell::new(0);
        let counts = count(6..8, far(), &|| {
            calls.set(calls.get() + 1);
            calls.get() > 1
        });
        assert!(counts[0].is_some() && counts[1].is_none());
        assert!(!held());
    }

    fn pair(index: usize) -> (Oid, Oid) {
        let mut bytes = [0; 20];
        bytes[..8].copy_from_slice(&(index as u64).to_be_bytes());
        let oid = Oid::from_bytes(&bytes).unwrap();
        (oid, oid)
    }

    #[test]
    fn reading_cached_pairs_keeps_the_other_pairs_of_the_read_and_the_bound() {
        let half = MAX_CACHED_PAIRS / 2;
        let mut cache = CountCache::default();
        for index in 0..=half {
            cache.insert(pair(index), (index, 0));
        }
        // The first half moved to the previous generation; fill the current.
        for index in half + 1..2 * half {
            cache.insert(pair(index), (0, 0));
        }
        assert_eq!(cache.current.len(), half);
        assert_eq!(cache.get(pair(0)), Some((0, 0)));
        assert_eq!(cache.get(pair(1)), Some((1, 0)));
        // Reads and new counts never keep more than the bound.
        for index in 2 * half..6 * half {
            for read in [index - half, index - 2 * half] {
                cache.get(pair(read));
            }
            cache.insert(pair(index), (0, 0));
            assert!(cache.current.len() + cache.previous.len() <= MAX_CACHED_PAIRS);
        }
        assert_eq!(cache.get(pair(6 * half - 1)), Some((0, 0)));
    }
}
