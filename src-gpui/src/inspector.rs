//! Read-only commit, index and stash inspection through the Git engine.
//! The bounded read lane keeps the native read alive until completion and retains
//! only the latest queued foreground intent. Immutable snapshots publish together.
use crate::{
    code_view::{CodeView, CodeViewEvent},
    file_browser::{
        FileBrowser, FileBrowserEvent, FilePresentation, FileTarget, FileView, PreparedDiff,
        ReadGate, action, is_activate, notice, status_icon,
    },
    format::{commit_date, format_count, initials, short_sha},
    image_preview::{ImagePreview, ImageTarget, supports_path},
    preferences::Preferences,
    syntax,
    theme::{MONO, Palette, RADIUS_MD, RADIUS_SM, icon, scrollbar_theme},
    view_cache::render_cached_view,
};
use accesskit::Role;
use gpui_kit::{assets::IconName, component::scroll::Scrollbar, prelude::*, *};
use mangrove_core::{Workspace, model::*};
use std::{
    collections::{HashMap, VecDeque},
    ops::Range,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

const MAX_CACHE_BYTES: usize = 8 * 1024 * 1024;
const MAX_CACHE_ENTRIES: usize = 256;
const MAX_CACHE_TOKENS: usize = 200_000;
pub enum InspectorEvent {
    ModeChanged(bool),
    NavigateCommit(String),
    WorkingTreeSummary {
        available: bool,
        staged: usize,
        unstaged: usize,
    },
    WorkingTreeCounts(Vec<(String, usize)>),
    DiffFontChanged(f32),
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Selection {
    None,
    Commit(String),
    Working,
    Stash(String),
}
#[derive(Clone, Debug)]
struct FileChoice {
    index: usize,
    key: String,
    path: String,
    old_path: Option<String>,
    status: String,
    area: String,
}
impl FileChoice {
    fn changed(file: &ChangedFile, area: &str) -> Self {
        Self {
            index: file.index,
            key: file.file_key.clone(),
            path: file.path.clone(),
            old_path: file.old_path.clone(),
            status: file.status.clone(),
            area: area.into(),
        }
    }
    fn local(file: &WorkingTreeFile, area: &str) -> Self {
        Self {
            key: file.key.clone(),
            ..Self::changed(&file.file, area)
        }
    }
    fn cache_key(&self) -> String {
        format!("{}:{}:{}", self.area, self.index, self.key)
    }
    fn same_identity(&self, other: &Self) -> bool {
        self.area == other.area && self.index == other.index && self.key == other.key
    }
}

/// The user's newest file intent is independent of the displayed snapshot.
/// During a mutable refresh it is resolved by opaque identity in the fresh
/// status, so an old index/path or an older status cannot select another file.
#[derive(Default)]
struct FileIntent {
    choice: Option<FileChoice>,
}

enum RefreshedIntent {
    Stale,
    Empty,
    Ready(FileChoice),
    Vanished,
}

impl FileIntent {
    fn choice(&self) -> Option<&FileChoice> {
        self.choice.as_ref()
    }

    fn clear(&mut self) {
        self.choice = None;
    }

    /// Return whether a patch can be read now; the intent is retained either way.
    fn select(&mut self, choice: FileChoice, refreshing: bool) -> bool {
        self.choice = Some(choice);
        !refreshing
    }

    /// A prepared preview is not a user choice. Restore only the requested
    /// opaque key; a missing key must not select the fallback preview.
    fn restore_commit(&mut self, preferred_key: Option<&str>, file: Option<&FileChoice>) {
        self.choice = file
            .filter(|file| preferred_key == Some(file.key.as_str()))
            .cloned();
    }

    fn resolve_status(
        &mut self,
        request: Revision,
        current: Revision,
        rows: &HashMap<String, Arc<Vec<FileChoice>>>,
    ) -> RefreshedIntent {
        if request.repository != current.repository || request.status != current.status {
            return RefreshedIntent::Stale;
        }
        let Some(intent) = &self.choice else {
            return RefreshedIntent::Empty;
        };
        if let Some(file) = rows
            .get(&intent.area)
            .and_then(|rows| rows.iter().find(|file| file.key == intent.key))
            .cloned()
        {
            self.choice = Some(file.clone());
            RefreshedIntent::Ready(file)
        } else {
            self.clear();
            RefreshedIntent::Vanished
        }
    }
}

struct DiffSnapshot {
    file: FileChoice,
    prepared: Option<PreparedDiff>,
    error: Option<String>,
}
impl DiffSnapshot {
    fn presentation(&self, revision: u64, target: FileTarget, loading: bool) -> FilePresentation {
        if loading {
            FilePresentation::loading(revision, target, self.prepared.clone())
        } else if let Some(prepared) = &self.prepared {
            FilePresentation::ready(revision, target, prepared.clone())
        } else {
            FilePresentation::failed(revision, target, self.error.clone().unwrap_or_default())
        }
    }
}
struct CachedDiff {
    prepared: PreparedDiff,
    bytes: usize,
    tokens: usize,
}
#[derive(Default)]
struct DiffCache {
    entries: HashMap<String, CachedDiff>,
    order: VecDeque<String>,
    bytes: usize,
    tokens: usize,
    background_full: bool,
}
impl DiffCache {
    fn clear(&mut self) {
        *self = Self::default();
    }
    fn get(&mut self, key: &str) -> Option<PreparedDiff> {
        let prepared = self.entries.get(key)?.prepared.clone();
        self.order.retain(|k| k != key);
        self.order.push_back(key.into());
        Some(prepared)
    }
    fn remember(&mut self, key: String, prepared: PreparedDiff, background: bool) {
        let tokens = prepared.tokens.iter().map(|line| line.len()).sum::<usize>();
        let bytes = prepared
            .diff
            .hunks
            .iter()
            .map(|h| h.header.len() + h.lines.iter().map(|l| l.content.len()).sum::<usize>())
            .sum::<usize>()
            + tokens * std::mem::size_of::<syntax::Token>();
        if bytes > MAX_CACHE_BYTES || tokens > MAX_CACHE_TOKENS {
            if background {
                self.background_full = true;
            }
            return;
        }
        let old = self.entries.get(&key);
        if background
            && (self.entries.len() + usize::from(old.is_none()) > MAX_CACHE_ENTRIES
                || self.bytes - old.map_or(0, |cached| cached.bytes) + bytes > MAX_CACHE_BYTES
                || self.tokens - old.map_or(0, |cached| cached.tokens) + tokens > MAX_CACHE_TOKENS)
        {
            self.background_full = true;
            return;
        }
        if let Some(old) = self.entries.remove(&key) {
            self.bytes -= old.bytes;
            self.tokens -= old.tokens;
        }
        self.order.retain(|k| k != &key);
        self.order.push_back(key.clone());
        self.bytes += bytes;
        self.tokens += tokens;
        self.entries.insert(
            key,
            CachedDiff {
                prepared,
                bytes,
                tokens,
            },
        );
        while self.entries.len() > MAX_CACHE_ENTRIES
            || self.bytes > MAX_CACHE_BYTES
            || self.tokens > MAX_CACHE_TOKENS
        {
            let Some(key) = self.order.pop_front() else {
                break;
            };
            if let Some(old) = self.entries.remove(&key) {
                self.bytes -= old.bytes;
                self.tokens -= old.tokens;
            }
        }
    }
    fn full(&self) -> bool {
        self.background_full
            || self.entries.len() >= MAX_CACHE_ENTRIES
            || self.bytes >= MAX_CACHE_BYTES
            || self.tokens >= MAX_CACHE_TOKENS
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Revision {
    status: u64,
    repository: u64,
    selection: u64,
    file: u64,
}
impl Revision {
    fn selection_matches(self, current: Self) -> bool {
        self.repository == current.repository && self.selection == current.selection
    }
    fn file_matches(self, current: Self) -> bool {
        self.selection_matches(current) && self.file == current.file
    }
    fn mutable_selection_matches(self, current: Self) -> bool {
        self.selection_matches(current) && self.status == current.status
    }
}
enum ReadKind {
    Commit {
        sha: String,
        preferred_key: Option<String>,
    },
    Stash(String),
    Status,
    Diff {
        selection: Selection,
        file: FileChoice,
        background: bool,
    },
}
struct ReadRequest {
    revision: Revision,
    id: String,
    kind: ReadKind,
}
enum ReadResult {
    Commit(Arc<CommitDetails>),
    Stash(Arc<StashDetails>),
    Status(Arc<WorkingTreeStatus>),
    Diff(PreparedDiff),
}

struct ColorRequest {
    revision: Revision,
    selection: Selection,
    file: FileChoice,
    prepared: PreparedDiff,
    background: bool,
}

impl ReadRequest {
    fn current(&self, revision: Revision) -> bool {
        match &self.kind {
            ReadKind::Status => {
                self.revision.repository == revision.repository
                    && self.revision.status == revision.status
            }
            ReadKind::Commit { .. } | ReadKind::Stash(_) => {
                self.revision.selection_matches(revision)
            }
            ReadKind::Diff { selection, .. } => {
                self.revision.file_matches(revision)
                    && (!matches!(selection, Selection::Working)
                        || self.revision.mutable_selection_matches(revision))
            }
        }
    }
}
impl ColorRequest {
    fn current(&self, revision: Revision) -> bool {
        self.revision.file_matches(revision)
            && (!matches!(self.selection, Selection::Working)
                || self.revision.mutable_selection_matches(revision))
    }
}

pub struct InspectorView {
    engine: Arc<Workspace>,
    gate: ReadGate,
    repository: Option<Arc<RepositoryInfo>>,
    selection: Selection,
    revision: Revision,
    details: Option<Arc<CommitDetails>>,
    commit_message: Option<(SharedString, SharedString)>,
    live_revision: Arc<Mutex<Revision>>,
    color_active: bool,
    color_pending: Option<ColorRequest>,
    color_task: Option<Task<()>>,
    selection_debounce: Option<Task<()>>,
    last_keyboard_selection: Option<Instant>,
    stash: Option<Arc<StashDetails>>,
    status: Option<Arc<WorkingTreeStatus>>,
    stash_area: String,
    snapshot: Option<DiffSnapshot>,
    intent: FileIntent,
    cache: DiffCache,
    details_loading: bool,
    diff_loading: bool,
    status_loading: bool,
    error: Option<String>,
    status_error: Option<String>,
    compact: bool,
    workspace_open: bool,
    viewer: Entity<CodeView>,
    image_preview: Entity<ImagePreview>,
    browser: Entity<FileBrowser>,
    viewer_dirty: bool,
    pending: Option<ReadRequest>,
    pending_status: bool,
    active: bool,
    read_task: Option<Task<()>>,
    preload_task: Option<Task<()>>,
    preload_cursor: usize,
    preload_rows: Arc<Vec<FileChoice>>,
    palette: Palette,
    preferences: Preferences,
    scroll: [UniformListScrollHandle; 3],
    message_scroll: ScrollHandle,
    sha_scroll: ScrollHandle,
    parent_scroll: ScrollHandle,
    stash_scroll: ScrollHandle,
    focus: [FocusHandle; 3],
    active_file: [usize; 3],
    subscriptions: Vec<Subscription>,
    restore_open: Option<bool>,
    file_rows: HashMap<String, Arc<Vec<FileChoice>>>,
    #[cfg(feature = "app-benchmark")]
    benchmark_scroll: f32,
}
impl EventEmitter<InspectorEvent> for InspectorView {}

impl InspectorView {
    fn update_live_revision(&mut self) {
        *self
            .live_revision
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = self.revision;
        if self
            .color_pending
            .as_ref()
            .is_some_and(|pending| !pending.current(self.revision))
        {
            self.color_pending = None;
        }
    }
    pub fn new(engine: Arc<Workspace>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let gate = Arc::new(Mutex::new(()));
        let viewer = cx.new(|cx| CodeView::new(window, cx));
        let image_preview = cx.new(|_| ImagePreview::new(engine.clone(), gate.clone()));
        let browser = cx.new(|cx| FileBrowser::new(engine.clone(), gate.clone(), window, cx));
        let code_subscription = cx.subscribe(&viewer, |_, _, event, cx| match event {
            CodeViewEvent::FontSizeChanged(size) => cx.emit(InspectorEvent::DiffFontChanged(*size)),
            CodeViewEvent::NavigateCommit(sha) => {
                cx.emit(InspectorEvent::NavigateCommit(sha.clone()))
            }
            CodeViewEvent::Parent => {}
        });
        let browser_subscription = cx.subscribe(&browser, |view, _, event, cx| match event {
            FileBrowserEvent::Close => view.close_workspace(cx),
            FileBrowserEvent::NavigateCommit(sha) => {
                cx.emit(InspectorEvent::NavigateCommit(sha.clone()))
            }
            FileBrowserEvent::FontSizeChanged(size) => {
                cx.emit(InspectorEvent::DiffFontChanged(*size))
            }
        });
        Self {
            engine,
            gate,
            repository: None,
            selection: Selection::None,
            revision: Revision {
                status: 0,
                repository: 0,
                selection: 0,
                file: 0,
            },
            details: None,
            commit_message: None,
            live_revision: Arc::new(Mutex::new(Revision {
                status: 0,
                repository: 0,
                selection: 0,
                file: 0,
            })),
            color_active: false,
            color_pending: None,
            color_task: None,
            selection_debounce: None,
            last_keyboard_selection: None,
            stash: None,
            status: None,
            stash_area: "worktree".into(),
            snapshot: None,
            intent: FileIntent::default(),
            cache: DiffCache::default(),
            details_loading: false,
            diff_loading: false,
            status_loading: false,
            error: None,
            status_error: None,
            compact: false,
            workspace_open: false,
            viewer,
            image_preview,
            browser,
            viewer_dirty: true,
            pending: None,
            pending_status: false,
            active: false,
            read_task: None,
            preload_task: None,
            preload_cursor: 0,
            preload_rows: Arc::default(),
            palette: Palette::default(),
            preferences: Preferences::default(),
            scroll: std::array::from_fn(|_| UniformListScrollHandle::new()),
            message_scroll: ScrollHandle::new(),
            sha_scroll: ScrollHandle::new(),
            parent_scroll: ScrollHandle::new(),
            stash_scroll: ScrollHandle::new(),
            focus: std::array::from_fn(|_| cx.focus_handle()),
            active_file: [0; 3],
            subscriptions: vec![code_subscription, browser_subscription],
            restore_open: None,
            file_rows: HashMap::new(),
            #[cfg(feature = "app-benchmark")]
            benchmark_scroll: 0.,
        }
    }
    pub fn clear_repository(&mut self, cx: &mut Context<Self>) {
        self.revision.repository += 1;
        self.revision.selection += 1;
        self.revision.file += 1;
        self.update_live_revision();
        self.clear_image_preview(cx);
        self.commit_message = None;
        self.selection_debounce = None;
        self.last_keyboard_selection = None;
        self.repository = None;
        self.selection = Selection::None;
        self.details = None;
        self.stash = None;
        self.status = None;
        self.pending = None;
        self.pending_status = false;
        self.cache.clear();
        self.preload_task = None;
        self.preload_cursor = 0;
        self.snapshot = None;
        self.intent.clear();
        self.restore_open = None;
        self.file_rows.clear();
        self.preload_rows = Arc::default();
        self.details_loading = false;
        self.diff_loading = false;
        self.status_loading = false;
        self.error = None;
        self.status_error = None;
        self.viewer_dirty = true;
        let presentation = FilePresentation::empty(self.revision.file);
        self.browser
            .update(cx, |view, cx| view.present(presentation, cx));
        self.close_workspace(cx);
        cx.notify();
    }
    pub fn set_repository(&mut self, repository: Arc<RepositoryInfo>, cx: &mut Context<Self>) {
        if self
            .repository
            .as_ref()
            .is_some_and(|old| old.id == repository.id)
        {
            self.repository = Some(repository);
            return;
        }
        let same_path = self
            .repository
            .as_ref()
            .is_some_and(|old| old.path == repository.path);
        self.revision.repository += 1;
        self.revision.selection += 1;
        self.revision.file += 1;
        self.update_live_revision();
        self.clear_image_preview(cx);
        self.repository = Some(repository);
        self.selection_debounce = None;
        self.last_keyboard_selection = None;
        self.pending = None;
        self.pending_status = true;
        self.cache.clear();
        self.preload_task = None;
        self.preload_cursor = 0;
        self.status = None;
        self.status_error = None;
        self.status_loading = true;
        self.details_loading = false;
        self.diff_loading = false;
        if !same_path {
            self.details = None;
            self.stash = None;
            self.intent.clear();
            self.snapshot = None;
            self.selection = Selection::None;
            self.close_workspace(cx);
        } else {
            self.restore_open = Some(self.workspace_open);
            if !matches!(self.selection, Selection::Commit(_))
                && self.intent.choice().is_none()
                && let Some(snapshot) = &self.snapshot
            {
                self.intent.select(snapshot.file.clone(), false);
            }
            let id = self.repository.as_ref().unwrap().id.clone();
            self.pending = match &self.selection {
                Selection::Commit(sha) if !sha.is_empty() => {
                    self.details_loading = true;
                    Some(ReadRequest {
                        revision: self.revision,
                        id,
                        kind: ReadKind::Commit {
                            sha: sha.clone(),
                            preferred_key: self.intent.choice().map(|file| file.key.clone()),
                        },
                    })
                }
                Selection::Stash(sha) => {
                    self.details_loading = true;
                    Some(ReadRequest {
                        revision: self.revision,
                        id,
                        kind: ReadKind::Stash(sha.clone()),
                    })
                }
                _ => None,
            };
        }
        self.error = None;
        self.viewer_dirty = true;
        let presentation = FilePresentation::empty(self.revision.file);
        self.browser
            .update(cx, |view, cx| view.present(presentation, cx));
        self.pump(cx);
        cx.notify();
    }
    pub fn set_theme(&mut self, palette: Palette, cx: &mut Context<Self>) {
        if self.palette == palette {
            return;
        }
        self.palette = palette;
        self.viewer.update(cx, |v, cx| v.set_theme(palette, cx));
        self.browser.update(cx, |v, cx| v.set_theme(palette, cx));
        cx.notify();
    }
    pub fn set_preferences(&mut self, preferences: Preferences, cx: &mut Context<Self>) {
        if self.preferences.font_size_offset == preferences.font_size_offset
            && self.preferences.diff_font_size == preferences.diff_font_size
            && self.preferences.show_image_previews == preferences.show_image_previews
            && self.preferences.theme == preferences.theme
        {
            self.preferences = preferences;
            return;
        }
        let old = 32. + self.preferences.font_size_offset * 2.;
        let new = 32. + preferences.font_size_offset * 2.;
        if old != new {
            for scroll in &self.scroll {
                let handle = scroll.0.borrow().base_handle.clone();
                let offset = handle.offset();
                handle.set_offset(point(offset.x, offset.y * new / old));
            }
        }
        self.preferences = preferences.clone();
        if !preferences.show_image_previews {
            self.clear_image_preview(cx);
        }
        self.viewer_dirty = true;
        self.viewer
            .update(cx, |v, cx| v.set_preferences(preferences.clone(), cx));
        self.browser
            .update(cx, |v, cx| v.set_preferences(preferences, cx));
        cx.notify();
    }
    pub fn central_active(&self) -> bool {
        !self.compact && self.workspace_open
    }
    pub fn compact(&self) -> bool {
        self.compact
    }
    pub fn command_revision(&self) -> [u64; 2] {
        [self.revision.selection, self.revision.file]
    }
    pub fn command_file(&self) -> Option<(&str, bool)> {
        let choice = self.intent.choice()?;
        if self.details_loading || self.diff_loading || self.status_loading {
            return None;
        }
        self.snapshot
            .as_ref()
            .filter(|snapshot| snapshot.file.same_identity(choice))?;
        Some((&choice.path, matches!(self.selection, Selection::Commit(_))))
    }
    pub fn command_parents(&self) -> Vec<String> {
        self.details
            .as_ref()
            .filter(|details| self.selection_sha() == Some(details.sha.as_str()))
            .map(|details| details.parents.clone())
            .unwrap_or_default()
    }
    pub fn show_file_view(&mut self, mode: FileView, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, commit)) = self.command_file() else {
            return;
        };
        if !commit && mode != FileView::Diff {
            return;
        }
        self.set_compact(false, cx);
        if commit {
            self.browser.update(cx, |view, cx| {
                view.show(mode, cx);
                view.focus(window, cx);
            });
        } else {
            self.viewer.read(cx).focus_handle(cx).focus(window, cx);
        }
    }
    pub fn selection_sha(&self) -> Option<&str> {
        if let Selection::Commit(sha) = &self.selection {
            Some(sha)
        } else {
            None
        }
    }
    pub fn selected_stash(&self) -> Option<&str> {
        if let Selection::Stash(sha) = &self.selection {
            Some(sha)
        } else {
            None
        }
    }
    pub fn is_working_tree(&self) -> bool {
        matches!(self.selection, Selection::Working)
    }
    #[cfg(feature = "app-benchmark")]
    pub fn details(&self) -> Option<&Arc<CommitDetails>> {
        self.details.as_ref()
    }
    #[cfg(feature = "app-benchmark")]
    pub fn is_coloring(&self) -> bool {
        self.snapshot
            .as_ref()
            .and_then(|snapshot| snapshot.prepared.as_ref())
            .is_some_and(|prepared| !prepared.syntax_ready)
    }
    #[cfg(feature = "app-benchmark")]
    pub fn is_loading(&self) -> bool {
        self.details_loading || self.diff_loading
    }
    #[cfg(feature = "app-benchmark")]
    pub fn details_ready(&self) -> bool {
        !self.details_loading && self.details.is_some()
    }
    pub fn diff(&self) -> Option<&Arc<FileDiff>> {
        self.snapshot
            .as_ref()
            .and_then(|s| s.prepared.as_ref())
            .map(|prepared| &prepared.diff)
    }
    #[cfg(feature = "app-benchmark")]
    pub fn selected_file_path(&self) -> Option<&str> {
        self.snapshot.as_ref().map(|s| s.file.path.as_str())
    }
    #[cfg(feature = "app-benchmark")]
    pub fn benchmark_readiness(&self, cx: &App) -> (bool, bool, usize) {
        if self.is_loading() {
            return (false, false, 0);
        }
        if !self.compact && matches!(self.selection, Selection::Commit(_)) {
            return self.browser.read(cx).benchmark_readiness(cx);
        }
        let viewer = self.viewer.read(cx);
        (
            !self.viewer_dirty && viewer.is_ready(),
            viewer.syntax_colored(),
            self.diff()
                .map(|diff| diff.hunks.iter().map(|h| h.lines.len()).sum())
                .unwrap_or(0),
        )
    }
    #[cfg(feature = "app-benchmark")]
    pub fn benchmark_scroll_to(&mut self, offset: f32, cx: &mut Context<Self>) {
        if !self.compact && matches!(self.selection, Selection::Commit(_)) {
            self.browser
                .update(cx, |view, cx| view.benchmark_scroll_to(offset, cx));
        } else {
            let delta = offset - self.benchmark_scroll;
            self.benchmark_scroll = offset;
            self.viewer
                .update(cx, |view, cx| view.scroll_by_pixels(delta, cx));
        }
        cx.notify();
    }
    #[cfg(feature = "app-benchmark")]
    pub fn benchmark_working_files(&self) -> (bool, usize, usize, f32) {
        let ready = self.is_working_tree()
            && !self.status_loading
            && self.status_error.is_none()
            && self.status.as_ref().is_some_and(|status| status.available);
        (
            ready,
            self.status
                .as_ref()
                .map_or(0, |status| status.unstaged_count),
            self.file_rows.get("unstaged").map_or(0, |rows| rows.len()),
            -f32::from(self.scroll[0].0.borrow().base_handle.offset().y),
        )
    }
    #[cfg(feature = "app-benchmark")]
    pub fn benchmark_open_working_file(&mut self, path: &str, cx: &mut Context<Self>) -> bool {
        let file = self
            .file_rows
            .get("unstaged")
            .and_then(|rows| rows.iter().find(|file| file.path == path))
            .cloned();
        if let Some(file) = file {
            self.choose(file, cx);
            true
        } else {
            false
        }
    }
    #[cfg(feature = "app-benchmark")]
    pub fn benchmark_scroll_files_to(&mut self, offset: f32, cx: &mut Context<Self>) {
        let scroll = self.scroll[0].0.borrow().base_handle.clone();
        scroll.set_offset(point(scroll.offset().x, px(-offset)));
        cx.notify();
    }
    pub fn refresh_status(&mut self, cx: &mut Context<Self>) {
        if self.repository.is_none() {
            return;
        }
        self.revision.status += 1;
        self.update_live_revision();
        self.pending_status = true;
        self.status_loading = true;
        self.status_error = None;
        if matches!(self.selection, Selection::Working) {
            self.revision.file += 1;
            self.update_live_revision();
            self.clear_image_preview(cx);
            self.cache.clear();
            self.preload_cursor = 0;
            self.preload_rows = Arc::default();
            self.preload_task = None;
            self.diff_loading = false;
            self.pending = None;
            self.viewer_dirty = true;
            self.restore_open = Some(self.workspace_open);
        }
        self.pump(cx);
        cx.notify();
    }
    fn prepare_selection(&mut self, selection: Selection, cx: &mut Context<Self>) -> bool {
        // Repeated keys only replace the pending commit intent. The retained
        // panel is already disabled until details arrive, so keep its scroll
        // handles and cached row callbacks together while it is unchanged.
        let unchanged_panel = matches!(self.selection, Selection::Commit(_))
            && matches!(selection, Selection::Commit(_))
            && self.details_loading
            && !self.workspace_open
            && !self.compact
            && self.error.is_none();
        self.revision.selection += 1;
        self.revision.file += 1;
        self.update_live_revision();
        self.clear_image_preview(cx);
        let keep_commit = matches!(self.selection, Selection::Commit(_))
            && matches!(selection, Selection::Commit(_));
        self.selection = selection;
        self.details_loading = true;
        self.diff_loading = false;
        if !keep_commit {
            self.details = None;
            self.snapshot = None;
            self.file_rows.clear();
        }
        self.stash = None;
        self.intent.clear();
        self.restore_open = None;
        self.preload_rows = Arc::default();
        self.error = None;
        self.cache.clear();
        self.preload_task = None;
        self.preload_cursor = 0;
        self.selection_debounce = None;
        self.pending = None;
        if !unchanged_panel {
            self.scroll = std::array::from_fn(|_| UniformListScrollHandle::new());
            self.active_file = [0; 3];
        }
        self.viewer_dirty = true;
        let presentation = if keep_commit
            && let (Some(details), Some(snapshot), Some(repository)) =
                (&self.details, &self.snapshot, &self.repository)
        {
            let file = &snapshot.file;
            let target = FileTarget {
                session_id: repository.id.clone(),
                sha: details.sha.clone(),
                file_key: file.key.clone(),
                path: file.path.clone(),
                old_path: file.old_path.clone(),
                status: file.status.clone(),
            };
            snapshot.presentation(self.revision.file, target, true)
        } else {
            FilePresentation::empty(self.revision.file)
        };
        self.browser
            .update(cx, |view, cx| view.present(presentation, cx));
        if !unchanged_panel {
            self.close_workspace(cx);
        }
        !unchanged_panel
    }
    pub fn select_commit(&mut self, sha: String, cx: &mut Context<Self>) {
        self.last_keyboard_selection = None;
        self.select_commit_after(sha, None, cx);
    }
    pub fn select_commit_keyboard(&mut self, sha: String, cx: &mut Context<Self>) {
        let now = Instant::now();
        let delay = keyboard_selection_delay(
            self.last_keyboard_selection
                .map(|previous| now.duration_since(previous)),
        );
        self.last_keyboard_selection = Some(now);
        self.select_commit_after(sha, delay, cx);
    }
    fn select_commit_after(
        &mut self,
        sha: String,
        delay: Option<Duration>,
        cx: &mut Context<Self>,
    ) {
        let Some(id) = self.repository.as_ref().map(|r| r.id.clone()) else {
            return;
        };
        let panel_changed = self.prepare_selection(Selection::Commit(sha.clone()), cx);
        let request = ReadRequest {
            revision: self.revision,
            id,
            kind: ReadKind::Commit {
                sha,
                preferred_key: None,
            },
        };
        if let Some(delay) = delay {
            self.selection_debounce = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(delay).await;
                let _ = this.update(cx, |view, cx| {
                    if request.current(view.revision) {
                        view.selection_debounce = None;
                        view.pending = Some(request);
                        view.pump(cx);
                    }
                });
            }));
        } else {
            self.pending = Some(request);
            self.pump(cx);
        }
        if panel_changed {
            cx.notify();
        }
    }
    pub fn select_stash(&mut self, sha: String, cx: &mut Context<Self>) {
        let Some(id) = self.repository.as_ref().map(|r| r.id.clone()) else {
            return;
        };
        self.prepare_selection(Selection::Stash(sha.clone()), cx);
        self.pending = Some(ReadRequest {
            revision: self.revision,
            id,
            kind: ReadKind::Stash(sha),
        });
        self.pump(cx);
        cx.notify();
    }
    pub fn select_working_tree(&mut self, cx: &mut Context<Self>) {
        if self.repository.is_none() {
            return;
        }
        self.prepare_selection(Selection::Working, cx);
        self.revision.status += 1;
        self.update_live_revision();
        self.rebuild_rows();
        self.details_loading = false;
        self.pending_status = true;
        self.status_loading = true;
        self.pump(cx);
        cx.notify();
    }
    /// Leave a working-tree selection that no longer has any selectable changes.
    /// Keep its repository/status context without issuing a commit read for "".
    pub fn clear_working_tree_selection(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.selection, Selection::Working) {
            return;
        }
        self.clear_selection(cx);
    }

    /// A removed stash must release its cached patch before restoring history.
    pub fn clear_stash_selection(&mut self, cx: &mut Context<Self>) -> bool {
        if !matches!(self.selection, Selection::Stash(_)) {
            return false;
        }
        self.clear_selection(cx);
        true
    }

    fn clear_selection(&mut self, cx: &mut Context<Self>) {
        self.prepare_selection(Selection::None, cx);
        self.details_loading = false;
        self.rebuild_rows();
        self.viewer.update(cx, |view, cx| view.clear(cx));
        self.viewer_dirty = false;
        cx.notify();
    }
    pub fn close_workspace(&mut self, cx: &mut Context<Self>) {
        let was_active = self.central_active();
        self.workspace_open = false;
        self.restore_open = None;
        self.preload_task = None;
        // Selecting search results also closes the workspace. Only a real
        // view switch should return focus to history; otherwise keep typing.
        if was_active {
            cx.emit(InspectorEvent::ModeChanged(false));
        }
        cx.notify();
    }
    pub fn set_compact(&mut self, compact: bool, cx: &mut Context<Self>) {
        let was_active = self.central_active();
        self.restore_open = None;
        self.compact = compact;
        if !compact && self.snapshot.is_some() {
            self.workspace_open = true;
        }
        self.viewer_dirty = true;
        if was_active != self.central_active() {
            cx.emit(InspectorEvent::ModeChanged(self.central_active()));
        }
        cx.notify();
    }
    fn rows(&self, area: &str) -> Arc<Vec<FileChoice>> {
        self.file_rows.get(area).cloned().unwrap_or_default()
    }
    fn rebuild_rows(&mut self) {
        self.file_rows.clear();
        match &self.selection {
            Selection::None => {}
            Selection::Commit(_) => {
                if let Some(details) = &self.details {
                    self.file_rows.insert(
                        String::new(),
                        Arc::new(
                            details
                                .files
                                .iter()
                                .map(|f| FileChoice::changed(f, ""))
                                .collect(),
                        ),
                    );
                }
            }
            Selection::Working => {
                if let Some(status) = &self.status {
                    for (area, files) in
                        [("staged", &status.staged), ("unstaged", &status.unstaged)]
                    {
                        self.file_rows.insert(
                            area.into(),
                            Arc::new(files.iter().map(|f| FileChoice::local(f, area)).collect()),
                        );
                    }
                }
            }
            Selection::Stash(_) => {
                if let Some(stash) = &self.stash {
                    for group in &stash.groups {
                        self.file_rows.insert(
                            group.area.clone(),
                            Arc::new(
                                group
                                    .files
                                    .iter()
                                    .map(|f| FileChoice::changed(f, &group.area))
                                    .collect(),
                            ),
                        );
                    }
                }
            }
        }
        self.preload_rows = match &self.selection {
            Selection::Commit(_) if self.details.as_ref().is_some_and(|d| d.file_count <= 1000) => {
                self.rows("")
            }
            Selection::Working => Arc::new(
                self.rows("unstaged")
                    .iter()
                    .chain(self.rows("staged").iter())
                    .cloned()
                    .collect(),
            ),
            _ => Arc::default(),
        };
    }
    #[cfg(feature = "app-benchmark")]
    pub fn select_file(&mut self, index: usize, cx: &mut Context<Self>) {
        let area = if matches!(self.selection, Selection::Stash(_)) {
            self.stash_area.as_str()
        } else {
            ""
        };
        let Some(file) = self.rows(area).iter().find(|f| f.index == index).cloned() else {
            return;
        };
        self.choose(file, cx);
    }
    fn choose(&mut self, file: FileChoice, cx: &mut Context<Self>) {
        if self.details_loading {
            return;
        }
        let Some(id) = self.repository.as_ref().map(|r| r.id.clone()) else {
            return;
        };
        self.revision.file += 1;
        self.update_live_revision();
        self.clear_image_preview(cx);
        let list = match &self.selection {
            Selection::Working if file.area == "staged" => 1,
            Selection::Stash(_) => 2,
            _ => 0,
        };
        if let Some(position) = self
            .rows(&file.area)
            .iter()
            .position(|row| row.same_identity(&file))
        {
            self.active_file[list] = position;
            self.scroll[list].scroll_to_item(position, ScrollStrategy::Nearest);
        }
        let can_read = self.intent.select(
            file.clone(),
            self.status_loading && matches!(self.selection, Selection::Working),
        );
        #[cfg(feature = "app-benchmark")]
        {
            self.benchmark_scroll = 0.;
        }
        self.diff_loading = true;
        self.error = None;
        self.viewer_dirty = true;
        self.workspace_open = true;
        self.preload_task = None;
        if let Selection::Commit(sha) = &self.selection {
            let target = FileTarget {
                session_id: id.clone(),
                sha: sha.clone(),
                file_key: file.key.clone(),
                path: file.path.clone(),
                old_path: file.old_path.clone(),
                status: file.status.clone(),
            };
            let presentation = FilePresentation::loading(self.revision.file, target, None);
            self.browser
                .update(cx, |view, cx| view.present(presentation, cx));
        }
        if !self.compact {
            cx.emit(InspectorEvent::ModeChanged(true));
        }
        if !can_read {
            // The latest key wins even if the status is still being read. Keep
            // the coherent display, then resolve the key in the fresh rows.
            self.pending = None;
            self.restore_open = Some(self.workspace_open);
            cx.notify();
            return;
        }
        if let Some(diff) = self.cache.get(&file.cache_key()) {
            self.color(file.clone(), diff.clone(), false, cx);
            self.publish_diff(file, Some(diff), None, cx);
        } else {
            self.pending = Some(ReadRequest {
                revision: self.revision,
                id,
                kind: ReadKind::Diff {
                    selection: self.selection.clone(),
                    file,
                    background: false,
                },
            });
            self.pump(cx);
        }
        cx.notify();
    }
    fn publish_diff(
        &mut self,
        file: FileChoice,
        prepared: Option<PreparedDiff>,
        error: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let snapshot = DiffSnapshot {
            file,
            prepared,
            error,
        };
        if let Selection::Commit(sha) = &self.selection
            && let Some(repository) = &self.repository
        {
            let target = FileTarget {
                session_id: repository.id.clone(),
                sha: sha.clone(),
                file_key: snapshot.file.key.clone(),
                path: snapshot.file.path.clone(),
                old_path: snapshot.file.old_path.clone(),
                status: snapshot.file.status.clone(),
            };
            let presentation = snapshot.presentation(self.revision.file, target, false);
            self.browser
                .update(cx, |view, cx| view.present(presentation, cx));
        }
        self.snapshot = Some(snapshot);
        self.diff_loading = false;
        self.viewer_dirty = true;
        if let Some(open) = self.restore_open.take() {
            let was_active = self.central_active();
            self.workspace_open = open;
            if was_active != self.central_active() {
                cx.emit(InspectorEvent::ModeChanged(self.central_active()));
            }
        }
        self.schedule_preload(cx);
        cx.notify();
    }
    fn change_stash_area(&mut self, area: String, cx: &mut Context<Self>) {
        if self.stash_area == area {
            return;
        }
        self.stash_area = area;
        self.preload_rows = Arc::default();
        self.preload_cursor = 0;
        self.preload_task = None;
        self.revision.file += 1;
        self.update_live_revision();
        self.clear_image_preview(cx);
        self.intent.clear();
        self.snapshot = None;
        self.diff_loading = false;
        self.viewer_dirty = true;
        self.close_workspace(cx);
        cx.notify();
    }
    fn pump(&mut self, cx: &mut Context<Self>) {
        if self.active || self.selection_debounce.is_some() {
            return;
        }
        let request = if let Some(pending) = self.pending.take() {
            Some(pending)
        } else if self.pending_status {
            self.pending_status = false;
            self.repository.as_ref().map(|r| ReadRequest {
                revision: self.revision,
                id: r.id.clone(),
                kind: ReadKind::Status,
            })
        } else {
            None
        };
        let Some(request) = request else {
            self.schedule_preload(cx);
            return;
        };
        self.active = true;
        let engine = self.engine.clone();
        let gate = self.gate.clone();
        let live_revision = self.live_revision.clone();
        self.read_task = Some(cx.spawn(async move |this, cx| {
            let (request, result) = cx
                .background_executor()
                .spawn(async move {
                    let current = || *live_revision.lock().unwrap_or_else(|p| p.into_inner());
                    if !request.current(current()) {
                        return (request, None);
                    }
                    #[cfg(feature = "app-benchmark")]
                    let gate_start = Instant::now();
                    let result = {
                        let _guard = gate.lock().unwrap_or_else(|p| p.into_inner());
                        #[cfg(feature = "app-benchmark")]
                        crate::perf::record_operation("gate_wait", gate_start.elapsed());
                        if !request.current(current()) {
                            return (request, None);
                        }
                        #[cfg(feature = "app-benchmark")]
                        let read_start = Instant::now();
                        #[cfg(feature = "app-benchmark")]
                        crate::perf::record_count("reads_started");
                        let result = match &request.kind {
                            ReadKind::Commit { sha, .. } => engine
                                .commit_details_shared(&request.id, sha)
                                .map(ReadResult::Commit),
                            ReadKind::Stash(sha) => engine
                                .stash_details(&request.id, sha)
                                .map(|v| ReadResult::Stash(Arc::new(v))),
                            ReadKind::Status => engine
                                .working_tree_status(&request.id)
                                .map(|v| ReadResult::Status(Arc::new(v))),
                            ReadKind::Diff {
                                selection, file, ..
                            } => match selection {
                                Selection::None => Err("Aucune sélection active.".into()),
                                Selection::Commit(sha) => {
                                    engine.file_diff(&request.id, sha, file.index)
                                }
                                Selection::Working => {
                                    engine.working_tree_diff(&request.id, &file.area, &file.key)
                                }
                                Selection::Stash(sha) => {
                                    engine.stash_diff(&request.id, sha, &file.area, &file.key)
                                }
                            }
                            .map(|value| ReadResult::Diff(PreparedDiff::raw(Arc::new(value)))),
                        };
                        #[cfg(feature = "app-benchmark")]
                        {
                            let stage = match &request.kind {
                                ReadKind::Commit { .. } => "details_git",
                                ReadKind::Diff { .. } => "patch_git",
                                _ => "status_git",
                            };
                            crate::perf::record_operation(stage, read_start.elapsed());
                        }
                        result
                    };
                    let result = request.current(current()).then_some(result);
                    (request, result)
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.active = false;
                if let Some(result) = result {
                    view.accept(request, result, cx);
                } else {
                    #[cfg(feature = "app-benchmark")]
                    crate::perf::record_count("stale_reads");
                }
                view.pump(cx);
            });
        }));
    }

    fn color(
        &mut self,
        file: FileChoice,
        prepared: PreparedDiff,
        background: bool,
        cx: &mut Context<Self>,
    ) {
        if prepared.syntax_ready {
            return;
        }
        self.color_pending = Some(ColorRequest {
            revision: self.revision,
            selection: self.selection.clone(),
            file,
            prepared,
            background,
        });
        self.pump_color(cx);
    }

    fn pump_color(&mut self, cx: &mut Context<Self>) {
        if self.color_active {
            return;
        }
        let Some(mut request) = self.color_pending.take() else {
            return;
        };
        self.color_active = true;
        let live_revision = self.live_revision.clone();
        self.color_task = Some(cx.spawn(async move |this, cx| {
            let (request, colored) = cx
                .background_executor()
                .spawn(async move {
                    #[cfg(feature = "app-benchmark")]
                    let syntax_start = Instant::now();
                    let tokens = syntax::cached_diff_tokens_cancellable(
                        &request.file.path,
                        &request.prepared.diff,
                        || {
                            !request
                                .current(*live_revision.lock().unwrap_or_else(|p| p.into_inner()))
                        },
                    );
                    #[cfg(feature = "app-benchmark")]
                    crate::perf::record_operation("syntax", syntax_start.elapsed());
                    let colored = if let Some(tokens) = tokens {
                        request.prepared.tokens = tokens;
                        request.prepared.syntax_ready = true;
                        true
                    } else {
                        false
                    };
                    (request, colored)
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.color_active = false;
                if colored && request.current(view.revision) {
                    view.cache.remember(
                        request.file.cache_key(),
                        request.prepared.clone(),
                        request.background,
                    );
                    if view
                        .snapshot
                        .as_ref()
                        .is_some_and(|snapshot| snapshot.file.same_identity(&request.file))
                    {
                        view.publish_diff(request.file, Some(request.prepared), None, cx);
                    }
                } else {
                    #[cfg(feature = "app-benchmark")]
                    crate::perf::record_count("stale_colors");
                }
                view.pump_color(cx);
                view.schedule_preload(cx);
            });
        }));
    }

    fn accept(
        &mut self,
        request: ReadRequest,
        result: Result<ReadResult, String>,
        cx: &mut Context<Self>,
    ) {
        if !request.current(self.revision) {
            #[cfg(feature = "app-benchmark")]
            crate::perf::record_count("stale_reads");
            return;
        }
        let mut changed = true;
        match request.kind {
            ReadKind::Status if request.revision.status == self.revision.status => {
                self.status_loading = false;
                match result {
                    Ok(ReadResult::Status(status)) => {
                        cx.emit(InspectorEvent::WorkingTreeSummary {
                            available: status.available,
                            staged: status.staged_count,
                            unstaged: status.unstaged_count,
                        });
                        let counts = if status.staged.len() == status.staged_count
                            && status.unstaged.len() == status.unstaged_count
                        {
                            [
                                "modified",
                                "added",
                                "deleted",
                                "renamed",
                                "copied",
                                "typechange",
                                "unknown",
                                "unmodified",
                            ]
                            .into_iter()
                            .filter_map(|kind| {
                                let count = status
                                    .staged
                                    .iter()
                                    .chain(&status.unstaged)
                                    .filter(|file| file.file.status == kind)
                                    .count();
                                (count > 0).then(|| (kind.to_owned(), count))
                            })
                            .collect()
                        } else {
                            Vec::new()
                        };
                        cx.emit(InspectorEvent::WorkingTreeCounts(counts));
                        self.status = Some(status);
                        self.status_error = None;
                        self.rebuild_rows();
                        if matches!(self.selection, Selection::Working) {
                            self.cache.clear();
                            match self.intent.resolve_status(
                                request.revision,
                                self.revision,
                                &self.file_rows,
                            ) {
                                RefreshedIntent::Ready(file) => self.choose(file, cx),
                                RefreshedIntent::Vanished => {
                                    self.snapshot = None;
                                    self.restore_open = None;
                                    self.diff_loading = false;
                                    self.pending = None;
                                    self.viewer_dirty = true;
                                    self.close_workspace(cx);
                                }
                                RefreshedIntent::Stale | RefreshedIntent::Empty => {}
                            }
                        }
                    }
                    Err(e) => {
                        self.status_error = Some(format!(
                            "Impossible de lire les modifications locales : {e}"
                        ));
                        if matches!(self.selection, Selection::Working) {
                            self.diff_loading = false;
                            self.viewer_dirty = true;
                        }
                    }
                    _ => {}
                }
            }
            ReadKind::Commit { .. } | ReadKind::Stash(_)
                if request.revision.selection_matches(self.revision) =>
            {
                self.details_loading = false;
                match result {
                    Ok(ReadResult::Commit(details)) => {
                        // The retained panel's list belongs to the old commit.
                        // Reset once for the new files, then reveal the chosen
                        // preview below after rebuilding rows.
                        self.scroll = std::array::from_fn(|_| UniformListScrollHandle::new());
                        self.active_file = [0; 3];
                        let preferred_key = match &request.kind {
                            ReadKind::Commit { preferred_key, .. } => preferred_key.as_deref(),
                            _ => None,
                        };
                        let file = preview_choice(&details, preferred_key);
                        self.intent.restore_commit(preferred_key, file.as_ref());
                        let mut message = details.message.splitn(2, '\n');
                        self.commit_message = Some((
                            message
                                .next()
                                .filter(|title| !title.is_empty())
                                .unwrap_or("Sans message")
                                .to_owned()
                                .into(),
                            message.next().unwrap_or("").trim().to_owned().into(),
                        ));
                        self.details = Some(details);
                        self.snapshot = None;
                        self.viewer_dirty = true;
                        if let Some(file) = file {
                            self.diff_loading = true;
                            if let (Some(repository), Selection::Commit(sha)) =
                                (&self.repository, &self.selection)
                            {
                                let target = FileTarget {
                                    session_id: repository.id.clone(),
                                    sha: sha.clone(),
                                    file_key: file.key.clone(),
                                    path: file.path.clone(),
                                    old_path: file.old_path.clone(),
                                    status: file.status.clone(),
                                };
                                let presentation =
                                    FilePresentation::loading(self.revision.file, target, None);
                                self.browser
                                    .update(cx, |view, cx| view.present(presentation, cx));
                            }
                            self.pending = Some(ReadRequest {
                                revision: self.revision,
                                id: request.id,
                                kind: ReadKind::Diff {
                                    selection: self.selection.clone(),
                                    file,
                                    background: false,
                                },
                            });
                        } else {
                            self.diff_loading = false;
                            let presentation = FilePresentation::empty(self.revision.file);
                            self.browser
                                .update(cx, |view, cx| view.present(presentation, cx));
                            if let Some(open) = self.restore_open.take() {
                                let was_active = self.central_active();
                                self.workspace_open = open;
                                if was_active != self.central_active() {
                                    cx.emit(InspectorEvent::ModeChanged(self.central_active()));
                                }
                            }
                        }
                    }
                    Ok(ReadResult::Stash(stash)) => {
                        self.stash_area = self
                            .intent
                            .choice()
                            .map(|file| file.area.clone())
                            .unwrap_or_else(|| {
                                stash
                                    .groups
                                    .iter()
                                    .find(|g| !g.files.is_empty())
                                    .map(|g| g.area.clone())
                                    .unwrap_or_else(|| "worktree".into())
                            });
                        self.stash = Some(stash);
                    }
                    Err(e) => {
                        self.error = Some(e);
                        self.details = None;
                        self.snapshot = None;
                        self.intent.clear();
                        self.viewer_dirty = true;
                        let presentation = FilePresentation::empty(self.revision.file);
                        self.browser
                            .update(cx, |view, cx| view.present(presentation, cx));
                    }
                    _ => {}
                }
                self.rebuild_rows();
                if matches!(self.selection, Selection::Commit(_))
                    && let Some(ReadRequest {
                        kind: ReadKind::Diff { file, .. },
                        ..
                    }) = &self.pending
                    && let Some(position) = self.rows("").iter().position(|row| row.key == file.key)
                {
                    self.active_file[0] = position;
                    self.scroll[0].scroll_to_item(position, ScrollStrategy::Nearest);
                }
                if matches!(self.selection, Selection::Stash(_))
                    && let Some(intent) = self.intent.choice().cloned()
                {
                    if let Some(file) = self
                        .rows(&intent.area)
                        .iter()
                        .find(|f| f.key == intent.key)
                        .cloned()
                    {
                        self.choose(file, cx);
                    } else {
                        self.intent.clear();
                        self.snapshot = None;
                        self.restore_open = None;
                        self.close_workspace(cx);
                    }
                }
                self.schedule_preload(cx);
            }
            ReadKind::Diff {
                file,
                background,
                selection,
            } if request.revision.selection_matches(self.revision)
                && (!matches!(selection, Selection::Working)
                    || !self.status_loading
                        && request.revision.mutable_selection_matches(self.revision)) =>
            {
                match result {
                    Ok(ReadResult::Diff(diff)) => {
                        self.cache
                            .remember(file.cache_key(), diff.clone(), background);
                        self.color(file.clone(), diff.clone(), background, cx);
                        if !background {
                            self.publish_diff(file, Some(diff), None, cx);
                        } else {
                            changed = false;
                        }
                    }
                    Err(error) if !background && request.revision.file_matches(self.revision) => {
                        self.publish_diff(file, None, Some(error), cx)
                    }
                    _ => {
                        changed = false;
                    }
                }
            }
            _ => {
                changed = false;
            }
        }
        if changed {
            cx.notify();
        }
    }
    fn schedule_preload(&mut self, cx: &mut Context<Self>) {
        if self.preload_task.is_some()
            || self.color_active
            || self.color_pending.is_some()
            || self.active
            || self.pending.is_some()
            || self.details_loading
            || self.diff_loading
            || matches!(self.selection, Selection::Working)
                && (self.status_loading || self.status_error.is_some())
            || self.cache.full()
        {
            return;
        }
        let files = self.preload_rows.clone();
        let mut wanted = None;
        while self.preload_cursor < files.len() {
            let file = files[self.preload_cursor].clone();
            self.preload_cursor += 1;
            if self.cache.entries.contains_key(&file.cache_key())
                || syntax::language(&file.path).is_none()
            {
                continue;
            }
            wanted = Some(file);
            break;
        }
        let (Some(file), Some(id)) = (wanted, self.repository.as_ref().map(|r| r.id.clone()))
        else {
            return;
        };
        let revision = self.revision;
        self.preload_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(150))
                .await;
            let _ = this.update(cx, |view, cx| {
                view.preload_task = None;
                if revision.selection_matches(view.revision)
                    && (!matches!(view.selection, Selection::Working)
                        || revision.mutable_selection_matches(view.revision)
                            && !view.status_loading
                            && view.status_error.is_none())
                    && !view.active
                    && view.pending.is_none()
                    && !view.diff_loading
                    && !view.cache.full()
                {
                    view.pending = Some(ReadRequest {
                        revision,
                        id,
                        kind: ReadKind::Diff {
                            selection: view.selection.clone(),
                            file,
                            background: true,
                        },
                    });
                    view.pump(cx);
                }
            });
        }));
    }
    fn sync_viewer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.viewer_dirty {
            return;
        }
        let image_target = self
            .snapshot
            .as_ref()
            .filter(|snapshot| {
                self.preferences.show_image_previews
                    && supports_path(&snapshot.file.path)
                    && !self.diff_loading
                    && !self.details_loading
                    && snapshot.error.is_none()
            })
            .and_then(|snapshot| {
                let repository = self.repository.as_ref()?;
                let file = &snapshot.file;
                let source = match &self.selection {
                    Selection::Commit(sha) => FileImageSource::Commit {
                        sha: sha.clone(),
                        file_key: file.key.clone(),
                        comparison: true,
                    },
                    Selection::Working => FileImageSource::WorkingTree {
                        area: file.area.clone(),
                        file_key: file.key.clone(),
                    },
                    Selection::Stash(sha) => FileImageSource::Stash {
                        sha: sha.clone(),
                        area: file.area.clone(),
                        file_key: file.key.clone(),
                    },
                    Selection::None => return None,
                };
                Some(ImageTarget {
                    session_id: repository.id.clone(),
                    source,
                })
            });
        self.image_preview.update(cx, |view, cx| {
            view.set_style(&self.preferences, self.compact, cx);
            view.present(image_target, self.revision.file, cx);
        });
        self.viewer_dirty = false;
        let prepared = self.snapshot.as_ref().and_then(|s| s.prepared.clone());
        let path = self
            .snapshot
            .as_ref()
            .map(|s| s.file.path.clone())
            .unwrap_or_default();
        let error = self.snapshot.as_ref().and_then(|s| s.error.clone());
        let loading = self.diff_loading || self.details_loading;
        #[cfg(feature = "app-benchmark")]
        let prepare_start = Instant::now();
        self.viewer.update(cx, |view, cx| {
            if let Some(prepared) = prepared {
                view.set_prepared_diff(prepared.diff, path, prepared.tokens, window, cx);
            } else {
                view.clear(cx);
            }
            view.set_loading(loading, cx);
            view.set_error(error, cx);
        });
        #[cfg(feature = "app-benchmark")]
        crate::perf::record_operation("ui_prepare", prepare_start.elapsed());
    }
    pub fn render_workspace(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        if matches!(self.selection, Selection::Commit(_)) {
            return div()
                .size_full()
                .min_w_0()
                .child(render_cached_view(self.browser.clone(), window))
                .into_any_element();
        }
        self.sync_viewer(window, cx);
        let p = self.palette;
        let o = self.preferences.font_size_offset;
        let path = self
            .snapshot
            .as_ref()
            .map(|s| s.file.path.clone())
            .or_else(|| self.intent.choice().map(|f| f.path.clone()))
            .unwrap_or_default();
        let controls = self.viewer.update(cx, |v, cx| v.font_controls(window, cx));
        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(p.reading_bg))
            .child(
                div()
                    .h(px(40.))
                    .flex_shrink_0()
                    .px(px(16.))
                    .flex()
                    .items_center()
                    .gap(px(9.))
                    .border_b_1()
                    .border_color(rgb(p.border))
                    .bg(rgb(p.panel))
                    .child(icon(IconName::FileText, 15., p.muted))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(12. + o))
                            .child(path),
                    )
                    .child(controls)
                    .child(
                        div()
                            .id("local-close")
                            .role(Role::Button)
                            .aria_label("Fermer le diff")
                            .focusable()
                            .cursor_pointer()
                            .child(icon(IconName::X, 16., p.muted))
                            .on_click(cx.listener(|v, _, _, cx| v.close_workspace(cx)))
                            .on_key_down(cx.listener(|v, e: &KeyDownEvent, _, cx| {
                                if is_activate(e) {
                                    v.close_workspace(cx);
                                    cx.stop_propagation();
                                }
                            })),
                    ),
            )
            .child(
                div()
                    .h(px(40.))
                    .flex_shrink_0()
                    .px(px(16.))
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .border_b_1()
                    .border_color(rgb(p.border))
                    .child(
                        action("local-back", "", false, false, p, o)
                            .child(icon(IconName::ArrowLeft, 13., p.text))
                            .child("Historique")
                            .on_click(cx.listener(|v, _, _, cx| v.close_workspace(cx)))
                            .on_key_down(cx.listener(|v, e: &KeyDownEvent, _, cx| {
                                if is_activate(e) {
                                    v.close_workspace(cx);
                                    cx.stop_propagation();
                                }
                            })),
                    )
                    .child(
                        div()
                            .text_size(px(11. + o))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(p.accent))
                            .child("Diff"),
                    )
                    .child(div().flex_1())
                    .children(self.diff().map(|diff| diff_counts(diff, p, o))),
            )
            .child(div().flex_1().min_h_0().child(self.diff_body(window, cx)))
            .into_any_element()
    }
    fn diff_body(&self, window: &Window, cx: &App) -> AnyElement {
        if self.image_preview.read(cx).has_target() {
            render_cached_view(self.image_preview.clone(), window)
        } else {
            render_cached_view(self.viewer.clone(), window)
        }
    }
    fn clear_image_preview(&mut self, cx: &mut Context<Self>) {
        self.image_preview
            .update(cx, |view, cx| view.present(None, self.revision.file, cx));
    }
    fn panel_heading(&self, text: &str, name: IconName) -> Stateful<Div> {
        div()
            .id(SharedString::from(format!("heading-{text}")))
            .role(Role::Heading)
            .aria_level(2)
            .aria_label(text.to_owned())
            .h(px(48.))
            .flex_shrink_0()
            .px(px(19.))
            .flex()
            .items_center()
            .gap(px(8.))
            .border_b_1()
            .border_color(rgb(self.palette.border))
            .text_size(px(12. + self.preferences.font_size_offset))
            .font_weight(FontWeight::SEMIBOLD)
            .child(icon(name, 16., self.palette.muted))
            .child(text.to_owned())
    }
    fn mode_controls(&self, stash: bool, cx: &mut Context<Self>) -> Div {
        let p = self.palette;
        let o = self.preferences.font_size_offset;
        div()
            .when(!stash, |s| {
                s.min_h(px(if matches!(self.selection, Selection::Working) {
                    42.
                } else {
                    36.
                }))
            })
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap(px(4.))
            .when(!stash, |s| {
                s.px(px(18.)).border_b_1().border_color(rgb(p.border))
            })
            .when(matches!(self.selection, Selection::Working), |s| {
                s.py(px(7.))
            })
            .when(
                !stash && !matches!(self.selection, Selection::Working),
                |s| s.pb(px(8.)),
            )
            .child(
                mode_button(
                    "central-mode",
                    if stash {
                        "Diff central"
                    } else {
                        "Vue centrale"
                    },
                    !self.compact,
                    p,
                    o,
                )
                .on_click(cx.listener(|v, _, _, cx| v.set_compact(false, cx)))
                .on_key_down(cx.listener(|v, e: &KeyDownEvent, _, cx| {
                    if is_activate(e) {
                        v.set_compact(false, cx);
                        cx.stop_propagation();
                    }
                })),
            )
            .child(
                mode_button("compact-mode", "Vue compacte", self.compact, p, o)
                    .on_click(cx.listener(|v, _, _, cx| v.set_compact(true, cx)))
                    .on_key_down(cx.listener(|v, e: &KeyDownEvent, _, cx| {
                        if is_activate(e) {
                            v.set_compact(true, cx);
                            cx.stop_propagation();
                        }
                    })),
            )
    }
    fn state(&self, title: &str, text: &str, name: IconName, error: bool) -> Div {
        let p = self.palette;
        let o = self.preferences.font_size_offset;
        div()
            .flex_1()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(12.))
            .p(px(30.))
            .text_center()
            .text_size(px(11. + o))
            .text_color(rgb(p.muted))
            .child(icon(name, 30., if error { p.danger } else { p.accent }))
            .child(
                div()
                    .text_color(rgb(p.text))
                    .text_size(px(13. + o))
                    .font_weight(FontWeight::MEDIUM)
                    .child(title.to_owned()),
            )
            .child(div().max_w(px(280.)).child(text.to_owned()))
    }
    fn file_count(&self, area: &str) -> usize {
        match &self.selection {
            Selection::None => 0,
            Selection::Commit(_) => self
                .details
                .as_ref()
                .map_or(0, |details| details.file_count),
            Selection::Working => self.status.as_ref().map_or(0, |status| {
                if area == "staged" {
                    status.staged_count
                } else {
                    status.unstaged_count
                }
            }),
            Selection::Stash(_) => self
                .stash
                .as_ref()
                .and_then(|details| details.groups.iter().find(|group| group.area == area))
                .map_or(0, |group| group.file_count),
        }
    }
    fn file_rows(
        &self,
        range: Range<usize>,
        area: &str,
        list: usize,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let files = self.rows(area);
        let p = self.palette;
        let o = self.preferences.font_size_offset;
        let disabled = self.details_loading;
        range
            .filter_map(|index| files.get(index).map(|file| (index, file)))
            .map(|(index, file)| {
                let selected = self.snapshot.as_ref().is_some_and(|s| {
                    s.file.same_identity(file)
                        && (!matches!(self.selection, Selection::Commit(_))
                            || self
                                .intent
                                .choice()
                                .is_some_and(|intent| intent.key == s.file.key))
                });
                let file = file.clone();
                let choice = file.clone();
                let (directory, name) = split_path(&file.path);
                let focus = self.focus[list].clone();
                div()
                    .id(SharedString::from(format!("file-{area}-{index}")))
                    .role(Role::ListBoxOption)
                    .aria_label(format!(
                        "{}, {}",
                        file.path,
                        file_status_label(&file.status)
                    ))
                    .aria_selected(selected)
                    .aria_position_in_set(index + 1)
                    .aria_size_of_set(self.file_count(area))
                    .when(self.active_file[list] == index, |row| {
                        row.aria_active_descendant()
                    })
                    .h(px(32. + o * 2.))
                    .w_full()
                    .px(px(10.))
                    .py(px(4.))
                    .rounded(RADIUS_MD)
                    .border_1()
                    .border_color(rgb(if selected { p.accent_border } else { p.panel }))
                    .bg(rgb(if selected { p.accent_soft } else { p.panel }))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .cursor_pointer()
                    .hover(move |s| s.bg(rgb(p.surface)))
                    .child(status_icon(&file.status, p))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .items_baseline()
                            .text_size(px(11. + o))
                            .child(
                                div()
                                    .min_w_0()
                                    .truncate()
                                    .text_color(rgb(p.muted))
                                    .child(directory),
                            )
                            .child(div().flex_shrink_0().max_w_full().truncate().child(name)),
                    )
                    .on_click(cx.listener(move |view, _, window, cx| {
                        if disabled {
                            return;
                        }
                        window.focus(&focus, cx);
                        view.active_file[list] = index;
                        view.scroll[list].scroll_to_item(index, ScrollStrategy::Nearest);
                        view.choose(choice.clone(), cx);
                    }))
                    .into_any_element()
            })
            .collect()
    }
    fn file_list(
        &self,
        area: &str,
        count: usize,
        list: usize,
        empty: &str,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let len = self.rows(area).len();
        let area_owned = area.to_owned();
        let keyboard_area = area.to_owned();
        let p = self.palette;
        let o = self.preferences.font_size_offset;
        let mut body = div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .px(px(7.))
            .pb(px(9.));
        if len == 0 {
            body = body.child(
                div()
                    .px(px(11.))
                    .py(px(6.))
                    .text_size(px(11. + o))
                    .text_color(rgb(p.muted))
                    .child(empty.to_owned()),
            );
        } else {
            body = body.child(
                div()
                    .relative()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .child(
                        uniform_list(
                            ("files", list),
                            len,
                            cx.processor(move |v, range, _, cx| {
                                v.file_rows(range, &area_owned, list, cx)
                            }),
                        )
                        .track_scroll(&self.scroll[list])
                        .flex_1()
                        .min_h_0(),
                    )
                    .child(scrollbar_theme(
                        Scrollbar::vertical(&self.scroll[list]).id(("files-scrollbar", list)),
                    )),
            );
        }
        if len < count {
            body = body.child(
                div()
                    .px(px(11.))
                    .py(px(6.))
                    .text_size(px(11. + o))
                    .text_color(rgb(p.muted))
                    .child(format!(
                        "Les {} premiers fichiers sur {} sont affichés.",
                        format_count(len),
                        format_count(count)
                    )),
            );
        }
        body.id(("files-focus", list))
            .track_focus(&self.focus[list])
            .on_key_down(cx.listener(move |v, e: &KeyDownEvent, _, cx| {
                v.file_keydown(&keyboard_area, list, e, cx)
            }))
            .into_any_element()
    }
    fn file_keydown(
        &mut self,
        area: &str,
        list: usize,
        event: &KeyDownEvent,
        cx: &mut Context<Self>,
    ) {
        if self.details_loading {
            return;
        }
        let files = self.rows(area);
        if files.is_empty() {
            return;
        }
        let current = self.active_file[list].min(files.len() - 1);
        let height = self.scroll[list]
            .0
            .borrow()
            .base_handle
            .bounds()
            .size
            .height;
        let page = (f32::from(height) / (32. + self.preferences.font_size_offset * 2.))
            .floor()
            .max(1.) as usize;
        let target = match event.keystroke.key.as_str() {
            "down" => current.saturating_add(1),
            "up" => current.saturating_sub(1),
            "pagedown" => current.saturating_add(page),
            "pageup" => current.saturating_sub(page),
            "home" => 0,
            "end" => files.len() - 1,
            "enter" | "space" => current,
            _ => return,
        }
        .min(files.len() - 1);
        cx.stop_propagation();
        if target == current && !matches!(event.keystroke.key.as_str(), "enter" | "space") {
            return;
        }
        self.active_file[list] = target;
        self.scroll[list].scroll_to_item(target, ScrollStrategy::Nearest);
        self.choose(files[target].clone(), cx);
    }
    fn compact_diff(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        self.sync_viewer(window, cx);
        let p = self.palette;
        let o = self.preferences.font_size_offset;
        let path = self
            .snapshot
            .as_ref()
            .map(|s| filename(&s.file.path))
            .unwrap_or_else(|| "Aperçu des modifications".into());
        let controls = self.viewer.update(cx, |v, cx| v.font_controls(window, cx));
        div()
            .flex_1()
            .min_h(px(110.))
            .flex()
            .flex_col()
            .bg(rgb(p.reading_bg))
            .border_t_1()
            .border_color(rgb(p.border))
            .child(
                div()
                    .h(px(40.))
                    .flex_shrink_0()
                    .px(px(18.))
                    .flex()
                    .items_center()
                    .gap(px(5.))
                    .bg(rgb(p.panel))
                    .border_b_1()
                    .border_color(rgb(p.border))
                    .child(icon(IconName::FileText, 13., p.muted))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(11. + o))
                            .child(path),
                    )
                    .child(controls),
            )
            .child(div().flex_1().min_h_0().child(self.diff_body(window, cx)))
            .into_any_element()
    }
    fn render_commit(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let p = self.palette;
        let o = self.preferences.font_size_offset;
        let mut panel = div()
            .id("commit-details-panel")
            .role(Role::Complementary)
            .aria_label("Détails du commit")
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(p.panel))
            .border_l_1()
            .border_color(rgb(p.border))
            .child(self.panel_heading("Commit", IconName::GitCommitHorizontal));
        if let Some(error) = &self.error {
            return panel
                .child(self.state(
                    "Impossible de lire le commit",
                    error,
                    IconName::CircleAlert,
                    true,
                ))
                .into_any_element();
        }
        let Some(details) = self.details.clone() else {
            return panel.child(if self.details_loading{self.state("Lecture du commit…","",IconName::GitCommitHorizontal,false)}else{self.state("Chaque commit a son histoire","Sélectionnez un commit pour découvrir son message, ses fichiers et ses modifications.",IconName::GitCommitHorizontal,false)}).into_any_element();
        };
        if let Some(limit) = &details.similarity_limited {
            panel = panel.child(notice(limit.clone(), p.warning, o));
        }
        let (title, description) = self.commit_message.clone().unwrap_or_default();
        let sha = details.sha.clone();
        let parent = if details.parents.len() > 1 {
            format!(
                "Commit de fusion · comparaison avec le premier parent {}",
                short_sha(&details.parents[0])
            )
        } else if details.parents.is_empty() {
            "Premier commit du dépôt".into()
        } else {
            String::new()
        };
        panel = panel.child(
            div()
                .h(relative(0.36))
                .min_h(px(220.))
                .max_h(px(290.))
                // GPUI lengths have no calc(): move 3o from bottom padding to
                // margin to add it after the clamp while keeping 16px below
                // the content.
                .mb(px(o * 3.))
                .flex_shrink_0()
                .flex()
                .flex_col()
                .gap(px(10.))
                .px(px(20.))
                .pt(px(20.))
                .pb(px(16. - o * 3.))
                .child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .relative()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .id("commit-message")
                                .role(Role::Document)
                                .aria_label("Message du commit")
                                .track_scroll(&self.message_scroll)
                                .flex_1()
                                .min_h_0()
                                .overflow_y_scroll()
                                // Preserve the source's stable 6px gutter.
                                .pr(px(6.))
                                .text_size(px(15. + o))
                                .font_weight(FontWeight::MEDIUM)
                                .line_height(relative(1.625))
                                .child(title)
                                .when(!description.is_empty(), |s| {
                                    s.child(
                                        div()
                                            .mt(px(10.))
                                            .text_size(px(12. + o))
                                            .line_height(relative(1.625))
                                            .text_color(rgb(p.muted))
                                            .font_weight(FontWeight::NORMAL)
                                            .child(description),
                                    )
                                }),
                        )
                        .child(scrollbar_theme(
                            Scrollbar::vertical(&self.message_scroll)
                                .id("commit-message-scrollbar"),
                        )),
                )
                .child(
                    div()
                        .h(px(38. + o * 3.))
                        .flex_shrink_0()
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .min_w_0()
                        .child(
                            div()
                                .size(px(32.))
                                .flex_shrink_0()
                                .flex()
                                .items_center()
                                .justify_center()
                                .rounded_full()
                                .bg(rgb(p.accent_soft))
                                .text_color(rgb(p.accent))
                                .text_size(px(11. + o))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(initials(&details.author)),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap(px(4.))
                                .child(
                                    div()
                                        .truncate()
                                        .text_size(px(12. + o))
                                        .line_height(px(16. + o * 2.))
                                        .font_weight(FontWeight::MEDIUM)
                                        .child(details.author.clone()),
                                )
                                .child(
                                    div()
                                        .truncate()
                                        .text_size(px(10. + o))
                                        .text_color(rgb(p.muted))
                                        .child(commit_date(details.time)),
                                ),
                        ),
                )
                .child(
                    div()
                        .id("commit-sha")
                        .role(Role::Button)
                        .aria_label("Copier le SHA complet du commit")
                        .focusable()
                        .h(px(36. + o * 2.))
                        .flex_shrink_0()
                        .min_w_0()
                        .flex()
                        .items_center()
                        .gap(px(12.))
                        .px(px(11.))
                        .rounded(RADIUS_MD)
                        .bg(rgb(p.bg))
                        .text_size(px(10. + o))
                        .cursor_pointer()
                        .child(div().flex_shrink_0().text_color(rgb(p.muted)).child("SHA"))
                        .child(
                            div()
                                .relative()
                                .flex_1()
                                .min_w_0()
                                .h_full()
                                .child(
                                    div()
                                        .id("full-sha")
                                        .track_scroll(&self.sha_scroll)
                                        .size_full()
                                        .min_w_0()
                                        .overflow_x_scroll()
                                        .flex()
                                        .items_center()
                                        .font_family(MONO)
                                        .whitespace_nowrap()
                                        .child(div().flex_shrink_0().child(sha.clone())),
                                )
                                .child(scrollbar_theme(
                                    Scrollbar::horizontal(&self.sha_scroll)
                                        .id("full-sha-scrollbar"),
                                )),
                        )
                        .on_click(cx.listener(move |_, _, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(sha.clone()))
                        })),
                )
                .child(
                    div()
                        .h(px(30. + o * 2.))
                        .flex_shrink_0()
                        .relative()
                        .child(
                            div()
                                .id("parent-note")
                                .track_scroll(&self.parent_scroll)
                                .size_full()
                                .overflow_y_scroll()
                                .text_size(px(10. + o))
                                .text_color(rgb(p.muted))
                                .child(parent),
                        )
                        .child(scrollbar_theme(
                            Scrollbar::vertical(&self.parent_scroll).id("parent-note-scrollbar"),
                        )),
                ),
        );
        let mut files = div()
            .min_h_0()
            .flex()
            .flex_col()
            .border_t_1()
            .border_color(rgb(p.border))
            .when(self.compact, |s| s.h(relative(0.30)))
            .when(!self.compact, |s| s.flex_1())
            .child(
                div()
                    .h(px(40.))
                    .flex_shrink_0()
                    .px(px(18.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(10.))
                    .text_size(px(11. + o))
                    .font_weight(FontWeight::MEDIUM)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child("Fichiers modifiés")
                            .child(
                                div()
                                    .h(px(17.))
                                    .px(px(5.))
                                    .rounded(RADIUS_SM)
                                    .bg(rgb(p.surface))
                                    .text_size(px(9. + o))
                                    .text_color(rgb(p.muted))
                                    .child(format_count(details.file_count)),
                            ),
                    )
                    .children(
                        self.snapshot
                            .as_ref()
                            .and_then(|s| s.prepared.as_ref().map(|prepared| &prepared.diff))
                            .map(|d| diff_counts(d, p, o)),
                    ),
            )
            .child(self.mode_controls(false, cx));
        files = files.child(self.file_list(
            "",
            details.file_count,
            0,
            "Aucun fichier modifié dans cette comparaison.",
            cx,
        ));
        panel = panel.child(files);
        if self.compact {
            panel = panel.child(self.compact_diff(window, cx));
        }
        panel.into_any_element()
    }
    fn working_section(
        &self,
        area: &str,
        count: usize,
        small: bool,
        empty: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = self.palette;
        let o = self.preferences.font_size_offset;
        let staged = area == "staged";
        let len = self.rows(area).len();
        let list = if staged { 1 } else { 0 };
        div()
            .min_h_0()
            .flex()
            .flex_col()
            .when(small, |s| {
                s.flex_shrink_0().max_h(relative(0.5)).h(px(if empty {
                    94.
                } else {
                    48. + len.min(6) as f32 * (32. + o * 2.) + 9.
                }))
            })
            .when(!small, |s| s.flex_1())
            .when(staged, |s| s.border_t_1().border_color(rgb(p.border)))
            .child(
                div()
                    .h(px(48.))
                    .flex_shrink_0()
                    .px(px(18.))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(icon(
                        if staged {
                            IconName::Check
                        } else {
                            IconName::FileText
                        },
                        14.,
                        p.muted,
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(
                                div()
                                    .text_size(px(11. + o))
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(if staged { "Indexés" } else { "Non indexés" }),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(9. + o))
                                    .text_color(rgb(p.muted))
                                    .child(if staged {
                                        "Comparaison avec HEAD"
                                    } else {
                                        "Comparaison avec l’index"
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .px(px(5.))
                            .rounded(RADIUS_SM)
                            .bg(rgb(p.surface))
                            .text_size(px(9. + o))
                            .text_color(rgb(p.muted))
                            .child(format_count(count)),
                    ),
            )
            .child(self.file_list(
                area,
                count,
                list,
                if staged {
                    "Aucun fichier indexé."
                } else {
                    "Aucun fichier non indexé."
                },
                cx,
            ))
            .into_any_element()
    }
    fn render_working(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let p = self.palette;
        let o = self.preferences.font_size_offset;
        let mut panel = div()
            .id("working-tree-panel")
            .role(Role::Complementary)
            .aria_label("Modifications locales")
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(p.panel))
            .border_l_1()
            .border_color(rgb(p.border))
            .child(self.panel_heading("Modifications locales", IconName::FolderGit2));
        let Some(status) = self.status.clone() else {
            return panel.child(self.state(if self.status_error.is_some(){"Impossible de lire les modifications locales"}else if self.status_loading{"Lecture des modifications locales…"}else{"Fichiers non commités"},self.status_error.as_deref().unwrap_or("Ouvrez un dépôt pour consulter les changements de son index et de ses fichiers de travail."),IconName::FolderGit2,self.status_error.is_some())).into_any_element();
        };
        if !status.available {
            return panel
                .child(self.state(
                    "Ce dépôt n’a pas d’arbre de travail",
                    "Consultez ses commits pour voir les fichiers versionnés.",
                    IconName::FolderGit2,
                    false,
                ))
                .into_any_element();
        }
        if let Some(error) = &self.status_error {
            panel = panel.child(notice(error.clone(), p.danger, o));
        }
        if let Some(limit) = &status.similarity_limited {
            panel = panel.child(notice(limit.clone(), p.warning, o));
        }
        panel = panel.child(self.mode_controls(false, cx));
        let unstaged_small = status.unstaged.len() < status.staged.len();
        let empty = if unstaged_small {
            status.unstaged.is_empty()
        } else {
            status.staged.is_empty()
        };
        panel = panel.child(
            div()
                .min_h_0()
                .flex()
                .flex_col()
                .when(self.compact, |s| s.h(relative(0.44)).flex_shrink_0())
                .when(!self.compact, |s| s.flex_1())
                .child(self.working_section(
                    "unstaged",
                    status.unstaged_count,
                    unstaged_small,
                    empty,
                    cx,
                ))
                .child(self.working_section(
                    "staged",
                    status.staged_count,
                    !unstaged_small,
                    empty,
                    cx,
                )),
        );
        if self.compact {
            panel = panel.child(self.compact_diff(window, cx));
        }
        panel.into_any_element()
    }
    fn render_stash(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let p = self.palette;
        let o = self.preferences.font_size_offset;
        let mut panel = div()
            .id("stash-details-panel")
            .role(Role::Complementary)
            .aria_label("Détails du stash")
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(p.panel))
            .border_l_1()
            .border_color(rgb(p.border))
            .child(self.panel_heading("Stash", IconName::Archive));
        if let Some(error) = &self.error {
            return panel
                .child(notice(error.clone(), p.danger, o))
                .into_any_element();
        }
        let Some(stash) = self.stash.clone() else {
            return panel
                .child(notice(
                    if self.details_loading {
                        "Lecture du stash…"
                    } else {
                        "Ce stash est indisponible. Actualisez le dépôt."
                    },
                    p.muted,
                    o,
                ))
                .into_any_element();
        };
        panel = panel.child(
            div()
                .flex_shrink_0()
                .flex()
                .flex_col()
                .gap(px(8.))
                .p(px(16.))
                .border_b_1()
                .border_color(rgb(p.border))
                .text_size(px(12. + o))
                .child(
                    div()
                        .max_h(px(96.))
                        .relative()
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .id("stash-message")
                                .role(Role::Document)
                                .aria_label("Message du stash")
                                .track_scroll(&self.stash_scroll)
                                .max_h(px(96.))
                                .overflow_y_scroll()
                                .text_size(px(14. + o))
                                .font_weight(FontWeight::MEDIUM)
                                .child(stash.message.clone()),
                        )
                        .child(scrollbar_theme(
                            Scrollbar::vertical(&self.stash_scroll).id("stash-message-scrollbar"),
                        )),
                )
                .child(div().text_color(rgb(p.muted)).child(format!(
                    "{} · {}",
                    stash.author,
                    commit_date(stash.time)
                )))
                .child(div().text_color(rgb(p.muted)).child(format!(
                    "Base {} · lecture seule",
                    short_sha(&stash.base_sha)
                )))
                .child(self.mode_controls(true, cx)),
        );
        panel = panel.child(
            div()
                .flex_shrink_0()
                .flex()
                .flex_wrap()
                .gap(px(4.))
                .p(px(8.))
                .border_b_1()
                .border_color(rgb(p.border))
                .children(stash.groups.iter().enumerate().map(|(index, group)| {
                    let area = group.area.clone();
                    let keyboard_area = area.clone();
                    let label = match area.as_str() {
                        "index" => "Index",
                        "untracked" => "Non suivis",
                        _ => "Répertoire de travail",
                    };
                    mode_button(
                        ("stash-area", index),
                        format!("{label} {}", format_count(group.file_count)),
                        area == self.stash_area,
                        p,
                        o,
                    )
                    .on_click(cx.listener(move |v, _, _, cx| v.change_stash_area(area.clone(), cx)))
                    .on_key_down(cx.listener(
                        move |v, e: &KeyDownEvent, _, cx| {
                            if is_activate(e) {
                                v.change_stash_area(keyboard_area.clone(), cx);
                                cx.stop_propagation();
                            }
                        },
                    ))
                })),
        );
        if let Some(group) = stash.groups.iter().find(|g| g.area == self.stash_area) {
            panel = panel.child(notice(group.comparison.clone(), p.muted, o));
            panel = panel.child(
                div()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .child(self.file_list(
                        &self.stash_area,
                        group.file_count,
                        2,
                        "Aucun fichier modifié dans cette comparaison.",
                        cx,
                    )),
            );
        }
        if self.compact {
            panel = panel.child(self.compact_diff(window, cx));
        }
        panel.into_any_element()
    }
}
fn keyboard_selection_delay(previous: Option<Duration>) -> Option<Duration> {
    previous
        .filter(|elapsed| *elapsed <= Duration::from_millis(150))
        .map(|_| Duration::from_millis(70))
}

fn preview_choice(details: &CommitDetails, preferred_key: Option<&str>) -> Option<FileChoice> {
    preferred_key
        .and_then(|key| details.files.iter().find(|file| file.file_key == key))
        .or_else(|| details.files.first())
        .map(|file| FileChoice::changed(file, ""))
}
impl Render for InspectorView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        #[cfg(feature = "app-benchmark")]
        crate::perf::record_count("inspector_render");
        let _keep_subscriptions = &self.subscriptions;
        match self.selection {
            Selection::None | Selection::Commit(_) => self.render_commit(window, cx),
            Selection::Working => self.render_working(window, cx),
            Selection::Stash(_) => self.render_stash(window, cx),
        }
    }
}
fn mode_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    selected: bool,
    p: Palette,
    o: f32,
) -> Stateful<Div> {
    let label = label.into();
    div()
        .id(id)
        .role(Role::Button)
        .aria_label(label.clone())
        .aria_toggled(if selected {
            accesskit::Toggled::True
        } else {
            accesskit::Toggled::False
        })
        .focusable()
        .px(px(8.))
        .py(px(4.))
        .rounded(RADIUS_MD)
        .border_1()
        .border_color(rgb(if selected { p.accent_border } else { p.panel }))
        .bg(rgb(if selected { p.accent_soft } else { p.panel }))
        .text_color(rgb(if selected { p.accent } else { p.muted }))
        .text_size(px(10. + o))
        .cursor_pointer()
        .hover(move |s| s.bg(rgb(p.surface)))
        .focus_visible(move |style| style.border_color(rgb(p.accent)))
        .child(label)
}
fn diff_counts(diff: &FileDiff, p: Palette, o: f32) -> Div {
    div()
        .flex()
        .gap(px(8.))
        .font_family(MONO)
        .text_size(px(10. + o))
        .child(
            div()
                .text_color(rgb(p.success))
                .child(format!("+{}", format_count(diff.additions))),
        )
        .child(
            div()
                .text_color(rgb(p.danger))
                .child(format!("−{}", format_count(diff.deletions))),
        )
}
fn file_status_label(status: &str) -> &'static str {
    match status {
        "added" => "Ajouté",
        "deleted" => "Supprimé",
        "modified" => "Modifié",
        "renamed" => "Renommé",
        "copied" => "Copié",
        "typechange" => "Type modifié",
        "unmodified" => "Inchangé",
        _ => "Statut inconnu",
    }
}
fn split_path(path: &str) -> (String, String) {
    match path.rfind('/') {
        Some(index) => (path[..=index].into(), path[index + 1..].into()),
        None => (String::new(), path.into()),
    }
}
fn filename(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).into()
}
#[cfg(test)]
mod tests {
    use super::{DiffCache, FileChoice, FileIntent, MAX_CACHE_ENTRIES, RefreshedIntent, Revision};
    use mangrove_core::model::{DiffHunk, DiffLine, FileDiff};
    use std::collections::HashMap;
    use std::sync::Arc;
    fn diff(bytes: usize) -> super::PreparedDiff {
        let diff = Arc::new(FileDiff {
            binary: false,
            truncated: false,
            additions: 1,
            deletions: 0,
            hunks: vec![DiffHunk {
                header: String::new(),
                lines: vec![DiffLine {
                    kind: "addition".into(),
                    old_line: None,
                    new_line: Some(1),
                    content: "x".repeat(bytes),
                }],
            }],
        });
        super::PreparedDiff {
            diff,
            tokens: Arc::default(),
            syntax_ready: true,
        }
    }
    #[test]
    fn keyboard_bursts_delay_only_repeated_selection_intents() {
        use std::time::Duration;
        assert_eq!(super::keyboard_selection_delay(None), None);
        assert_eq!(
            super::keyboard_selection_delay(Some(Duration::from_millis(33))),
            Some(Duration::from_millis(70))
        );
        assert_eq!(
            super::keyboard_selection_delay(Some(Duration::from_millis(90))),
            Some(Duration::from_millis(70))
        );
        assert_eq!(
            super::keyboard_selection_delay(Some(Duration::from_millis(151))),
            None
        );
    }
    #[test]
    fn obsolete_preloads_and_color_are_cancelled_without_cancelling_status() {
        let revision = Revision {
            status: 1,
            repository: 1,
            selection: 2,
            file: 3,
        };
        let next_file = Revision {
            file: 4,
            ..revision
        };
        let next_status = Revision {
            status: 2,
            ..revision
        };
        let file = choice("a", 0, "unstaged");
        let preload = super::ReadRequest {
            revision,
            id: "session".into(),
            kind: super::ReadKind::Diff {
                selection: super::Selection::Working,
                file: file.clone(),
                background: true,
            },
        };
        assert!(preload.current(revision));
        assert!(!preload.current(next_file));
        assert!(!preload.current(next_status));
        let color = super::ColorRequest {
            revision,
            selection: super::Selection::Working,
            file,
            prepared: diff(1),
            background: true,
        };
        assert!(!color.current(next_file));
        assert!(!color.current(next_status));
        let status = super::ReadRequest {
            revision,
            id: "session".into(),
            kind: super::ReadKind::Status,
        };
        assert!(status.current(next_file));
        assert!(!status.current(next_status));
        let commit = super::ReadRequest {
            revision,
            id: "session".into(),
            kind: super::ReadKind::Commit {
                sha: "immutable".into(),
                preferred_key: None,
            },
        };
        assert!(commit.current(next_status));
        assert!(!commit.current(Revision {
            selection: 3,
            ..revision
        }));
        assert!(!commit.current(Revision {
            repository: 2,
            ..revision
        }));
    }
    #[test]
    fn replacing_generation_cannot_publish_old_file_or_old_repository() {
        let r = Revision {
            status: 1,
            repository: 1,
            selection: 2,
            file: 3,
        };
        assert!(!r.file_matches(Revision { file: 4, ..r }));
        assert!(r.selection_matches(Revision { file: 4, ..r }));
        assert!(!r.selection_matches(Revision { selection: 3, ..r }));
        assert!(!r.selection_matches(Revision { repository: 2, ..r }));
        assert!(!r.mutable_selection_matches(Revision { status: 2, ..r }));
    }

    fn choice(key: &str, index: usize, area: &str) -> FileChoice {
        FileChoice {
            index,
            key: key.into(),
            path: "same-displayed-path.ts".into(),
            old_path: None,
            status: "modified".into(),
            area: area.into(),
        }
    }

    #[test]
    fn latest_file_intent_during_refresh_resolves_only_in_the_fresh_status() {
        let mut intent = FileIntent::default();
        assert!(intent.select(choice("a", 0, "unstaged"), false));
        assert!(!intent.select(choice("b", 1, "unstaged"), true));
        assert!(!intent.select(choice("c", 2, "staged"), true));
        let current = Revision {
            repository: 1,
            status: 2,
            selection: 3,
            file: 5,
        };
        let rows = HashMap::from([
            (
                "unstaged".into(),
                Arc::new(vec![choice("c", 20, "unstaged")]),
            ),
            ("staged".into(), Arc::new(vec![choice("c", 9, "staged")])),
        ]);
        assert!(matches!(
            intent.resolve_status(
                Revision {
                    status: 1,
                    ..current
                },
                current,
                &rows
            ),
            RefreshedIntent::Stale
        ));
        assert_eq!(intent.choice().unwrap().index, 2);
        let RefreshedIntent::Ready(resolved) = intent.resolve_status(current, current, &rows)
        else {
            panic!("the latest intent must resume after refresh");
        };
        assert_eq!(resolved.key, "c");
        assert_eq!(resolved.area, "staged");
        assert_eq!(resolved.index, 9);
        assert_eq!(intent.choice().unwrap().index, 9);
    }

    #[test]
    fn vanished_opaque_intent_cannot_select_a_replacement_with_the_same_display_path() {
        let mut intent = FileIntent::default();
        intent.select(choice("old-raw-bytes", 0, "unstaged"), true);
        let current = Revision {
            repository: 4,
            status: 2,
            selection: 3,
            file: 5,
        };
        let rows = HashMap::from([(
            "unstaged".into(),
            Arc::new(vec![choice("new-raw-bytes", 0, "unstaged")]),
        )]);
        assert!(matches!(
            intent.resolve_status(
                Revision {
                    repository: 3,
                    ..current
                },
                current,
                &rows
            ),
            RefreshedIntent::Stale
        ));
        assert_eq!(intent.choice().unwrap().key, "old-raw-bytes");
        assert!(matches!(
            intent.resolve_status(current, current, &rows),
            RefreshedIntent::Vanished
        ));
        assert!(intent.choice().is_none());
        assert!(matches!(
            intent.resolve_status(current, current, &rows),
            RefreshedIntent::Empty
        ));
    }
    #[test]
    fn diff_cache_evicts_least_recently_used_foreground_entries() {
        let mut c = DiffCache::default();
        for i in 0..MAX_CACHE_ENTRIES {
            c.remember(i.to_string(), diff(1), false);
        }
        c.get("0");
        c.remember("new".into(), diff(1), false);
        assert_eq!(c.entries.len(), MAX_CACHE_ENTRIES);
        assert!(c.entries.contains_key("0"));
        assert!(!c.entries.contains_key("1"));
        assert_eq!(c.bytes, MAX_CACHE_ENTRIES);
    }
    #[test]
    fn background_budget_stops_instead_of_rotating_through_the_commit() {
        let mut c = DiffCache::default();
        c.remember("big".into(), diff(super::MAX_CACHE_BYTES), true);
        c.remember("next".into(), diff(10), true);
        assert!(c.full());
        assert_eq!(c.entries.len(), 1);
        assert_eq!(c.bytes, super::MAX_CACHE_BYTES);
    }
    #[test]
    fn refresh_releases_all_diff_bytes_and_resumes_prefetch_budget() {
        let mut c = DiffCache::default();
        c.remember("a".into(), diff(1024), false);
        c.background_full = true;
        c.clear();
        assert_eq!(c.bytes, 0);
        assert!(c.entries.is_empty());
        assert!(!c.full());
    }
    #[test]
    fn opaque_worktree_identity_does_not_use_display_path() {
        let a = super::FileChoice {
            index: 0,
            key: "lossless:a".into(),
            path: "�".into(),
            old_path: None,
            status: "modified".into(),
            area: "unstaged".into(),
        };
        let b = super::FileChoice {
            key: "lossless:b".into(),
            ..a.clone()
        };
        assert_ne!(a.cache_key(), b.cache_key());
        assert!(!a.same_identity(&b));
        assert!(a.same_identity(&super::FileChoice {
            path: "another display path".into(),
            ..a.clone()
        }));
        assert!(!a.same_identity(&super::FileChoice {
            area: "staged".into(),
            ..a.clone()
        }));
        assert!(!a.same_identity(&super::FileChoice {
            index: 1,
            ..a.clone()
        }));
    }
    #[test]
    fn initial_commit_preview_is_ready_without_selection_and_restores_only_chosen_key() {
        use std::{
            fs,
            process::Command,
            time::{SystemTime, UNIX_EPOCH},
        };
        let base = std::env::temp_dir().join(format!(
            "mangrove-inspector-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = base.join("repository");
        fs::create_dir_all(&path).unwrap();
        struct Fixture(std::path::PathBuf);
        impl Drop for Fixture {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        let _fixture = Fixture(base.clone());
        let git = |args: &[&str]| {
            let output = Command::new("git")
                .args(args)
                .current_dir(&path)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8_lossy(&output.stdout).trim().to_owned()
        };
        git(&["init", "--initial-branch=main"]);
        fs::write(path.join("a.rs"), "pub fn first() -> usize { 1 }\n").unwrap();
        fs::write(path.join("b.rs"), "pub fn second() -> usize { 2 }\n").unwrap();
        git(&["add", "a.rs", "b.rs"]);
        git(&[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.test",
            "commit",
            "-m",
            "initial snapshot",
        ]);
        let sha = git(&["rev-parse", "HEAD"]);
        let workspace = mangrove_core::Workspace::new(base.join("cache")).unwrap();
        let repo = workspace.open_repository(path.to_str().unwrap()).unwrap();
        let details = workspace.commit_details_shared(&repo.id, &sha).unwrap();
        let first = super::preview_choice(&details, None).unwrap();
        let raw = super::PreparedDiff::raw(Arc::new(
            workspace.file_diff(&repo.id, &sha, first.index).unwrap(),
        ));
        assert_eq!(details.sha, sha);
        assert_eq!(first.path, details.files[0].path);
        assert_eq!(first.path, "a.rs");
        assert_eq!(raw.diff.additions, 1);
        assert!(!raw.syntax_ready);
        assert!(raw.tokens.is_empty());
        let mut intent = FileIntent::default();
        intent.restore_commit(None, Some(&first));
        assert!(intent.choice().is_none());

        let key = &details.files[1].file_key;
        let restored = super::preview_choice(&details, Some(key)).unwrap();
        assert_eq!(restored.path, "b.rs");
        intent.restore_commit(Some(key), Some(&restored));
        assert_eq!(intent.choice().unwrap().path, "b.rs");
        let patch = workspace.file_diff(&repo.id, &sha, restored.index).unwrap();
        assert!(
            patch.hunks[0]
                .lines
                .iter()
                .any(|line| line.content.contains("second"))
        );

        let missing_key = "missing-file-key";
        let fallback = super::preview_choice(&details, Some(missing_key)).unwrap();
        assert_eq!(fallback.path, "a.rs");
        intent.restore_commit(Some(missing_key), Some(&fallback));
        assert!(intent.choice().is_none());
        intent.select(first.clone(), false);
        intent.restore_commit(None, Some(&first));
        assert!(intent.choice().is_none());
    }

    #[test]
    fn token_cache_budget_stops_prefetch_before_retaining_unbounded_color_data() {
        let mut cache = DiffCache::default();
        let mut prepared = diff(1);
        prepared.tokens = Arc::new(vec![vec![
            crate::syntax::Token {
                range: 0..1,
                kind: crate::syntax::Kind::Keyword
            };
            super::MAX_CACHE_TOKENS + 1
        ]]);
        cache.remember("oversized".into(), prepared, true);
        assert!(cache.full());
        assert!(cache.entries.is_empty());
        assert_eq!(cache.tokens, 0);
    }
}
