use crate::format::count_label;
use gpui_kit::SharedString;
use icu_normalizer::{DecomposingNormalizer, properties::CanonicalCombiningClassMapBorrowed};
use mangrove_core::model::{CommitPage, CommitRow, IndexProgress, Reference};
use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    ops::Range,
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

/// A non-empty filter looks up the result offset instead of the walk position.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PositionRead {
    Selection {
        generation: u64,
        sha: String,
        filter: String,
    },
    Jump {
        generation: u64,
        repository_generation: u64,
        sha: String,
        filter: String,
        deadline: Instant,
    },
}

impl PositionRead {
    pub fn sha(&self) -> &str {
        match self {
            Self::Selection { sha, .. } | Self::Jump { sha, .. } => sha,
        }
    }

    pub fn filter(&self) -> &str {
        match self {
            Self::Selection { filter, .. } | Self::Jump { filter, .. } => filter,
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

/// Display priority in the reference column.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ReferenceRank {
    Head,
    Local,
    Tag,
    Remote,
}

impl ReferenceRank {
    fn of(reference: &Reference, head: Option<&str>, head_name: &str) -> Self {
        match reference.kind.as_str() {
            "local" if head == Some(reference.sha.as_str()) && reference.label == head_name => {
                Self::Head
            }
            "local" => Self::Local,
            "tag" => Self::Tag,
            _ => Self::Remote,
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::Head => "branche courante",
            Self::Local => "branche locale",
            Self::Tag => "tag",
            Self::Remote => "branche distante",
        }
    }
}

const MAX_BADGES: usize = 3;
// Border, padding, icon and gap; the « +N » chip has no icon.
const BADGE_CHROME: f32 = 25.;
const CHIP_CHROME: f32 = 12.;
const BADGE_GAP: f32 = 4.;
const MAX_TOOLTIP_REFERENCES: usize = 16;

pub struct ReferenceBadge {
    pub label: SharedString,
    pub rank: ReferenceRank,
    chars: usize,
}

pub struct ReferenceGroup {
    /// Displayed references first, each part in display priority.
    pub references: Arc<Vec<Reference>>,
    ranks: Vec<ReferenceRank>,
    /// Solo displays only its own branches.
    pub displayed: usize,
    pub badges: Vec<ReferenceBadge>,
}

impl ReferenceGroup {
    fn new(
        references: Vec<Reference>,
        solo: &HashSet<&str>,
        head: Option<&str>,
        head_name: &str,
    ) -> Self {
        let mut ranked: Vec<_> = references
            .into_iter()
            .map(|reference| {
                let hidden = !solo.is_empty() && !solo.contains(reference.name.as_str());
                (
                    hidden,
                    ReferenceRank::of(&reference, head, head_name),
                    reference,
                )
            })
            .collect();
        ranked.sort_by_key(|(hidden, rank, _)| (*hidden, *rank));
        let displayed = ranked.iter().filter(|(hidden, ..)| !hidden).count();
        let (ranks, references): (Vec<_>, Vec<_>) = ranked
            .into_iter()
            .map(|(_, rank, reference)| (rank, reference))
            .unzip();
        let badges = references
            .iter()
            .zip(&ranks)
            .take(displayed.min(MAX_BADGES))
            .map(|(reference, rank)| ReferenceBadge {
                label: reference.label.clone().into(),
                rank: *rank,
                chars: reference.label.chars().count(),
            })
            .collect();
        Self {
            references: Arc::new(references),
            ranks,
            displayed,
            badges,
        }
    }

    /// Bounded tooltip text listing the displayed references.
    pub fn tooltip(&self) -> String {
        let mut lines: Vec<_> = self
            .references
            .iter()
            .zip(&self.ranks)
            .take(self.displayed.min(MAX_TOOLTIP_REFERENCES))
            .map(|(reference, rank)| format!("{} · {}", reference.label, rank.description()))
            .collect();
        if self.displayed > MAX_TOOLTIP_REFERENCES {
            lines.push(count_label(
                self.displayed - MAX_TOOLTIP_REFERENCES,
                "autre référence",
                "autres références",
            ));
        }
        lines.join("\n")
    }

    /// Badges that fit a column `width` wide. The first one always shows,
    /// truncated if needed; the others only fit beside the « +N » chip.
    pub fn fitting_badges(&self, width: f32, char_width: f32) -> usize {
        let mut used = 0.;
        let mut shown = 0;
        for (index, badge) in self.badges.iter().enumerate() {
            let gap = if index == 0 { 0. } else { BADGE_GAP };
            let badge_width = BADGE_CHROME + badge.chars as f32 * char_width;
            let hidden = self.displayed - index - 1;
            let chip = if hidden == 0 {
                0.
            } else {
                BADGE_GAP + CHIP_CHROME + (hidden.ilog10() as f32 + 2.) * char_width
            };
            if index > 0 && used + gap + badge_width + chip > width {
                break;
            }
            used += gap + badge_width;
            shown = index + 1;
        }
        shown
    }
}

pub fn group_references(
    references: &[Reference],
    solo: &[String],
    head: Option<&str>,
    head_name: &str,
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
            let group = ReferenceGroup::new(references, &solo, head, head_name);
            (sha, Arc::new(group))
        })
        .collect()
}

/// Search words as the unicode61 index matches them: case- and
/// accent-insensitive word prefixes.
pub fn highlight_terms(filter: &str) -> Vec<String> {
    let mut terms: Vec<_> = filter
        .split_whitespace()
        .map(|term| fold(term.trim_matches(|character: char| !character.is_alphanumeric())))
        .filter(|term| !term.is_empty())
        .take(12)
        .collect();
    terms.sort();
    terms.dedup();
    terms
}

fn fold(text: &str) -> String {
    Folded::new(text).text
}

/// Folded text, mapped back to whole source characters.
struct Folded {
    text: String,
    /// Source character of each folded byte; ASCII text maps to itself.
    sources: Option<Vec<Range<usize>>>,
}

impl Folded {
    fn new(text: &str) -> Self {
        if text.is_ascii() {
            return Self {
                text: text.to_ascii_lowercase(),
                sources: None,
            };
        }
        let (text, sources) = folded(text);
        Self {
            text,
            sources: Some(sources),
        }
    }

    fn source(&self, folded: Range<usize>) -> Range<usize> {
        match &self.sources {
            Some(sources) => sources[folded.start].start..sources[folded.end - 1].end,
            None => folded,
        }
    }
}

fn folded(text: &str) -> (String, Vec<Range<usize>>) {
    let combining = CanonicalCombiningClassMapBorrowed::new();
    let normalizer = DecomposingNormalizer::new_nfd();
    let mut folded = String::with_capacity(text.len());
    let mut sources: Vec<Range<usize>> = Vec::with_capacity(text.len());
    let mut previous = 0;
    for (start, character) in text.char_indices() {
        let end = start + character.len_utf8();
        let before = folded.len();
        folded.extend(
            normalizer
                .normalize_iter(std::iter::once(character))
                .filter(|character| combining.get_u8(*character) == 0)
                .flat_map(char::to_lowercase),
        );
        if folded.len() == before {
            // A lone combining mark stays with its base character.
            let length = sources.len();
            for source in &mut sources[length - previous..] {
                source.end = end;
            }
        } else {
            previous = folded.len() - before;
            sources.extend(std::iter::repeat_n(start..end, previous));
        }
    }
    (folded, sources)
}

/// Source ranges where a term starts a word of `text`.
pub fn highlight_ranges(text: &str, terms: &[String]) -> Vec<Range<usize>> {
    if terms.is_empty() {
        return Vec::new();
    }
    let folded = Folded::new(text);
    let mut ranges: Vec<Range<usize>> = terms
        .iter()
        .flat_map(|term| folded.text.match_indices(term.as_str()))
        .filter(|(start, _)| {
            folded.text[..*start]
                .chars()
                .next_back()
                .is_none_or(|character| !character.is_alphanumeric())
        })
        .map(|(start, matched)| folded.source(start..start + matched.len()))
        .collect();
    ranges.sort_by_key(|range| range.start);
    let mut merged: Vec<Range<usize>> = Vec::with_capacity(ranges.len());
    for range in ranges {
        match merged.last_mut() {
            Some(previous) if range.start <= previous.end => {
                previous.end = previous.end.max(range.end)
            }
            _ => merged.push(range),
        }
    }
    merged
}

/// Source range of a prefix search, as SHA and reference queries match.
pub fn prefix_range(text: &str, filter: &str) -> Option<Range<usize>> {
    let filter = fold(filter);
    let folded = Folded::new(text);
    (!filter.is_empty() && folded.text.starts_with(&filter)).then(|| folded.source(0..filter.len()))
}

/// First result offset whose walk position is at least `target`. Results
/// keep walk order, so a present commit is found at this offset.
pub fn result_offset<E>(
    total: usize,
    target: usize,
    mut position: impl FnMut(usize) -> Result<Option<usize>, E>,
) -> Result<usize, E> {
    let (mut low, mut high) = (0, total);
    while low < high {
        let middle = low + (high - low) / 2;
        match position(middle)? {
            Some(found) if found < target => low = middle + 1,
            _ => high = middle,
        }
    }
    Ok(low)
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
            filter: String::new(),
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
            filter: String::new(),
        });
        let jump = PositionRead::Jump {
            generation: 3,
            repository_generation: 4,
            sha: "latest".into(),
            filter: "fix".into(),
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

    fn reference(kind: &str, label: &str) -> Reference {
        let prefix = match kind {
            "local" => "refs/heads",
            "remote" => "refs/remotes",
            _ => "refs/tags",
        };
        Reference {
            name: format!("{prefix}/{label}"),
            label: label.into(),
            kind: kind.into(),
            sha: "same".into(),
        }
    }

    fn labels(group: &ReferenceGroup) -> Vec<&str> {
        group
            .references
            .iter()
            .map(|reference| reference.label.as_str())
            .collect()
    }

    #[test]
    fn shared_reference_groups_keep_badges_bounded_and_solo_order_intact() {
        let references: Vec<_> = (0..10_000)
            .map(|index| reference("local", &format!("branch-{index}")))
            .collect();
        let groups = group_references(&references, &[], None, "main");
        let group = groups.get("same").unwrap();
        assert_eq!(group.references.len(), 10_000);
        assert_eq!(group.displayed, 10_000);
        assert_eq!(group.badges.len(), MAX_BADGES);
        assert_eq!(group.badges[0].label, "branch-0");
        assert_eq!(group.badges[1].label, "branch-1");
        let tooltip = group.tooltip();
        assert_eq!(tooltip.lines().count(), MAX_TOOLTIP_REFERENCES + 1);
        assert!(tooltip.ends_with("9\u{202f}984 autres références"));
        let solo = [references[999].name.clone(), references[88].name.clone()];
        let groups = group_references(&references, &solo, None, "main");
        let group = groups.get("same").unwrap();
        assert_eq!(group.displayed, 2);
        assert_eq!(group.badges.len(), 2);
        assert_eq!(group.badges[0].label, "branch-88");
        assert_eq!(group.badges[1].label, "branch-999");
        // Hidden branches stay available to the Solo menu, after displayed ones.
        assert_eq!(group.references.len(), 10_000);
        assert_eq!(group.references[2].label, "branch-0");
    }

    #[test]
    fn references_put_the_current_branch_then_locals_tags_and_remotes_first() {
        let references = [
            reference("remote", "origin/main"),
            reference("tag", "v1.0"),
            reference("local", "feature"),
            reference("local", "main"),
            reference("remote", "origin/feature"),
            reference("tag", "v0.9"),
        ];
        let groups = group_references(&references, &[], Some("same"), "main");
        let group = groups.get("same").unwrap();
        assert_eq!(
            labels(group),
            [
                "main",
                "feature",
                "v1.0",
                "v0.9",
                "origin/main",
                "origin/feature"
            ]
        );
        let ranks: Vec<_> = group.badges.iter().map(|badge| badge.rank).collect();
        assert_eq!(
            ranks,
            [
                ReferenceRank::Head,
                ReferenceRank::Local,
                ReferenceRank::Tag
            ]
        );
        assert_eq!(
            group.tooltip().lines().next(),
            Some("main · branche courante")
        );
        // The same branch name elsewhere, or a detached HEAD, is not current.
        for (head, name) in [(Some("other"), "main"), (Some("same"), "HEAD détachée")] {
            let groups = group_references(&references, &[], head, name);
            let group = groups.get("same").unwrap();
            assert_eq!(group.badges[0].rank, ReferenceRank::Local);
            assert_eq!(labels(group)[..2], ["feature", "main"]);
        }
    }

    #[test]
    fn narrow_reference_columns_keep_one_badge_and_count_the_others() {
        let group = |labels: &[&str]| {
            let references: Vec<_> = labels
                .iter()
                .map(|label| reference("local", label))
                .collect();
            group_references(&references, &[], None, "")
                .remove("same")
                .unwrap()
        };
        let single = group(&["a-very-long-branch-name-that-never-fits"]);
        assert_eq!(single.fitting_badges(40., 7.), 1);
        let many = group(&["main", "dev", "qa", "release"]);
        assert_eq!(many.fitting_badges(40., 7.), 1);
        // « main » (25 + 4 × 7), « dev » (4 + 25 + 3 × 7) and « +2 » (4 + 12 + 2 × 7).
        assert_eq!(many.fitting_badges(132., 7.), 1);
        assert_eq!(many.fitting_badges(133., 7.), 2);
        assert_eq!(many.fitting_badges(1_000., 7.), MAX_BADGES);
        let pair = group(&["main", "dev"]);
        // The last badge needs no chip.
        assert_eq!(pair.fitting_badges(103., 7.), 2);
        assert_eq!(pair.fitting_badges(102., 7.), 1);
    }

    #[test]
    fn highlights_follow_word_prefixes_without_case_or_accents() {
        let terms = highlight_terms("  Écol (fix)  fix ÉCOL ");
        assert_eq!(terms, ["ecol", "fix"]);
        let text = "Fix: prefix école, Ecole";
        let ranges = highlight_ranges(text, &terms);
        let matched: Vec<_> = ranges.iter().map(|range| &text[range.clone()]).collect();
        assert_eq!(matched, ["Fix", "écol", "Ecol"]);
        // Decomposed accents and case folds that change UTF-8 length stay whole.
        let decomposed = "ÉCOLE e\u{301}cole İstanbul";
        let ranges = highlight_ranges(decomposed, &highlight_terms("ecole istan"));
        let matched: Vec<_> = ranges
            .iter()
            .map(|range| &decomposed[range.clone()])
            .collect();
        assert_eq!(matched, ["ÉCOLE", "e\u{301}cole", "İstan"]);
        assert!(highlight_ranges(text, &[]).is_empty());
        assert!(highlight_terms(" -- ").is_empty());
        let overlapping = highlight_ranges("release", &highlight_terms("rel release"));
        assert_eq!((overlapping.len(), &overlapping[0]), (1, &(0..7)));
    }

    #[test]
    fn prefix_highlights_match_sha_and_reference_queries() {
        assert_eq!(prefix_range("abc1234", "ABC"), Some(0..3));
        assert_eq!(prefix_range("abc1234", "bc"), None);
        assert_eq!(prefix_range("Évolution", "evo"), Some(0..4));
        assert_eq!(prefix_range("abc", ""), None);
    }

    #[test]
    fn result_offsets_are_found_by_walk_position_in_logarithmic_reads() {
        let positions = [3, 8, 13, 21, 34, 55];
        let mut reads = 0;
        let mut offset = |target| {
            result_offset(positions.len(), target, |index| {
                reads += 1;
                Ok::<_, ()>(positions.get(index).copied())
            })
            .unwrap()
        };
        assert_eq!(offset(21), 3);
        assert_eq!(offset(3), 0);
        assert_eq!(offset(55), 5);
        // Absent commits resolve to a neighbour, rejected by the SHA check.
        assert_eq!(offset(22), 4);
        assert_eq!(offset(99), 6);
        assert!(reads <= 5 * 3);
        assert_eq!(
            result_offset(0, 1, |_| Err::<Option<usize>, _>("unused")),
            Ok(0)
        );
        assert_eq!(
            result_offset(4, 1, |_| Err::<Option<usize>, _>("io")),
            Err("io")
        );
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
