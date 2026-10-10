use crate::{
    command_palette::shortcut_label,
    format::format_count,
    graph::{self, ROW_HEIGHT},
    history::{
        HistoryCache, HistoryPage, HistoryReadSlots, PAGE_SIZE, PageRequests, PositionRead,
        PositionRequests, ReferenceGroup, group_references,
    },
    theme::{self, Palette, RADIUS_MD, RADIUS_SM},
};
use gpui_kit::{
    assets::IconName,
    component::input::{Input, InputEvent, InputState},
    component::scroll::Scrollbar,
    prelude::*,
    *,
};
use mangrove_core::{Workspace, model::*};
use std::{
    collections::{HashMap, HashSet},
    ops::Range,
    sync::Arc,
    time::{Duration, Instant},
};

#[derive(Clone, Debug)]
pub enum HistoryEvent {
    Selected { sha: String, keyboard: bool },
    ToggleSolo(String),
    ExitSoloAndNavigate(String),
    SelectWorkingTree,
    ClearSolo,
    RefreshRepository,
    Changed,
}

/// One native history view, kept alive across pane switches and tab changes.
/// All SQLite/libgit2 reads run on the background executor; drawing visits only
/// the visible fixed-height rows, with a twelve-page bounded cache.
pub struct HistoryView {
    engine: Arc<Workspace>,
    repository: Option<Arc<RepositoryInfo>>,
    references: HashMap<String, Arc<ReferenceGroup>>,
    solo_banner: Option<SharedString>,
    head_label: SharedString,
    pages: HistoryCache,
    page_requests: PageRequests,
    position_requests: PositionRequests,
    failed: HashSet<usize>,
    total: usize,
    progress: IndexProgress,
    retired_index_run: Option<String>,
    search_indexed: usize,
    search_refreshed: Option<Instant>,
    query: String,
    search: Entity<InputState>,
    _search_subscription: Subscription,
    filter: String,
    debounce_pending: bool,
    repository_generation: u64,
    navigation_generation: u64,
    requested_index: Option<usize>,
    requested_keyboard: bool,
    jump_pending: bool,
    selected_sha: String,
    autoselect: bool,
    selected_index: usize,
    selection_needs_position: bool,
    error: Option<String>,
    scroll: UniformListScrollHandle,
    focus: FocusHandle,
    working_focus: FocusHandle,
    menu_focus: FocusHandle,
    menu: Option<BranchMenu>,
    palette: Palette,
    font_size: f32,
    width: f32,
    #[cfg(feature = "app-benchmark")]
    visible_range: Range<usize>,
    max_lanes: usize,
    lane_offset: usize,
    working: WorkingSummary,
    index_task: Option<Task<()>>,
    query_task: Option<Task<()>>,
    navigation_task: Option<Task<()>>,
}

#[derive(Default)]
struct WorkingSummary {
    available: bool,
    staged: usize,
    unstaged: usize,
    selected: bool,
    counts: Vec<(String, usize)>,
}

impl WorkingSummary {
    fn has_changes(&self) -> bool {
        self.available && (self.staged > 0 || self.unstaged > 0)
    }
}

struct BranchMenu {
    references: Vec<Reference>,
    position: Point<Pixels>,
    selected: usize,
    measure_index: usize,
    scroll: UniformListScrollHandle,
}

impl BranchMenu {
    fn new(references: &[Reference], position: Point<Pixels>) -> Option<Self> {
        let references: Vec<_> = references
            .iter()
            .filter(|reference| reference.kind == "local" || reference.kind == "remote")
            .cloned()
            .collect();
        let measure_index = references
            .iter()
            .enumerate()
            .max_by_key(|(_, reference)| reference.label.chars().count())?
            .0;
        Some(Self {
            references,
            position,
            selected: 0,
            measure_index,
            scroll: UniformListScrollHandle::new(),
        })
    }

    fn navigate_key(&mut self, key: &str) -> bool {
        self.selected = match key {
            "down" => (self.selected + 1) % self.references.len(),
            "up" => (self.selected + self.references.len() - 1) % self.references.len(),
            "home" => 0,
            "end" => self.references.len() - 1,
            _ => return false,
        };
        self.scroll
            .scroll_to_item(self.selected, ScrollStrategy::Nearest);
        true
    }

    fn row_height(font_size: f32) -> Pixels {
        px(16. + ((11. + font_size) * 1.5).max(14.))
    }

    fn viewport_height(&self, font_size: f32, window_height: Pixels) -> Pixels {
        (Self::row_height(font_size) * self.references.len())
            .min((window_height - px(28.)).max(px(0.)))
    }
}

impl EventEmitter<HistoryEvent> for HistoryView {}
impl Focusable for HistoryView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl HistoryView {
    pub fn new(
        engine: Arc<Workspace>,
        palette: Palette,
        font_size: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let search = cx
            .new(|cx| InputState::new(window, cx).placeholder("Message, auteur, SHA, référence…"));
        let search_subscription = cx.subscribe_in(&search, window, |view, input, event, _, cx| {
            if matches!(event, InputEvent::Change) {
                view.set_query(input.read(cx).value().chars().take(256).collect(), cx);
            }
        });
        let read_slots = HistoryReadSlots::default();
        Self {
            engine,
            repository: None,
            references: HashMap::new(),
            solo_banner: None,
            head_label: "HEAD".into(),
            pages: HistoryCache::default(),
            page_requests: PageRequests::new(read_slots.clone()),
            position_requests: PositionRequests::new(read_slots),
            failed: HashSet::new(),
            total: 0,
            progress: IndexProgress {
                indexed: 0,
                complete: false,
                error: None,
                index_run: String::new(),
            },
            retired_index_run: None,
            search_indexed: 0,
            search_refreshed: None,
            query: String::new(),
            search,
            _search_subscription: search_subscription,
            filter: String::new(),
            debounce_pending: false,
            repository_generation: 0,
            navigation_generation: 0,
            requested_index: None,
            requested_keyboard: false,
            jump_pending: false,
            selected_sha: String::new(),
            autoselect: true,
            selected_index: 0,
            selection_needs_position: false,
            error: None,
            scroll: UniformListScrollHandle::new(),
            focus: cx.focus_handle(),
            working_focus: cx.focus_handle(),
            menu_focus: cx.focus_handle(),
            menu: None,
            palette,
            font_size,
            width: 800.,
            #[cfg(feature = "app-benchmark")]
            visible_range: 0..0,
            max_lanes: 1,
            lane_offset: 0,
            working: WorkingSummary::default(),
            index_task: None,
            query_task: None,
            navigation_task: None,
        }
    }

    #[cfg(feature = "app-benchmark")]
    pub fn selected_sha(&self) -> &str {
        &self.selected_sha
    }
    #[cfg(feature = "app-benchmark")]
    pub fn selected_index(&self) -> usize {
        self.selected_index
    }
    #[cfg(feature = "app-benchmark")]
    pub fn total(&self) -> usize {
        self.total
    }
    #[cfg(feature = "app-benchmark")]
    pub fn index_complete(&self) -> bool {
        self.progress.complete
    }
    pub fn filtered(&self) -> bool {
        !self.filter.is_empty()
    }
    pub fn loading(&self) -> bool {
        self.debounce_pending || self.page_requests.loading()
    }
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }
    pub fn clear_error(&mut self, cx: &mut Context<Self>) {
        if self.error.take().is_some() {
            cx.emit(HistoryEvent::Changed);
            cx.notify();
        }
    }
    #[cfg(feature = "app-benchmark")]
    pub fn visible_loaded_rows(&self) -> usize {
        self.visible_range
            .clone()
            .filter(|index| self.pages.row(*index).is_some())
            .count()
    }
    #[cfg(feature = "app-benchmark")]
    pub fn scroll_to_offset(&mut self, offset: f32, cx: &mut Context<Self>) {
        self.scroll
            .0
            .borrow()
            .base_handle
            .set_offset(point(px(0.), px(-offset.max(0.))));
        cx.notify();
    }
    pub fn focus_search(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.search.update(cx, |input, cx| input.focus(window, cx));
    }

    pub fn focus_list(&self, window: &mut Window, cx: &mut Context<Self>) {
        if self.working.selected && self.show_working_row() {
            self.working_focus.focus(window, cx);
        } else {
            self.focus.focus(window, cx);
        }
    }

    pub fn set_theme(&mut self, palette: Palette, font_size: f32, cx: &mut Context<Self>) {
        if palette == self.palette && font_size == self.font_size {
            return;
        }
        self.palette = palette;
        self.font_size = font_size;
        cx.notify();
    }

    pub fn set_working_tree_summary(
        &mut self,
        available: bool,
        staged: usize,
        unstaged: usize,
        selected: bool,
        cx: &mut Context<Self>,
    ) {
        if (available, staged, unstaged, selected)
            == (
                self.working.available,
                self.working.staged,
                self.working.unstaged,
                self.working.selected,
            )
        {
            return;
        }
        self.working.available = available;
        self.working.staged = staged;
        self.working.unstaged = unstaged;
        self.working.selected = selected;
        cx.notify();
    }

    pub fn set_working_tree_counts(
        &mut self,
        counts: Vec<(String, usize)>,
        cx: &mut Context<Self>,
    ) {
        if self.working.counts != counts {
            self.working.counts = counts;
            cx.notify();
        }
    }

    pub fn set_working_tree_selection(&mut self, selected: bool, cx: &mut Context<Self>) {
        if selected != self.working.selected {
            self.working.selected = selected;
            cx.notify();
        }
    }

    pub fn has_working_tree_changes(&self) -> bool {
        self.working.has_changes()
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.repository_generation += 1;
        self.cancel_pages();
        self.cancel_navigation();
        self.index_task = None;
        self.query_task = None;
        self.repository = None;
        self.references.clear();
        self.solo_banner = None;
        self.head_label = "HEAD".into();
        self.total = 0;
        self.retired_index_run = None;
        self.progress = IndexProgress {
            indexed: 0,
            complete: false,
            error: None,
            index_run: String::new(),
        };
        self.query.clear();
        self.filter.clear();
        self.debounce_pending = false;
        self.selected_sha.clear();
        self.autoselect = true;
        self.selected_index = 0;
        self.selection_needs_position = false;
        self.error = None;
        self.menu = None;
        self.working = WorkingSummary::default();
        self.scroll = UniformListScrollHandle::new();
        #[cfg(feature = "app-benchmark")]
        {
            self.visible_range = 0..0;
        }
        self.max_lanes = 1;
        self.lane_offset = 0;
        self.changed(cx);
    }

    pub fn set_repository(
        &mut self,
        info: Arc<RepositoryInfo>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let same_path = self
            .repository
            .as_ref()
            .is_some_and(|old| old.path == info.path);
        let same_session = self
            .repository
            .as_ref()
            .is_some_and(|old| old.id == info.id);
        self.menu = None;
        if same_session {
            self.repository = Some(info.clone());
            let recovered = !self.failed.is_empty();
            self.failed.clear();
            if recovered {
                self.error = None;
                self.request_page(0, cx);
            }
            if self.merge_progress(&info.index) || recovered {
                self.request_visible_pages(cx);
                self.start_polling(cx);
                self.changed(cx);
            }
            return;
        }
        if !same_path {
            self.clear(cx);
        }
        self.references = group_references(&info.references, &info.solo_refs);
        let solo: HashSet<_> = info.solo_refs.iter().map(String::as_str).collect();
        let solo_labels: Vec<_> = info
            .references
            .iter()
            .filter(|reference| solo.contains(reference.name.as_str()))
            .map(|reference| reference.label.as_str())
            .collect();
        self.solo_banner =
            (!solo_labels.is_empty()).then(|| format!("Solo : {}", solo_labels.join(", ")).into());
        self.head_label = format!("HEAD · {}", info.head_name).into();
        self.repository_generation += 1;
        self.page_requests.reset();
        self.position_requests.clear();
        self.failed.clear();
        self.pages.mark_stale();
        self.cancel_navigation();
        self.index_task = None;
        self.query_task = None;
        self.debounce_pending = false;
        self.filter = normalized_query(&self.query);
        self.repository = Some(info.clone());
        self.progress = info.index.clone();
        self.retired_index_run = None;
        if self.filter.is_empty() {
            self.total = info.index.indexed;
        }
        self.search_indexed = 0;
        self.search_refreshed = None;
        self.selection_needs_position = !self.selected_sha.is_empty();
        self.error = info.index.error.clone();
        self.max_lanes = 1;
        self.lane_offset = 0;
        if !same_path
            && info.solo_refs.is_empty()
            && let Some(head) = info.head.clone()
        {
            // Worktrees share the global history. Its first row can belong to
            // another branch, so open at this checkout's HEAD, even while its
            // position is still being indexed or lies beyond page zero.
            self.jump_to(head, window, cx);
        } else {
            self.request_page(0, cx);
            // Completed indexes do not poll; rebase preserved keyboard selection now.
            self.refresh_position(cx);
        }
        self.start_polling(cx);
        self.changed(cx);
    }

    fn changed(&mut self, cx: &mut Context<Self>) {
        self.retire_pages(cx);
        cx.emit(HistoryEvent::Changed);
        cx.notify();
    }

    fn retire_pages(&mut self, cx: &mut Context<Self>) {
        let retired = self.pages.take_retired();
        if !retired.is_empty() {
            cx.background_executor()
                .spawn(async move {
                    drop(retired);
                })
                .detach();
        }
    }

    fn page_useful(&self, page: usize) -> bool {
        self.pages.useful(page)
    }

    fn cancel_pages(&mut self) {
        self.page_requests.reset();
        self.position_requests.clear();
        self.failed.clear();
        self.pages.clear();
        self.search_indexed = 0;
        self.search_refreshed = None;
    }

    fn cancel_navigation(&mut self) {
        self.navigation_generation += 1;
        self.requested_index = None;
        self.requested_keyboard = false;
        self.pages.set_requested_page(None);
        self.jump_pending = false;
        self.navigation_task = None;
        self.position_requests.clear();
    }

    pub fn set_query(&mut self, query: String, cx: &mut Context<Self>) {
        let next = normalized_query(&query);
        let old = normalized_query(&self.query);
        self.query = query;
        if next == old {
            return;
        }
        self.query_task = None;
        self.cancel_navigation();
        self.cancel_pages();
        self.selected_sha.clear();
        self.autoselect = true;
        self.selected_index = 0;
        self.total = 0;
        self.error = None;
        self.menu = None;
        self.debounce_pending = true;
        self.scroll = UniformListScrollHandle::new();
        let generation = self.page_requests.generation();
        self.query_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(160))
                .await;
            let _ = this.update(cx, |view, cx| {
                if generation != view.page_requests.generation() {
                    return;
                }
                view.debounce_pending = false;
                view.filter = next;
                if view.filter.is_empty() {
                    view.total = view.progress.indexed;
                }
                view.request_page(0, cx);
                view.changed(cx);
            });
        }));
        self.changed(cx);
    }

    fn merge_progress(&mut self, incoming: &IndexProgress) -> bool {
        let new_run = self.progress.index_run != incoming.index_run;
        if new_run && self.retired_index_run.as_deref() == Some(&incoming.index_run) {
            // A position lookup can observe publication before an already
            // running status/open finishes. The superseded run cannot return.
            return false;
        }
        if new_run {
            // Each session publishes at most one replacement traversal.
            self.retired_index_run = Some(self.progress.index_run.clone());
        }
        let indexed = if new_run {
            incoming.indexed
        } else {
            self.progress.indexed.max(incoming.indexed)
        };
        // A refresh can start a background rebuild of the published run.
        let changed = new_run
            || indexed != self.progress.indexed
            || incoming.complete != self.progress.complete
            || incoming.error != self.progress.error;
        self.progress = IndexProgress {
            indexed,
            complete: incoming.complete,
            error: incoming.error.clone(),
            index_run: incoming.index_run.clone(),
        };
        if new_run {
            self.page_requests.reset();
            self.position_requests.clear();
            self.pages.mark_stale();
            self.failed.clear();
            self.search_indexed = 0;
            self.search_refreshed = None;
            self.selection_needs_position = !self.selected_sha.is_empty();
            self.max_lanes = 1;
            if !self.filtered() {
                self.total = indexed;
            }
        }
        changed
    }

    fn start_polling(&mut self, cx: &mut Context<Self>) {
        if self.progress.complete || self.progress.error.is_some() {
            return;
        }
        let generation = self.repository_generation;
        let Some(info) = &self.repository else {
            return;
        };
        let id = info.id.clone();
        let engine = self.engine.clone();
        self.index_task = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(350))
                    .await;
                let engine = engine.clone();
                let id = id.clone();
                let result = cx
                    .background_executor()
                    .spawn(async move { engine.index_status(&id) })
                    .await;
                let Ok(stop) = this.update(cx, |view, cx| {
                    if view.repository_generation != generation {
                        return true;
                    }
                    match result {
                        Ok(progress) => {
                            let changed = view.merge_progress(&progress);
                            if !view.debounce_pending {
                                if view.filter.is_empty() {
                                    view.total = view.total.max(view.progress.indexed);
                                    view.pages.invalidate_trailing(view.total);
                                } else if view.search_indexed < view.progress.indexed
                                    && (view.progress.complete
                                        || view.search_refreshed.is_none_or(|last| {
                                            last.elapsed() >= Duration::from_millis(1400)
                                        }))
                                {
                                    // Refresh a bounded snapshot without blanking the visible
                                    // list on each index tick. Old pages remain until replaced.
                                    view.pages.mark_stale();
                                    view.search_refreshed = Some(Instant::now());
                                }
                                view.request_visible_pages(cx);
                                if view.selection_needs_position {
                                    view.refresh_position(cx);
                                }
                            }
                            if changed {
                                view.changed(cx);
                            }
                            view.progress.complete || view.progress.error.is_some()
                        }
                        Err(error) => {
                            view.error = Some(error);
                            view.changed(cx);
                            true
                        }
                    }
                }) else {
                    return;
                };
                if stop {
                    return;
                }
            }
        }));
    }

    fn accept_page(&mut self, page: HistoryPage, cx: &mut Context<Self>) -> bool {
        // A response from an older published run must not roll progress back.
        if page.index.index_run != self.progress.index_run {
            // A newly published run can arrive before the next status tick.
            // Wait for that authoritative tick rather than spin on hot reads.
            self.pages.defer(page.offset / PAGE_SIZE);
            self.pages.retire(page);
            return false;
        }
        let mut changed = page.index.indexed > self.progress.indexed;
        self.progress.indexed = self.progress.indexed.max(page.index.indexed);
        if let Some(error) = page.index.error.clone() {
            self.error = Some(error);
        }
        if self.filtered() {
            if page.index.indexed < self.search_indexed {
                self.pages.retire(page);
                return false;
            }
            if page.index.indexed > self.search_indexed {
                self.pages.mark_stale();
                self.search_indexed = page.index.indexed;
                self.search_refreshed = Some(Instant::now());
                changed = true;
            }
        }
        let total = if self.filtered() {
            page.total
        } else {
            page.total.max(self.progress.indexed)
        };
        changed |= total != self.total;
        self.total = total;
        let first = if page.offset == 0 {
            page.rows.first().map(|row| row.commit.sha.clone())
        } else {
            None
        };
        self.max_lanes = self.max_lanes.max(page.max_lanes);
        changed |= self.pages.insert(page, self.total);
        if self.autoselect
            && self.selected_sha.is_empty()
            && self.requested_index.is_none()
            && !self.jump_pending
            && let Some(sha) = first
        {
            self.select(sha, 0, false, cx);
        }
        if let Some(index) = self.requested_index
            && self.pages.current(index / PAGE_SIZE)
            && let Some(row) = self.pages.row(index)
        {
            self.select(row.sha.clone(), index, self.requested_keyboard, cx);
        }
        if self.selection_needs_position
            && self.filtered()
            && let Some(position) = self.pages.find_sha(&self.selected_sha)
            && self.pages.current(position / PAGE_SIZE)
        {
            self.selected_index = position;
            self.selection_needs_position = false;
            changed = true;
        }
        if changed {
            self.changed(cx);
        }
        true
    }

    fn request_visible_pages(&mut self, cx: &mut Context<Self>) {
        self.request_page(0, cx);
        let visible = self.pages.visible_page;
        for page in visible.saturating_sub(1)..=visible.saturating_add(1) {
            if page * PAGE_SIZE < self.total {
                self.request_page(page, cx);
            }
        }
    }

    fn request_page(&mut self, page: usize, cx: &mut Context<Self>) {
        if self.debounce_pending
            || self.pages.current(page)
            || !self.pages.can_request(page)
            || !self.page_useful(page)
            || self.failed.contains(&page)
            || self.repository.is_none()
        {
            return;
        }
        self.page_requests.request(page);
        self.pump_pages(cx);
    }

    fn pump_pages(&mut self, cx: &mut Context<Self>) {
        if self.debounce_pending {
            return;
        }
        let Some(info) = &self.repository else {
            return;
        };
        let id = info.id.clone();
        while let Some((read, lease)) = self.page_requests.try_start() {
            let page = read.page;
            if self.pages.current(page)
                || !self.pages.can_request(page)
                || !self.page_useful(page)
                || self.failed.contains(&page)
            {
                self.page_requests.finish(read);
                drop(lease);
                continue;
            }
            let engine = self.engine.clone();
            let id = id.clone();
            let filter = self.filter.clone();
            #[cfg(feature = "app-benchmark")]
            crate::perf::record_count("list_commits_read");
            cx.spawn(async move |this, cx| {
                let (lease, result) = cx
                    .background_executor()
                    .spawn(async move {
                        #[cfg(feature = "app-benchmark")]
                        let started = Instant::now();
                        let result = engine
                            .list_commits(&id, page * PAGE_SIZE, PAGE_SIZE, &filter)
                            .map(HistoryPage::new);
                        #[cfg(feature = "app-benchmark")]
                        crate::perf::record_operation("history_page", started.elapsed());
                        (lease, result)
                    })
                    .await;
                let _ = this.update(cx, |view, cx| {
                    let mut failed = false;
                    if view.page_requests.finish(read) {
                        match result {
                            Ok(result) => {
                                let accepted = if view.page_useful(page) {
                                    view.accept_page(result, cx)
                                } else {
                                    view.pages.retire(result);
                                    true
                                };
                                // A trailing page from an older SQLite snapshot
                                // must wait for another physically available slot.
                                if !accepted
                                    && view.page_useful(page)
                                    && view.pages.can_request(page)
                                    && page * PAGE_SIZE < view.total
                                {
                                    view.page_requests.request(page);
                                }
                            }
                            Err(error) => {
                                view.failed.insert(page);
                                view.error = Some(error);
                                failed = true;
                            }
                        }
                    } else if let Ok(page) = result {
                        view.pages.retire(page);
                    }
                    // Accept/reject first. Stale reads still release a physical
                    // slot and wake this generation's bounded page intents.
                    drop(lease);
                    if let Some(index) = view.requested_index {
                        let page = index / PAGE_SIZE;
                        if !view.pages.current(page)
                            && view.pages.can_request(page)
                            && !view.failed.contains(&page)
                        {
                            view.page_requests.request(page);
                        }
                    }
                    view.retire_pages(cx);
                    view.pump_pages(cx);
                    if failed {
                        view.changed(cx);
                    } else {
                        cx.notify();
                    }
                });
            })
            .detach();
        }
    }

    pub fn clear_selection(&mut self, cx: &mut Context<Self>) {
        self.cancel_navigation();
        self.selected_sha.clear();
        self.autoselect = false;
        self.selected_index = 0;
        cx.notify();
    }

    pub fn restore_commit_selection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.cancel_navigation();
        self.working.selected = false;
        self.selected_sha.clear();
        self.selected_index = 0;
        // A refresh can deliver the clean status before page zero. Keep the
        // first-commit selection armed until that page becomes available.
        self.autoselect = true;
        self.select_index(0, window, cx);
        cx.notify();
    }

    fn select(&mut self, sha: String, position: usize, keyboard: bool, cx: &mut Context<Self>) {
        self.cancel_navigation();
        if self.selected_sha == sha && self.selected_index == position && !self.working.selected {
            return;
        }
        self.working.selected = false;
        self.autoselect = false;
        self.selected_sha = sha.clone();
        self.selected_index = position;
        self.selection_needs_position = false;
        self.error = None;
        cx.emit(HistoryEvent::Selected { sha, keyboard });
        cx.notify();
    }

    pub fn select_index(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.select_index_with_source(index, false, window, cx);
    }

    pub fn select_index_keyboard(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_index_with_source(index, true, window, cx);
    }

    fn select_index_with_source(
        &mut self,
        index: usize,
        keyboard: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.total == 0 || self.debounce_pending {
            return;
        }
        self.cancel_navigation();
        let target = index.min(self.total - 1);
        self.focus.focus(window, cx);
        self.scroll_to_index(target);
        if self.pages.current(target / PAGE_SIZE)
            && let Some(row) = self.pages.row(target)
        {
            self.select(row.sha.clone(), target, keyboard, cx);
        } else {
            self.requested_index = Some(target);
            self.requested_keyboard = keyboard;
            self.pages.set_requested_page(Some(target / PAGE_SIZE));
            self.request_page(target / PAGE_SIZE, cx);
            cx.notify();
        }
    }

    pub fn navigate(&mut self, direction: isize, window: &mut Window, cx: &mut Context<Self>) {
        if self.working.selected && direction >= 0 {
            self.select_index_keyboard(0, window, cx);
            return;
        }
        if direction < 0 && self.selected_index == 0 && self.show_working_row() {
            self.cancel_navigation();
            self.working.selected = true;
            self.working_focus.focus(window, cx);
            cx.emit(HistoryEvent::SelectWorkingTree);
            cx.notify();
            return;
        }
        if self.selected_sha.is_empty() && self.requested_index.is_none() {
            self.select_index_keyboard(0, window, cx);
            return;
        }
        let target = self
            .requested_index
            .unwrap_or(self.selected_index)
            .saturating_add_signed(direction);
        self.select_index_keyboard(target, window, cx);
    }

    fn refresh_position(&mut self, cx: &mut Context<Self>) {
        if self.filtered() || self.selected_sha.is_empty() || !self.selection_needs_position {
            return;
        }
        self.position_requests.request(PositionRead::Selection {
            generation: self.page_requests.generation(),
            sha: self.selected_sha.clone(),
        });
        self.pump_position(cx);
    }

    fn position_is_current(&self, read: &PositionRead) -> bool {
        match read {
            PositionRead::Selection { generation, sha } => {
                *generation == self.page_requests.generation()
                    && sha == &self.selected_sha
                    && self.selection_needs_position
                    && !self.filtered()
            }
            PositionRead::Jump {
                generation,
                repository_generation,
                ..
            } => {
                *generation == self.navigation_generation
                    && *repository_generation == self.repository_generation
                    && self.jump_pending
            }
        }
    }

    fn pump_position(&mut self, cx: &mut Context<Self>) {
        let Some(info) = &self.repository else {
            return;
        };
        let id = info.id.clone();
        while let Some((read, lease)) = self.position_requests.try_start() {
            if !self.position_is_current(&read) {
                self.position_requests.finish(&read);
                drop(lease);
                continue;
            }
            let engine = self.engine.clone();
            let id = id.clone();
            let sha = read.sha().to_owned();
            let started_run = self.progress.index_run.clone();
            cx.spawn(async move |this, cx| {
                let (lease, result) = cx
                    .background_executor()
                    .spawn(async move {
                        let result = engine.commit_position_with_status(&id, &sha);
                        (lease, result)
                    })
                    .await;
                let _ = this.update(cx, |view, cx| {
                    view.position_requests.finish(&read);
                    if view.position_is_current(&read) {
                        if result.as_ref().is_ok_and(|(_, progress)| {
                            !position_snapshot_can_publish(
                                &started_run,
                                &view.progress.index_run,
                                &progress.index_run,
                            )
                        }) {
                            // Status polling published a different traversal
                            // while this lookup read the previous snapshot.
                            view.position_requests.request(read);
                        } else {
                            view.accept_position(read, result, cx);
                        }
                    }
                    drop(lease);
                    view.pump_position(cx);
                });
            })
            .detach();
        }
    }

    fn accept_position(
        &mut self,
        read: PositionRead,
        result: mangrove_core::Result<(Option<usize>, IndexProgress)>,
        cx: &mut Context<Self>,
    ) {
        let PositionRead::Jump {
            ref sha, deadline, ..
        } = read
        else {
            if let Ok((Some(position), progress)) = result {
                let changed = self.merge_progress(&progress);
                self.selected_index = position;
                self.selection_needs_position = false;
                if changed {
                    self.request_visible_pages(cx);
                    self.changed(cx);
                } else {
                    cx.notify();
                }
            }
            return;
        };
        match result {
            Ok((Some(position), progress)) => {
                self.jump_pending = false;
                self.merge_progress(&progress);
                self.total = self.total.max(progress.indexed);
                // Keep reference jumps visible as the surrounding panels load.
                self.scroll.scroll_to_item(position, ScrollStrategy::Center);
                if self.pages.current(position / PAGE_SIZE)
                    && let Some(row) = self.pages.row(position)
                {
                    // Rendering can load the target page before its position
                    // lookup completes. A cached page will not emit another
                    // accept_page callback, so select its row immediately.
                    self.select(row.sha.clone(), position, false, cx);
                } else {
                    self.requested_index = Some(position);
                    self.pages.set_requested_page(Some(position / PAGE_SIZE));
                    self.request_page(position / PAGE_SIZE, cx);
                }
                self.changed(cx);
            }
            Ok((None, progress)) => {
                self.merge_progress(&progress);
                if progress.complete
                    && progress.error.is_none()
                    && self
                        .repository
                        .as_ref()
                        .is_some_and(|info| !info.solo_refs.is_empty())
                {
                    self.jump_pending = false;
                    cx.emit(HistoryEvent::ExitSoloAndNavigate(sha.clone()));
                    return;
                }
                if progress.complete || progress.error.is_some() || Instant::now() >= deadline {
                    self.jump_pending = false;
                    self.error = Some(
                        progress
                            .error
                            .unwrap_or_else(|| "Cette référence n’est pas encore indexée.".into()),
                    );
                    self.request_page(0, cx);
                    self.changed(cx);
                } else {
                    self.navigation_task = Some(cx.spawn(async move |this, cx| {
                        cx.background_executor()
                            .timer(Duration::from_millis(100))
                            .await;
                        let _ = this.update(cx, |view, cx| {
                            if view.position_is_current(&read) {
                                view.position_requests.request(read);
                                view.pump_position(cx);
                            }
                        });
                    }));
                }
            }
            Err(error) => {
                self.jump_pending = false;
                self.error = Some(error);
                self.changed(cx);
            }
        }
    }

    pub fn jump_to(&mut self, sha: String, _window: &mut Window, cx: &mut Context<Self>) {
        if self.repository.is_none() {
            return;
        }
        self.query_task = None;
        self.cancel_navigation();
        self.cancel_pages();
        self.query.clear();
        self.filter.clear();
        self.debounce_pending = false;
        self.total = self.progress.indexed;
        self.selected_sha.clear();
        self.autoselect = false;
        self.selected_index = 0;
        self.jump_pending = true;
        self.position_requests.request(PositionRead::Jump {
            generation: self.navigation_generation,
            repository_generation: self.repository_generation,
            sha,
            deadline: Instant::now() + Duration::from_secs(30),
        });
        self.pump_position(cx);
        self.changed(cx);
    }

    fn scroll_to_index(&self, target: usize) {
        let top = f32::from(-self.scroll.0.borrow().base_handle.offset().y) / ROW_HEIGHT;
        self.scroll.scroll_to_item(
            target,
            if (target as f32) < top {
                ScrollStrategy::Top
            } else {
                ScrollStrategy::Bottom
            },
        );
    }

    fn graph_width(&self) -> f32 {
        if self.width <= 620. { 92. } else { 116. }
    }
    fn visible_lanes(&self) -> usize {
        ((self.graph_width() - 30.) / 18.).floor().max(1.) as usize
    }
    fn show_working_row(&self) -> bool {
        self.working.has_changes() && self.filter.is_empty() && !self.debounce_pending
    }

    fn open_menu(
        &mut self,
        refs: &[Reference],
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(menu) = BranchMenu::new(refs, position) else {
            return;
        };
        self.menu = Some(menu);
        self.menu_focus.focus(window, cx);
        cx.notify();
    }

    fn close_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.menu.take().is_some() {
            self.focus.focus(window, cx);
            cx.notify();
        }
    }

    fn context_position(&self, index: usize) -> Point<Pixels> {
        let state = self.scroll.0.borrow();
        let bounds = state.base_handle.bounds();
        point(
            bounds.left() + px(20.),
            (bounds.top() + px((index as f32 + 1.) * ROW_HEIGHT) + state.base_handle.offset().y)
                .min(bounds.bottom()),
        )
    }

    fn menu_label(&self, reference: &Reference) -> &'static str {
        let solo = self.repository.as_ref().map(|info| &info.solo_refs);
        if solo.is_some_and(|refs| refs.contains(&reference.name)) {
            "Retirer du Solo"
        } else if solo.is_some_and(|refs| !refs.is_empty()) {
            "Ajouter au Solo"
        } else {
            "Solo"
        }
    }

    fn menu_rows(
        &self,
        range: Range<usize>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let Some(menu) = self.menu.as_ref() else {
            return Vec::new();
        };
        let palette = self.palette;
        let multiple = menu.references.len() > 1;
        let selected = menu.selected;
        range
            .filter_map(|index| {
                let reference = menu.references.get(index)?;
                let name = reference.name.clone();
                Some(
                    div()
                        .id(("solo-menu-item", index))
                        .role(Role::MenuItem)
                        .focusable()
                        .tab_index(0)
                        .tab_stop(false)
                        .aria_label(format!(
                            "{} : {}",
                            self.menu_label(reference),
                            reference.label
                        ))
                        .aria_position_in_set(index + 1)
                        .aria_size_of_set(menu.references.len())
                        .when(
                            index == selected && self.menu_focus.is_focused(window),
                            |style| style.aria_active_descendant(),
                        )
                        .h(BranchMenu::row_height(self.font_size))
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .px(px(10.))
                        .rounded(RADIUS_SM)
                        .cursor_pointer()
                        .text_size(px(11. + self.font_size))
                        .line_height(relative(1.5))
                        .text_color(rgb(palette.text))
                        .when(index == selected, |style| {
                            style.bg(rgb(palette.accent_soft))
                        })
                        .hover(|style| style.bg(rgb(palette.accent_soft)))
                        .child(theme::icon(IconName::Focus, 14., palette.text))
                        .child(div().flex_shrink_0().child(self.menu_label(reference)))
                        .when(multiple, |style| {
                            style.child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_color(rgb(palette.muted))
                                    .child(format!(
                                        "{} · {}",
                                        if reference.kind == "local" {
                                            "Locale"
                                        } else {
                                            "Distante"
                                        },
                                        reference.label
                                    )),
                            )
                        })
                        .on_click(cx.listener(move |view, _, window, cx| {
                            view.close_menu(window, cx);
                            cx.emit(HistoryEvent::ToggleSolo(name.clone()));
                        }))
                        .into_any_element(),
                )
            })
            .collect()
    }

    fn menu_element(&self, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let menu = self.menu.as_ref()?;
        let palette = self.palette;
        let max_width = (window.viewport_size().width - px(16.))
            .max(px(0.))
            .min(px(580.));
        let content = div()
            .id("branch-context-menu")
            .role(Role::Menu)
            .aria_label("Actions Solo des branches")
            .track_focus(&self.menu_focus)
            .min_w(px(210.).min(max_width))
            .max_w(max_width)
            .occlude()
            .p(px(5.))
            .rounded(RADIUS_MD)
            .border_1()
            .border_color(rgb(palette.border))
            .bg(rgb(palette.panel))
            .shadow_lg()
            .on_mouse_down_out(cx.listener(|view, _, window, cx| view.close_menu(window, cx)))
            .on_key_down(cx.listener(|view, event: &KeyDownEvent, window, cx| {
                let Some(menu) = &mut view.menu else {
                    return;
                };
                if menu.navigate_key(&event.keystroke.key) {
                    cx.notify();
                    cx.stop_propagation();
                    return;
                }
                match event.keystroke.key.as_str() {
                    "escape" | "tab" => {
                        view.close_menu(window, cx);
                    }
                    "enter" | "space" => {
                        let name = menu.references[menu.selected].name.clone();
                        view.close_menu(window, cx);
                        cx.emit(HistoryEvent::ToggleSolo(name));
                    }
                    _ => return,
                }
                cx.stop_propagation();
            }))
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .relative()
                    .h(menu.viewport_height(self.font_size, window.viewport_size().height))
                    .overflow_hidden()
                    .child(
                        uniform_list(
                            "branch-menu-references",
                            menu.references.len(),
                            cx.processor(|view, range, window, cx| {
                                view.menu_rows(range, window, cx)
                            }),
                        )
                        .with_width_from_item(Some(menu.measure_index))
                        .track_scroll(&menu.scroll)
                        .h_full(),
                    )
                    .child(theme::scrollbar_theme(
                        Scrollbar::vertical(&menu.scroll).id("branch-menu-scrollbar"),
                    )),
            );
        Some(
            deferred(
                anchored()
                    .position(menu.position)
                    .snap_to_window_with_margin(px(8.))
                    .child(content),
            )
            .into_any_element(),
        )
    }

    fn headings(&self, cx: &mut Context<Self>) -> AnyElement {
        let palette = self.palette;
        let graph_width = self.graph_width();
        let visible = self.visible_lanes();
        div()
            .flex()
            .items_center()
            .h(px(35.))
            .flex_shrink_0()
            .w_full()
            .pr(px(14.))
            .gap(px(if self.width <= 620. { 9. } else { 12. }))
            .border_t_1()
            .border_b_1()
            .border_color(rgb(palette.border))
            .text_size(px(10. + self.font_size))
            .text_color(rgb(palette.muted))
            .child(
                div()
                    .w(px(graph_width))
                    .flex_shrink_0()
                    .pl(px(20.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child("Graphe")
                    .when(self.max_lanes > visible && !self.filtered(), |style| {
                        style.child(div().flex().gap(px(2.)).children(
                            [-1isize, 1].into_iter().map(|direction| {
                                let disabled = if direction < 0 {
                                    self.lane_offset == 0
                                } else {
                                    self.lane_offset + visible >= self.max_lanes
                                };
                                div()
                                    .id(if direction < 0 {
                                        "graph-pan-left"
                                    } else {
                                        "graph-pan-right"
                                    })
                                    .role(Role::Button)
                                    .focusable()
                                    .tab_index(0)
                                    .tab_stop(!disabled)
                                    .aria_label(if direction < 0 {
                                        "Décaler le graphe vers la gauche"
                                    } else {
                                        "Décaler le graphe vers la droite"
                                    })
                                    .when(disabled, |style| {
                                        style.a11y_synthetic_children(|tree| {
                                            tree.parent_node().set_disabled();
                                        })
                                    })
                                    .w(px(14.))
                                    .h(px(18.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .when(disabled, |style| style.opacity(0.3))
                                    .when(!disabled, |style| style.cursor_pointer())
                                    .child(theme::icon(
                                        if direction < 0 {
                                            IconName::ArrowLeft
                                        } else {
                                            IconName::ArrowRight
                                        },
                                        10.,
                                        palette.muted,
                                    ))
                                    .on_click(cx.listener(move |view, _, _, cx| {
                                        if disabled {
                                            return;
                                        }
                                        view.lane_offset = view
                                            .lane_offset
                                            .saturating_add_signed(direction * 2)
                                            .min(
                                                view.max_lanes.saturating_sub(view.visible_lanes()),
                                            );
                                        cx.notify();
                                    }))
                            }),
                        ))
                    }),
            )
            .child(div().flex_1().min_w_0().child("Message du commit"))
            .when(self.width > 620., |style| {
                style
                    .child(div().w(px(132.)).flex_shrink_0().child("Auteur"))
                    .child(div().w(px(72.)).flex_shrink_0().child("Date"))
            })
            .child(
                div()
                    .w(px(if self.width <= 620. { 64. } else { 72. }))
                    .flex_shrink_0()
                    .child("SHA"),
            )
            .into_any_element()
    }

    fn working_row(&self, cx: &mut Context<Self>) -> AnyElement {
        let palette = self.palette;
        div()
            .id("working-tree-row")
            .role(Role::Button)
            .track_focus(&self.working_focus)
            .tab_index(0)
            .aria_label(format!(
                "Modifications non commitées, {} changements",
                format_count(self.working.staged + self.working.unstaged)
            ))
            .aria_toggled(if self.working.selected {
                Toggled::True
            } else {
                Toggled::False
            })
            .h(px(44.))
            .w_full()
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(px(10.))
            .pl(px(20.))
            .pr(px(16.))
            .bg(rgb(if self.working.selected {
                palette.selection
            } else {
                palette.accent_soft
            }))
            .border_b_1()
            .border_color(rgb(palette.border))
            .when(self.working.selected, |style| {
                style.border_l_2().border_color(rgb(palette.accent))
            })
            .cursor_pointer()
            .hover(|style| style.bg(rgb(palette.hover)))
            .text_size(px(12. + self.font_size))
            .text_color(rgb(palette.text))
            .child(theme::icon(IconName::FilePenLine, 17., palette.accent))
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .font_weight(FontWeight::MEDIUM)
                    .child("Modifications non commitées"),
            )
            .children(self.working.counts.iter().map(|(status, count)| {
                let (icon, color) = match status.as_str() {
                    "added" => (IconName::Plus, palette.success),
                    "deleted" => (IconName::Minus, palette.danger),
                    "renamed" => (IconName::ArrowRight, palette.renamed),
                    _ => (IconName::PenLine, palette.warning),
                };
                div()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .font_family(theme::MONO)
                    .text_size(px(10. + self.font_size))
                    .text_color(rgb(color))
                    .child(theme::icon(icon, 13., color))
                    .child(count.to_string())
            }))
            .child(
                div()
                    .ml_auto()
                    .font_family(theme::MONO)
                    .text_size(px(10. + self.font_size))
                    .text_color(rgb(palette.muted))
                    .child(format_count(self.working.staged + self.working.unstaged)),
            )
            .on_click(cx.listener(|view, _, window, cx| {
                view.cancel_navigation();
                view.working.selected = true;
                view.working_focus.focus(window, cx);
                cx.emit(HistoryEvent::SelectWorkingTree);
                cx.notify();
            }))
            .on_key_down(cx.listener(|view, event: &KeyDownEvent, window, cx| {
                if event.keystroke.key == "down" {
                    view.select_index(0, window, cx);
                    cx.stop_propagation();
                }
            }))
            .into_any_element()
    }

    fn history_rows(
        &mut self,
        range: Range<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        if let Some(size) = self.scroll.0.borrow().last_item_size {
            let width = f32::from(size.item.width);
            if (width - self.width).abs() > 0.5 {
                self.width = width;
                let weak = cx.entity().downgrade();
                window.defer(cx, move |_, cx| {
                    let _ = weak.update(cx, |_, cx| cx.notify());
                });
            }
        }
        self.lane_offset = self
            .lane_offset
            .min(self.max_lanes.saturating_sub(self.visible_lanes()));
        if range.len() > 1 {
            self.pages.set_visible_page(range.start / PAGE_SIZE);
            #[cfg(feature = "app-benchmark")]
            {
                self.visible_range = range.clone();
            }
        }
        let start_page = range.start / PAGE_SIZE;
        let end_page = range.end.saturating_sub(1) / PAGE_SIZE;
        for page in start_page..=end_page {
            self.request_page(page, cx);
        }
        let palette = self.palette;
        let graph_width = self.graph_width();
        let small = self.width <= 620.;
        let hide_refs = self.width <= 480.;
        let filtered = self.filtered();
        range
            .map(|index| {
                let Some(prepared) = self.pages.row_shared(index) else {
                    return div()
                        .id(("commit-loading", index))
                        .role(Role::ListBoxOption)
                        .aria_label("Chargement du commit…")
                        .aria_position_in_set(index + 1)
                        .aria_size_of_set(self.total)
                        .a11y_synthetic_children(|tree| {
                            tree.parent_node().set_busy();
                            tree.parent_node().set_disabled();
                        })
                        .h(px(ROW_HEIGHT))
                        .w_full()
                        .flex()
                        .items_center()
                        .pl(px(graph_width + 12.))
                        .text_size(px(11. + self.font_size))
                        .text_color(rgb(palette.muted))
                        .child("Chargement…")
                        .into_any_element();
                };
                let row = prepared.commit.clone();
                let selected = row.sha == self.selected_sha && !self.working.selected;
                let sha = row.sha.clone();
                let group = self.references.get(&row.sha).cloned();
                let refs = group.as_ref().map(|group| group.references.clone());
                let context_refs = refs.clone();
                let accessible_refs = refs.clone();
                let has_branches = group.as_ref().is_some_and(|group| group.has_branches);
                let displayed_refs = if hide_refs {
                    &[][..]
                } else {
                    group
                        .as_ref()
                        .map_or(&[][..], |group| group.badges.as_slice())
                };
                let subject = prepared.subject.clone();
                let font_size = self.font_size;
                let entity = cx.entity().downgrade();
                let menu_entity = entity.clone();
                let element = div()
                    .id(("commit", index))
                    .when(window.is_a11y_active(), |style| {
                        style.accessibility_id(format!("commit-option-{index}"))
                    })
                    .role(Role::ListBoxOption)
                    .focusable()
                    .tab_index(0)
                    .tab_stop(false)
                    .when(window.is_a11y_active(), |style| {
                        style
                            .aria_label(format!("{}, {}", subject, row.author))
                            .aria_description(format!("{}, {}", prepared.date, prepared.short_sha))
                    })
                    .aria_selected(selected)
                    .aria_position_in_set(index + 1)
                    .aria_size_of_set(self.total)
                    // Accessible focus can land on the row itself. Only report
                    // a descendant while focus belongs to the list container.
                    .when(selected && self.focus.is_focused(window), |style| {
                        style.aria_active_descendant()
                    })
                    .flex()
                    .items_center()
                    .h(px(ROW_HEIGHT))
                    .w_full()
                    .relative()
                    .overflow_hidden()
                    .pr(px(14.))
                    .gap(px(if small { 9. } else { 12. }))
                    .when(selected, |style| style.bg(rgb(palette.selection)))
                    .when(!selected, |style| {
                        style.hover(|style| style.bg(rgb(palette.hover)))
                    })
                    .cursor_pointer()
                    .text_size(px(11. + font_size))
                    .text_color(rgb(palette.muted))
                    .child(div().absolute().left_0().right_0().bottom_0().h(px(1.)).bg(
                        if selected {
                            rgb(palette.accent_border).into()
                        } else {
                            with_alpha(palette.border, 0.3)
                        },
                    ))
                    .when(selected, |style| {
                        style.child(
                            div()
                                .absolute()
                                .left_0()
                                .right_0()
                                .top_0()
                                .h(px(1.))
                                .bg(rgb(palette.accent_border)),
                        )
                    })
                    .child(graph::row(
                        row.clone(),
                        selected,
                        filtered,
                        self.lane_offset,
                        graph_width,
                        palette,
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .gap(px(7.))
                            .overflow_hidden()
                            .children(displayed_refs.iter().enumerate().map(
                                |(ref_index, reference)| {
                                    let tag = reference.kind == "tag";
                                    let color = if tag { palette.warning } else { palette.accent };
                                    let references = reference.clone();
                                    let weak = entity.clone();
                                    let activate_entity = entity.clone();
                                    let context_entity = entity.clone();
                                    let menu_references = reference.clone();
                                    div()
                                        .id(("commit-ref", ref_index))
                                        .role(Role::Button)
                                        .focusable()
                                        .tab_index(0)
                                        .tab_stop(false)
                                        .when(window.is_a11y_active(), |style| {
                                            style.aria_label(format!(
                                                "{} {}",
                                                if tag { "Tag" } else { "Branche" },
                                                reference.label,
                                            ))
                                        })
                                        .on_a11y_action(
                                            AccessibleAction::Click,
                                            move |_, window, cx| {
                                                let _ = activate_entity.update(cx, |view, cx| {
                                                    view.close_menu(window, cx);
                                                    view.select_index(index, window, cx);
                                                });
                                            },
                                        )
                                        .when(!tag, |style| {
                                            style.on_a11y_action(
                                                AccessibleAction::ShowContextMenu,
                                                move |_, window, cx| {
                                                    let _ =
                                                        context_entity.update(cx, |view, cx| {
                                                            view.open_menu(
                                                                std::slice::from_ref(
                                                                    menu_references.as_ref(),
                                                                ),
                                                                view.context_position(index),
                                                                window,
                                                                cx,
                                                            );
                                                        });
                                                },
                                            )
                                        })
                                        .flex()
                                        .items_center()
                                        .gap(px(4.))
                                        .max_w(px(136.))
                                        .min_w_0()
                                        .px(px(5.))
                                        .py(px(3.))
                                        .rounded(RADIUS_SM)
                                        .border_1()
                                        .border_color(if tag {
                                            with_alpha(color, 0.28)
                                        } else {
                                            rgb(palette.accent_border).into()
                                        })
                                        .bg(if tag {
                                            with_alpha(color, 0.08)
                                        } else {
                                            rgb(palette.accent_soft).into()
                                        })
                                        .text_size(px(9. + font_size))
                                        .text_color(rgb(color))
                                        .child(theme::icon(
                                            if tag {
                                                IconName::Tag
                                            } else {
                                                IconName::GitBranch
                                            },
                                            10.,
                                            color,
                                        ))
                                        .child(
                                            div()
                                                .min_w_0()
                                                .truncate()
                                                .child(reference.label.clone()),
                                        )
                                        .on_mouse_down(
                                            MouseButton::Right,
                                            move |event, window, cx| {
                                                if references.kind == "tag" {
                                                    return;
                                                }
                                                let _ = weak.update(cx, |view, cx| {
                                                    view.open_menu(
                                                        std::slice::from_ref(references.as_ref()),
                                                        event.position,
                                                        window,
                                                        cx,
                                                    )
                                                });
                                                cx.stop_propagation();
                                            },
                                        )
                                },
                            ))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(px(12. + font_size))
                                    .text_color(rgb(palette.text))
                                    .child(subject),
                            )
                            .when(row.parents.len() > 1, |style| {
                                style.child(theme::icon(
                                    IconName::GitCommitHorizontal,
                                    13.,
                                    palette.muted,
                                ))
                            }),
                    )
                    .when(!small, |style| {
                        style
                            .child(
                                div()
                                    .w(px(132.))
                                    .flex_shrink_0()
                                    .flex()
                                    .items_center()
                                    .gap(px(6.))
                                    .overflow_hidden()
                                    .child(
                                        div()
                                            .size(px(21.))
                                            .flex_shrink_0()
                                            .rounded_full()
                                            .bg(rgb(palette.surface))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .text_size(px(8. + font_size))
                                            .text_color(rgb(palette.text))
                                            .child(prepared.initials.clone()),
                                    )
                                    .child(
                                        div().min_w_0().truncate().child(prepared.author.clone()),
                                    ),
                            )
                            .child(
                                div()
                                    .w(px(72.))
                                    .flex_shrink_0()
                                    .child(prepared.date.clone()),
                            )
                    })
                    .child(
                        div()
                            .w(px(if small { 64. } else { 72. }))
                            .flex_shrink_0()
                            .text_right()
                            .font_family(theme::MONO)
                            .text_size(px(10. + font_size))
                            .child(prepared.short_sha.clone()),
                    )
                    .when(selected, |style| {
                        style.child(
                            div()
                                .absolute()
                                .left_0()
                                .top_0()
                                .bottom_0()
                                .w(px(2.))
                                .bg(rgb(palette.accent)),
                        )
                    })
                    .on_click(cx.listener(move |view, _, window, cx| {
                        view.close_menu(window, cx);
                        view.focus.focus(window, cx);
                        view.select(sha.clone(), index, false, cx);
                    }))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |view, event: &MouseDownEvent, window, cx| {
                            view.open_menu(
                                context_refs
                                    .as_deref()
                                    .map(Vec::as_slice)
                                    .unwrap_or_default(),
                                event.position,
                                window,
                                cx,
                            );
                            cx.stop_propagation();
                        }),
                    )
                    .when(window.is_a11y_active() && has_branches, |style| {
                        style.on_a11y_action(
                            AccessibleAction::ShowContextMenu,
                            move |_, window, cx| {
                                let _ = menu_entity.update(cx, |view, cx| {
                                    view.open_menu(
                                        accessible_refs
                                            .as_deref()
                                            .map(Vec::as_slice)
                                            .unwrap_or_default(),
                                        view.context_position(index),
                                        window,
                                        cx,
                                    );
                                });
                            },
                        )
                    });
                element.into_any_element()
            })
            .collect()
    }
}

impl Render for HistoryView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(feature = "app-benchmark")]
        crate::perf::record_count("history_render");
        let palette = self.palette;
        if self.search.read(cx).value().as_ref() != self.query {
            let query = self.query.clone();
            self.search
                .update(cx, |input, cx| input.set_value(query, window, cx));
        }
        let mut root = div()
            .id("history-view")
            .size_full()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(rgb(palette.reading_bg))
            .font_family(theme::SANS)
            .text_color(rgb(palette.text))
            .child(
                div()
                    .h(px(71.))
                    .w_full()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px(px(22.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .min_w_0()
                            .gap(px(10.))
                            .child(theme::icon(IconName::GitBranch, 17., palette.accent))
                            .child(
                                div()
                                    .id("history-heading")
                                    .role(Role::Heading)
                                    .aria_label("Historique")
                                    .aria_level(2)
                                    .text_size(px(16. + self.font_size))
                                    .font_weight(FontWeight::MEDIUM)
                                    .child("Historique"),
                            )
                            .child(
                                div()
                                    .id("history-head-reference")
                                    .role(Role::Label)
                                    .when(window.is_a11y_active(), |style| {
                                        style.aria_label(self.head_label.clone())
                                    })
                                    .ml(px(6.))
                                    .max_w(px(180.))
                                    .min_w_0()
                                    .truncate()
                                    .px(px(8.))
                                    .py(px(5.))
                                    .rounded(RADIUS_SM)
                                    .border_1()
                                    .border_color(rgb(palette.border))
                                    .bg(rgb(palette.accent_soft))
                                    .text_size(px(10. + self.font_size))
                                    .text_color(rgb(palette.accent))
                                    .child(self.head_label.clone()),
                            ),
                    )
                    .child(
                        div()
                            .id("refresh-history")
                            .role(Role::Button)
                            .focusable()
                            .tab_index(0)
                            .aria_label("Actualiser le dépôt")
                            .size(px(30.))
                            .flex_shrink_0()
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(RADIUS_SM)
                            .cursor_pointer()
                            .hover(|style| style.bg(rgb(palette.hover)))
                            .child(theme::icon(IconName::RefreshCw, 14., palette.muted))
                            .on_click(
                                cx.listener(|_, _, _, cx| cx.emit(HistoryEvent::RefreshRepository)),
                            ),
                    ),
            );
        if let Some(solo_banner) = &self.solo_banner {
            root = root.child(
                div()
                    .w_full()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(16.))
                    .py(px(8.))
                    .border_b_1()
                    .border_color(with_alpha(palette.warning, 0.3))
                    .bg(with_alpha(palette.warning, 0.1))
                    .text_color(rgb(palette.warning))
                    .text_size(px(12. + self.font_size))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(solo_banner.clone()),
                    )
                    .child(
                        div()
                            .id("clear-solo")
                            .role(Role::Button)
                            .focusable()
                            .tab_index(0)
                            .aria_label("Quitter le mode Solo")
                            .flex_shrink_0()
                            .rounded(RADIUS_MD)
                            .border_1()
                            .border_color(with_alpha(palette.warning, 0.3))
                            .px(px(8.))
                            .py(px(4.))
                            .cursor_pointer()
                            .hover(|style| style.bg(with_alpha(palette.warning, 0.1)))
                            .child("Quitter le mode Solo")
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(HistoryEvent::ClearSolo))),
                    ),
            );
        }
        root =
            root.child(
                div()
                    .id("history-search-toolbar")
                    .role(Role::Toolbar)
                    .aria_label("Recherche dans l’historique")
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap(px(17.))
                    .mx(px(22.))
                    .mb(px(18.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h(px(34.))
                            .flex()
                            .items_center()
                            .gap(px(9.))
                            .px(px(11.))
                            .rounded(RADIUS_MD)
                            .border_1()
                            .border_color(rgb(palette.border))
                            .bg(rgb(palette.bg))
                            .child(theme::icon(IconName::Search, 15., palette.muted))
                            .child(
                                Input::new(&self.search)
                                    .appearance(false)
                                    .bordered(false)
                                    .focus_bordered(false)
                                    .aria_label("Rechercher des commits")
                                    .flex_1()
                                    .min_w_0()
                                    .text_size(px(11. + self.font_size)),
                            )
                            .when(self.query.is_empty(), |style| {
                                style.child(
                                    div()
                                        .flex_shrink_0()
                                        .rounded(RADIUS_SM)
                                        .border_1()
                                        .border_color(rgb(palette.border))
                                        .px(px(5.))
                                        .py(px(2.))
                                        .text_size(px(9. + self.font_size))
                                        .text_color(rgb(palette.muted))
                                        .child(if cfg!(target_os = "macos") {
                                            "⌘ K"
                                        } else {
                                            "Ctrl K"
                                        }),
                                )
                            })
                            .when(!self.query.is_empty(), |style| {
                                style.child(
                                    div()
                                        .id("clear-search")
                                        .role(Role::Button)
                                        .focusable()
                                        .tab_index(0)
                                        .aria_label("Effacer la recherche")
                                        .flex_shrink_0()
                                        .cursor_pointer()
                                        .child(theme::icon(IconName::X, 12., palette.muted))
                                        .on_click(cx.listener(|view, _, window, cx| {
                                            view.set_query(String::new(), cx);
                                            view.focus_search(window, cx);
                                        })),
                                )
                            }),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(4.))
                            .flex_shrink_0()
                            .text_size(px(10. + self.font_size))
                            .child(
                                div()
                                    .font_family(theme::MONO)
                                    .child(format_count(self.total)),
                            )
                            .child(div().text_color(rgb(palette.muted)).child(
                                if self.filtered() {
                                    "résultats"
                                } else {
                                    "commits"
                                },
                            )),
                    ),
            );
        root = root.child(self.headings(cx));
        if self.show_working_row() {
            root = root.child(self.working_row(cx));
        }
        let mut viewport = div()
            .id("history-viewport")
            .role(Role::ListBox)
            .aria_label("Historique des commits")
            .aria_size_of_set(self.total)
            .when(self.loading() || !self.progress.complete, |style| {
                style.a11y_synthetic_children(|tree| {
                    tree.parent_node().set_busy();
                })
            })
            .tab_index(0)
            .flex_1()
            .min_h_0()
            .w_full()
            .relative()
            .overflow_hidden()
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|view, event: &KeyDownEvent, window, cx| {
                match event.keystroke.key.as_str() {
                    "down" => view.navigate(1, window, cx),
                    "up" => view.navigate(-1, window, cx),
                    "home" => view.select_index_keyboard(0, window, cx),
                    "end" => view.select_index_keyboard(view.total.saturating_sub(1), window, cx),
                    "contextmenu" | "f10"
                        if event.keystroke.key == "contextmenu"
                            || event.keystroke.modifiers.shift =>
                    {
                        if let Some(references) = view.references.get(&view.selected_sha).cloned() {
                            view.open_menu(
                                references.references.as_slice(),
                                view.context_position(view.selected_index),
                                window,
                                cx,
                            );
                        }
                    }
                    "escape" if view.menu.is_some() => view.close_menu(window, cx),
                    _ => return,
                }
                cx.stop_propagation();
            }))
            .on_scroll_wheel(cx.listener(|view, _, window, cx| view.close_menu(window, cx)));
        if self.total > 0 {
            viewport = viewport
                .child(
                    uniform_list(
                        "commit-history",
                        self.total,
                        cx.processor(|view, range: Range<usize>, window, cx| {
                            view.history_rows(range, window, cx)
                        }),
                    )
                    .track_scroll(&self.scroll)
                    .size_full(),
                )
                .child(theme::scrollbar_theme(
                    Scrollbar::new(&self.scroll).id("history-scrollbar"),
                ));
        } else if !self.loading() && self.progress.complete {
            viewport = viewport.child(
                div()
                    .absolute()
                    .size_full()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(13.))
                    .text_size(px(12. + self.font_size))
                    .text_color(rgb(palette.muted))
                    .child(theme::icon(
                        if self.filtered() {
                            IconName::SearchX
                        } else {
                            IconName::GitCommitHorizontal
                        },
                        32.,
                        palette.muted,
                    ))
                    .child(
                        div()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(palette.text))
                            .child(if self.filtered() {
                                "Aucun commit trouvé"
                            } else {
                                "Ce dépôt n’a pas encore de commit"
                            }),
                    )
                    .child(if self.filtered() {
                        "Essayez un autre message, auteur ou SHA."
                    } else {
                        "Son historique apparaîtra ici."
                    }),
            );
        }
        let status = if !self.progress.complete {
            format!(
                "Indexation · {} commits",
                format_count(self.progress.indexed)
            )
        } else if self.progress.error.is_some() {
            "Indexation interrompue".to_owned()
        } else {
            "Index local prêt".to_owned()
        };
        root.child(viewport)
            .child(
                div()
                    .h(px(31.))
                    .flex_shrink_0()
                    .w_full()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px(px(20.))
                    .border_t_1()
                    .border_color(rgb(palette.border))
                    .bg(rgb(palette.panel))
                    .text_size(px(9. + self.font_size))
                    .text_color(rgb(palette.muted))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .when(
                                self.progress.complete && self.progress.error.is_none(),
                                |style| style.child(div().size(px(4.)).bg(rgb(palette.success))),
                            )
                            .when(
                                !self.progress.complete || self.progress.error.is_some(),
                                |style| {
                                    style.child(theme::icon(
                                        IconName::CircleDot,
                                        11.,
                                        palette.muted,
                                    ))
                                },
                            )
                            .child(status),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.))
                            .opacity(0.7)
                            .child(theme::icon(IconName::Command, 11., palette.muted))
                            .child(format!(
                                "{} Rechercher · {} Commandes",
                                shortcut_label("K"),
                                shortcut_label("P"),
                            )),
                    ),
            )
            .children(self.menu_element(window, cx))
    }
}

// A lookup may observe publication before the next poll. It can advance the
// run that it started with, but never restore a run superseded while in flight.
fn position_snapshot_can_publish(started: &str, current: &str, snapshot: &str) -> bool {
    started == current || snapshot == current
}

fn normalized_query(query: &str) -> String {
    query.trim().chars().take(256).collect()
}
fn with_alpha(color: u32, alpha: f32) -> Hsla {
    let mut value: Hsla = rgb(color).into();
    value.a = alpha;
    value
}

pub fn short_date(timestamp: i64) -> String {
    use chrono::Datelike;
    let Some(date) = chrono::DateTime::from_timestamp(timestamp, 0) else {
        return "Date inconnue".into();
    };
    let date = date.with_timezone(&chrono::Local);
    let months = [
        "janv.", "févr.", "mars", "avr.", "mai", "juin", "juil.", "août", "sept.", "oct.", "nov.",
        "déc.",
    ];
    format!("{:02} {}", date.day(), months[date.month0() as usize])
}
#[cfg(test)]
mod navigation_tests;

#[cfg(test)]
mod tests {
    use super::{
        BranchMenu, WorkingSummary, format_count, normalized_query, position_snapshot_can_publish,
    };
    use crate::format::{initials, short_sha};
    use gpui_kit::{ScrollStrategy, point, px};
    use mangrove_core::model::Reference;

    fn reference(index: usize, kind: &str) -> Reference {
        Reference {
            name: format!("refs/{kind}/{index}"),
            label: format!("Branch {index}"),
            kind: kind.into(),
            sha: "commit".into(),
        }
    }

    #[test]
    fn position_lookup_cannot_restore_a_traversal_superseded_by_polling() {
        assert!(position_snapshot_can_publish("old", "old", "old"));
        // The lookup can publish a newly completed traversal before polling.
        assert!(position_snapshot_can_publish("old", "old", "new"));
        // An older read must retry once polling has already published the new run.
        assert!(!position_snapshot_can_publish("old", "new", "old"));
        assert!(position_snapshot_can_publish("old", "new", "new"));
    }

    #[test]
    fn long_branch_menu_keyboard_reveals_its_selected_reference() {
        let mut references: Vec<_> = (0..10_000)
            .map(|index| reference(index, if index % 2 == 0 { "local" } else { "remote" }))
            .collect();
        references.insert(0, reference(99_999, "tag"));
        let mut menu = BranchMenu::new(&references, point(px(0.), px(0.))).unwrap();
        assert_eq!(menu.references.len(), 10_000);
        for (key, expected) in [("end", 9_999), ("down", 0), ("up", 9_999), ("home", 0)] {
            assert!(menu.navigate_key(key));
            assert_eq!(menu.selected, expected);
            assert_eq!(
                menu.references[expected].name,
                references[expected + 1].name
            );
            let state = menu.scroll.0.borrow();
            let reveal = state.deferred_scroll_to_item.as_ref().unwrap();
            assert_eq!(reveal.item_index, expected);
            assert_eq!(reveal.strategy, ScrollStrategy::Nearest);
        }
        assert!(!menu.navigate_key("enter"));
        assert!(BranchMenu::new(&[reference(0, "tag")], point(px(0.), px(0.))).is_none());
    }

    #[test]
    fn branch_menu_preserves_small_height_and_bounds_large_menus_to_the_window() {
        let position = point(px(0.), px(0.));
        let small = BranchMenu::new(&[reference(0, "local")], position).unwrap();
        let many: Vec<_> = (0..128).map(|index| reference(index, "remote")).collect();
        let large = BranchMenu::new(&many, position).unwrap();
        for font_size in [0., 1., 5.] {
            assert_eq!(
                small.viewport_height(font_size, px(920.)),
                BranchMenu::row_height(font_size)
            );
            assert_eq!(large.viewport_height(font_size, px(920.)), px(892.));
            assert_eq!(large.viewport_height(font_size, px(640.)), px(612.));
        }
    }

    #[test]
    fn working_tree_row_requires_actual_changes_in_an_available_checkout() {
        for (available, staged, unstaged, visible) in [
            (true, 0, 0, false),
            (true, 1, 0, true),
            (true, 0, 1, true),
            (false, 1, 1, false),
        ] {
            let summary = WorkingSummary {
                available,
                staged,
                unstaged,
                ..Default::default()
            };
            assert_eq!(summary.has_changes(), visible);
        }
    }
    #[test]
    fn unicode_author_and_query_limits_follow_display_characters() {
        assert_eq!(initials("  Élodie   🐚  Martin "), "É🐚");
        assert_eq!(initials("  "), "");
        assert_eq!(normalized_query(&"é".repeat(300)).chars().count(), 256);
        assert_eq!(normalized_query("  a b  "), "a b");
        assert_eq!(short_sha("abc"), "abc");
        assert_eq!(format_count(1_234_567), "1\u{202f}234\u{202f}567");
    }
}
