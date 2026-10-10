//! Ahead/behind badges of the local branches that track an upstream. One
//! background read at a time resolves the upstreams, then counts the pending
//! pairs in chunks of about 100 ms: the current branch, the rows in view, then
//! the others in sidebar order. A new generation stops a chunk between two
//! pairs and drops its answer; the engine keeps the counts it computed.
use gpui_kit::{App, Context, Div, Task, div, prelude::*, rgb};
use mangrove_core::{
    MAX_AHEAD_BEHIND_PAIRS, Workspace,
    model::{AheadBehind, RepositoryInfo, TrackingList, UpstreamGone, UpstreamState},
};
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use crate::{
    format::{count_label, format_count},
    sidebar::tree_positions,
    theme::{MONO, Palette},
};

/// A chunk returns after this delay, plus at most the pair being counted.
const CHUNK: Duration = Duration::from_millis(100);

/// Width of a badge text: Menlo and its fallbacks advance about 0.6 em per
/// character.
pub fn text_width(text: &str, font_size: f32) -> f32 {
    text.chars().count() as f32 * font_size * 0.6
}

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    Counts {
        ahead: usize,
        behind: usize,
    },
    Gone {
        reason: UpstreamGone,
        remote: String,
    },
    Unavailable(String),
    /// The configuration of the remote cannot be read.
    Invalid(String),
    Pending,
}

/// What a branch row and the HEAD chip of the history show.
#[derive(Clone, Debug, PartialEq)]
pub struct Badge {
    /// `origin/main`, or `main` for a branch of this repository.
    pub upstream: String,
    pub local: bool,
    /// Counts of a shallow clone stop at its boundaries.
    pub approximate: bool,
    pub status: Status,
    /// A value of the previous read, dimmed until its pair is counted again.
    pub stale: bool,
}

impl Badge {
    /// `↑3 ↓12`, with only the sides that are not zero, each capped at
    /// `999+`; one arrow when `compact`. An up-to-date branch has none.
    pub fn text(&self, compact: bool) -> Option<String> {
        match &self.status {
            Status::Counts {
                ahead: 0,
                behind: 0,
            } => None,
            Status::Counts { ahead, behind } if compact => Some(
                match (*ahead > 0, *behind > 0) {
                    (true, true) => "↕",
                    (true, false) => "↑",
                    _ => "↓",
                }
                .into(),
            ),
            Status::Counts { ahead, behind } => {
                let sides: Vec<_> = [('↑', *ahead), ('↓', *behind)]
                    .into_iter()
                    .filter(|(_, count)| *count > 0)
                    .map(|(arrow, count)| {
                        if count > 999 {
                            format!("{arrow}999+")
                        } else {
                            format!("{arrow}{count}")
                        }
                    })
                    .collect();
                let prefix = if self.approximate { "≈" } else { "" };
                Some(format!("{prefix}{}", sides.join(" ")))
            }
            Status::Gone { .. } => Some("disparu".into()),
            Status::Unavailable(_) | Status::Invalid(_) | Status::Pending => None,
        }
    }

    /// The badge's container: values in muted monospace, a gone upstream in
    /// the warning color, dimmed while stale. It never wraps nor shrinks.
    pub fn element(&self, palette: &Palette) -> Div {
        let element = div().flex_shrink_0().whitespace_nowrap();
        let (element, color) = match self.status {
            Status::Gone { .. } => (element, palette.warning),
            _ => (element.font_family(MONO), palette.muted),
        };
        element.text_color(rgb(if self.stale {
            palette.dimmed(color)
        } else {
            color
        }))
    }

    /// Exact values for the tooltip and assistive technologies:
    /// « 3 commits en avance, 12 en retard sur origin/main ».
    pub fn description(&self) -> String {
        let target = if self.local {
            format!("la branche locale {}", self.upstream)
        } else {
            self.upstream.clone()
        };
        let commits = |count| count_label(count, "commit", "commits");
        let mut text = match &self.status {
            Status::Counts {
                ahead: 0,
                behind: 0,
            } => format!("À jour avec {target}"),
            Status::Counts { ahead, behind: 0 } => {
                format!("{} en avance sur {target}", commits(*ahead))
            }
            Status::Counts { ahead: 0, behind } => {
                format!("{} en retard sur {target}", commits(*behind))
            }
            Status::Counts { ahead, behind } => format!(
                "{} en avance, {} en retard sur {target}",
                commits(*ahead),
                format_count(*behind)
            ),
            Status::Gone { reason, remote } => format!(
                "Upstream disparu : {}",
                match reason {
                    UpstreamGone::MissingRef if self.local => format!("{target} n’existe plus"),
                    UpstreamGone::MissingRef => format!("{target} n’existe plus localement"),
                    UpstreamGone::MissingRemote => format!("le remote {remote} n’existe plus"),
                    UpstreamGone::NoMatchingRefspec => format!(
                        "aucune refspec de fetch du remote {remote} ne correspond à {}",
                        self.upstream
                    ),
                }
            ),
            Status::Unavailable(error) => {
                format!("Comptage indisponible sur {target} : {error}")
            }
            Status::Invalid(error) => format!("Upstream illisible : {error}"),
            Status::Pending => format!("Comptage en cours sur {target}"),
        };
        if self.approximate && matches!(self.status, Status::Counts { .. }) {
            text.push_str("\nValeurs approximatives : clone superficiel");
        }
        if self.stale {
            text.push_str("\nActualisation en cours");
        }
        text
    }
}

/// The commits of a count.
#[derive(Clone)]
struct Pair {
    branch_sha: String,
    upstream: String,
    upstream_sha: String,
}

/// A badge of the previous generation, with the pair of its count.
struct Previous {
    badge: Badge,
    pair: Option<Pair>,
}

enum Job {
    /// Closes the repository a replaced count may have kept open.
    Release {
        id: String,
    },
    Resolve {
        id: String,
    },
    Count {
        id: String,
        list: Arc<TrackingList>,
        order: Vec<usize>,
    },
}

/// Generations, the single read lane and the badges, without GPUI.
#[derive(Default)]
struct Tracking {
    generation: u64,
    repository: Option<Arc<RepositoryInfo>>,
    /// Upstreams in sidebar order, once read for this generation.
    list: Option<Arc<TrackingList>>,
    positions: HashMap<String, usize>,
    counts: Vec<Option<AheadBehind>>,
    previous: HashMap<String, Previous>,
    /// The counts key of the previous values: other shallow boundaries or
    /// grafts make them unreliable even for unchanged pairs.
    previous_key: Option<String>,
    /// The session of a count answered after its generation was replaced.
    release: Option<String>,
    /// A failed read stops this generation instead of retrying in a loop.
    failed: bool,
    reading: bool,
}

impl Tracking {
    /// A refresh of the same path keeps the badges: values whose refs did
    /// not move stay exact, the others are dimmed until counted again.
    fn set_repository(&mut self, info: Arc<RepositoryInfo>) {
        let same_path = self
            .repository
            .as_ref()
            .is_some_and(|previous| previous.path == info.path);
        let mut previous = HashMap::new();
        if same_path {
            previous = self.snapshot();
            let shas: HashMap<_, _> = info
                .references
                .iter()
                .map(|reference| (reference.name.as_str(), reference.sha.as_str()))
                .collect();
            for (branch, entry) in &mut previous {
                let unchanged = entry.pair.as_ref().is_some_and(|pair| {
                    shas.get(branch.as_str()) == Some(&pair.branch_sha.as_str())
                        && shas.get(pair.upstream.as_str()) == Some(&pair.upstream_sha.as_str())
                });
                entry.badge.stale |=
                    !unchanged && matches!(entry.badge.status, Status::Counts { .. });
            }
        }
        let previous_key = same_path
            .then(|| match &self.list {
                Some(list) => Some(list.counts_key.clone()),
                None => self.previous_key.clone(),
            })
            .flatten();
        self.clear();
        self.previous = previous;
        self.previous_key = previous_key;
        self.repository = Some(info);
    }

    fn clear(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.repository = None;
        self.list = None;
        self.positions.clear();
        self.counts.clear();
        self.previous.clear();
        self.previous_key = None;
        self.failed = false;
    }

    /// Current badges, or the previous ones still waiting for this generation.
    fn snapshot(&mut self) -> HashMap<String, Previous> {
        let Some(list) = self.list.clone() else {
            return std::mem::take(&mut self.previous);
        };
        list.branches
            .iter()
            .filter_map(|branch| {
                let badge = self.badge(&branch.branch)?;
                let pair = if badge.stale {
                    self.previous.get(&branch.branch)?.pair.clone()
                } else {
                    match (&branch.state, &branch.upstream, &badge.status) {
                        (
                            UpstreamState::Tracking { sha },
                            Some(upstream),
                            Status::Counts { .. },
                        ) => Some(Pair {
                            branch_sha: branch.sha.clone(),
                            upstream: upstream.clone(),
                            upstream_sha: sha.clone(),
                        }),
                        (_, _, Status::Pending) => return None,
                        _ => None,
                    }
                };
                Some((branch.branch.clone(), Previous { badge, pair }))
            })
            .collect()
    }

    fn badge(&self, branch: &str) -> Option<Badge> {
        let Some(list) = &self.list else {
            return self.previous.get(branch).map(|entry| entry.badge.clone());
        };
        let index = *self.positions.get(branch)?;
        let entry = &list.branches[index];
        let status = match (&entry.state, &self.counts[index]) {
            (UpstreamState::Gone { reason }, _) => Status::Gone {
                reason: *reason,
                remote: entry.remote.clone(),
            },
            (UpstreamState::Invalid { error }, _) => Status::Invalid(error.clone()),
            (_, Some(AheadBehind::Counts { ahead, behind })) => Status::Counts {
                ahead: *ahead,
                behind: *behind,
            },
            (_, Some(AheadBehind::Unavailable { error })) => Status::Unavailable(error.clone()),
            (UpstreamState::Tracking { .. }, None) => {
                if let Some(previous) = self.previous.get(branch) {
                    return Some(Badge {
                        stale: true,
                        ..previous.badge.clone()
                    });
                }
                Status::Pending
            }
        };
        Some(Badge {
            upstream: entry.label.clone(),
            local: entry.remote == ".",
            approximate: list.shallow,
            status,
            stale: false,
        })
    }

    /// The next read of the lane, if it is free: the upstreams, then a chunk
    /// of counts. `visible` gives the branches in view, top to bottom.
    fn next_job(&mut self, visible: impl FnOnce() -> Vec<String>) -> Option<Job> {
        if self.reading {
            return None;
        }
        if let Some(id) = self.release.take() {
            self.reading = true;
            return Some(Job::Release { id });
        }
        if self.failed {
            return None;
        }
        let id = self.repository.as_ref()?.id.clone();
        let job = match &self.list {
            None => Job::Resolve { id },
            Some(list) => {
                let order = self.pending(&visible());
                if order.is_empty() {
                    return None;
                }
                Job::Count {
                    id,
                    list: list.clone(),
                    order,
                }
            }
        };
        self.reading = true;
        Some(job)
    }

    /// Pairs still to count: the current branch, then `visible`, then the
    /// others in sidebar order.
    fn pending(&self, visible: &[String]) -> Vec<usize> {
        let (Some(list), Some(info)) = (&self.list, &self.repository) else {
            return Vec::new();
        };
        let waiting = |index: usize| {
            self.counts[index].is_none()
                && matches!(list.branches[index].state, UpstreamState::Tracking { .. })
        };
        let head = format!("refs/heads/{}", info.head_name);
        let mut queued = vec![false; list.branches.len()];
        let mut order = Vec::new();
        for name in std::iter::once(&head).chain(visible) {
            if let Some(&index) = self.positions.get(name)
                && waiting(index)
                && !queued[index]
            {
                queued[index] = true;
                order.push(index);
            }
        }
        order.extend((0..list.branches.len()).filter(|&index| waiting(index) && !queued[index]));
        order.truncate(MAX_AHEAD_BEHIND_PAIRS);
        order
    }

    /// Frees the lane; only the current generation publishes its upstreams.
    fn finish_resolution(
        &mut self,
        generation: u64,
        result: Result<Arc<TrackingList>, String>,
    ) -> bool {
        self.reading = false;
        if generation != self.generation {
            return false;
        }
        // The previous values stay; the next refresh reads again.
        let Ok(list) = result else {
            self.failed = true;
            return true;
        };
        self.positions = list
            .branches
            .iter()
            .enumerate()
            .map(|(index, branch)| (branch.branch.clone(), index))
            .collect();
        // An unchanged pair keeps its count without another read, unless
        // shallow boundaries or grafts changed.
        let same_key = self.previous_key.as_deref() == Some(list.counts_key.as_str());
        self.counts = list
            .branches
            .iter()
            .map(|branch| {
                let UpstreamState::Tracking { sha } = &branch.state else {
                    return None;
                };
                let previous = self.previous.get(&branch.branch).filter(|_| same_key)?;
                let pair = previous.pair.as_ref()?;
                let Status::Counts { ahead, behind } = previous.badge.status else {
                    return None;
                };
                (pair.branch_sha == branch.sha && pair.upstream_sha == *sha)
                    .then_some(AheadBehind::Counts { ahead, behind })
            })
            .collect();
        self.list = Some(list);
        true
    }

    /// Frees the lane; only the current generation publishes its counts. A
    /// count stopped by its deadline keeps its repository open: when its
    /// generation was replaced meanwhile, the next read releases it.
    fn finish_counts(
        &mut self,
        id: &str,
        generation: u64,
        order: &[usize],
        result: Result<Vec<Option<AheadBehind>>, String>,
    ) -> bool {
        self.reading = false;
        if generation != self.generation {
            if result.is_ok() {
                self.release = Some(id.to_owned());
            }
            return false;
        }
        match result {
            Ok(counts) => {
                for (&index, count) in order.iter().zip(counts) {
                    if count.is_some() {
                        self.counts[index] = count;
                    }
                }
            }
            Err(_) => self.failed = true,
        }
        true
    }
}

/// Local branches in view, top to bottom.
type Visible = Box<dyn Fn(&App) -> Vec<String>>;

pub struct TrackingState {
    engine: Arc<Workspace>,
    tracking: Tracking,
    /// The current generation, read by a running chunk to stop early.
    live: Arc<AtomicU64>,
    visible: Visible,
    /// The native read cannot be interrupted. Its task keeps the lane until
    /// it ends, even when a new generation discards its answer.
    task: Option<Task<()>>,
}

impl TrackingState {
    /// `visible` names the local branches in view, top to bottom. It is read
    /// only when a count starts, never from `set_repository` or `clear`.
    pub fn new(engine: Arc<Workspace>, visible: impl Fn(&App) -> Vec<String> + 'static) -> Self {
        Self {
            engine,
            tracking: Tracking::default(),
            live: Arc::default(),
            visible: Box::new(visible),
            task: None,
        }
    }

    pub fn set_repository(&mut self, info: Arc<RepositoryInfo>, cx: &mut Context<Self>) {
        self.tracking.set_repository(info);
        self.live.store(self.tracking.generation, Ordering::Release);
        cx.notify();
        self.pump(cx);
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.tracking.clear();
        self.live.store(self.tracking.generation, Ordering::Release);
        cx.notify();
    }

    /// The badge of a local branch, by full name. A branch without an
    /// upstream has none.
    pub fn badge(&self, branch: &str) -> Option<Badge> {
        self.tracking.badge(branch)
    }

    fn pump(&mut self, cx: &mut Context<Self>) {
        let visible = &self.visible;
        let app: &App = cx;
        let Some(job) = self.tracking.next_job(|| visible(app)) else {
            return;
        };
        let generation = self.tracking.generation;
        let engine = self.engine.clone();
        let live = self.live.clone();
        self.task = Some(cx.spawn(async move |this, cx| match job {
            Job::Release { id } => {
                cx.background_executor()
                    .spawn(async move {
                        // An empty call leaves nothing to count: the engine
                        // closes the repository of an unfinished count.
                        let _ = engine.ahead_behind(&id, &[], Instant::now(), &|| true);
                    })
                    .await;
                let _ = this.update(cx, |state, cx| {
                    state.tracking.reading = false;
                    state.pump(cx);
                });
            }
            Job::Resolve { id } => {
                let result = cx
                    .background_executor()
                    .spawn(async move {
                        engine.branch_tracking(&id).map(|mut list| {
                            let positions = tree_positions(
                                list.branches.iter().map(|branch| branch.branch.as_str()),
                            );
                            list.branches
                                .sort_by_key(|branch| positions.get(&branch.branch).copied());
                            Arc::new(list)
                        })
                    })
                    .await;
                let _ = this.update(cx, |state, cx| {
                    if state.tracking.finish_resolution(generation, result) {
                        cx.notify();
                    }
                    state.pump(cx);
                });
            }
            Job::Count { id, list, order } => {
                let session = id.clone();
                let (order, result) = cx
                    .background_executor()
                    .spawn(async move {
                        let (order, pairs): (Vec<_>, Vec<_>) = order
                            .into_iter()
                            .filter_map(|index| {
                                let branch = &list.branches[index];
                                let UpstreamState::Tracking { sha } = &branch.state else {
                                    return None;
                                };
                                Some((index, (branch.sha.clone(), sha.clone())))
                            })
                            .unzip();
                        #[cfg(feature = "app-benchmark")]
                        let started = Instant::now();
                        let result =
                            engine.ahead_behind(&id, &pairs, Instant::now() + CHUNK, &|| {
                                live.load(Ordering::Acquire) != generation
                            });
                        #[cfg(feature = "app-benchmark")]
                        crate::perf::record_operation("tracking_chunk", started.elapsed());
                        (order, result)
                    })
                    .await;
                let _ = this.update(cx, |state, cx| {
                    if state
                        .tracking
                        .finish_counts(&session, generation, &order, result)
                    {
                        cx.notify();
                    }
                    state.pump(cx);
                });
            }
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::{Badge, Job, Status, Tracking, TrackingState};
    use gpui_kit::{AppContext, TestAppContext};
    use mangrove_core::{
        Workspace,
        model::{
            AheadBehind, BranchTracking, IndexProgress, Reference, RepositoryInfo, TrackingList,
            UpstreamGone, UpstreamState,
        },
    };
    use std::sync::Arc;

    /// A repository whose references are `(name, sha)`.
    fn info(path: &str, id: &str, references: &[(&str, &str)]) -> Arc<RepositoryInfo> {
        Arc::new(RepositoryInfo {
            id: id.into(),
            name: path.into(),
            path: path.into(),
            head: None,
            head_name: "main".into(),
            references: references
                .iter()
                .map(|(name, sha)| Reference {
                    name: (*name).into(),
                    label: name.rsplit('/').next().unwrap().into(),
                    kind: if name.starts_with("refs/heads/") {
                        "local"
                    } else {
                        "remote"
                    }
                    .into(),
                    sha: (*sha).into(),
                })
                .collect(),
            solo_refs: Vec::new(),
            remotes: Vec::new(),
            index: IndexProgress {
                index_run: String::new(),
                indexed: 0,
                complete: true,
                error: None,
            },
        })
    }

    /// `name` at `sha`, tracking `origin/name` at `upstream`, or gone without one.
    fn branch(name: &str, sha: &str, upstream: Option<&str>) -> BranchTracking {
        BranchTracking {
            branch: format!("refs/heads/{name}"),
            sha: sha.into(),
            remote: "origin".into(),
            upstream: Some(format!("refs/remotes/origin/{name}")),
            label: format!("origin/{name}"),
            state: match upstream {
                Some(sha) => UpstreamState::Tracking { sha: sha.into() },
                None => UpstreamState::Gone {
                    reason: UpstreamGone::MissingRef,
                },
            },
        }
    }

    fn list(branches: Vec<BranchTracking>) -> Arc<TrackingList> {
        keyed(branches, "complete")
    }

    /// A list whose counts depend on `key`, as shallow boundaries make them.
    fn keyed(branches: Vec<BranchTracking>, key: &str) -> Arc<TrackingList> {
        Arc::new(TrackingList {
            branches,
            shallow: key != "complete",
            counts_key: key.into(),
        })
    }

    fn counts(ahead: usize, behind: usize) -> Option<AheadBehind> {
        Some(AheadBehind::Counts { ahead, behind })
    }

    fn resolve(model: &mut Tracking, branches: Vec<BranchTracking>) {
        resolve_keyed(model, list(branches));
    }

    fn resolve_keyed(model: &mut Tracking, list: Arc<TrackingList>) {
        assert!(matches!(
            model.next_job(Vec::new),
            Some(Job::Resolve { .. })
        ));
        assert!(model.finish_resolution(model.generation, Ok(list)));
    }

    /// Starts the next chunk and returns the branch names it counts.
    fn next_chunk(model: &mut Tracking, visible: &[&str]) -> (Vec<usize>, Vec<String>) {
        let visible = || {
            visible
                .iter()
                .map(|name| format!("refs/heads/{name}"))
                .collect()
        };
        let Some(Job::Count { list, order, .. }) = model.next_job(visible) else {
            panic!("no chunk to count");
        };
        let names = order
            .iter()
            .map(|&index| list.branches[index].label["origin/".len()..].to_owned())
            .collect();
        (order, names)
    }

    fn status(model: &Tracking, name: &str) -> Option<(Status, bool)> {
        model
            .badge(&format!("refs/heads/{name}"))
            .map(|badge| (badge.status, badge.stale))
    }

    #[test]
    fn one_read_at_a_time_and_only_the_current_generation_publishes() {
        let mut model = Tracking::default();
        model.set_repository(info("/repository", "1", &[]));
        assert!(matches!(model.next_job(Vec::new), Some(Job::Resolve { id }) if id == "1"));
        assert!(model.next_job(Vec::new).is_none(), "reads overlapped");

        // A refresh waits for the native read, then resolves for itself.
        let old = model.generation;
        model.set_repository(info("/repository", "2", &[]));
        assert!(model.next_job(Vec::new).is_none());
        assert!(!model.finish_resolution(old, Ok(list(vec![branch("main", "a", Some("b"))]))));
        assert_eq!(status(&model, "main"), None);
        assert!(matches!(model.next_job(Vec::new), Some(Job::Resolve { id }) if id == "2"));
        assert!(model.finish_resolution(
            model.generation,
            Ok(list(vec![branch("main", "a", Some("b"))]))
        ));
        assert_eq!(status(&model, "main"), Some((Status::Pending, false)));

        let (order, _) = next_chunk(&mut model, &[]);
        let old = model.generation;
        model.clear();
        assert!(!model.finish_counts("2", old, &order, Ok(vec![counts(1, 0)])));
        assert_eq!(status(&model, "main"), None);
        // The replaced count may have kept its repository open.
        assert!(matches!(model.next_job(Vec::new), Some(Job::Release { id }) if id == "2"));
        assert!(model.next_job(Vec::new).is_none(), "reads overlapped");
        model.reading = false;
        assert!(model.next_job(Vec::new).is_none());

        // A failed read stops its generation instead of retrying.
        model.set_repository(info("/repository", "3", &[]));
        assert!(model.next_job(Vec::new).is_some());
        assert!(model.finish_resolution(model.generation, Err("fermé".into())));
        assert!(model.next_job(Vec::new).is_none());
    }

    #[test]
    fn a_failed_upstream_read_keeps_the_previous_badges() {
        let references = [
            ("refs/heads/main", "m1"),
            ("refs/remotes/origin/main", "o1"),
        ];
        let mut model = Tracking::default();
        model.set_repository(info("/repository", "1", &references));
        resolve(&mut model, vec![branch("main", "m1", Some("o1"))]);
        let (order, _) = next_chunk(&mut model, &[]);
        assert!(model.finish_counts("1", model.generation, &order, Ok(vec![counts(2, 1)])));
        // A queued refresh replaced the session read by this generation.
        model.set_repository(info("/repository", "2", &references));
        assert!(model.next_job(Vec::new).is_some());
        assert!(model.finish_resolution(model.generation, Err("fermé".into())));
        assert_eq!(
            status(&model, "main"),
            Some((
                Status::Counts {
                    ahead: 2,
                    behind: 1
                },
                false
            ))
        );
    }

    #[test]
    fn other_shallow_boundaries_dim_unchanged_pairs_until_they_are_counted_again() {
        let references = [
            ("refs/heads/main", "m1"),
            ("refs/remotes/origin/main", "o1"),
        ];
        let mut model = Tracking::default();
        model.set_repository(info("/repository", "1", &references));
        resolve_keyed(
            &mut model,
            keyed(vec![branch("main", "m1", Some("o1"))], "shallow"),
        );
        let (order, _) = next_chunk(&mut model, &[]);
        assert!(model.finish_counts("1", model.generation, &order, Ok(vec![counts(2, 0)])));
        assert!(model.badge("refs/heads/main").unwrap().approximate);

        // `git fetch --unshallow` moves no ref.
        model.set_repository(info("/repository", "2", &references));
        resolve(&mut model, vec![branch("main", "m1", Some("o1"))]);
        assert_eq!(
            status(&model, "main"),
            Some((
                Status::Counts {
                    ahead: 2,
                    behind: 0
                },
                true
            ))
        );
        let (order, names) = next_chunk(&mut model, &[]);
        assert_eq!(names, ["main"]);
        assert!(model.finish_counts("2", model.generation, &order, Ok(vec![counts(5, 0)])));
        let badge = model.badge("refs/heads/main").unwrap();
        assert_eq!(
            (badge.status, badge.stale, badge.approximate),
            (
                Status::Counts {
                    ahead: 5,
                    behind: 0
                },
                false,
                false
            )
        );
    }

    #[test]
    fn counts_start_with_the_current_branch_then_the_rows_in_view_then_the_sidebar_order() {
        let mut model = Tracking::default();
        model.set_repository(info("/repository", "1", &[]));
        resolve(
            &mut model,
            ["alpha", "beta", "gone", "main", "zeta"]
                .into_iter()
                .map(|name| branch(name, "a", (name != "gone").then_some("b")))
                .collect(),
        );
        let (order, names) = next_chunk(&mut model, &["zeta", "gone", "main", "beta"]);
        assert_eq!(names, ["main", "zeta", "beta", "alpha"]);
        // A chunk that stops early leaves its other pairs for the next one.
        assert!(model.finish_counts(
            "1",
            model.generation,
            &order,
            Ok(vec![counts(0, 0), counts(1, 2), None, None])
        ));
        assert_eq!(
            status(&model, "zeta"),
            Some((
                Status::Counts {
                    ahead: 1,
                    behind: 2
                },
                false
            ))
        );
        assert_eq!(
            status(&model, "gone"),
            Some((
                Status::Gone {
                    reason: UpstreamGone::MissingRef,
                    remote: "origin".into()
                },
                false
            ))
        );
        let (_, names) = next_chunk(&mut model, &["alpha"]);
        assert_eq!(names, ["alpha", "beta"]);
    }

    #[test]
    fn a_refresh_keeps_exact_values_and_dims_moved_pairs_until_they_are_counted_again() {
        let references = [
            ("refs/heads/feature", "f1"),
            ("refs/heads/main", "m1"),
            ("refs/remotes/origin/feature", "u1"),
            ("refs/remotes/origin/main", "o1"),
        ];
        let mut model = Tracking::default();
        model.set_repository(info("/repository", "1", &references));
        resolve(
            &mut model,
            vec![
                branch("feature", "f1", Some("u1")),
                branch("main", "m1", Some("o1")),
            ],
        );
        let (order, _) = next_chunk(&mut model, &[]);
        assert!(model.finish_counts(
            "1",
            model.generation,
            &order,
            Ok(vec![counts(2, 1), counts(0, 3)])
        ));

        // A fetch moved origin/feature.
        let mut moved = references;
        moved[2].1 = "u2";
        model.set_repository(info("/repository", "2", &moved));
        assert_eq!(
            status(&model, "main"),
            Some((
                Status::Counts {
                    ahead: 2,
                    behind: 1
                },
                false
            ))
        );
        assert_eq!(
            status(&model, "feature"),
            Some((
                Status::Counts {
                    ahead: 0,
                    behind: 3
                },
                true
            ))
        );
        resolve(
            &mut model,
            vec![
                branch("feature", "f1", Some("u2")),
                branch("main", "m1", Some("o1")),
            ],
        );
        assert_eq!(
            status(&model, "feature"),
            Some((
                Status::Counts {
                    ahead: 0,
                    behind: 3
                },
                true
            ))
        );
        let (order, names) = next_chunk(&mut model, &[]);
        assert_eq!(names, ["feature"], "an unchanged pair keeps its count");
        assert!(model.finish_counts("1", model.generation, &order, Ok(vec![counts(0, 4)])));
        assert_eq!(
            status(&model, "feature"),
            Some((
                Status::Counts {
                    ahead: 0,
                    behind: 4
                },
                false
            ))
        );

        // Another repository starts empty.
        model.set_repository(info("/other", "3", &references));
        assert_eq!(status(&model, "main"), None);
    }

    #[test]
    fn badges_show_capped_non_zero_sides_and_describe_exact_values_in_french() {
        let badge = |status| Badge {
            upstream: "origin/main".into(),
            local: false,
            approximate: false,
            status,
            stale: false,
        };
        let counts = |ahead, behind| badge(Status::Counts { ahead, behind });
        let cases = [
            (
                counts(3, 12),
                Some("↑3 ↓12"),
                "3 commits en avance, 12 en retard sur origin/main",
            ),
            (
                counts(1, 0),
                Some("↑1"),
                "1 commit en avance sur origin/main",
            ),
            (
                counts(0, 1),
                Some("↓1"),
                "1 commit en retard sur origin/main",
            ),
            (counts(0, 0), None, "À jour avec origin/main"),
            (
                counts(1_000, 999),
                Some("↑999+ ↓999"),
                "1\u{202f}000 commits en avance, 999 en retard sur origin/main",
            ),
            // Exact values keep the French thousands separator.
            (
                counts(1_100, 1_500),
                Some("↑999+ ↓999+"),
                "1\u{202f}100 commits en avance, 1\u{202f}500 en retard sur origin/main",
            ),
            (
                counts(0, 1_500),
                Some("↓999+"),
                "1\u{202f}500 commits en retard sur origin/main",
            ),
        ];
        for (badge, text, description) in cases {
            assert_eq!(badge.text(false).as_deref(), text);
            assert_eq!(badge.description(), description);
        }
        assert_eq!(counts(3, 12).text(true).as_deref(), Some("↕"));
        assert_eq!(counts(3, 0).text(true).as_deref(), Some("↑"));
        assert_eq!(counts(0, 2).text(true).as_deref(), Some("↓"));
        assert_eq!(counts(0, 0).text(true), None);

        let shallow = Badge {
            approximate: true,
            local: true,
            upstream: "main".into(),
            ..counts(2, 0)
        };
        assert_eq!(shallow.text(false).as_deref(), Some("≈↑2"));
        assert_eq!(
            shallow.description(),
            "2 commits en avance sur la branche locale main\nValeurs approximatives : clone superficiel"
        );
        let gone = |reason| {
            badge(Status::Gone {
                reason,
                remote: "origin".into(),
            })
        };
        assert_eq!(
            gone(UpstreamGone::MissingRef).text(false).as_deref(),
            Some("disparu")
        );
        assert_eq!(
            gone(UpstreamGone::MissingRef).description(),
            "Upstream disparu : origin/main n’existe plus localement"
        );
        assert_eq!(
            gone(UpstreamGone::MissingRemote).description(),
            "Upstream disparu : le remote origin n’existe plus"
        );
        assert_eq!(
            gone(UpstreamGone::NoMatchingRefspec).description(),
            "Upstream disparu : aucune refspec de fetch du remote origin ne correspond à origin/main"
        );
        let local = Badge {
            local: true,
            upstream: "main".into(),
            ..gone(UpstreamGone::MissingRef)
        };
        assert_eq!(
            local.description(),
            "Upstream disparu : la branche locale main n’existe plus"
        );
        let invalid = badge(Status::Invalid("refspec invalide".into()));
        assert_eq!(invalid.text(false), None);
        assert_eq!(
            invalid.description(),
            "Upstream illisible : refspec invalide"
        );
        let unavailable = badge(Status::Unavailable("objet absent".into()));
        assert_eq!(unavailable.text(false), None);
        assert_eq!(
            unavailable.description(),
            "Comptage indisponible sur origin/main : objet absent"
        );
        let stale = Badge {
            stale: true,
            ..counts(0, 3)
        };
        assert_eq!(
            stale.description(),
            "3 commits en retard sur origin/main\nActualisation en cours"
        );
    }

    fn commit(repository: &git2::Repository, parents: &[git2::Oid], time: i64) -> git2::Oid {
        let tree = repository
            .find_tree(repository.treebuilder(None).unwrap().write().unwrap())
            .unwrap();
        let signature =
            git2::Signature::new("Test", "test@example.test", &git2::Time::new(time, 0)).unwrap();
        let parents: Vec<_> = parents
            .iter()
            .map(|id| repository.find_commit(*id).unwrap())
            .collect();
        repository
            .commit(
                None,
                &signature,
                &signature,
                "commit",
                &tree,
                &parents.iter().collect::<Vec<_>>(),
            )
            .unwrap()
    }

    #[gpui_kit::test]
    fn counts_reach_the_badges_and_a_fetch_recounts_the_moved_upstream(cx: &mut TestAppContext) {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("repository");
        let repository = git2::Repository::init(&path).unwrap();
        repository.set_head("refs/heads/main").unwrap();
        let base = commit(&repository, &[], 1_700_000_000);
        let upstream = commit(&repository, &[base], 1_700_000_060);
        let local = commit(&repository, &[base], 1_700_000_120);
        repository
            .reference("refs/heads/main", local, true, "test")
            .unwrap();
        repository
            .reference("refs/remotes/origin/main", upstream, true, "test")
            .unwrap();
        repository.remote("origin", "../origin.git").unwrap();
        let mut config = repository.config().unwrap();
        config.set_str("branch.main.remote", "origin").unwrap();
        config
            .set_str("branch.main.merge", "refs/heads/main")
            .unwrap();
        let engine = Arc::new(Workspace::new(root.path().join("cache")).unwrap());
        let tracking =
            cx.update(|cx| cx.new(|_| TrackingState::new(engine.clone(), |_| Vec::new())));
        let open = |cx: &mut TestAppContext| {
            let info = Arc::new(engine.open_repository(path.to_str().unwrap()).unwrap());
            tracking.update(cx, |tracking, cx| tracking.set_repository(info, cx));
        };
        let main = |cx: &mut TestAppContext| {
            tracking.read_with(cx, |tracking, _| {
                tracking
                    .badge("refs/heads/main")
                    .map(|badge| (badge.status, badge.stale))
            })
        };
        open(cx);
        cx.run_until_parked();
        assert_eq!(
            main(cx),
            Some((
                Status::Counts {
                    ahead: 1,
                    behind: 1
                },
                false
            ))
        );

        let fetched = commit(&repository, &[upstream], 1_700_000_180);
        repository
            .reference("refs/remotes/origin/main", fetched, true, "fetch")
            .unwrap();
        open(cx);
        assert_eq!(
            main(cx),
            Some((
                Status::Counts {
                    ahead: 1,
                    behind: 1
                },
                true
            ))
        );
        cx.run_until_parked();
        assert_eq!(
            main(cx),
            Some((
                Status::Counts {
                    ahead: 1,
                    behind: 2
                },
                false
            ))
        );
        assert_eq!(
            repository
                .find_reference("refs/heads/main")
                .unwrap()
                .target(),
            Some(local)
        );
    }
}
