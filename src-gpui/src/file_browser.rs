//! File revision browsing. One native read is retained until it actually settles;
//! replacing a view only replaces the queued intent, never creates an unbounded queue.
use crate::{
    code_view::{CodeView, CodeViewEvent, Highlights, ReadingPosition, hunk_shortcut},
    format::{commit_date, compact_date, count_label, format_count, short_sha},
    image_preview::{ImagePreview, ImageTarget, supports_path},
    preferences::{DiffLayout, Preferences},
    theme::{MONO, Palette, RADIUS_MD, RADIUS_SM, icon},
    view_cache::render_cached_view,
};
use accesskit::Role;
use gpui_kit::{
    assets::IconName,
    component::{scroll::Scrollbar, tooltip::Tooltip},
    prelude::*,
    *,
};
use mangrove_core::{Workspace, model::*};
use std::{
    ops::Range,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

pub type ReadGate = Arc<Mutex<()>>;

/// Changes kept in a file's history; older ones are not listed.
const MAX_HISTORY_ENTRIES: usize = 2_000;
/// Commits read without finding a change before the history asks to go on.
const AUTO_SCAN: usize = 4_096;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileTarget {
    pub session_id: String,
    pub sha: String,
    pub file_key: String,
    pub path: String,
    pub old_path: Option<String>,
    pub status: String,
}
impl FileTarget {
    /// The same file revision, perhaps read through another session.
    fn same_revision(&self, other: &Self) -> bool {
        self.is(
            &other.sha,
            &other.file_key,
            &other.path,
            &other.old_path,
            &other.status,
        )
    }
    fn is_change(&self, change: &FileHistoryEntry) -> bool {
        self.is(
            &change.sha,
            &change.file_key,
            &change.path,
            &change.old_path,
            &change.status,
        )
    }
    fn is(
        &self,
        sha: &str,
        key: &str,
        path: &str,
        old_path: &Option<String>,
        status: &str,
    ) -> bool {
        self.sha == sha
            && self.file_key == key
            && self.path == path
            && self.old_path == *old_path
            && self.status == status
    }
}

/// A patch, its syntax and its changed words always belong to the same
/// file revision.
#[derive(Clone)]
pub struct PreparedDiff {
    pub diff: Arc<FileDiff>,
    /// The compared texts, read until the highlights are ready: a prepared
    /// patch can stay cached long after, and never needs them again.
    pub sources: Option<Arc<DiffSources>>,
    pub highlights: Highlights,
    pub highlights_ready: bool,
}
impl PreparedDiff {
    pub fn raw(mut diff: FileDiff) -> Self {
        Self {
            sources: diff.sources.take(),
            diff: Arc::new(diff),
            highlights: Highlights::default(),
            highlights_ready: false,
        }
    }
}

/// The source revision shown by the inspector is published as one value. A
/// loading presentation can retain the previous, coherent patch; an error or
/// completion cannot finish a newer source's loading state.
#[derive(Clone)]
pub struct FilePresentation {
    revision: u64,
    target: Option<FileTarget>,
    prepared: Option<PreparedDiff>,
    state: PresentationState,
}

#[derive(Clone)]
enum PresentationState {
    Ready,
    Loading,
    Failed(String),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PresentationChange {
    Ignored,
    Replaced,
    /// The same revision, read through the new session of a refreshed repository.
    Rekeyed,
    Updated,
}

impl FilePresentation {
    pub fn empty(revision: u64) -> Self {
        Self {
            revision,
            target: None,
            prepared: None,
            state: PresentationState::Ready,
        }
    }

    pub fn loading(revision: u64, target: FileTarget, prepared: Option<PreparedDiff>) -> Self {
        Self {
            revision,
            target: Some(target),
            prepared,
            state: PresentationState::Loading,
        }
    }

    pub fn ready(revision: u64, target: FileTarget, prepared: PreparedDiff) -> Self {
        Self {
            revision,
            target: Some(target),
            prepared: Some(prepared),
            state: PresentationState::Ready,
        }
    }

    pub fn failed(revision: u64, target: FileTarget, error: String) -> Self {
        Self {
            revision,
            target: Some(target),
            prepared: None,
            state: PresentationState::Failed(error),
        }
    }

    fn loading_source(&self) -> bool {
        matches!(self.state, PresentationState::Loading)
    }

    fn highlights(&self) -> Option<Highlights> {
        self.prepared
            .as_ref()
            .filter(|prepared| prepared.highlights_ready)
            .map(|prepared| prepared.highlights.clone())
    }

    fn owns_diff(&self, diff: &Arc<FileDiff>) -> bool {
        self.prepared
            .as_ref()
            .is_some_and(|prepared| Arc::ptr_eq(&prepared.diff, diff))
    }

    fn error(&self) -> Option<String> {
        match &self.state {
            PresentationState::Failed(error) => Some(error.clone()),
            _ => None,
        }
    }

    fn accept(&mut self, next: Self) -> PresentationChange {
        if next.revision < self.revision {
            return PresentationChange::Ignored;
        }
        let rekeyed = match (&self.target, &next.target) {
            (Some(old), Some(new)) => old.session_id != new.session_id && old.same_revision(new),
            _ => false,
        };
        let replaced = next.revision != self.revision || next.target != self.target;
        *self = next;
        if rekeyed {
            PresentationChange::Rekeyed
        } else if replaced {
            PresentationChange::Replaced
        } else {
            PresentationChange::Updated
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FileView {
    #[default]
    Diff,
    File,
    History,
    Blame,
}
impl FileView {
    fn label(self) -> &'static str {
        match self {
            Self::Diff => "Diff",
            Self::File => "Fichier",
            Self::History => "Historique du fichier",
            Self::Blame => "Blame",
        }
    }
    /// This view when it can present `target`, otherwise the diff.
    fn for_target(self, target: Option<&FileTarget>) -> Self {
        let applies = match (self, target) {
            (Self::Diff, _) => true,
            (_, None) => false,
            (_, Some(target)) if target.file_key.is_empty() => false,
            (Self::History, _) => true,
            (Self::File, Some(target)) => target.status != "deleted",
            (Self::Blame, Some(target)) => {
                target.status != "deleted" && !supports_path(&target.path)
            }
        };
        if applies { self } else { Self::Diff }
    }
}

pub enum FileBrowserEvent {
    Close,
    NavigateCommit(String),
    FontSizeChanged(f32),
    LayoutChanged(DiffLayout),
    /// The inspector's patch was skipped: read it up to `PreviewLimit::MAX`.
    ShowLarge,
}

/// Changes of a file, newest first, listed from `root` one page at a time.
struct FileHistory {
    root: FileTarget,
    entries: Vec<FileHistoryEntry>,
    next_cursor: Option<String>,
    scanned: usize,
    /// Commits read since the last page that found a change.
    idle: usize,
    limitation: Option<String>,
    selected: Option<usize>,
}
impl FileHistory {
    fn new(root: FileTarget) -> Self {
        Self {
            root,
            entries: Vec::new(),
            next_cursor: None,
            scanned: 0,
            idle: 0,
            limitation: None,
            selected: None,
        }
    }
    /// The cursor of the next page; `None` once the history is complete.
    fn next_page(&self) -> Option<Option<String>> {
        if self.scanned == 0 {
            Some(None)
        } else {
            self.next_cursor.clone().map(Some)
        }
    }
    fn append(&mut self, page: FileHistoryPage) {
        self.idle = if page.entries.is_empty() {
            self.idle + page.scanned
        } else {
            0
        };
        self.scanned += page.scanned;
        self.entries.extend(page.entries);
        self.next_cursor = page.next_cursor;
        self.limitation = page.limitation;
        let full = self.entries.len() == MAX_HISTORY_ENTRIES && self.next_cursor.is_some();
        if full || self.entries.len() > MAX_HISTORY_ENTRIES {
            self.entries.truncate(MAX_HISTORY_ENTRIES);
            self.next_cursor = None;
            self.limitation = Some(format!(
                "Historique limité aux {} changements les plus récents.",
                format_count(MAX_HISTORY_ENTRIES)
            ));
        }
    }
    /// Whether reaching the end of the list reads the next page by itself.
    /// A long run of commits without change waits for an explicit request.
    fn reads_on(&self) -> bool {
        self.next_cursor.is_some() && self.idle < AUTO_SCAN
    }
    fn target(&self, index: usize) -> Option<FileTarget> {
        let entry = self.entries.get(index)?;
        Some(FileTarget {
            session_id: self.root.session_id.clone(),
            sha: entry.sha.clone(),
            file_key: entry.file_key.clone(),
            path: entry.path.clone(),
            old_path: entry.old_path.clone(),
            status: entry.status.clone(),
        })
    }
    fn position(&self, target: &FileTarget) -> Option<usize> {
        self.entries
            .iter()
            .position(|change| target.is_change(change))
    }
    /// The history lists this revision: its root or one of its changes.
    fn lists(&self, target: &FileTarget) -> bool {
        self.root.same_revision(target) || self.position(target).is_some()
    }
}

enum ReadKind {
    Diff(PreviewLimit),
    Content {
        blame: bool,
        existing: Option<Arc<FileContent>>,
        limit: PreviewLimit,
    },
    /// A page of the history, after the cursor.
    History(Option<String>),
    Inspect(String),
}
struct ReadRequest {
    generation: u64,
    target: FileTarget,
    kind: ReadKind,
}
enum ReadResult {
    Diff(Arc<FileDiff>),
    Content(Arc<FileContent>, Option<Arc<FileBlame>>),
    History(FileHistoryPage),
    Inspect(Arc<CommitDetails>),
}

struct ColorRequest {
    generation: u64,
    target: FileTarget,
    diff: Arc<FileDiff>,
}

pub struct FileBrowser {
    engine: Arc<Workspace>,
    gate: ReadGate,
    source: FilePresentation,
    target: Option<FileTarget>,
    mode: FileView,
    /// The last view chosen by the user, restored on each new file when it applies.
    preferred: FileView,
    generation: u64,
    live_generation: Arc<AtomicU64>,
    color_active: bool,
    color_pending: Option<ColorRequest>,
    color_task: Option<Task<()>>,
    content: Option<Arc<FileContent>>,
    diff: Option<Arc<FileDiff>>,
    diff_highlights: Option<Highlights>,
    /// The user asked to read this target up to `PreviewLimit::MAX`.
    show_large: bool,
    blame: Option<Arc<FileBlame>>,
    history: Option<FileHistory>,
    /// History pages have their own generation: choosing a change keeps them.
    history_generation: u64,
    live_history: Arc<AtomicU64>,
    history_loading: bool,
    history_error: Option<String>,
    pending_history: Option<ReadRequest>,
    /// The view of the change selected in the history.
    detail: FileView,
    inspected: Option<Arc<CommitDetails>>,
    inspection_sha: Option<String>,
    inspection_error: Option<String>,
    error: Option<String>,
    loading: bool,
    pending: Option<ReadRequest>,
    pending_inspection: Option<ReadRequest>,
    active: bool,
    /// Shown in the central view; a hidden browser starts no file reads.
    visible: bool,
    task: Option<Task<()>>,
    viewer: Entity<CodeView>,
    image_preview: Entity<ImagePreview>,
    viewer_dirty: bool,
    palette: Palette,
    preferences: Preferences,
    history_scroll: UniformListScrollHandle,
    history_focus: FocusHandle,
    inspection_scroll: ScrollHandle,
    subscriptions: Vec<Subscription>,
    focus: FocusHandle,
    #[cfg(feature = "app-benchmark")]
    benchmark_scroll: f32,
}
impl EventEmitter<FileBrowserEvent> for FileBrowser {}

impl FileBrowser {
    /// Keys move through the shown code, or through the history's changes.
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        if self.mode == FileView::History {
            self.history_focus.focus(window, cx);
        } else {
            self.viewer.read(cx).focus_handle(cx).focus(window, cx);
        }
    }
    pub fn new(
        engine: Arc<Workspace>,
        gate: ReadGate,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let viewer = cx.new(|cx| {
            let mut viewer = CodeView::new(window, cx);
            viewer.set_split_host(true, cx);
            viewer
        });
        let image_preview = cx.new(|_| ImagePreview::new(engine.clone(), gate.clone()));
        let subscription = cx.subscribe(&viewer, |view, _, event, cx| match event {
            CodeViewEvent::NavigateCommit(sha) => view.inspect_commit(sha.clone(), cx),
            CodeViewEvent::FontSizeChanged(size) => {
                cx.emit(FileBrowserEvent::FontSizeChanged(*size))
            }
            CodeViewEvent::LayoutChanged(layout) => {
                cx.emit(FileBrowserEvent::LayoutChanged(*layout))
            }
            // The header shows the layout toggle.
            CodeViewEvent::LayoutSwitched => cx.notify(),
            CodeViewEvent::Parent => view.parent_version(cx),
            CodeViewEvent::ShowLarge => view.show_large(cx),
        });
        Self {
            engine,
            gate,
            source: FilePresentation::empty(0),
            target: None,
            mode: FileView::Diff,
            preferred: FileView::Diff,
            generation: 0,
            live_generation: Arc::new(AtomicU64::new(0)),
            color_active: false,
            color_pending: None,
            color_task: None,
            content: None,
            diff: None,
            diff_highlights: None,
            show_large: false,
            blame: None,
            history: None,
            history_generation: 0,
            live_history: Arc::new(AtomicU64::new(0)),
            history_loading: false,
            history_error: None,
            pending_history: None,
            detail: FileView::Diff,
            inspected: None,
            inspection_sha: None,
            inspection_error: None,
            error: None,
            loading: false,
            pending: None,
            pending_inspection: None,
            active: false,
            visible: false,
            task: None,
            viewer,
            image_preview,
            viewer_dirty: true,
            palette: Palette::default(),
            preferences: Preferences::default(),
            history_scroll: UniformListScrollHandle::new(),
            history_focus: cx.focus_handle().tab_stop(true),
            inspection_scroll: ScrollHandle::new(),
            subscriptions: vec![subscription],
            focus: cx.focus_handle(),
            #[cfg(feature = "app-benchmark")]
            benchmark_scroll: 0.,
        }
    }

    pub fn present(&mut self, presentation: FilePresentation, cx: &mut Context<Self>) {
        let was_loading = self.source.loading_source();
        let change = self.source.accept(presentation);
        match change {
            PresentationChange::Ignored => return,
            PresentationChange::Rekeyed => {
                if let Some(session) = self.source.target.as_ref().map(|t| t.session_id.clone()) {
                    self.rekey(&session, cx);
                }
            }
            _ => {}
        }
        let target = self.source.target.clone();
        let diff = self
            .source
            .prepared
            .as_ref()
            .map(|prepared| prepared.diff.clone());
        if change == PresentationChange::Replaced {
            self.select_target(target.clone(), diff.clone(), cx);
            self.mode = self.preferred.for_target(target.as_ref());
        }
        if self.target == target {
            self.diff = diff;
            self.diff_highlights = self.source.highlights();
            if self.view() == FileView::Diff {
                self.error = self.source.error();
            }
            self.viewer_dirty = true;
        }
        // A retained File, History or Blame view reads once its source is ready.
        if (was_loading || change == PresentationChange::Replaced) && self.visible {
            self.load_retained_view(cx);
        }
        cx.notify();
    }
    /// Follows the shown revisions into the new session of a refreshed
    /// repository. Reads queued or running for the old session start again.
    pub fn rekey(&mut self, session_id: &str, cx: &mut Context<Self>) {
        for target in [
            self.source.target.as_mut(),
            self.target.as_mut(),
            self.history.as_mut().map(|history| &mut history.root),
        ]
        .into_iter()
        .flatten()
        {
            target.session_id = session_id.to_owned();
        }
        // Reads that failed with the closed session are read again too.
        let failed = self.error.take().is_some();
        let reading = (self.loading || failed) && self.target != self.source.target;
        let paging = self.history_loading || self.history_error.is_some();
        let inspecting = self.inspection_sha.take();
        self.cancel_reads();
        self.cancel_history_reads();
        self.pending_inspection = None;
        // A hidden view reads once shown; the end of the list asks for its page.
        if !self.visible {
            return;
        }
        if reading || !self.source.loading_source() {
            self.load(cx);
        }
        if paging {
            self.load_more(cx);
        }
        if let Some(sha) = inspecting {
            self.inspect_commit(sha, cx);
        }
        if self.diff.is_some() && self.diff_highlights.is_none() {
            self.resume_diff_color(cx);
        }
    }
    /// The view of the shown revision: the history shows its selected change.
    fn view(&self) -> FileView {
        match self.mode {
            FileView::History => self.detail.for_target(self.target.as_ref()),
            mode => mode,
        }
    }
    /// The history has a change to show beside its list.
    fn shows_change(&self) -> bool {
        self.mode != FileView::History
            || self
                .history
                .as_ref()
                .is_some_and(|history| history.selected.is_some())
    }
    pub fn reading_position(&self, cx: &App) -> Option<ReadingPosition> {
        self.viewer.read(cx).reading_position()
    }
    pub fn restore_reading_position(&mut self, position: ReadingPosition, cx: &mut App) {
        self.viewer
            .update(cx, |viewer, _| viewer.restore_reading_position(position));
    }
    pub fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if self.visible == visible {
            return;
        }
        self.visible = visible;
        if visible {
            self.load_retained_view(cx);
        }
    }
    fn load_retained_view(&mut self, cx: &mut Context<Self>) {
        if self.mode != FileView::Diff
            && !self.source.loading_source()
            && self.target == self.source.target
        {
            self.load(cx);
        }
    }
    pub fn set_theme(&mut self, palette: Palette, cx: &mut Context<Self>) {
        if self.palette == palette {
            return;
        }
        self.palette = palette;
        self.viewer.update(cx, |v, cx| v.set_theme(palette, cx));
        cx.notify();
    }
    pub fn set_preferences(&mut self, preferences: Preferences, cx: &mut Context<Self>) {
        let old = self.preferences.preview_limit();
        self.apply_preferences(preferences, cx);
        if old != self.preferences.preview_limit() {
            self.limit_changed(old, cx);
        }
    }
    fn apply_preferences(&mut self, preferences: Preferences, cx: &mut Context<Self>) {
        if self.preferences.same_appearance(&preferences) {
            self.preferences = preferences;
            return;
        }
        self.preferences = preferences.clone();
        if !preferences.show_image_previews {
            self.image_preview
                .update(cx, |view, cx| view.present(None, self.generation, cx));
        }
        self.viewer_dirty = true;
        self.viewer
            .update(cx, |v, cx| v.set_preferences(preferences, cx));
        cx.notify();
    }
    pub fn font_controls(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        self.viewer
            .update(cx, |view, cx| view.font_controls(window, cx))
    }
    pub fn jump_hunk(&mut self, forward: bool, cx: &mut Context<Self>) {
        if self.view() == FileView::Diff && self.shows_change() {
            self.viewer
                .update(cx, |view, cx| view.jump_hunk(forward, cx));
        }
    }
    #[cfg(feature = "app-benchmark")]
    pub fn benchmark_readiness(&self, cx: &App) -> (bool, bool, usize) {
        let viewer = self.viewer.read(cx);
        (
            !self.viewer_dirty
                && !self.loading
                && !self.source.loading_source()
                && viewer.is_ready(),
            viewer.syntax_colored(),
            self.diff
                .as_ref()
                .map(|diff| diff.hunks.iter().map(|h| h.lines.len()).sum())
                .unwrap_or(0),
        )
    }
    #[cfg(feature = "app-benchmark")]
    pub fn benchmark_split(&self, cx: &App) -> bool {
        self.viewer.read(cx).is_split()
    }
    #[cfg(feature = "app-benchmark")]
    pub fn benchmark_scroll_to(&mut self, offset: f32, cx: &mut Context<Self>) {
        let delta = offset - self.benchmark_scroll;
        self.benchmark_scroll = offset;
        self.viewer
            .update(cx, |view, cx| view.scroll_by_pixels(delta, cx));
        cx.notify();
    }
    /// Cancels the reads of the shown revision before it shows another view.
    fn cancel_reads(&mut self) {
        self.generation += 1;
        self.live_generation
            .store(self.generation, Ordering::Release);
        self.color_pending = None;
        self.pending = None;
        self.error = None;
        self.loading = false;
        self.viewer_dirty = true;
    }
    fn cancel_history_reads(&mut self) {
        self.history_generation += 1;
        self.live_history
            .store(self.history_generation, Ordering::Release);
        self.pending_history = None;
        self.history_loading = false;
        self.history_error = None;
    }
    /// Shows another file or source; its history is listed again once shown.
    fn select_target(
        &mut self,
        target: Option<FileTarget>,
        diff: Option<Arc<FileDiff>>,
        cx: &mut Context<Self>,
    ) {
        self.change_revision(target, diff, cx);
        self.cancel_history_reads();
        self.history = None;
        self.history_scroll = UniformListScrollHandle::new();
    }
    /// Shows another revision of the file; the listed history stays.
    fn change_revision(
        &mut self,
        target: Option<FileTarget>,
        diff: Option<Arc<FileDiff>>,
        cx: &mut Context<Self>,
    ) {
        self.cancel_reads();
        self.image_preview
            .update(cx, |view, cx| view.present(None, self.generation, cx));
        self.target = target;
        self.diff = diff;
        self.diff_highlights = None;
        self.show_large = false;
        self.content = None;
        self.blame = None;
        self.inspected = None;
        self.inspection_sha = None;
        self.inspection_error = None;
        self.pending_inspection = None;
        #[cfg(feature = "app-benchmark")]
        {
            self.benchmark_scroll = 0.;
        }
        cx.notify();
    }
    fn restore(&mut self, cx: &mut Context<Self>) {
        if self.loading {
            return;
        }
        let mode = self.mode;
        let target = self.source.target.clone();
        let prepared = self.source.prepared.clone();
        self.select_target(
            target,
            prepared.as_ref().map(|prepared| prepared.diff.clone()),
            cx,
        );
        self.diff_highlights = self.source.highlights();
        self.show(mode, cx);
    }
    pub fn show(&mut self, mode: FileView, cx: &mut Context<Self>) {
        if self.target.as_ref().is_none_or(|t| t.file_key.is_empty())
            || self.source.loading_source()
        {
            return;
        }
        self.preferred = mode;
        if self.mode == mode && self.loading {
            return;
        }
        self.cancel_reads();
        self.close_inspection(cx);
        self.mode = mode;
        self.load(cx);
    }
    fn load(&mut self, cx: &mut Context<Self>) {
        let Some(target) = self.target.clone() else {
            return;
        };
        if self.mode == FileView::History {
            match &self.history {
                Some(history) if history.lists(&target) => {
                    if history.selected.is_none() {
                        // The list reads on until it can select a change.
                        if history.entries.is_empty() && history.reads_on() || history.scanned == 0
                        {
                            self.load_more(cx);
                        }
                        cx.notify();
                        return;
                    }
                }
                _ => {
                    self.history = Some(FileHistory::new(target));
                    self.history_scroll = UniformListScrollHandle::new();
                    self.load_more(cx);
                    cx.notify();
                    return;
                }
            }
        }
        // The inspector reads the patch of its own revision.
        if self.view() == FileView::Diff
            && self.diff.is_none()
            && self.source.loading_source()
            && self.target == self.source.target
        {
            cx.notify();
            return;
        }
        let limit = if self.show_large {
            PreviewLimit::MAX
        } else {
            self.preferences.preview_limit()
        };
        let kind = match self.view() {
            FileView::Diff if self.diff.is_none() => ReadKind::Diff(limit),
            FileView::Diff => {
                // Switching tabs cancels the old generation. Resume coloring a
                // retained revision patch when returning before syntax finished.
                self.resume_diff_color(cx);
                cx.notify();
                return;
            }
            FileView::File if self.content.is_none() => ReadKind::Content {
                blame: false,
                existing: self.content.clone(),
                limit,
            },
            FileView::Blame if self.blame.is_none() => ReadKind::Content {
                blame: true,
                existing: self.content.clone(),
                limit,
            },
            _ => {
                cx.notify();
                return;
            }
        };
        self.loading = true;
        self.error = None;
        self.pending = Some(ReadRequest {
            generation: self.generation,
            target,
            kind,
        });
        self.viewer_dirty = true;
        self.pump(cx);
        cx.notify();
    }
    /// Reads the history's next page, the first one included.
    fn load_more(&mut self, cx: &mut Context<Self>) {
        if self.history_loading {
            return;
        }
        let Some((root, Some(cursor))) = self
            .history
            .as_ref()
            .map(|history| (history.root.clone(), history.next_page()))
        else {
            return;
        };
        self.history_loading = true;
        self.history_error = None;
        self.pending_history = Some(ReadRequest {
            generation: self.history_generation,
            target: root,
            kind: ReadKind::History(cursor),
        });
        self.pump(cx);
    }
    /// After commits without any change, the user asks to read further.
    fn read_further(&mut self, cx: &mut Context<Self>) {
        if let Some(history) = &mut self.history {
            history.idle = 0;
        }
        self.history_error = None;
        self.load_more(cx);
        cx.notify();
    }
    fn append_history(&mut self, page: FileHistoryPage, cx: &mut Context<Self>) {
        let Some(history) = &mut self.history else {
            return;
        };
        history.append(page);
        if history.selected.is_none() {
            // The shown revision when it is a change, otherwise the newest one.
            let index = self
                .target
                .as_ref()
                .and_then(|target| history.position(target))
                .or((!history.entries.is_empty()).then_some(0));
            match index {
                Some(index) => self.select_change(index, cx),
                None if history.entries.is_empty() && history.reads_on() => self.load_more(cx),
                None => {}
            }
        }
        self.viewer_dirty = true;
        cx.notify();
    }
    fn select_change(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(history) = &mut self.history else {
            return;
        };
        let Some(target) = history.target(index) else {
            return;
        };
        history.selected = Some(index);
        self.history_scroll
            .scroll_to_item(index, ScrollStrategy::Nearest);
        if self.target.as_ref() != Some(&target) {
            // The inspector's revision keeps its prepared patch.
            let source = (self.source.target.as_ref() == Some(&target))
                .then(|| self.source.prepared.clone())
                .flatten();
            self.change_revision(
                Some(target),
                source.as_ref().map(|prepared| prepared.diff.clone()),
                cx,
            );
            self.diff_highlights = source.and_then(|_| self.source.highlights());
        }
        self.load(cx);
    }
    fn move_change(&mut self, step: isize, cx: &mut Context<Self>) {
        let Some(history) = &self.history else {
            return;
        };
        if history.entries.is_empty() {
            return;
        }
        let last = history.entries.len() - 1;
        let index = match history.selected {
            Some(selected) => selected.saturating_add_signed(step).min(last),
            None => 0,
        };
        let reads_on = index == last && history.reads_on();
        if Some(index) != history.selected {
            self.select_change(index, cx);
        }
        // The last change reads the next page, as scrolling to it would.
        if reads_on {
            self.load_more(cx);
        }
    }
    fn show_detail(&mut self, view: FileView, cx: &mut Context<Self>) {
        if self.detail == view {
            return;
        }
        self.detail = view;
        self.cancel_reads();
        self.load(cx);
    }
    /// Reads the shown file again up to the highest preview limit.
    fn show_large(&mut self, cx: &mut Context<Self>) {
        if self.view() == FileView::Diff
            && self
                .diff
                .as_ref()
                .is_some_and(|diff| self.source.owns_diff(diff))
        {
            // The inspector reads the selected commit's own patch.
            cx.emit(FileBrowserEvent::ShowLarge);
            return;
        }
        self.show_large = true;
        let diff = self
            .diff
            .as_ref()
            .is_some_and(|diff| diff.oversized.is_some() && !self.source.owns_diff(diff));
        let content = self
            .content
            .as_ref()
            .is_some_and(|content| content.oversized.is_some());
        self.read_again(diff, content, cx);
    }
    /// Reads again what the old limit could have answered differently. Blame
    /// keeps its own bound and is read again only with new contents.
    fn limit_changed(&mut self, old: PreviewLimit, cx: &mut Context<Self>) {
        if self.show_large {
            return;
        }
        let new = self.preferences.preview_limit();
        // A read left whole under one limit stays whole under a higher one.
        let diff = self.diff.as_ref().is_some_and(|diff| {
            !self.source.owns_diff(diff) && (diff.oversized.is_some() || new < old)
        });
        let content = self.content.as_ref().is_some_and(|content| {
            content.oversized.is_some() || content.size as u64 > new.bytes()
        });
        self.read_again(diff, content, cx);
    }
    /// Drops the stale diff and contents, then restarts the view's read if
    /// it showed them or was still reading under the previous limit. Other
    /// views read again once shown.
    fn read_again(&mut self, diff: bool, content: bool, cx: &mut Context<Self>) {
        if diff {
            self.diff = None;
            self.diff_highlights = None;
        }
        if content {
            self.content = None;
            self.blame = None;
        }
        let (stale, reading) = match self.view() {
            FileView::Diff => (diff, self.diff.is_none()),
            FileView::File | FileView::Blame => (content, self.content.is_none()),
            FileView::History => (false, false),
        };
        if stale || self.loading && reading {
            self.loading = false;
            self.show(self.mode, cx);
        }
    }
    fn parent_version(&mut self, cx: &mut Context<Self>) {
        let (Some(target), Some(parent)) = (
            self.target.clone(),
            self.content.as_ref().and_then(|c| c.parent_sha.clone()),
        ) else {
            return;
        };
        let mode = self.mode;
        self.select_target(
            Some(FileTarget {
                sha: parent,
                status: "unmodified".into(),
                old_path: None,
                ..target
            }),
            None,
            cx,
        );
        self.show(mode, cx);
    }
    fn inspect_commit(&mut self, sha: String, cx: &mut Context<Self>) {
        if self.inspected.as_ref().is_some_and(|c| c.sha == sha)
            || self.inspection_sha.as_ref() == Some(&sha)
        {
            return;
        }
        let Some(target) = self.target.clone() else {
            return;
        };
        self.inspected = None;
        self.inspection_error = None;
        self.inspection_sha = Some(sha.clone());
        self.pending_inspection = Some(ReadRequest {
            generation: self.generation,
            target,
            kind: ReadKind::Inspect(sha),
        });
        self.pump(cx);
        cx.notify();
    }
    fn close_inspection(&mut self, cx: &mut Context<Self>) {
        self.inspected = None;
        self.inspection_error = None;
        self.inspection_sha = None;
        self.pending_inspection = None;
        cx.notify();
    }
    fn pump(&mut self, cx: &mut Context<Self>) {
        if self.active {
            return;
        }
        let Some(request) = self
            .pending
            .take()
            .or_else(|| self.pending_history.take())
            .or_else(|| self.pending_inspection.take())
        else {
            return;
        };
        self.active = true;
        let engine = self.engine.clone();
        let gate = self.gate.clone();
        let live_generation = if matches!(request.kind, ReadKind::History(_)) {
            self.live_history.clone()
        } else {
            self.live_generation.clone()
        };
        self.task = Some(cx.spawn(async move |this, cx| {
            let (request, result) = cx
                .background_executor()
                .spawn(async move {
                    if live_generation.load(Ordering::Acquire) != request.generation {
                        return (request, None);
                    }
                    #[cfg(feature = "app-benchmark")]
                    let gate_start = std::time::Instant::now();
                    let _guard = gate.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                    #[cfg(feature = "app-benchmark")]
                    crate::perf::record_operation("gate_wait", gate_start.elapsed());
                    if live_generation.load(Ordering::Acquire) != request.generation {
                        return (request, None);
                    }
                    #[cfg(feature = "app-benchmark")]
                    let read_start = std::time::Instant::now();
                    #[cfg(feature = "app-benchmark")]
                    crate::perf::record_count("reads_started");
                    let t = &request.target;
                    let result: Result<ReadResult, String> = (|| {
                        Ok(match &request.kind {
                            ReadKind::Diff(limit) => {
                                let diff = Arc::new(engine.file_revision_diff(
                                    &t.session_id,
                                    &t.sha,
                                    &t.file_key,
                                    *limit,
                                )?);
                                ReadResult::Diff(diff)
                            }
                            ReadKind::Content {
                                blame,
                                existing,
                                limit,
                            } => {
                                let content = match existing {
                                    Some(content) => content.clone(),
                                    None => Arc::new(engine.file_content(
                                        &t.session_id,
                                        &t.sha,
                                        &t.file_key,
                                        *limit,
                                    )?),
                                };
                                if live_generation.load(Ordering::Acquire) != request.generation {
                                    return Err(
                                        "Lecture remplacée par une sélection plus récente.".into(),
                                    );
                                }
                                let attribution = if *blame
                                    && content.kind == "text"
                                    && !content.lines.is_empty()
                                    && crate::code_view::blame_unavailable(&content).is_none()
                                {
                                    Some(Arc::new(engine.file_blame(
                                        &t.session_id,
                                        &t.sha,
                                        &t.file_key,
                                    )?))
                                } else {
                                    None
                                };
                                ReadResult::Content(content, attribution)
                            }
                            ReadKind::History(cursor) => ReadResult::History(engine.file_history(
                                &t.session_id,
                                &t.sha,
                                &t.file_key,
                                cursor.as_deref(),
                            )?),
                            ReadKind::Inspect(sha) => ReadResult::Inspect(
                                engine.commit_details_shared(&t.session_id, sha)?,
                            ),
                        })
                    })();
                    #[cfg(feature = "app-benchmark")]
                    {
                        let stage = match &request.kind {
                            ReadKind::Diff(_) => "patch_git",
                            ReadKind::Inspect(_) => "details_git",
                            _ => "file_git",
                        };
                        crate::perf::record_operation(stage, read_start.elapsed());
                    }
                    let result = (live_generation.load(Ordering::Acquire) == request.generation)
                        .then_some(result);
                    (request, result)
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.active = false;
                #[cfg(feature = "app-benchmark")]
                if request.generation != view.generation || result.is_none() {
                    crate::perf::record_count("stale_reads");
                }
                if let ReadKind::History(_) = &request.kind {
                    if request.generation == view.history_generation
                        && view
                            .history
                            .as_ref()
                            .is_some_and(|history| history.root == request.target)
                        && let Some(result) = result
                    {
                        view.history_loading = false;
                        match result {
                            Ok(ReadResult::History(page)) => view.append_history(page, cx),
                            Err(e) => view.history_error = Some(e),
                            _ => {}
                        }
                        cx.notify();
                    }
                } else if request.generation == view.generation
                    && view.target.as_ref() == Some(&request.target)
                    && let Some(result) = result
                {
                    let inspection = matches!(&request.kind, ReadKind::Inspect(_));
                    if inspection {
                        if let ReadKind::Inspect(sha) = &request.kind
                            && view.inspection_sha.as_ref() == Some(sha)
                        {
                            view.inspection_sha = None;
                            match result {
                                Ok(ReadResult::Inspect(details)) => view.inspected = Some(details),
                                Err(e) => view.inspection_error = Some(e),
                                _ => {}
                            }
                        }
                    } else {
                        view.loading = false;
                        match result {
                            Ok(ReadResult::Diff(diff)) => {
                                view.diff = Some(diff);
                                view.diff_highlights = None;
                                view.resume_diff_color(cx);
                            }
                            Ok(ReadResult::Content(content, blame)) => {
                                view.content = Some(content);
                                view.blame = blame;
                            }
                            Err(e) => view.error = Some(e),
                            _ => {}
                        }
                        view.viewer_dirty = true;
                    }
                    cx.notify();
                }
                view.pump(cx);
            });
        }));
    }
    fn resume_diff_color(&mut self, cx: &mut Context<Self>) {
        let (Some(target), Some(diff)) = (&self.target, &self.diff) else {
            return;
        };
        if self.diff_highlights.is_some() || self.source.owns_diff(diff) {
            // The inspector owns the raw source patch's separate syntax lane.
            return;
        }
        self.color_pending = Some(ColorRequest {
            generation: self.generation,
            target: target.clone(),
            diff: diff.clone(),
        });
        self.pump_color(cx);
    }
    fn pump_color(&mut self, cx: &mut Context<Self>) {
        if self.color_active {
            return;
        }
        let Some(request) = self.color_pending.take() else {
            return;
        };
        self.color_active = true;
        let live_generation = self.live_generation.clone();
        self.color_task = Some(cx.spawn(async move |this, cx| {
            let (request, highlights) = cx
                .background_executor()
                .spawn(async move {
                    let highlights = Highlights::of_diff(
                        &request.target.path,
                        &request.diff,
                        request.diff.sources.as_deref(),
                        || live_generation.load(Ordering::Acquire) != request.generation,
                    );
                    (request, highlights)
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.color_active = false;
                if request.generation == view.generation
                    && view.target.as_ref() == Some(&request.target)
                    && let Some(highlights) = highlights
                {
                    view.diff_highlights = Some(highlights);
                    view.viewer_dirty = true;
                    cx.notify();
                } else {
                    #[cfg(feature = "app-benchmark")]
                    crate::perf::record_count("stale_colors");
                }
                view.pump_color(cx);
            });
        }));
    }
    fn sync_viewer(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.viewer_dirty {
            return;
        }
        let view = self.view();
        let shown = self.shows_change();
        let image_target = self
            .target
            .as_ref()
            .filter(|target| {
                self.preferences.show_image_previews
                    && supports_path(&target.path)
                    && matches!(view, FileView::Diff | FileView::File)
                    && shown
                    && !self.source.loading_source()
                    && self.error.is_none()
            })
            .map(|target| ImageTarget {
                session_id: target.session_id.clone(),
                source: FileImageSource::Commit {
                    sha: target.sha.clone(),
                    file_key: target.file_key.clone(),
                    comparison: view == FileView::Diff,
                },
            });
        self.image_preview.update(cx, |view, cx| {
            view.set_style(&self.preferences, false, cx);
            view.present(image_target, self.generation, cx);
        });
        self.viewer_dirty = false;
        let mode = view;
        let diff = self.diff.clone().filter(|_| shown);
        let diff_highlights = self.diff_highlights.clone();
        let content = self.content.clone().filter(|_| shown);
        let blame = self.blame.clone();
        let path = self
            .target
            .as_ref()
            .map(|t| t.path.clone())
            .unwrap_or_default();
        let loading = (self.loading || self.source.loading_source() || !shown)
            .then(|| reading_label(if shown { mode } else { FileView::History }, &path));
        let error = self.error.clone();
        #[cfg(feature = "app-benchmark")]
        let prepare_start = std::time::Instant::now();
        self.viewer.update(cx, |v, cx| {
            match mode {
                FileView::Diff => {
                    if let Some(diff) = diff {
                        // Raw patches stay readable without starting a second
                        // CodeView syntax job while the browser/inspector colors.
                        v.set_prepared_diff(
                            diff,
                            path,
                            diff_highlights.unwrap_or_default(),
                            window,
                            cx,
                        );
                    } else {
                        v.clear(cx);
                    }
                }
                FileView::File | FileView::Blame => {
                    if let Some(content) = content {
                        v.set_content(content, window, cx);
                        v.set_blame(blame, cx);
                        v.set_show_blame(mode == FileView::Blame, cx);
                    } else {
                        v.clear(cx);
                    }
                }
                FileView::History => {}
            }
            v.set_loading(loading, cx);
            v.set_error(error, cx);
        });
        #[cfg(feature = "app-benchmark")]
        crate::perf::record_operation("ui_prepare", prepare_start.elapsed());
    }
    fn history_row_height(&self) -> f32 {
        52. + self.preferences.font_size_offset.max(0.) * 2.
    }
    /// Changes in `range`, then the end of the list, which reads the next page.
    fn history_rows(&mut self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let count = self
            .history
            .as_ref()
            .map_or(0, |history| history.entries.len());
        if range.end > count
            && !self.history_loading
            && self.history_error.is_none()
            && self.history.as_ref().is_some_and(FileHistory::reads_on)
        {
            self.load_more(cx);
        }
        let Some(history) = &self.history else {
            return Vec::new();
        };
        let p = self.palette;
        let o = self.preferences.font_size_offset;
        let height = self.history_row_height();
        range
            .map(|index| {
                let Some(entry) = history.entries.get(index) else {
                    return self.history_end(history, height, cx);
                };
                let selected = history.selected == Some(index);
                let mut details = vec![entry.author.clone(), compact_date(entry.time)];
                if let Some(old) = &entry.old_path {
                    details.insert(0, format!("Renommé depuis {}", file_name(old)));
                }
                if entry.merge {
                    details.push("fusion".into());
                }
                div()
                    .id(("file-history", index))
                    .role(Role::ListBoxOption)
                    .aria_label(entry.summary.clone())
                    .aria_description(details.join(", "))
                    .aria_selected(selected)
                    .aria_position_in_set(index + 1)
                    .aria_size_of_set(history.entries.len())
                    .when(selected, |row| row.aria_active_descendant())
                    .h(px(height))
                    .w_full()
                    .flex()
                    .items_center()
                    .gap(px(9.))
                    .px(px(12.))
                    .relative()
                    .border_b_1()
                    .border_color(rgb(if selected { p.accent_border } else { p.border }))
                    .bg(rgb(if selected { p.selection } else { p.reading_bg }))
                    .cursor_pointer()
                    .hover(move |s| s.bg(rgb(if selected { p.selection } else { p.hover })))
                    // As in the repository history, with a bar that stays visible in light themes.
                    .when(selected, |row| {
                        row.child(
                            div()
                                .absolute()
                                .left_0()
                                .right_0()
                                .top_0()
                                .h(px(1.))
                                .bg(rgb(p.accent_border)),
                        )
                        .child(
                            div()
                                .absolute()
                                .left_0()
                                .top_0()
                                .bottom_0()
                                .w(px(3.))
                                .bg(rgb(p.accent)),
                        )
                    })
                    .child(status_icon(&entry.status, p))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(3.))
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(12. + o))
                                    .child(entry.summary.clone()),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(11. + o))
                                    .text_color(rgb(p.muted))
                                    .child(format!(
                                        "{} · {}",
                                        details.join(" · "),
                                        short_sha(&entry.sha)
                                    )),
                            ),
                    )
                    .on_click(cx.listener(move |view, _, window, cx| {
                        view.history_focus.focus(window, cx);
                        view.select_change(index, cx);
                    }))
                    .into_any_element()
            })
            .collect()
    }
    /// What remains to read, or why the history ends.
    fn history_end(
        &self,
        history: &FileHistory,
        height: f32,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = self.palette;
        let o = self.preferences.font_size_offset;
        let scanned = count_label(history.scanned, "commit parcouru", "commits parcourus");
        let (text, button): (String, Option<(&str, bool)>) = if self.history_loading {
            ("Lecture de l’historique…".into(), None)
        } else if self.history_error.is_some() {
            ("Échec de la lecture.".into(), Some(("Réessayer", false)))
        } else if history.next_cursor.is_some() {
            (
                format!("{} commits sans changement.", format_count(history.idle)),
                Some(("Continuer", true)),
            )
        } else if history.entries.is_empty() {
            (format!("Aucun changement trouvé · {scanned}"), None)
        } else {
            (
                format!(
                    "{} · {scanned}",
                    count_label(history.entries.len(), "changement", "changements")
                ),
                None,
            )
        };
        div()
            .id("file-history-end")
            .h(px(height))
            .w_full()
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(12.))
            .text_size(px(11. + o))
            .text_color(rgb(p.muted))
            .child(div().flex_1().min_w_0().line_clamp(3).child(text))
            .children(button.map(|(label, further)| {
                action("file-history-more", label, false, false, p, o)
                    .flex_shrink_0()
                    .on_click(cx.listener(move |view, _, _, cx| {
                        if further {
                            view.read_further(cx);
                        } else {
                            view.history_error = None;
                            view.load_more(cx);
                            cx.notify();
                        }
                    }))
            }))
            .into_any_element()
    }
    fn history_key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        if keystroke.modifiers.platform || keystroke.modifiers.control || keystroke.modifiers.alt {
            return;
        }
        let page = (self
            .history_scroll
            .0
            .borrow()
            .base_handle
            .bounds()
            .size
            .height
            / px(self.history_row_height()))
        .floor()
        .max(1.) as isize;
        let step = match keystroke.key.as_str() {
            "up" => -1,
            "down" => 1,
            "pageup" => -page,
            "pagedown" => page,
            "home" => isize::MIN / 2,
            "end" => isize::MAX / 2,
            _ => return,
        };
        self.move_change(step, cx);
        cx.stop_propagation();
    }
}

impl FileBrowser {
    fn render_history_list(&mut self, cx: &mut Context<Self>) -> Div {
        let p = self.palette;
        let o = self.preferences.font_size_offset;
        let count = self
            .history
            .as_ref()
            .map_or(0, |history| history.entries.len());
        let rows = count + usize::from(self.history.is_some());
        let heading = match &self.history {
            Some(history) if history.scanned > 0 || self.history_error.is_some() => {
                let total = count_label(count, "changement", "changements");
                match (count, history.next_cursor.is_some()) {
                    (0, true) => "Aucun changement trouvé pour l’instant".into(),
                    (_, true) => format!("Au moins {total}"),
                    _ => total,
                }
            }
            _ => "Lecture…".into(),
        };
        // Errors and limits can be long: they wrap above the list rather than in a row.
        let warning = match (&self.history_error, &self.history) {
            (Some(error), _) => Some((error.clone(), p.danger)),
            (None, Some(history)) => history.limitation.clone().map(|text| (text, p.muted)),
            _ => None,
        };
        let note = SharedString::from(
            "Premier parent uniquement aux fusions.\nLes renommages détectés sont suivis ; copies et renommages ambigus peuvent interrompre le suivi.",
        );
        div()
            .w(relative(0.34))
            .min_w(px(200.))
            .max_w(px(400.))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .border_r_1()
            .border_color(rgb(p.border))
            .child(
                div()
                    .h(px(32.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .px(px(12.))
                    .border_b_1()
                    .border_color(rgb(p.border))
                    .text_size(px(11. + o))
                    .text_color(rgb(p.muted))
                    .child(div().flex_1().min_w_0().truncate().child(heading))
                    .child(
                        div()
                            .id("file-history-note")
                            .role(Role::Image)
                            .aria_label(note.clone())
                            .tooltip(move |window, cx| Tooltip::new(note.clone()).build(window, cx))
                            .child(icon(IconName::Info, 13., p.muted)),
                    ),
            )
            .children(warning.map(|(text, color)| {
                notice(text, color, o)
                    .px(px(12.))
                    .border_b_1()
                    .border_color(rgb(p.border))
            }))
            .child(
                div()
                    .id("file-history")
                    .role(Role::ListBox)
                    .aria_label("Changements du fichier")
                    .track_focus(&self.history_focus)
                    .on_key_down(cx.listener(Self::history_key_down))
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .child(
                        uniform_list(
                            "file-history-rows",
                            rows,
                            cx.processor(|view, range: Range<usize>, _, cx| {
                                view.history_rows(range, cx)
                            }),
                        )
                        .size_full()
                        .track_scroll(&self.history_scroll),
                    )
                    .child(crate::theme::scrollbar_theme(
                        Scrollbar::vertical(&self.history_scroll).id("file-history-scrollbar"),
                    )),
            )
    }
    /// The selected change: its commit, its view and its patch or contents.
    fn render_change(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let p = self.palette;
        let o = self.preferences.font_size_offset;
        let mut pane = div().flex_1().min_w_0().flex().flex_col();
        let entry = self.history.as_ref().and_then(|history| {
            history
                .selected
                .and_then(|index| history.entries.get(index))
        });
        let Some(entry) = entry else {
            let path = self.target.as_ref().map_or("", |t| t.path.as_str());
            let message = match &self.history {
                _ if self.history_error.is_some() => "L’historique n’a pas pu être lu.".into(),
                Some(history) if !self.history_loading && history.scanned > 0 => {
                    if history.next_cursor.is_some() {
                        format!(
                            "Aucun changement dans les {} commits parcourus : « Continuer » poursuit la recherche.",
                            format_count(history.scanned)
                        )
                    } else {
                        "Aucun changement à afficher.".into()
                    }
                }
                // The list starts once the inspector's revision is ready.
                _ => reading_label(FileView::History, path),
            };
            return pane.child(
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .p(px(20.))
                    .text_size(px(12. + o))
                    .text_color(rgb(p.muted))
                    .child(div().w_full().text_center().child(message)),
            );
        };
        let sha = entry.sha.clone();
        let metadata = format!(
            "{} · {} · {}{}",
            entry.author,
            commit_date(entry.time),
            short_sha(&entry.sha),
            entry
                .old_path
                .as_ref()
                .map(|old| format!(" · {}", path_label(&entry.path, Some(old))))
                .unwrap_or_default()
        );
        let details = SharedString::from(format!("{}\n{metadata}", entry.summary));
        let file_available = FileView::File.for_target(self.target.as_ref()) == FileView::File;
        let view = self.view();
        pane = pane.child(
            div()
                .min_h(px(44.))
                .flex_shrink_0()
                .flex()
                .flex_wrap()
                .items_center()
                .gap_x(px(8.))
                .gap_y(px(4.))
                .px(px(12.))
                .py(px(6.))
                .border_b_1()
                .border_color(rgb(p.border))
                .child(status_icon(&entry.status, p))
                // The actions wrap below before the commit's details shrink much.
                .child(
                    div()
                        .id("history-change")
                        .flex_grow(1.)
                        .flex_shrink(1.)
                        .flex_basis(px(300.))
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .tooltip(move |window, cx| Tooltip::new(details.clone()).build(window, cx))
                        .child(
                            div()
                                .truncate()
                                .text_size(px(12. + o))
                                .child(entry.summary.clone()),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_size(px(11. + o))
                                .text_color(rgb(p.muted))
                                .child(metadata),
                        ),
                )
                .child(
                    div().flex().flex_shrink_0().gap(px(4.)).children(
                        [
                            (FileView::Diff, "Diff", true),
                            (FileView::File, "Fichier", file_available),
                        ]
                        .map(|(detail, label, available)| {
                            action(
                                ("history-detail", detail as usize),
                                label,
                                view == detail,
                                !available,
                                p,
                                o,
                            )
                            .on_click(cx.listener(
                                move |view, _, _, cx| {
                                    if available {
                                        view.show_detail(detail, cx)
                                    }
                                },
                            ))
                        }),
                    ),
                )
                .child(
                    action("history-commit", "Voir dans le dépôt", false, false, p, o)
                        .flex_shrink_0()
                        .on_click(cx.listener(move |_, _, _, cx| {
                            cx.emit(FileBrowserEvent::NavigateCommit(sha.clone()))
                        })),
                ),
        );
        if let Some(error) = &self.error {
            pane = pane.child(
                notice(error.clone(), p.danger, o).child(
                    action("retry-file", "Réessayer", false, self.loading, p, o)
                        .on_click(cx.listener(|v, _, _, cx| v.show(v.mode, cx))),
                ),
            );
        }
        let preview = if self.image_preview.read(cx).has_target() {
            render_cached_view(self.image_preview.clone(), window)
        } else {
            render_cached_view(self.viewer.clone(), window)
        };
        pane.child(div().flex_1().min_h_0().child(preview))
    }
}

fn file_name(path: &str) -> &str {
    path.rsplit('/').next().unwrap_or(path)
}

impl Render for FileBrowser {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let _keep_subscriptions = &self.subscriptions;
        self.sync_viewer(window, cx);
        let p = self.palette;
        let o = self.preferences.font_size_offset;
        let path = self
            .target
            .as_ref()
            .map(|t| path_label(&t.path, t.old_path.as_deref()))
            .unwrap_or_default();
        let sha = self
            .target
            .as_ref()
            .map(|t| short_sha(&t.sha))
            .unwrap_or_default();
        let busy = self.source.loading_source();
        // A history listed from the inspector's revision already leads back to it.
        let original = self.target == self.source.target
            || self.mode == FileView::History
                && self
                    .history
                    .as_ref()
                    .zip(self.source.target.as_ref())
                    .is_some_and(|(history, source)| history.root == *source);
        let controls = self.font_controls(window, cx);
        let diff_mode = self.view() == FileView::Diff && self.shows_change();
        let hunks = diff_mode.then(|| self.viewer.update(cx, |view, cx| view.hunk_controls(cx)));
        let layout = (diff_mode && !self.image_preview.read(cx).has_target())
            .then(|| {
                self.viewer
                    .update(cx, |view, cx| view.layout_controls(window, cx))
            })
            .flatten();
        let counts = self
            .diff
            .as_deref()
            .filter(|_| diff_mode)
            .and_then(line_counts)
            .map(|(additions, deletions)| diff_counts(additions, deletions, p, o));
        let mut root = div()
            .id("file-workspace")
            .role(Role::Region)
            .aria_label("Consultation du fichier sélectionné")
            .size_full()
            .min_w_0()
            .flex()
            .flex_col()
            .bg(rgb(p.reading_bg))
            .text_color(rgb(p.text))
            .track_focus(&self.focus)
            .on_key_down(cx.listener(|view, e: &KeyDownEvent, _, cx| {
                if e.keystroke.key == "escape" {
                    cx.emit(FileBrowserEvent::Close);
                    cx.stop_propagation();
                } else if let Some(forward) = hunk_shortcut(e) {
                    view.jump_hunk(forward, cx);
                    cx.stop_propagation();
                }
            }))
            .child(
                div()
                    .min_h(px(40.))
                    .flex_shrink_0()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_x(px(9.))
                    .gap_y(px(4.))
                    .px(px(16.))
                    .py(px(6.))
                    .bg(rgb(p.panel))
                    .border_b_1()
                    .border_color(rgb(p.border))
                    .child(icon(IconName::FileText, 15., p.muted))
                    // In a narrow view the controls wrap below rather than
                    // squeezing the path down to its ellipsis.
                    .child(
                        path_title("file-path", path, o)
                            .flex_1()
                            .min_w(px(160.))
                            .when(busy, |s| s.opacity(0.6)),
                    )
                    .children(counts)
                    .child(
                        div()
                            .font_family(MONO)
                            .text_size(px(11. + o))
                            .text_color(rgb(p.muted))
                            .when(busy, |s| s.opacity(0.6))
                            .child(sha),
                    )
                    .children(layout)
                    .children(hunks)
                    .child(controls)
                    .child(
                        div()
                            .id("close-file")
                            .role(Role::Button)
                            .aria_label("Fermer le fichier")
                            .tab_index(0)
                            .size(px(26.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(RADIUS_SM)
                            .border_1()
                            .border_color(rgb(p.panel))
                            .cursor_pointer()
                            .hover(move |s| s.bg(rgb(p.surface)))
                            .focus_visible(move |s| s.border_color(rgb(p.accent)))
                            .child(icon(IconName::X, 16., p.muted))
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(FileBrowserEvent::Close))),
                    ),
            )
            .child(
                div()
                    .min_h(px(40.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .flex_wrap()
                    .gap(px(6.))
                    .px(px(12.))
                    .py(px(5.))
                    .border_b_1()
                    .border_color(rgb(p.border))
                    .child(
                        action("return-history", "", false, false, p, o)
                            .aria_label("Retour à l’historique du dépôt")
                            .child(icon(IconName::ArrowLeft, 13., p.text))
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(FileBrowserEvent::Close))),
                    )
                    .children((!original && self.source.target.is_some()).then(|| {
                        action(
                            "restore-revision",
                            format!(
                                "Retour à {}",
                                self.source
                                    .target
                                    .as_ref()
                                    .map(|s| short_sha(&s.sha))
                                    .unwrap_or_default()
                            ),
                            false,
                            self.loading,
                            p,
                            o,
                        )
                        .on_click(cx.listener(|v, _, _, cx| v.restore(cx)))
                    }))
                    .children(
                        [
                            FileView::Diff,
                            FileView::File,
                            FileView::History,
                            FileView::Blame,
                        ]
                        .into_iter()
                        .enumerate()
                        .map(|(index, mode)| {
                            action(
                                ("file-view", index),
                                mode.label(),
                                mode == self.mode,
                                busy || self.target.is_none(),
                                p,
                                o,
                            )
                            .on_click(cx.listener(move |v, _, _, cx| v.show(mode, cx)))
                        }),
                    ),
            );
        if let Some(error) = self
            .error
            .clone()
            .filter(|_| self.mode != FileView::History)
        {
            root = root.child(
                notice(error, p.danger, o).child(
                    action("retry-file", "Réessayer", false, self.loading, p, o)
                        .on_click(cx.listener(|v, _, _, cx| v.show(v.mode, cx))),
                ),
            );
        }
        if self.mode == FileView::History {
            root = root.child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(self.render_history_list(cx))
                    .child(self.render_change(window, cx)),
            );
        } else {
            if self.mode == FileView::Blame {
                root=root.child(notice("Attributions sur le premier parent, jusqu’à 256 commits. Sélectionnez un groupe pour consulter son commit. Un bord jaune indique une frontière du parcours.",p.muted,o));
                if self.inspected.is_some()
                    || self.inspection_sha.is_some()
                    || self.inspection_error.is_some()
                {
                    let mut inspection = div()
                        .flex_shrink_0()
                        .max_h(px(185.))
                        .p(px(10.))
                        .bg(rgb(p.panel))
                        .border_b_1()
                        .border_color(rgb(p.border))
                        .flex()
                        .gap(px(8.));
                    let mut body = div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(8.))
                        .text_size(px(11. + o));
                    if let Some(commit) = &self.inspected {
                        let sha = commit.sha.clone();
                        body = body
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .flex_wrap()
                                    .gap(px(6.))
                                    .child(format!(
                                        "{} · {} · {}",
                                        short_sha(&sha),
                                        commit.author,
                                        commit_date(commit.time)
                                    ))
                                    .child(
                                        action(
                                            "inspect-jump",
                                            "Voir dans le dépôt",
                                            false,
                                            false,
                                            p,
                                            o,
                                        )
                                        .on_click(
                                            cx.listener(move |_, _, _, cx| {
                                                cx.emit(FileBrowserEvent::NavigateCommit(
                                                    sha.clone(),
                                                ))
                                            }),
                                        ),
                                    ),
                            )
                            .child(
                                div()
                                    .max_h(px(130.))
                                    .min_h_0()
                                    .relative()
                                    .flex()
                                    .flex_col()
                                    .child(
                                        div()
                                            .id("inspection-message")
                                            .role(Role::Document)
                                            .aria_label("Message du commit consulté")
                                            .track_scroll(&self.inspection_scroll)
                                            .min_h_0()
                                            .max_h(px(130.))
                                            .overflow_y_scroll()
                                            .child(commit.message.clone()),
                                    )
                                    .child(crate::theme::scrollbar_theme(
                                        Scrollbar::vertical(&self.inspection_scroll)
                                            .id("inspection-message-scrollbar"),
                                    )),
                            );
                    }
                    if self.inspection_sha.is_some() {
                        body = body.child("Lecture du commit…");
                    }
                    if let Some(e) = &self.inspection_error {
                        body = body.child(div().text_color(rgb(p.danger)).child(e.clone()));
                    }
                    inspection = inspection.child(body).child(
                        div()
                            .id("inspection-close")
                            .role(Role::Button)
                            .aria_label("Fermer le commit consulté")
                            .tab_index(0)
                            .size(px(22.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(RADIUS_SM)
                            .border_1()
                            .border_color(rgb(p.panel))
                            .cursor_pointer()
                            .focus_visible(move |s| s.border_color(rgb(p.accent)))
                            .child(icon(IconName::X, 13., p.muted))
                            .on_click(cx.listener(|v, _, _, cx| v.close_inspection(cx))),
                    );
                    root = root.child(inspection);
                }
                if let Some(limit) = self.blame.as_ref().and_then(|b| b.limitation.as_ref()) {
                    root = root.child(notice(limit.clone(), p.muted, o));
                }
            }
            let preview = if self.image_preview.read(cx).has_target() {
                render_cached_view(self.image_preview.clone(), window)
            } else {
                render_cached_view(self.viewer.clone(), window)
            };
            root = root.child(div().flex_1().min_h_0().child(preview));
        }
        root
    }
}

pub fn action(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    selected: bool,
    disabled: bool,
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
        .when(!disabled, |button| button.tab_index(0))
        .flex()
        .items_center()
        .gap(px(6.))
        .px(px(7.))
        .py(px(4.))
        .rounded(RADIUS_MD)
        .border_1()
        .border_color(rgb(if selected { p.accent } else { p.border }))
        .bg(rgb(p.surface))
        .text_color(rgb(if selected { p.accent } else { p.text }))
        .text_size(px(11. + o))
        .when(disabled, |s| s.opacity(0.4))
        .when(!disabled, |s| {
            s.cursor_pointer()
                .hover(move |s| s.text_color(rgb(p.accent)).border_color(rgb(p.accent)))
        })
        .focus_visible(move |style| style.border_color(rgb(p.accent)))
        .child(label)
}
pub fn notice(text: impl Into<SharedString>, color: u32, o: f32) -> Div {
    div()
        .flex_shrink_0()
        .px(px(16.))
        .py(px(9.))
        .text_size(px(11. + o))
        .text_color(rgb(color))
        .child(text.into())
}
/// Header text of a file, with the previous path of a rename or copy.
pub fn path_label(path: &str, old_path: Option<&str>) -> String {
    match old_path.filter(|old| *old != path) {
        Some(old) => format!("{old} → {path}"),
        None => path.to_owned(),
    }
}
/// A path cut from its start, so the file name stays visible; full text on hover.
pub fn path_title(id: &'static str, label: String, o: f32) -> Stateful<Div> {
    let label = SharedString::from(label);
    let tooltip = label.clone();
    div()
        .id(id)
        .min_w_0()
        .overflow_hidden()
        .whitespace_nowrap()
        .text_ellipsis_start()
        .text_size(px(12. + o))
        .tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx))
        .child(label)
}
/// Message of an empty view while its file is read.
pub fn reading_label(view: FileView, path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    match view {
        _ if name.is_empty() => "Lecture…".into(),
        FileView::Diff => format!("Lecture du diff de {name}…"),
        FileView::Blame => format!("Lecture du blame de {name}…"),
        FileView::File => format!("Lecture de {name}…"),
        FileView::History => format!("Lecture de l’historique de {name}…"),
    }
}
/// Exact line totals, unknown when the preview limit skipped the patch.
pub fn line_counts(diff: &FileDiff) -> Option<(usize, usize)> {
    diff.oversized
        .is_none()
        .then_some((diff.additions, diff.deletions))
}
pub fn diff_counts(additions: usize, deletions: usize, p: Palette, o: f32) -> Div {
    div()
        .flex()
        .flex_shrink_0()
        .gap(px(8.))
        .font_family(MONO)
        .text_size(px(11. + o))
        .child(
            div()
                .text_color(rgb(p.success))
                .child(format!("+{}", format_count(additions))),
        )
        .child(
            div()
                .text_color(rgb(p.danger))
                .child(format!("−{}", format_count(deletions))),
        )
}
pub fn status_icon(status: &str, p: Palette) -> gpui_kit::component::Icon {
    let (name, color) = match status {
        "added" => (IconName::Plus, p.success),
        "deleted" => (IconName::Minus, p.danger),
        "modified" => (IconName::Pencil, p.warning),
        "renamed" => (IconName::ArrowUpRight, p.renamed),
        "copied" => (IconName::Copy, p.warning),
        "typechange" => (IconName::Replace, p.warning),
        "unmodified" => (IconName::Equal, p.muted),
        _ => (IconName::CircleQuestionMark, p.muted),
    };
    icon(name, 15., color)
}
#[cfg(test)]
mod tests {
    use super::{
        AUTO_SCAN, FileBrowser, FileHistory, FilePresentation, FileTarget, FileView, Highlights,
        MAX_HISTORY_ENTRIES, PreparedDiff, PresentationChange, line_counts, path_label,
        reading_label,
    };
    use gpui_kit::{AppContext, TestAppContext, WindowOptions};
    use mangrove_core::{
        Workspace,
        model::{DiffHunk, FileDiff, FileHistoryEntry, FileHistoryPage, Oversized},
    };
    use std::sync::Arc;

    fn change(sha: &str, key: &str) -> FileHistoryEntry {
        FileHistoryEntry {
            sha: sha.into(),
            path: format!("{key}.rs"),
            file_key: key.into(),
            old_path: None,
            status: "modified".into(),
            summary: format!("change {sha}"),
            author: "Léa".into(),
            time: 0,
            merge: false,
        }
    }

    fn page(entries: Vec<FileHistoryEntry>, next: Option<&str>, scanned: usize) -> FileHistoryPage {
        FileHistoryPage {
            entries,
            next_cursor: next.map(str::to_owned),
            scanned,
            limitation: None,
        }
    }

    fn target(key: &str) -> FileTarget {
        FileTarget {
            session_id: "repository-session".into(),
            sha: format!("commit-{key}"),
            file_key: key.into(),
            path: format!("{key}.rs"),
            old_path: None,
            status: "modified".into(),
        }
    }

    fn prepared() -> PreparedDiff {
        PreparedDiff {
            sources: None,
            diff: Arc::new(FileDiff {
                sources: None,
                binary: false,
                oversized: None,
                truncated: false,
                additions: 0,
                deletions: 0,
                hunks: Vec::new(),
            }),
            highlights: Highlights::default(),
            highlights_ready: true,
        }
    }

    #[test]
    fn prepared_source_is_published_atomically_and_kept_coherent_while_loading() {
        let original = target("original");
        let old_patch = prepared();
        let mut source = FilePresentation::ready(1, original.clone(), old_patch.clone());
        assert!(
            source.accept(FilePresentation::loading(
                2,
                original.clone(),
                Some(old_patch.clone())
            )) == PresentationChange::Replaced
        );
        assert!(source.loading_source());
        assert_eq!(source.target.as_ref(), Some(&original));
        assert!(Arc::ptr_eq(
            &source.prepared.as_ref().unwrap().diff,
            &old_patch.diff
        ));
        assert!(
            source
                .prepared
                .as_ref()
                .unwrap()
                .highlights
                .same(&old_patch.highlights)
        );
        let selected = target("selected");
        let patch = prepared();
        assert!(
            source.accept(FilePresentation::ready(2, selected.clone(), patch.clone()))
                == PresentationChange::Replaced
        );
        assert!(!source.loading_source());
        assert!(source.error().is_none());
        assert_eq!(source.target.as_ref(), Some(&selected));
        assert!(Arc::ptr_eq(
            &source.prepared.as_ref().unwrap().diff,
            &patch.diff
        ));
        assert!(
            source
                .prepared
                .as_ref()
                .unwrap()
                .highlights
                .same(&patch.highlights)
        );
        assert!(
            source.accept(FilePresentation::loading(3, selected, None))
                == PresentationChange::Replaced
        );
    }

    #[test]
    fn raw_source_has_no_completed_highlights_and_keeps_its_single_color_owner() {
        let patch = prepared();
        let raw = PreparedDiff {
            highlights: Highlights::default(),
            highlights_ready: false,
            ..patch.clone()
        };
        let mut source = FilePresentation::ready(1, target("selected"), raw);
        assert!(source.highlights().is_none());
        assert!(source.owns_diff(&patch.diff));
        // A patch read for another revision must be colored by the browser,
        // including after leaving and returning to the Diff tab.
        assert!(!source.owns_diff(&prepared().diff));
        assert!(
            source.accept(FilePresentation::ready(
                1,
                target("selected"),
                patch.clone()
            )) == PresentationChange::Updated
        );
        assert!(source.highlights().unwrap().same(&patch.highlights));
    }

    #[test]
    fn stale_patch_or_failure_cannot_finish_a_newer_source_or_restore_a_closed_source() {
        let selected = target("selected");
        let mut source = FilePresentation::loading(3, selected.clone(), None);
        assert!(
            source.accept(FilePresentation::failed(
                2,
                target("old"),
                "old failure".into()
            )) == PresentationChange::Ignored
        );
        assert!(
            source.accept(FilePresentation::ready(2, target("old"), prepared()))
                == PresentationChange::Ignored
        );
        assert!(source.loading_source());
        assert!(source.prepared.is_none());
        assert!(source.error().is_none());
        assert_eq!(source.target.as_ref(), Some(&selected));
        assert!(
            source.accept(FilePresentation::failed(
                3,
                selected.clone(),
                "current failure".into()
            )) == PresentationChange::Updated
        );
        assert!(!source.loading_source());
        assert!(source.prepared.is_none());
        assert_eq!(source.error().as_deref(), Some("current failure"));
        assert!(source.accept(FilePresentation::empty(4)) == PresentationChange::Replaced);
        assert!(
            source.accept(FilePresentation::ready(3, selected, prepared()))
                == PresentationChange::Ignored
        );
        assert!(source.target.is_none());
        assert!(source.prepared.is_none());
        assert!(source.error().is_none());
    }

    #[test]
    fn a_refreshed_session_keeps_the_revision_but_a_new_choice_restarts_it() {
        let original = target("a");
        let mut source = FilePresentation::ready(1, original.clone(), prepared());
        let moved = FileTarget {
            session_id: "refreshed-session".into(),
            ..original.clone()
        };
        assert!(
            source.accept(FilePresentation::loading(2, moved.clone(), None))
                == PresentationChange::Rekeyed
        );
        assert!(
            source.accept(FilePresentation::ready(2, moved.clone(), prepared()))
                == PresentationChange::Updated
        );
        // Choosing the same file again in the same session restarts its view.
        assert!(
            source.accept(FilePresentation::loading(3, moved.clone(), None))
                == PresentationChange::Replaced
        );
        let other = FileTarget {
            session_id: "other-session".into(),
            ..target("b")
        };
        assert!(
            source.accept(FilePresentation::ready(4, other, prepared()))
                == PresentationChange::Replaced
        );
    }

    #[test]
    fn history_pages_accumulate_until_a_long_run_without_change() {
        let mut history = FileHistory::new(target("a"));
        assert_eq!(history.next_page(), Some(None));
        history.append(page(vec![change("c9", "a")], Some("p1"), 40));
        assert_eq!(history.next_page(), Some(Some("p1".into())));
        assert!(history.reads_on());
        let mut quiet = 0;
        while quiet < AUTO_SCAN {
            history.append(page(Vec::new(), Some("p2"), 256));
            quiet += 256;
            assert_eq!(history.reads_on(), quiet < AUTO_SCAN);
        }
        // A change found later resets the run; the end of history stops it.
        history.append(page(vec![change("c1", "a")], Some("p3"), 3));
        assert!(history.reads_on());
        history.append(page(Vec::new(), None, 2));
        assert_eq!(history.next_page(), None);
        assert!(!history.reads_on());
        assert_eq!(history.entries.len(), 2);
        assert_eq!(history.scanned, 40 + quiet + 5);
    }

    #[test]
    fn history_keeps_its_most_recent_changes_and_finds_them_by_revision() {
        // A complete history of exactly the bound is not announced as cut.
        let mut complete = FileHistory::new(target("a"));
        let entries = (0..MAX_HISTORY_ENTRIES)
            .map(|n| change(&format!("c{n}"), "a"))
            .collect();
        complete.append(page(entries, None, 300));
        assert!(complete.limitation.is_none());
        let mut history = FileHistory::new(target("a"));
        let entries = (0..MAX_HISTORY_ENTRIES + 5)
            .map(|n| change(&format!("c{n}"), "a"))
            .collect();
        history.append(page(entries, Some("more"), 300));
        assert_eq!(history.entries.len(), MAX_HISTORY_ENTRIES);
        assert_eq!(history.next_page(), None);
        assert!(history.limitation.is_some());
        let mut seventh = history.target(7).unwrap();
        assert_eq!(seventh.sha, "c7");
        seventh.session_id = "refreshed-session".into();
        assert_eq!(history.position(&seventh), Some(7));
        assert!(history.lists(&seventh));
        assert!(history.lists(&target("a")));
        assert!(!history.lists(&target("b")));
    }

    #[test]
    fn renamed_paths_show_both_sides_and_reading_names_the_file() {
        assert_eq!(path_label("src/new.rs", None), "src/new.rs");
        assert_eq!(path_label("src/new.rs", Some("src/new.rs")), "src/new.rs");
        assert_eq!(
            path_label("src/new.rs", Some("lib/old.rs")),
            "lib/old.rs → src/new.rs"
        );
        assert_eq!(
            reading_label(FileView::Blame, "src/deep/main.rs"),
            "Lecture du blame de main.rs…"
        );
        assert_eq!(
            reading_label(FileView::Diff, "README.md"),
            "Lecture du diff de README.md…"
        );
        assert_eq!(reading_label(FileView::File, "a/b.rs"), "Lecture de b.rs…");
        assert_eq!(
            reading_label(FileView::History, "a/b.rs"),
            "Lecture de l’historique de b.rs…"
        );
        assert_eq!(reading_label(FileView::Diff, ""), "Lecture…");
    }

    #[test]
    fn line_counts_are_unknown_only_for_patches_skipped_by_size() {
        let mut diff = FileDiff {
            sources: None,
            binary: false,
            oversized: None,
            truncated: false,
            additions: 8,
            deletions: 2,
            hunks: Vec::new(),
        };
        assert_eq!(line_counts(&diff), Some((8, 2)));
        diff.truncated = true;
        // A first line above the display budget leaves no hunk to show.
        assert_eq!(line_counts(&diff), Some((8, 2)));
        diff.oversized = Some(Oversized {
            size: 3 << 20,
            limit: 2 << 20,
        });
        assert_eq!(line_counts(&diff), None);
        diff.oversized = None;
        diff.hunks.push(DiffHunk {
            header: "@@ -1 +1 @@".into(),
            lines: Vec::new(),
        });
        // A patch cut for display keeps the libgit2 totals of the whole file.
        assert_eq!(line_counts(&diff), Some((8, 2)));
    }

    #[test]
    fn views_apply_only_to_files_they_can_present() {
        let file = target("file");
        let deleted = FileTarget {
            status: "deleted".into(),
            ..target("deleted")
        };
        let image = FileTarget {
            path: "logo.png".into(),
            ..target("image")
        };
        for view in [
            FileView::Diff,
            FileView::File,
            FileView::History,
            FileView::Blame,
        ] {
            assert_eq!(view.for_target(Some(&file)), view);
            assert_eq!(view.for_target(None), FileView::Diff);
        }
        assert_eq!(FileView::File.for_target(Some(&deleted)), FileView::Diff);
        assert_eq!(FileView::Blame.for_target(Some(&deleted)), FileView::Diff);
        assert_eq!(
            FileView::History.for_target(Some(&deleted)),
            FileView::History
        );
        assert_eq!(FileView::File.for_target(Some(&image)), FileView::File);
        assert_eq!(FileView::Blame.for_target(Some(&image)), FileView::Diff);
    }

    #[gpui_kit::test]
    fn chosen_view_survives_file_changes_while_it_applies(cx: &mut TestAppContext) {
        let root = tempfile::tempdir().unwrap();
        let engine = Arc::new(Workspace::new(root.path().join("cache")).unwrap());
        cx.update(gpui_kit::init);
        let browser = cx.update(|cx| {
            gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                cx.new(|cx| FileBrowser::new(engine, Arc::default(), window, cx))
            })
            .unwrap()
            .1
        });
        browser.update(cx, |view, cx| {
            // Hidden behind the inspector panel, a retained view waits to be shown.
            view.preferred = FileView::Blame;
            view.present(FilePresentation::ready(1, target("a"), prepared()), cx);
            assert_eq!(view.mode, FileView::Blame);
            assert!(!view.loading);
            view.set_visible(true, cx);
            assert!(view.loading);
            view.loading = false;
            view.show(FileView::Blame, cx);
            assert_eq!(view.mode, FileView::Blame);
            // A newly chosen file keeps Blame and reads it once its source is ready.
            view.present(FilePresentation::loading(2, target("b"), None), cx);
            assert_eq!(view.mode, FileView::Blame);
            assert!(!view.loading);
            view.present(FilePresentation::ready(2, target("b"), prepared()), cx);
            assert!(view.loading);
            let deleted = FileTarget {
                status: "deleted".into(),
                ..target("c")
            };
            view.present(FilePresentation::ready(3, deleted, prepared()), cx);
            assert_eq!(view.mode, FileView::Diff);
            view.present(FilePresentation::ready(4, target("d"), prepared()), cx);
            assert_eq!(view.mode, FileView::Blame);
            view.show(FileView::Diff, cx);
            view.present(FilePresentation::ready(5, target("e"), prepared()), cx);
            assert_eq!(view.mode, FileView::Diff);
        });
    }

    /// A browser without repository: its reads fail, the tests settle them.
    fn browser(cx: &mut TestAppContext) -> (tempfile::TempDir, gpui_kit::Entity<FileBrowser>) {
        let root = tempfile::tempdir().unwrap();
        let engine = Arc::new(Workspace::new(root.path().join("cache")).unwrap());
        cx.update(gpui_kit::init);
        let browser = cx.update(|cx| {
            gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                cx.new(|cx| FileBrowser::new(engine, Arc::default(), window, cx))
            })
            .unwrap()
            .1
        });
        (root, browser)
    }

    #[gpui_kit::test]
    fn history_keeps_its_changes_while_they_are_shown(cx: &mut TestAppContext) {
        let (_root, browser) = browser(cx);
        browser.update(cx, |view, cx| {
            // A revision that does not change the file, as HEAD may not.
            let head = FileTarget {
                status: "unmodified".into(),
                ..target("a")
            };
            view.present(FilePresentation::ready(1, head.clone(), prepared()), cx);
            view.set_visible(true, cx);
            view.show(FileView::History, cx);
            assert!(view.history_loading);
            assert_eq!(view.history.as_ref().unwrap().root, head);
            view.history_loading = false;
            let changes = vec![change("c3", "a"), change("c2", "a"), change("c1", "a")];
            view.append_history(page(changes, Some("next"), 12), cx);
            // The newest change is shown beside the list.
            assert_eq!(view.history.as_ref().unwrap().selected, Some(0));
            assert_eq!(view.target.as_ref().unwrap().sha, "c3");
            assert_eq!(view.view(), FileView::Diff);
            view.select_change(2, cx);
            assert_eq!(view.target.as_ref().unwrap().sha, "c1");
            view.show_detail(FileView::File, cx);
            assert_eq!(view.view(), FileView::File);
            // Other views show the chosen change; the list waits for them.
            view.show(FileView::Blame, cx);
            assert_eq!(view.target.as_ref().unwrap().sha, "c1");
            view.show(FileView::History, cx);
            let history = view.history.as_ref().unwrap();
            assert_eq!((history.selected, history.entries.len()), (Some(2), 3));
            assert_eq!(history.root, head);
            // Another file lists its own history.
            view.present(FilePresentation::ready(2, target("b"), prepared()), cx);
            assert_eq!(view.mode, FileView::History);
            let history = view.history.as_ref().unwrap();
            assert_eq!(history.root, target("b"));
            assert!(history.entries.is_empty());
        });
    }

    #[gpui_kit::test]
    fn the_inspector_revision_keeps_its_history_when_the_repository_is_refreshed(
        cx: &mut TestAppContext,
    ) {
        let (_root, browser) = browser(cx);
        browser.update(cx, |view, cx| {
            let selected = FileTarget {
                sha: "c3".into(),
                ..target("a")
            };
            view.present(FilePresentation::ready(1, selected.clone(), prepared()), cx);
            view.set_visible(true, cx);
            view.show(FileView::History, cx);
            view.history_loading = false;
            let changes = vec![change("c3", "a"), change("c2", "a")];
            view.append_history(page(changes, Some("next"), 2), cx);
            // The selected commit's own change keeps the inspector's patch.
            assert_eq!(view.history.as_ref().unwrap().selected, Some(0));
            assert!(view.diff.is_some());
            view.select_change(1, cx);
            // Reads that failed while the old session closed are read again.
            view.loading = false;
            view.error = Some("Ce dépôt a été fermé ou actualisé.".into());
            view.history_error = Some("Ce dépôt a été fermé ou actualisé.".into());
            let moved = FileTarget {
                session_id: "refreshed-session".into(),
                ..selected
            };
            view.present(FilePresentation::loading(2, moved.clone(), None), cx);
            assert!(view.error.is_none() && view.history_error.is_none());
            assert!(view.loading && view.history_loading);
            view.present(FilePresentation::ready(2, moved, prepared()), cx);
            let history = view.history.as_ref().unwrap();
            assert_eq!(history.selected, Some(1));
            assert_eq!(history.root.session_id, "refreshed-session");
            assert_eq!(view.target.as_ref().unwrap().sha, "c2");
            assert_eq!(
                view.target.as_ref().unwrap().session_id,
                "refreshed-session"
            );
        });
    }

    #[test]
    fn displayed_sha_keeps_seven_characters() {
        assert_eq!(super::short_sha("0123456789abcdef"), "0123456");
    }

    /// The inspector shows the file browser through the view cache.
    struct CachedHost {
        browser: gpui_kit::Entity<FileBrowser>,
    }

    impl gpui_kit::Render for CachedHost {
        fn render(
            &mut self,
            window: &mut gpui_kit::Window,
            _: &mut gpui_kit::Context<Self>,
        ) -> impl gpui_kit::IntoElement {
            use gpui_kit::{ParentElement, Styled, div};
            div()
                .size_full()
                .child(crate::view_cache::render_cached_view(
                    self.browser.clone(),
                    window,
                ))
        }
    }

    #[gpui_kit::test]
    fn the_layout_toggle_acts_once_per_key_press_and_keeps_the_focus(cx: &mut TestAppContext) {
        use crate::preferences::{DiffLayout, Preferences};
        use gpui_kit::test::TestWindowExt;
        use mangrove_core::model::DiffLine;
        let root = tempfile::tempdir().unwrap();
        let engine = Arc::new(Workspace::new(root.path().join("cache")).unwrap());
        cx.update(gpui_kit::init);
        let (window, host) = cx.update(|cx| {
            gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                let browser = cx.new(|cx| FileBrowser::new(engine, Arc::default(), window, cx));
                cx.new(|_| CachedHost { browser })
            })
            .unwrap()
        });
        let browser = cx.read(|cx| host.read(cx).browser.clone());
        let line = |kind: &str, content: &str| DiffLine {
            kind: kind.into(),
            old_line: Some(1),
            new_line: Some(1),
            content: content.into(),
        };
        let edited = PreparedDiff {
            sources: None,
            diff: Arc::new(FileDiff {
                hunks: vec![DiffHunk {
                    header: "@@ -1,2 +1,2 @@".into(),
                    lines: vec![
                        line("context", "fn main() {}"),
                        line("deletion", "let value = 1;"),
                        line("addition", "let value = 2;"),
                    ],
                }],
                ..FileDiff::default()
            }),
            highlights: Highlights::default(),
            highlights_ready: true,
        };
        browser.update(cx, |view, cx| {
            view.present(FilePresentation::ready(1, target("a"), edited), cx);
            view.set_visible(true, cx);
        });
        // The application saves the asked layout and hands it back.
        let asked = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let recorded = asked.clone();
        let _subscription = cx.update(|cx| {
            cx.subscribe(
                &browser,
                move |browser, event: &super::FileBrowserEvent, cx| {
                    if let super::FileBrowserEvent::LayoutChanged(layout) = event {
                        recorded.borrow_mut().push(*layout);
                        let preferences = Preferences {
                            diff_layout: *layout,
                            ..Preferences::default()
                        };
                        browser.update(cx, |view, cx| view.set_preferences(preferences, cx));
                    }
                },
            )
        });
        cx.run_until_parked();
        // The layout toggle is the first tab stop of the header.
        let focused = cx
            .update_window(window, |_, window, cx| {
                window.focus_next(cx);
                window.focused(cx)
            })
            .unwrap();
        assert!(focused.is_some());
        let press = |cx: &mut TestAppContext, key: &str| {
            cx.update_window(window, |_, window, cx| {
                window.press(key, cx);
                window.focused(cx)
            })
            .unwrap()
        };
        assert_eq!(press(cx, "enter"), focused);
        cx.run_until_parked();
        assert_eq!(*asked.borrow(), [DiffLayout::Split]);
        assert_eq!(press(cx, "space"), focused);
        cx.run_until_parked();
        assert_eq!(*asked.borrow(), [DiffLayout::Split, DiffLayout::Unified]);
    }
}
