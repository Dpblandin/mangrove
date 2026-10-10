use gpui_kit::SharedString;
use mangrove_core::model::{CommitPage, CommitRow, IndexProgress, Reference};
use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Instant,
};

pub const PAGE_SIZE: usize = 128;
const MAX_PAGES: usize = 12;
const MAX_PAGE_BYTES: usize = 64 * 1024 * 1024;
const MAX_GRAPH_EDGES: usize = 2_000_000;
const MAX_SUBJECT_CHARS: usize = 500;

#[derive(Default)]
struct ReadCounts {
    pages: AtomicUsize,
    position: AtomicUsize,
}

/// Physical reads survive logical cancellation. Their leases move into the
/// background task and return with its result, rather than resetting on refresh.
#[derive(Clone, Default)]
pub struct HistoryReadSlots(Arc<ReadCounts>);

#[derive(Clone, Copy)]
enum ReadKind {
    Page,
    Position,
}

pub struct ReadLease {
    slots: HistoryReadSlots,
    kind: ReadKind,
}

impl HistoryReadSlots {
    fn acquire(&self, kind: ReadKind) -> Option<ReadLease> {
        let (count, limit) = match kind {
            ReadKind::Page => (&self.0.pages, 2),
            ReadKind::Position => (&self.0.position, 1),
        };
        count
            .fetch_update(Ordering::AcqRel, Ordering::Relaxed, |live| {
                (live < limit).then_some(live + 1)
            })
            .ok()
            .map(|_| ReadLease {
                slots: self.clone(),
                kind,
            })
    }
}

impl Drop for ReadLease {
    fn drop(&mut self) {
        let count = match self.kind {
            ReadKind::Page => &self.slots.0.pages,
            ReadKind::Position => &self.slots.0.position,
        };
        count.fetch_sub(1, Ordering::AcqRel);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageRead {
    pub page: usize,
    pub generation: u64,
}

pub struct PageRequests {
    slots: HistoryReadSlots,
    generation: u64,
    pending: HashSet<usize>,
    wanted: VecDeque<usize>,
}

impl PageRequests {
    pub fn new(slots: HistoryReadSlots) -> Self {
        Self {
            slots,
            generation: 0,
            pending: HashSet::new(),
            wanted: VecDeque::new(),
        }
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn loading(&self) -> bool {
        !self.pending.is_empty() || !self.wanted.is_empty()
    }

    pub fn reset(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.pending.clear();
        self.wanted.clear();
    }

    pub fn request(&mut self, page: usize) {
        if self.pending.contains(&page) || self.wanted.contains(&page) {
            return;
        }
        // Measurement page, viewport pages and the keyboard selection stay
        // bounded even if the user crosses many pages while old reads finish.
        if self.wanted.len() == 4 {
            self.wanted.pop_front();
        }
        self.wanted.push_back(page);
    }

    pub fn try_start(&mut self) -> Option<(PageRead, ReadLease)> {
        let page = *self.wanted.front()?;
        let lease = self.slots.acquire(ReadKind::Page)?;
        self.wanted.pop_front();
        self.pending.insert(page);
        Some((
            PageRead {
                page,
                generation: self.generation,
            },
            lease,
        ))
    }

    pub fn finish(&mut self, read: PageRead) -> bool {
        if read.generation != self.generation {
            return false;
        }
        self.pending.remove(&read.page)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PositionRead {
    Selection {
        generation: u64,
        sha: String,
    },
    Jump {
        generation: u64,
        repository_generation: u64,
        sha: String,
        deadline: Instant,
    },
}

impl PositionRead {
    pub fn sha(&self) -> &str {
        match self {
            Self::Selection { sha, .. } | Self::Jump { sha, .. } => sha,
        }
    }
}

pub struct PositionRequests {
    slots: HistoryReadSlots,
    active: Option<PositionRead>,
    wanted: Option<PositionRead>,
}

impl PositionRequests {
    pub fn new(slots: HistoryReadSlots) -> Self {
        Self {
            slots,
            active: None,
            wanted: None,
        }
    }

    pub fn clear(&mut self) {
        self.wanted = None;
    }

    pub fn request(&mut self, read: PositionRead) {
        self.wanted = (self.active.as_ref() != Some(&read)).then_some(read);
    }

    pub fn try_start(&mut self) -> Option<(PositionRead, ReadLease)> {
        if self.active.is_some() || self.wanted.is_none() {
            return None;
        }
        let lease = self.slots.acquire(ReadKind::Position)?;
        let read = self.wanted.take().unwrap();
        self.active = Some(read.clone());
        Some((read, lease))
    }

    pub fn finish(&mut self, read: &PositionRead) {
        if self.active.as_ref() == Some(read) {
            self.active = None;
        }
    }
}

pub struct ReferenceGroup {
    pub references: Arc<Vec<Reference>>,
    pub badges: Vec<Arc<Reference>>,
    pub has_branches: bool,
}

pub fn group_references(
    references: &[Reference],
    solo: &[String],
) -> HashMap<String, Arc<ReferenceGroup>> {
    let solo: HashSet<_> = solo.iter().map(String::as_str).collect();
    let mut groups = HashMap::<String, Vec<Reference>>::new();
    for reference in references {
        groups
            .entry(reference.sha.clone())
            .or_default()
            .push(reference.clone());
    }
    groups
        .into_iter()
        .map(|(sha, references)| {
            let has_branches = references
                .iter()
                .any(|reference| reference.kind == "local" || reference.kind == "remote");
            let badges = badges_for_solo(&references, &solo);
            (
                sha,
                Arc::new(ReferenceGroup {
                    references: Arc::new(references),
                    badges,
                    has_branches,
                }),
            )
        })
        .collect()
}

#[cfg(test)]
fn reference_badges(references: &[Reference], solo: &[String]) -> Vec<Arc<Reference>> {
    let solo: HashSet<_> = solo.iter().map(String::as_str).collect();
    badges_for_solo(references, &solo)
}

fn badges_for_solo(references: &[Reference], solo: &HashSet<&str>) -> Vec<Arc<Reference>> {
    references
        .iter()
        .filter(|reference| solo.is_empty() || solo.contains(reference.name.as_str()))
        .take(2)
        .map(|reference| Arc::new(reference.clone()))
        .collect()
}

/// Text used by a virtualized row is prepared once, away from the UI thread.
/// The full commit remains available for selection and inspector operations.
pub struct HistoryRow {
    pub commit: Arc<CommitRow>,
    pub subject: SharedString,
    pub author: SharedString,
    pub initials: SharedString,
    pub date: SharedString,
    pub short_sha: SharedString,
}

impl HistoryRow {
    fn new(commit: CommitRow) -> Self {
        let subject = if commit.summary.is_empty() {
            "(sans message)".to_owned()
        } else {
            let mut chars = commit.summary.chars();
            let mut subject: String = chars.by_ref().take(MAX_SUBJECT_CHARS).collect();
            if chars.next().is_some() {
                subject.push('…');
            }
            subject
        };
        Self {
            subject: subject.into(),
            author: commit.author.clone().into(),
            initials: crate::format::initials(&commit.author).into(),
            date: crate::format::compact_date(commit.time).into(),
            short_sha: crate::format::short_sha(&commit.sha).into(),
            commit: Arc::new(commit),
        }
    }

    fn allocation_bytes(&self) -> usize {
        let row = &self.commit;
        std::mem::size_of::<Self>()
            + std::mem::size_of::<CommitRow>()
            + 4 * std::mem::size_of::<usize>()
            + row.sha.capacity()
            + row.summary.capacity()
            + row.author.capacity()
            + row.email.capacity()
            + row.parents.capacity() * std::mem::size_of::<String>()
            + row.parents.iter().map(String::capacity).sum::<usize>()
            + row.graph.edges.capacity() * std::mem::size_of::<mangrove_core::model::GraphEdge>()
            + self.subject.len()
            + self.author.len()
            + self.initials.len()
            + self.date.len()
            + self.short_sha.len()
    }
}

pub struct HistoryPage {
    pub offset: usize,
    pub total: usize,
    pub index: IndexProgress,
    pub rows: Vec<Arc<HistoryRow>>,
    pub max_lanes: usize,
    measurement: Option<Arc<HistoryRow>>,
    bytes: usize,
    edges: usize,
    epoch: u64,
}

impl HistoryPage {
    pub fn new(page: CommitPage) -> Self {
        let rows: Vec<_> = page
            .commits
            .into_iter()
            .map(|row| Arc::new(HistoryRow::new(row)))
            .collect();
        let bytes = rows.capacity() * std::mem::size_of::<Arc<HistoryRow>>()
            + rows.iter().map(|row| row.allocation_bytes()).sum::<usize>();
        let edges = rows.iter().map(|row| row.commit.graph.edges.len()).sum();
        let max_lanes = rows
            .iter()
            .map(|row| row.commit.graph.width)
            .max()
            .unwrap_or(1);
        let measurement = if page.offset == 0 {
            rows.first().map(|first| {
                let commit = &first.commit;
                Arc::new(HistoryRow::new(CommitRow {
                    sha: commit.sha.clone(),
                    summary: first.subject.to_string(),
                    author: commit.author.clone(),
                    email: String::new(),
                    time: commit.time,
                    parents: commit.parents.iter().take(2).cloned().collect(),
                    position: commit.position,
                    graph: mangrove_core::model::GraphRow {
                        lane: commit.graph.lane,
                        color: commit.graph.color,
                        width: commit.graph.width,
                        edges: Vec::new(),
                    },
                }))
            })
        } else {
            None
        };
        Self {
            offset: page.offset,
            total: page.total,
            index: page.index,
            rows,
            max_lanes,
            measurement,
            bytes,
            edges,
            epoch: 0,
        }
    }
}

#[derive(Default)]
pub struct HistoryCache {
    pages: BTreeMap<usize, HistoryPage>,
    retired: Vec<HistoryPage>,
    rejected: HashSet<usize>,
    measurement: Option<Arc<HistoryRow>>,
    requested_page: Option<usize>,
    bytes: usize,
    edges: usize,
    epoch: u64,
    pub visible_page: usize,
}

impl HistoryCache {
    #[cfg(test)]
    pub fn contains(&self, page: usize) -> bool {
        self.pages.contains_key(&page)
    }

    pub fn current(&self, page: usize) -> bool {
        self.pages
            .get(&page)
            .is_some_and(|cached| cached.epoch == self.epoch)
    }

    pub fn can_request(&self, page: usize) -> bool {
        !self.rejected.contains(&page)
    }

    pub fn defer(&mut self, page: usize) {
        self.rejected.insert(page);
    }

    pub fn useful(&self, page: usize) -> bool {
        page == 0 || page.abs_diff(self.visible_page) <= 1 || self.requested_page == Some(page)
    }

    pub fn set_visible_page(&mut self, page: usize) {
        if self.visible_page != page {
            self.visible_page = page;
            self.rejected.clear();
        }
    }

    pub fn set_requested_page(&mut self, page: Option<usize>) {
        self.requested_page = page;
        if let Some(page) = page {
            self.rejected.remove(&page);
        }
    }

    pub fn row(&self, index: usize) -> Option<&CommitRow> {
        if index == 0 && !self.pages.contains_key(&0) {
            return self.measurement.as_ref().map(|row| row.commit.as_ref());
        }
        self.pages
            .get(&(index / PAGE_SIZE))?
            .rows
            .get(index % PAGE_SIZE)
            .map(|row| row.commit.as_ref())
    }

    pub fn row_shared(&self, index: usize) -> Option<Arc<HistoryRow>> {
        if index == 0 && !self.pages.contains_key(&0) {
            return self.measurement.clone();
        }
        self.pages
            .get(&(index / PAGE_SIZE))?
            .rows
            .get(index % PAGE_SIZE)
            .cloned()
    }

    fn remove(&mut self, page: usize) {
        if let Some(retired) = self.pages.remove(&page) {
            self.bytes -= retired.bytes;
            self.edges -= retired.edges;
            self.retired.push(retired);
        }
    }

    /// Visible stale data survives until the replacement page has arrived.
    pub fn mark_stale(&mut self) {
        self.epoch = self.epoch.wrapping_add(1);
        self.rejected.clear();
    }

    pub fn clear(&mut self) {
        self.retired
            .extend(std::mem::take(&mut self.pages).into_values());
        self.bytes = 0;
        self.edges = 0;
        self.measurement = None;
        self.requested_page = None;
        self.mark_stale();
    }

    pub fn take_retired(&mut self) -> Vec<HistoryPage> {
        std::mem::take(&mut self.retired)
    }

    pub fn retire(&mut self, page: HistoryPage) {
        self.retired.push(page);
    }

    pub fn find_sha(&self, sha: &str) -> Option<usize> {
        self.pages.iter().find_map(|(page, cached)| {
            cached
                .rows
                .iter()
                .position(|row| row.commit.sha == sha)
                .map(|index| page * PAGE_SIZE + index)
        })
    }

    pub fn invalidate_trailing(&mut self, total: usize) -> bool {
        let mut changed = false;
        for (page, cached) in &mut self.pages {
            if cached.epoch == self.epoch
                && cached.rows.len() < PAGE_SIZE
                && page * PAGE_SIZE + cached.rows.len() < total
            {
                cached.epoch = self.epoch.wrapping_sub(1);
                changed = true;
            }
        }
        changed
    }

    pub fn insert(&mut self, mut page: HistoryPage, total: usize) -> bool {
        let invalidated = self.invalidate_trailing(total);
        let number = page.offset / PAGE_SIZE;
        if page.rows.len() < PAGE_SIZE && page.offset + page.rows.len() < total {
            self.retired.push(page);
            return invalidated;
        }
        if self.pages.get(&number).is_some_and(|cached| {
            cached.epoch == self.epoch
                && cached.rows.len() == page.rows.len()
                && cached
                    .rows
                    .iter()
                    .zip(&page.rows)
                    .all(|(old, new)| old.commit.sha == new.commit.sha)
        }) {
            self.retired.push(page);
            return invalidated;
        }
        page.epoch = self.epoch;
        if number == 0 {
            // Item zero is measured by uniform_list on every layout. Its
            // bounded presentation survives even if its heavy page is evicted.
            self.measurement = page.measurement.take();
        }
        self.remove(number);
        self.bytes += page.bytes;
        self.edges += page.edges;
        self.pages.insert(number, page);
        while self.pages.len() > MAX_PAGES
            || self.bytes > MAX_PAGE_BYTES
            || self.edges > MAX_GRAPH_EDGES
        {
            // Retain one useful oversized page if it alone exceeds the budget:
            // dropping its graph would break horizontal navigation. All other
            // pages, including measurement page zero, are evicted in that case.
            if self.pages.len() == 1 {
                break;
            }
            let useful = self.requested_page.unwrap_or(self.visible_page);
            let farthest = self
                .pages
                .keys()
                .copied()
                .max_by_key(|page| {
                    (
                        *page != useful,
                        *page != 0,
                        page.abs_diff(self.visible_page),
                    )
                })
                .unwrap();
            self.remove(farthest);
            self.rejected.insert(farthest);
        }
        self.pages.contains_key(&number) || invalidated
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mangrove_core::model::GraphRow;

    #[test]
    fn generation_reset_waits_for_physical_pages_and_wakes_current_intent() {
        let slots = HistoryReadSlots::default();
        let mut requests = PageRequests::new(slots.clone());
        requests.request(0);
        requests.request(1);
        let (old_zero, first_lease) = requests.try_start().unwrap();
        let (old_one, second_lease) = requests.try_start().unwrap();
        for _ in 0..20 {
            requests.reset();
            requests.request(0);
            assert!(requests.loading());
            assert!(requests.try_start().is_none());
        }
        assert!(!requests.finish(old_zero));
        // Rejection alone must not release a read still owned by its task.
        assert!(requests.try_start().is_none());
        drop(first_lease);
        let (current, current_lease) = requests.try_start().unwrap();
        assert_eq!(current.page, 0);
        assert_eq!(current.generation, 20);
        // A stale response for the same page cannot remove current pending state.
        assert!(!requests.finish(old_zero));
        assert!(requests.pending.contains(&0));
        requests.request(2);
        assert!(requests.try_start().is_none());
        assert!(!requests.finish(old_one));
        drop(second_lease);
        let (next, next_lease) = requests.try_start().unwrap();
        assert_eq!(next.page, 2);
        assert!(requests.finish(current));
        drop(current_lease);
        assert!(requests.finish(next));
        drop(next_lease);
        assert!(!requests.loading());
    }

    #[test]
    fn rapid_viewport_changes_keep_only_a_bounded_set_of_latest_page_intents() {
        let mut requests = PageRequests::new(HistoryReadSlots::default());
        requests.request(0);
        requests.request(1);
        let (_, first) = requests.try_start().unwrap();
        let (_, second) = requests.try_start().unwrap();
        for page in 2..10_000 {
            requests.request(page);
        }
        assert_eq!(
            requests.wanted.iter().copied().collect::<Vec<_>>(),
            [9_996, 9_997, 9_998, 9_999]
        );
        assert!(requests.try_start().is_none());
        drop(first);
        assert_eq!(requests.try_start().unwrap().0.page, 9_996);
        drop(second);
    }

    #[test]
    fn position_polling_and_jump_share_one_physical_slot_across_cancellation() {
        let mut requests = PositionRequests::new(HistoryReadSlots::default());
        let old = PositionRead::Selection {
            generation: 1,
            sha: "old".into(),
        };
        requests.request(old.clone());
        let (old_read, old_lease) = requests.try_start().unwrap();
        for _ in 0..20 {
            requests.request(old.clone());
            assert!(requests.try_start().is_none());
            assert!(requests.wanted.is_none());
        }
        requests.clear();
        requests.request(PositionRead::Selection {
            generation: 2,
            sha: "new".into(),
        });
        let jump = PositionRead::Jump {
            generation: 3,
            repository_generation: 4,
            sha: "latest".into(),
            deadline: Instant::now(),
        };
        requests.request(jump.clone());
        assert!(requests.try_start().is_none());
        requests.finish(&old_read);
        assert!(requests.try_start().is_none());
        drop(old_lease);
        let (current, lease) = requests.try_start().unwrap();
        assert_eq!(current, jump);
        requests.finish(&old_read);
        assert_eq!(requests.active.as_ref(), Some(&jump));
        requests.finish(&current);
        drop(lease);
        assert!(requests.try_start().is_none());
    }

    #[test]
    fn shared_reference_groups_keep_badges_bounded_and_solo_order_intact() {
        let references: Vec<_> = (0..10_000)
            .map(|index| Reference {
                name: format!("refs/heads/branch-{index}"),
                label: format!("branch-{index}"),
                kind: "local".into(),
                sha: "same".into(),
            })
            .collect();
        let groups = group_references(&references, &[]);
        let shared = groups.get("same").unwrap().references.clone();
        assert!(Arc::ptr_eq(
            &shared,
            &groups.get("same").unwrap().references
        ));
        let badges = reference_badges(&shared, &[]);
        assert_eq!(badges.len(), 2);
        assert_eq!(badges[0].name, references[0].name);
        assert_eq!(badges[1].name, references[1].name);
        let solo = [references[999].name.clone(), references[88].name.clone()];
        let badges = reference_badges(&shared, &solo);
        assert_eq!(badges.len(), 2);
        assert_eq!(badges[0].name, references[88].name);
        assert_eq!(badges[1].name, references[999].name);
        assert_eq!(shared.len(), 10_000);
    }

    fn rows(offset: usize, count: usize) -> Vec<CommitRow> {
        (offset..offset + count)
            .map(|position| CommitRow {
                sha: position.to_string(),
                summary: String::new(),
                author: String::new(),
                email: String::new(),
                time: 0,
                parents: vec![],
                position,
                graph: GraphRow {
                    lane: 0,
                    color: 0,
                    width: 1,
                    edges: vec![],
                },
            })
            .collect()
    }

    fn insert(cache: &mut HistoryCache, offset: usize, rows: Vec<CommitRow>, total: usize) -> bool {
        cache.insert(
            HistoryPage::new(CommitPage {
                commits: rows,
                offset,
                total,
                index: IndexProgress {
                    indexed: total,
                    complete: true,
                    error: None,
                    index_run: "test".into(),
                },
            }),
            total,
        )
    }

    #[test]
    fn indexing_growth_invalidates_a_previously_partial_page() {
        let mut cache = HistoryCache::default();
        insert(&mut cache, 0, rows(0, 3), 3);
        assert!(cache.row(2).is_some());
        insert(&mut cache, PAGE_SIZE, rows(PAGE_SIZE, PAGE_SIZE), 300);
        assert!(cache.row(2).is_some());
        assert!(!cache.current(0));
        // A response from an older SQLite snapshot cannot restore stale rows.
        insert(&mut cache, 0, rows(0, 3), 300);
        assert!(cache.contains(0));
        assert!(!cache.current(0));
        insert(&mut cache, 0, rows(0, PAGE_SIZE), 300);
        assert_eq!(cache.row(127).unwrap().position, 127);
    }

    #[test]
    fn long_scroll_bounds_memory_and_keeps_the_measurement_and_visible_pages() {
        let mut cache = HistoryCache::default();
        for page in 0..50 {
            cache.visible_page = page;
            insert(
                &mut cache,
                page * PAGE_SIZE,
                rows(page * PAGE_SIZE, PAGE_SIZE),
                10_000,
            );
        }
        assert_eq!(cache.pages.len(), MAX_PAGES);
        assert!(cache.row(0).is_some());
        assert!(cache.row(49 * PAGE_SIZE + 127).is_some());
        assert!(!cache.contains(1));
    }

    #[test]
    fn unchanged_progress_page_preserves_shared_rows() {
        let mut cache = HistoryCache::default();
        assert!(insert(&mut cache, 0, rows(0, PAGE_SIZE), PAGE_SIZE));
        let before = cache.row_shared(30).unwrap();
        assert!(!insert(&mut cache, 0, rows(0, PAGE_SIZE), PAGE_SIZE));
        assert!(Arc::ptr_eq(&before, &cache.row_shared(30).unwrap()));
    }

    #[test]
    fn filtered_positions_are_result_offsets_and_not_walk_positions() {
        let mut cache = HistoryCache::default();
        insert(&mut cache, 0, rows(900, 4), 4);
        assert_eq!(cache.find_sha("903"), Some(3));
        assert_eq!(cache.row(3).unwrap().position, 903);
    }

    #[test]
    fn a_late_distant_page_is_abandoned_after_eviction_until_the_viewport_moves() {
        let mut cache = HistoryCache::default();
        cache.set_visible_page(20);
        insert(&mut cache, 0, rows(0, PAGE_SIZE), 100_000);
        for page in 15..26 {
            insert(
                &mut cache,
                page * PAGE_SIZE,
                rows(page * PAGE_SIZE, PAGE_SIZE),
                100_000,
            );
        }
        assert_eq!(cache.pages.len(), MAX_PAGES);
        assert!(!insert(
            &mut cache,
            100 * PAGE_SIZE,
            rows(100 * PAGE_SIZE, PAGE_SIZE),
            100_000
        ));
        assert!(!cache.contains(100));
        assert!(!cache.useful(100));
        assert!(!cache.can_request(100));
        assert!(cache.current(20));
        assert!(!cache.take_retired().is_empty());
        // Explicit keyboard intent is useful before the scroll has painted its
        // new range, and can retry a page previously rejected by the budget.
        cache.set_requested_page(Some(100));
        assert!(cache.useful(100));
        assert!(cache.can_request(100));
        insert(
            &mut cache,
            100 * PAGE_SIZE,
            rows(100 * PAGE_SIZE, PAGE_SIZE),
            100_000,
        );
        assert!(cache.current(100));
    }

    #[test]
    fn wide_graphs_evict_by_edges_and_allocated_bytes_before_the_page_limit() {
        use mangrove_core::model::{GraphEdge, GraphEdgeKind};
        let mut cache = HistoryCache::default();
        for page in 0..6 {
            let mut commits = rows(page * PAGE_SIZE, PAGE_SIZE);
            for row in &mut commits {
                row.graph.width = 5000;
                row.graph.edges = (0..5000)
                    .map(|lane| GraphEdge {
                        from: lane,
                        to: lane,
                        color: lane % 8,
                        kind: GraphEdgeKind::Through,
                    })
                    .collect();
            }
            cache.set_visible_page(page);
            insert(&mut cache, page * PAGE_SIZE, commits, 10_000);
            drop(cache.take_retired());
            assert!(cache.bytes <= MAX_PAGE_BYTES);
            assert!(cache.edges <= MAX_GRAPH_EDGES);
            assert!(cache.current(page));
        }
        assert!(cache.pages.len() < MAX_PAGES);
        assert!(cache.row(0).is_some());
    }

    #[test]
    fn only_one_useful_oversized_page_survives_and_measurement_does_not_pin_its_graph() {
        use mangrove_core::model::GraphEdge;
        let mut cache = HistoryCache::default();
        let oversized_rows = |page| {
            let mut commits = rows(page * PAGE_SIZE, PAGE_SIZE);
            commits[0].graph.edges =
                Vec::with_capacity(MAX_PAGE_BYTES / std::mem::size_of::<GraphEdge>() + 1);
            commits
        };
        insert(&mut cache, 0, oversized_rows(0), 100_000);
        assert!(cache.bytes > MAX_PAGE_BYTES);
        cache.set_visible_page(20);
        insert(&mut cache, 20 * PAGE_SIZE, oversized_rows(20), 100_000);
        assert_eq!(cache.pages.len(), 1);
        assert!(cache.current(20));
        assert!(!cache.contains(0));
        assert!(!cache.can_request(0));
        assert!(cache.row(0).unwrap().graph.edges.is_empty());
        drop(cache.take_retired());
    }

    #[test]
    fn refreshing_a_search_or_published_run_keeps_stale_rows_until_replaced() {
        let mut cache = HistoryCache::default();
        insert(&mut cache, 0, rows(100, PAGE_SIZE), PAGE_SIZE * 2);
        insert(&mut cache, PAGE_SIZE, rows(400, PAGE_SIZE), PAGE_SIZE * 2);
        let first = cache.row_shared(0).unwrap();
        let second = cache.row_shared(PAGE_SIZE).unwrap();
        cache.mark_stale();
        assert!(!cache.current(0));
        assert!(Arc::ptr_eq(&first, &cache.row_shared(0).unwrap()));
        insert(&mut cache, 0, rows(101, PAGE_SIZE), PAGE_SIZE * 2);
        assert!(cache.current(0));
        assert!(!cache.current(1));
        assert!(Arc::ptr_eq(&second, &cache.row_shared(PAGE_SIZE).unwrap()));
        assert_eq!(cache.find_sha("101"), Some(0));
    }

    #[test]
    fn display_subject_is_unicode_bounded_while_the_original_commit_is_preserved() {
        let mut commits = rows(0, 1);
        commits[0].summary = "é🐚".repeat(25_000);
        let prepared = HistoryRow::new(commits.pop().unwrap());
        assert_eq!(prepared.subject.chars().count(), MAX_SUBJECT_CHARS + 1);
        assert!(prepared.subject.ends_with('…'));
        assert_eq!(prepared.commit.summary.chars().count(), 50_000);
    }

    #[test]
    fn an_empty_replacement_removes_stale_matches_without_repeated_reads() {
        let mut cache = HistoryCache::default();
        insert(&mut cache, 0, rows(0, 4), 4);
        cache.mark_stale();
        assert!(insert(&mut cache, 0, Vec::new(), 0));
        assert!(cache.current(0));
        assert!(cache.row(0).is_none());
    }
}
