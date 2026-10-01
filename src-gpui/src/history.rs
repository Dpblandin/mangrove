use mangrove_core::model::{CommitRow, Reference};
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

pub fn group_references(references: &[Reference]) -> HashMap<String, Arc<Vec<Reference>>> {
    let mut groups = HashMap::<String, Vec<Reference>>::new();
    for reference in references {
        groups
            .entry(reference.sha.clone())
            .or_default()
            .push(reference.clone());
    }
    groups
        .into_iter()
        .map(|(sha, references)| (sha, Arc::new(references)))
        .collect()
}

pub fn reference_badges(references: &[Reference], solo: &[String]) -> Vec<Arc<Reference>> {
    references
        .iter()
        .filter(|reference| solo.is_empty() || solo.contains(&reference.name))
        .take(2)
        .map(|reference| Arc::new(reference.clone()))
        .collect()
}

#[derive(Default)]
pub struct HistoryCache {
    pages: BTreeMap<usize, Vec<Arc<CommitRow>>>,
    pub visible_page: usize,
}

impl HistoryCache {
    pub fn contains(&self, page: usize) -> bool {
        self.pages.contains_key(&page)
    }

    pub fn row(&self, index: usize) -> Option<&CommitRow> {
        self.pages
            .get(&(index / PAGE_SIZE))?
            .get(index % PAGE_SIZE)
            .map(Arc::as_ref)
    }

    pub fn row_shared(&self, index: usize) -> Option<Arc<CommitRow>> {
        self.pages
            .get(&(index / PAGE_SIZE))?
            .get(index % PAGE_SIZE)
            .cloned()
    }

    pub fn clear(&mut self) {
        self.pages.clear();
    }

    pub fn find_sha(&self, sha: &str) -> Option<usize> {
        self.pages.iter().find_map(|(page, rows)| {
            rows.iter()
                .position(|row| row.sha == sha)
                .map(|index| page * PAGE_SIZE + index)
        })
    }

    pub fn invalidate_trailing(&mut self, total: usize) -> bool {
        let count = self.pages.len();
        self.pages
            .retain(|page, rows| rows.len() == PAGE_SIZE || page * PAGE_SIZE + rows.len() >= total);
        self.pages.len() != count
    }

    pub fn insert(&mut self, offset: usize, rows: Vec<CommitRow>, total: usize) -> bool {
        // An incomplete trailing page becomes stale when indexing appends rows.
        let invalidated = self.invalidate_trailing(total);
        if rows.is_empty() || (rows.len() < PAGE_SIZE && offset + rows.len() < total) {
            return invalidated;
        }
        let page = offset / PAGE_SIZE;
        if self.pages.get(&page).is_some_and(|cached| {
            cached.len() == rows.len()
                && cached
                    .iter()
                    .zip(&rows)
                    .all(|(old, new)| old.sha == new.sha)
        }) {
            // Metadata and graph geometry are immutable inside one index run.
            // Repeated progress polling must not allocate or invalidate the list.
            return invalidated;
        }
        self.pages
            .insert(page, rows.into_iter().map(Arc::new).collect());
        while self.pages.len() > MAX_PAGES {
            // GPUI measures item zero for the list's fixed height; keep its page
            // resident so that scrolling never repeatedly fetches and evicts it.
            if let Some(farthest) = self
                .pages
                .keys()
                .copied()
                .filter(|page| *page != 0)
                .max_by_key(|page| page.abs_diff(self.visible_page))
            {
                self.pages.remove(&farthest);
            }
        }
        true
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
        let groups = group_references(&references);
        let shared = groups.get("same").unwrap().clone();
        assert!(Arc::ptr_eq(&shared, groups.get("same").unwrap()));
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

    #[test]
    fn indexing_growth_invalidates_a_previously_partial_page() {
        let mut cache = HistoryCache::default();
        cache.insert(0, rows(0, 3), 3);
        assert!(cache.row(2).is_some());
        cache.insert(PAGE_SIZE, rows(PAGE_SIZE, PAGE_SIZE), 300);
        assert!(cache.row(2).is_none());
        // A response from an older SQLite snapshot cannot restore stale rows.
        cache.insert(0, rows(0, 3), 300);
        assert!(!cache.contains(0));
        cache.insert(0, rows(0, PAGE_SIZE), 300);
        assert_eq!(cache.row(127).unwrap().position, 127);
    }

    #[test]
    fn long_scroll_bounds_memory_and_keeps_the_measurement_and_visible_pages() {
        let mut cache = HistoryCache::default();
        for page in 0..50 {
            cache.visible_page = page;
            cache.insert(page * PAGE_SIZE, rows(page * PAGE_SIZE, PAGE_SIZE), 10_000);
        }
        assert_eq!(cache.pages.len(), MAX_PAGES);
        assert!(cache.row(0).is_some());
        assert!(cache.row(49 * PAGE_SIZE + 127).is_some());
        assert!(!cache.contains(1));
    }

    #[test]
    fn unchanged_progress_page_preserves_shared_rows() {
        let mut cache = HistoryCache::default();
        assert!(cache.insert(0, rows(0, PAGE_SIZE), PAGE_SIZE));
        let before = cache.row_shared(30).unwrap();
        assert!(!cache.insert(0, rows(0, PAGE_SIZE), PAGE_SIZE));
        assert!(Arc::ptr_eq(&before, &cache.row_shared(30).unwrap()));
    }

    #[test]
    fn filtered_positions_are_result_offsets_and_not_walk_positions() {
        let mut cache = HistoryCache::default();
        cache.insert(0, rows(900, 4), 4);
        assert_eq!(cache.find_sha("903"), Some(3));
        assert_eq!(cache.row(3).unwrap().position, 903);
    }
}
