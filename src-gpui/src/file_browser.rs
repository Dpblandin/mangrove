//! File revision browsing. One native read is retained until it actually settles;
//! replacing a view only replaces the queued intent, never creates an unbounded queue.
use crate::{
    code_view::{CodeView, CodeViewEvent, hunk_shortcut},
    format::{commit_date, count_label, format_count, short_sha},
    image_preview::{ImagePreview, ImageTarget, supports_path},
    preferences::Preferences,
    syntax::{self, Tokens},
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
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};

pub type ReadGate = Arc<Mutex<()>>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileTarget {
    pub session_id: String,
    pub sha: String,
    pub file_key: String,
    pub path: String,
    pub old_path: Option<String>,
    pub status: String,
}

/// A patch and its syntax always belong to the same file revision.
#[derive(Clone)]
pub struct PreparedDiff {
    pub diff: Arc<FileDiff>,
    pub tokens: Tokens,
    pub syntax_ready: bool,
}
impl PreparedDiff {
    pub fn raw(diff: Arc<FileDiff>) -> Self {
        Self {
            diff,
            tokens: Arc::new(Vec::new()),
            syntax_ready: false,
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

    fn tokens(&self) -> Option<Tokens> {
        self.prepared
            .as_ref()
            .filter(|prepared| prepared.syntax_ready)
            .map(|prepared| prepared.tokens.clone())
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
        let replaced = next.revision != self.revision || next.target != self.target;
        *self = next;
        if replaced {
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
}

struct HistoryRequest {
    cursor: Option<String>,
    direction: i8,
}
enum ReadKind {
    Diff,
    Content {
        blame: bool,
        existing: Option<Arc<FileContent>>,
    },
    History(Option<HistoryRequest>),
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
    History(Arc<FileHistoryPage>),
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
    diff_tokens: Option<Tokens>,
    blame: Option<Arc<FileBlame>>,
    history: Option<Arc<FileHistoryPage>>,
    cursor: Option<String>,
    previous_cursors: Vec<Option<String>>,
    failed_page: Option<HistoryRequest>,
    inspected: Option<Arc<CommitDetails>>,
    inspection_sha: Option<String>,
    inspection_error: Option<String>,
    error: Option<String>,
    loading: bool,
    pending: Option<ReadRequest>,
    pending_inspection: Option<ReadRequest>,
    active: bool,
    task: Option<Task<()>>,
    viewer: Entity<CodeView>,
    image_preview: Entity<ImagePreview>,
    viewer_dirty: bool,
    palette: Palette,
    preferences: Preferences,
    history_scroll: ScrollHandle,
    inspection_scroll: ScrollHandle,
    subscriptions: Vec<Subscription>,
    focus: FocusHandle,
    #[cfg(feature = "app-benchmark")]
    benchmark_scroll: f32,
}
impl EventEmitter<FileBrowserEvent> for FileBrowser {}

impl FileBrowser {
    pub fn focus(&self, window: &mut Window, cx: &mut App) {
        self.focus.focus(window, cx);
    }
    pub fn new(
        engine: Arc<Workspace>,
        gate: ReadGate,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let viewer = cx.new(|cx| CodeView::new(window, cx));
        let image_preview = cx.new(|_| ImagePreview::new(engine.clone(), gate.clone()));
        let subscription = cx.subscribe(&viewer, |view, _, event, cx| match event {
            CodeViewEvent::NavigateCommit(sha) => view.inspect_commit(sha.clone(), cx),
            CodeViewEvent::FontSizeChanged(size) => {
                cx.emit(FileBrowserEvent::FontSizeChanged(*size))
            }
            CodeViewEvent::Parent => view.parent_version(cx),
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
            diff_tokens: None,
            blame: None,
            history: None,
            cursor: None,
            previous_cursors: Vec::new(),
            failed_page: None,
            inspected: None,
            inspection_sha: None,
            inspection_error: None,
            error: None,
            loading: false,
            pending: None,
            pending_inspection: None,
            active: false,
            task: None,
            viewer,
            image_preview,
            viewer_dirty: true,
            palette: Palette::default(),
            preferences: Preferences::default(),
            history_scroll: ScrollHandle::new(),
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
        if change == PresentationChange::Ignored {
            return;
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
            self.diff_tokens = self.source.tokens();
            if self.mode == FileView::Diff {
                self.error = self.source.error();
            }
            self.viewer_dirty = true;
        }
        // A retained File, History or Blame view reads once its source is ready.
        if self.mode != FileView::Diff
            && !self.source.loading_source()
            && (was_loading || change == PresentationChange::Replaced)
            && self.target == self.source.target
        {
            self.load(None, cx);
        }
        cx.notify();
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
        if self.preferences.font_size_offset == preferences.font_size_offset
            && self.preferences.diff_font_size == preferences.diff_font_size
            && self.preferences.show_image_previews == preferences.show_image_previews
            && self.preferences.theme == preferences.theme
        {
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
        if self.mode == FileView::Diff {
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
    pub fn benchmark_scroll_to(&mut self, offset: f32, cx: &mut Context<Self>) {
        let delta = offset - self.benchmark_scroll;
        self.benchmark_scroll = offset;
        self.viewer
            .update(cx, |view, cx| view.scroll_by_pixels(delta, cx));
        cx.notify();
    }
    fn select_target(
        &mut self,
        target: Option<FileTarget>,
        diff: Option<Arc<FileDiff>>,
        cx: &mut Context<Self>,
    ) {
        self.generation += 1;
        self.live_generation
            .store(self.generation, Ordering::Release);
        self.color_pending = None;
        self.image_preview
            .update(cx, |view, cx| view.present(None, self.generation, cx));
        self.target = target;
        self.diff = diff;
        self.diff_tokens = None;
        self.content = None;
        self.blame = None;
        self.history = None;
        self.cursor = None;
        self.previous_cursors.clear();
        self.failed_page = None;
        self.inspected = None;
        self.inspection_sha = None;
        self.inspection_error = None;
        self.error = None;
        self.pending = None;
        self.pending_inspection = None;
        self.loading = false;
        self.viewer_dirty = true;
        self.history_scroll = ScrollHandle::new();
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
        self.diff_tokens = self.source.tokens();
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
        let retry = if self.mode == mode {
            self.failed_page.take()
        } else {
            None
        };
        self.generation += 1;
        self.live_generation
            .store(self.generation, Ordering::Release);
        self.color_pending = None;
        self.pending = None;
        self.close_inspection(cx);
        self.mode = mode;
        self.error = None;
        self.loading = false;
        self.viewer_dirty = true;
        self.load(retry, cx);
    }
    fn load(&mut self, page: Option<HistoryRequest>, cx: &mut Context<Self>) {
        let Some(target) = self.target.clone() else {
            return;
        };
        let kind = match self.mode {
            FileView::Diff if self.diff.is_none() => ReadKind::Diff,
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
            },
            FileView::Blame if self.blame.is_none() => ReadKind::Content {
                blame: true,
                existing: self.content.clone(),
            },
            FileView::History if self.history.is_none() || page.is_some() => {
                ReadKind::History(Some(page.unwrap_or_else(|| HistoryRequest {
                    cursor: self.cursor.clone(),
                    direction: 0,
                })))
            }
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
    fn page(&mut self, direction: i8, cx: &mut Context<Self>) {
        if self.mode != FileView::History || self.loading {
            return;
        }
        let cursor = if direction > 0 {
            let Some(c) = self.history.as_ref().and_then(|h| h.next_cursor.clone()) else {
                return;
            };
            Some(c)
        } else {
            let Some(c) = self.previous_cursors.last() else {
                return;
            };
            c.clone()
        };
        self.generation += 1;
        self.live_generation
            .store(self.generation, Ordering::Release);
        self.color_pending = None;
        self.failed_page = None;
        self.load(Some(HistoryRequest { cursor, direction }), cx);
    }
    fn select_revision(&mut self, index: usize, mode: FileView, cx: &mut Context<Self>) {
        let (Some(target), Some(history)) = (&self.target, &self.history) else {
            return;
        };
        let Some(entry) = history.entries.get(index) else {
            return;
        };
        let next = FileTarget {
            session_id: target.session_id.clone(),
            sha: entry.sha.clone(),
            file_key: entry.file_key.clone(),
            path: entry.path.clone(),
            old_path: entry.old_path.clone(),
            status: entry.status.clone(),
        };
        self.select_target(Some(next), None, cx);
        self.show(mode, cx);
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
            .or_else(|| self.pending_inspection.take())
        else {
            return;
        };
        self.active = true;
        let engine = self.engine.clone();
        let gate = self.gate.clone();
        let live_generation = self.live_generation.clone();
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
                            ReadKind::Diff => {
                                let diff = Arc::new(engine.file_revision_diff(
                                    &t.session_id,
                                    &t.sha,
                                    &t.file_key,
                                )?);
                                ReadResult::Diff(diff)
                            }
                            ReadKind::Content { blame, existing } => {
                                let content = match existing {
                                    Some(content) => content.clone(),
                                    None => Arc::new(engine.file_content(
                                        &t.session_id,
                                        &t.sha,
                                        &t.file_key,
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
                            ReadKind::History(page) => {
                                ReadResult::History(Arc::new(engine.file_history(
                                    &t.session_id,
                                    &t.sha,
                                    &t.file_key,
                                    page.as_ref().and_then(|p| p.cursor.as_deref()),
                                )?))
                            }
                            ReadKind::Inspect(sha) => ReadResult::Inspect(
                                engine.commit_details_shared(&t.session_id, sha)?,
                            ),
                        })
                    })();
                    #[cfg(feature = "app-benchmark")]
                    {
                        let stage = match &request.kind {
                            ReadKind::Diff => "patch_git",
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
                if request.generation == view.generation
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
                                view.diff_tokens = None;
                                view.resume_diff_color(cx);
                            }
                            Ok(ReadResult::Content(content, blame)) => {
                                view.content = Some(content);
                                view.blame = blame;
                            }
                            Ok(ReadResult::History(history)) => {
                                if let ReadKind::History(Some(page)) = request.kind {
                                    if page.direction > 0 {
                                        view.previous_cursors.push(view.cursor.clone());
                                        if view.previous_cursors.len() > 12 {
                                            view.previous_cursors.remove(0);
                                        }
                                    } else if page.direction < 0 {
                                        view.previous_cursors.pop();
                                    }
                                    view.cursor = page.cursor;
                                }
                                view.history = Some(history);
                                view.failed_page = None;
                            }
                            Err(e) => {
                                view.error = Some(e);
                                if let ReadKind::History(page) = request.kind {
                                    view.failed_page = page;
                                }
                            }
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
        if self.diff_tokens.is_some() || self.source.owns_diff(diff) {
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
            let (request, tokens) = cx
                .background_executor()
                .spawn(async move {
                    #[cfg(feature = "app-benchmark")]
                    let syntax_start = std::time::Instant::now();
                    let tokens = syntax::cached_diff_tokens_cancellable(
                        &request.target.path,
                        &request.diff,
                        || live_generation.load(Ordering::Acquire) != request.generation,
                    );
                    #[cfg(feature = "app-benchmark")]
                    crate::perf::record_operation("syntax", syntax_start.elapsed());
                    (request, tokens)
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.color_active = false;
                if request.generation == view.generation
                    && view.target.as_ref() == Some(&request.target)
                    && let Some(tokens) = tokens
                {
                    view.diff_tokens = Some(tokens);
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
        let image_target = self
            .target
            .as_ref()
            .filter(|target| {
                self.preferences.show_image_previews
                    && supports_path(&target.path)
                    && matches!(self.mode, FileView::Diff | FileView::File)
                    && !self.source.loading_source()
                    && self.error.is_none()
            })
            .map(|target| ImageTarget {
                session_id: target.session_id.clone(),
                source: FileImageSource::Commit {
                    sha: target.sha.clone(),
                    file_key: target.file_key.clone(),
                    comparison: self.mode == FileView::Diff,
                },
            });
        self.image_preview.update(cx, |view, cx| {
            view.set_style(&self.preferences, false, cx);
            view.present(image_target, self.generation, cx);
        });
        self.viewer_dirty = false;
        let mode = self.mode;
        let diff = self.diff.clone();
        let diff_tokens = self.diff_tokens.clone();
        let content = self.content.clone();
        let blame = self.blame.clone();
        let path = self
            .target
            .as_ref()
            .map(|t| t.path.clone())
            .unwrap_or_default();
        let loading =
            (self.loading || self.source.loading_source()).then(|| reading_label(mode, &path));
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
                            diff_tokens.unwrap_or_default(),
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
    fn history_rows(
        &self,
        range: std::ops::Range<usize>,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let Some(history) = &self.history else {
            return Vec::new();
        };
        let p = self.palette;
        let o = self.preferences.font_size_offset;
        range
            .map(|index| {
                let entry = &history.entries[index];
                div()
                    .id(("file-history", index))
                    .role(Role::ListItem)
                    .aria_label(entry.summary.clone())
                    .flex()
                    .items_center()
                    .gap(px(9.))
                    .p(px(12.))
                    .border_b_1()
                    .border_color(rgb(p.border))
                    .child(status_icon(&entry.status, p))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(5.))
                            .child(div().text_size(px(12. + o)).child(entry.summary.clone()))
                            .child(div().text_size(px(11. + o)).text_color(rgb(p.muted)).child(
                                format!(
                                    "{} · {} · {}{}",
                                    entry.author,
                                    commit_date(entry.time),
                                    short_sha(&entry.sha),
                                    if entry.merge {
                                        " · Fusion, premier parent"
                                    } else {
                                        ""
                                    }
                                ),
                            ))
                            .children(entry.old_path.as_ref().map(|old| {
                                div()
                                    .font_family(MONO)
                                    .text_size(px(11. + o))
                                    .text_color(rgb(p.muted))
                                    .child(format!("{old} → {}", entry.path))
                            })),
                    )
                    .child(
                        action(("history-diff", index), "Diff", false, false, p, o)
                            .on_click(cx.listener(move |v, _, _, cx| {
                                v.select_revision(index, FileView::Diff, cx)
                            }))
                            .on_key_down(cx.listener(move |v, e: &KeyDownEvent, _, cx| {
                                if is_activate(e) {
                                    v.select_revision(index, FileView::Diff, cx);
                                    cx.stop_propagation();
                                }
                            })),
                    )
                    .child(
                        action(("history-content", index), "Fichier", false, false, p, o)
                            .on_click(cx.listener(move |v, _, _, cx| {
                                v.select_revision(index, FileView::File, cx)
                            }))
                            .on_key_down(cx.listener(move |v, e: &KeyDownEvent, _, cx| {
                                if is_activate(e) {
                                    v.select_revision(index, FileView::File, cx);
                                    cx.stop_propagation();
                                }
                            })),
                    )
                    .into_any_element()
            })
            .collect()
    }
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
        let original = self.target == self.source.target;
        let controls = self.font_controls(window, cx);
        let diff_mode = self.mode == FileView::Diff;
        let hunks = diff_mode.then(|| self.viewer.update(cx, |view, cx| view.hunk_controls(cx)));
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
                    .h(px(40.))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap(px(9.))
                    .px(px(16.))
                    .bg(rgb(p.panel))
                    .border_b_1()
                    .border_color(rgb(p.border))
                    .child(icon(IconName::FileText, 15., p.muted))
                    .child(
                        path_title("file-path", path, o)
                            .flex_1()
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
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(FileBrowserEvent::Close)))
                            .on_key_down(cx.listener(|_, e: &KeyDownEvent, _, cx| {
                                if is_activate(e) {
                                    cx.emit(FileBrowserEvent::Close);
                                    cx.stop_propagation();
                                }
                            })),
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
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(FileBrowserEvent::Close)))
                            .on_key_down(cx.listener(|_, e: &KeyDownEvent, _, cx| {
                                if is_activate(e) {
                                    cx.emit(FileBrowserEvent::Close);
                                    cx.stop_propagation();
                                }
                            })),
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
                        .on_key_down(cx.listener(
                            |v, e: &KeyDownEvent, _, cx| {
                                if is_activate(e) {
                                    v.restore(cx);
                                    cx.stop_propagation();
                                }
                            },
                        ))
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
                            .on_key_down(cx.listener(
                                move |v, e: &KeyDownEvent, _, cx| {
                                    if is_activate(e) {
                                        v.show(mode, cx);
                                        cx.stop_propagation();
                                    }
                                },
                            ))
                        }),
                    ),
            );
        if let Some(error) = &self.error {
            root = root.child(
                notice(error.clone(), p.danger, o).child(
                    action("retry-file", "Réessayer", false, self.loading, p, o)
                        .on_click(cx.listener(|v, _, _, cx| v.show(v.mode, cx)))
                        .on_key_down(cx.listener(|v, e: &KeyDownEvent, _, cx| {
                            if is_activate(e) {
                                v.show(v.mode, cx);
                                cx.stop_propagation();
                            }
                        })),
                ),
            );
        }
        if self.mode == FileView::History {
            root=root.child(notice("Premier parent uniquement aux fusions. Les renommages détectés sont suivis ; copies et renommages ambigus peuvent interrompre le suivi.",p.muted,o));
            let count = self.history.as_ref().map(|h| h.entries.len()).unwrap_or(0);
            root = root.child(
                div()
                    .flex_1()
                    .min_h_0()
                    .relative()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .id("file-history")
                            .role(Role::List)
                            .aria_label("Historique du fichier")
                            .track_scroll(&self.history_scroll)
                            .flex_1()
                            .min_h_0()
                            .overflow_y_scroll()
                            .children(self.history_rows(0..count, cx)),
                    )
                    .child(crate::theme::scrollbar_theme(
                        Scrollbar::vertical(&self.history_scroll).id("file-history-scrollbar"),
                    )),
            );
            if let Some(history) = &self.history {
                if history.entries.is_empty() {
                    root = root.child(notice(
                        format!(
                            "Aucun changement dans les {} commits parcourus sur cette page.",
                            history.scanned
                        ),
                        p.muted,
                        o,
                    ));
                }
                if let Some(limit) = &history.limitation {
                    root = root.child(notice(limit.clone(), p.muted, o));
                }
            }
            root = root.child(
                div()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(8.))
                    .p(px(10.))
                    .border_t_1()
                    .border_color(rgb(p.border))
                    .text_size(px(11. + o))
                    .text_color(rgb(p.muted))
                    .child(
                        action(
                            "history-previous",
                            "Page précédente",
                            false,
                            self.loading || self.previous_cursors.is_empty(),
                            p,
                            o,
                        )
                        .on_click(cx.listener(|v, _, _, cx| v.page(-1, cx)))
                        .on_key_down(cx.listener(
                            |v, e: &KeyDownEvent, _, cx| {
                                if is_activate(e) {
                                    v.page(-1, cx);
                                    cx.stop_propagation();
                                }
                            },
                        )),
                    )
                    .child(
                        self.history
                            .as_ref()
                            .map(|h| {
                                format!(
                                    "{} · {} parcourus",
                                    count_label(h.entries.len(), "changement", "changements"),
                                    count_label(h.scanned, "commit", "commits")
                                )
                            })
                            .unwrap_or_else(|| {
                                if self.loading {
                                    "Chargement…".into()
                                } else {
                                    String::new()
                                }
                            }),
                    )
                    .child(
                        action(
                            "history-next",
                            "Page suivante",
                            false,
                            self.loading
                                || self
                                    .history
                                    .as_ref()
                                    .is_none_or(|h| h.next_cursor.is_none()),
                            p,
                            o,
                        )
                        .on_click(cx.listener(|v, _, _, cx| v.page(1, cx)))
                        .on_key_down(cx.listener(
                            |v, e: &KeyDownEvent, _, cx| {
                                if is_activate(e) {
                                    v.page(1, cx);
                                    cx.stop_propagation();
                                }
                            },
                        )),
                    ),
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
                                        .on_click({
                                            let sha = sha.clone();
                                            cx.listener(move |_, _, _, cx| {
                                                cx.emit(FileBrowserEvent::NavigateCommit(
                                                    sha.clone(),
                                                ))
                                            })
                                        })
                                        .on_key_down(
                                            cx.listener(move |_, e: &KeyDownEvent, _, cx| {
                                                if is_activate(e) {
                                                    cx.emit(FileBrowserEvent::NavigateCommit(
                                                        sha.clone(),
                                                    ));
                                                    cx.stop_propagation();
                                                }
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
                            .on_click(cx.listener(|v, _, _, cx| v.close_inspection(cx)))
                            .on_key_down(cx.listener(|v, e: &KeyDownEvent, _, cx| {
                                if is_activate(e) {
                                    v.close_inspection(cx);
                                    cx.stop_propagation();
                                }
                            })),
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

pub fn is_activate(event: &KeyDownEvent) -> bool {
    matches!(event.keystroke.key.as_str(), "enter" | "space")
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
        FileView::File | FileView::History => format!("Lecture de {name}…"),
    }
}
/// Exact line totals, unknown for files skipped before reading their patch.
pub fn line_counts(diff: &FileDiff) -> Option<(usize, usize)> {
    (!diff.truncated || !diff.hunks.is_empty()).then_some((diff.additions, diff.deletions))
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
        FileBrowser, FilePresentation, FileTarget, FileView, PreparedDiff, PresentationChange,
        line_counts, path_label, reading_label,
    };
    use gpui_kit::{AppContext, TestAppContext, WindowOptions};
    use mangrove_core::{
        Workspace,
        model::{DiffHunk, FileDiff},
    };
    use std::sync::Arc;

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
            diff: Arc::new(FileDiff {
                binary: false,
                truncated: false,
                additions: 0,
                deletions: 0,
                hunks: Vec::new(),
            }),
            tokens: Arc::default(),
            syntax_ready: true,
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
        assert!(Arc::ptr_eq(
            &source.prepared.as_ref().unwrap().tokens,
            &old_patch.tokens
        ));
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
        assert!(Arc::ptr_eq(
            &source.prepared.as_ref().unwrap().tokens,
            &patch.tokens
        ));
        assert!(
            source.accept(FilePresentation::loading(3, selected, None))
                == PresentationChange::Replaced
        );
    }

    #[test]
    fn raw_source_has_no_completed_tokens_and_keeps_its_single_color_owner() {
        let patch = prepared();
        let raw = PreparedDiff::raw(patch.diff.clone());
        let mut source = FilePresentation::ready(1, target("selected"), raw);
        assert!(source.tokens().is_none());
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
        assert!(source.tokens().is_some());
        assert!(Arc::ptr_eq(&source.tokens().unwrap(), &patch.tokens));
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
        assert_eq!(reading_label(FileView::Diff, ""), "Lecture…");
    }

    #[test]
    fn line_counts_are_unknown_only_for_patches_skipped_by_size() {
        let mut diff = FileDiff {
            binary: false,
            truncated: false,
            additions: 8,
            deletions: 2,
            hunks: Vec::new(),
        };
        assert_eq!(line_counts(&diff), Some((8, 2)));
        diff.truncated = true;
        assert_eq!(line_counts(&diff), None);
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
            view.present(FilePresentation::ready(1, target("a"), prepared()), cx);
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

    #[test]
    fn displayed_sha_keeps_seven_characters() {
        assert_eq!(super::short_sha("0123456789abcdef"), "0123456");
    }
}
