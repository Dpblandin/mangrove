//! File revision browsing. One native read is retained until it actually settles;
//! replacing a view only replaces the queued intent, never creates an unbounded queue.
use crate::{
    code_view::{CodeView, CodeViewEvent},
    format::{commit_date, short_sha},
    image_preview::{ImagePreview, ImageTarget, supports_path},
    preferences::Preferences,
    syntax::{self, Tokens},
    theme::{MONO, Palette, icon},
};
use accesskit::Role;
use gpui_kit::{assets::IconName, component::scroll::Scrollbar, prelude::*, *};
use mangrove_core::{Workspace, model::*};
use std::sync::{Arc, Mutex};

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
    Diff(Arc<FileDiff>, Tokens),
    Content(Arc<FileContent>, Option<Arc<FileBlame>>),
    History(Arc<FileHistoryPage>),
    Inspect(Arc<CommitDetails>),
}

pub struct FileBrowser {
    engine: Arc<Workspace>,
    gate: ReadGate,
    source: FilePresentation,
    target: Option<FileTarget>,
    mode: FileView,
    generation: u64,
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
            generation: 0,
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
            self.mode = FileView::Diff;
        }
        if self.target == target {
            self.diff = diff;
            self.diff_tokens = self
                .source
                .prepared
                .as_ref()
                .map(|prepared| prepared.tokens.clone());
            self.error = self.source.error();
            self.viewer_dirty = true;
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
        self.diff_tokens = prepared.map(|prepared| prepared.tokens);
        self.show(mode, cx);
    }
    pub fn show(&mut self, mode: FileView, cx: &mut Context<Self>) {
        if self.target.as_ref().is_none_or(|t| t.file_key.is_empty())
            || self.source.loading_source()
        {
            return;
        }
        if self.mode == mode && self.loading {
            return;
        }
        let retry = if self.mode == mode {
            self.failed_page.take()
        } else {
            None
        };
        self.generation += 1;
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
        self.task = Some(cx.spawn(async move |this, cx| {
            let (request, result) = cx
                .background_executor()
                .spawn(async move {
                    let _guard = gate.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
                    let t = &request.target;
                    let result: Result<ReadResult, String> = (|| {
                        Ok(match &request.kind {
                            ReadKind::Diff => {
                                let diff = Arc::new(engine.file_revision_diff(
                                    &t.session_id,
                                    &t.sha,
                                    &t.file_key,
                                )?);
                                let tokens = syntax::cached_diff_tokens(&t.path, &diff);
                                ReadResult::Diff(diff, tokens)
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
                            ReadKind::Inspect(sha) => ReadResult::Inspect(Arc::new(
                                engine.commit_details(&t.session_id, sha)?,
                            )),
                        })
                    })();
                    (request, result)
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.active = false;
                if request.generation == view.generation
                    && view.target.as_ref() == Some(&request.target)
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
                            Ok(ReadResult::Diff(diff, tokens)) => {
                                view.diff = Some(diff);
                                view.diff_tokens = Some(tokens);
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
        let loading = self.loading || self.source.loading_source();
        let error = self.error.clone();
        self.viewer.update(cx, |v, cx| {
            match mode {
                FileView::Diff => {
                    if let Some(diff) = diff {
                        if let Some(tokens) = diff_tokens {
                            v.set_prepared_diff(diff, path, tokens, window, cx);
                        } else {
                            v.set_diff(diff, path, window, cx);
                        }
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
                            .child(div().text_size(px(10. + o)).text_color(rgb(p.muted)).child(
                                format!(
                                    "{} · {} · {}{}",
                                    entry.author,
                                    commit_date(entry.time),
                                    short_sha(&entry.sha),
                                    if entry.merge {
                                        " · Merge, premier parent"
                                    } else {
                                        ""
                                    }
                                ),
                            ))
                            .children(entry.old_path.as_ref().map(|old| {
                                div()
                                    .font_family(MONO)
                                    .text_size(px(10. + o))
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
            .map(|t| t.path.clone())
            .unwrap_or_default();
        let sha = self
            .target
            .as_ref()
            .map(|t| short_sha(&t.sha))
            .unwrap_or_default();
        let busy = self.source.loading_source();
        let original = self.target == self.source.target;
        let controls = self.font_controls(window, cx);
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
            .on_key_down(cx.listener(|_, e: &KeyDownEvent, _, cx| {
                if e.keystroke.key == "escape" {
                    cx.emit(FileBrowserEvent::Close);
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
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(px(12. + o))
                            .child(path),
                    )
                    .child(
                        div()
                            .font_family(MONO)
                            .text_size(px(10. + o))
                            .text_color(rgb(p.muted))
                            .child(sha),
                    )
                    .child(controls)
                    .child(
                        div()
                            .id("close-file")
                            .role(Role::Button)
                            .aria_label("Fermer le fichier")
                            .focusable()
                            .size(px(26.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(4.))
                            .cursor_pointer()
                            .hover(move |s| s.bg(rgb(p.surface)))
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
            root=root.child(notice("Premier parent uniquement aux merges. Les renommages détectés sont suivis ; copies et renommages ambigus peuvent interrompre le suivi.",p.muted,o));
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
                    .text_size(px(10. + o))
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
                                    "{} changements · {} commits parcourus",
                                    h.entries.len(),
                                    h.scanned
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
                            .cursor_pointer()
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
                self.image_preview.clone().into_any_element()
            } else {
                self.viewer.clone().into_any_element()
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
        .when(!disabled, |button| button.focusable())
        .flex()
        .items_center()
        .gap(px(6.))
        .px(px(7.))
        .py(px(4.))
        .rounded(px(4.))
        .border_1()
        .border_color(rgb(if selected { p.accent } else { p.border }))
        .bg(rgb(p.surface))
        .text_color(rgb(if selected { p.accent } else { p.text }))
        .text_size(px(10. + o))
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
        .text_size(px(10. + o))
        .text_color(rgb(color))
        .child(text.into())
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
    use super::{FilePresentation, FileTarget, PreparedDiff, PresentationChange};
    use mangrove_core::model::FileDiff;
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
    fn displayed_sha_keeps_seven_characters() {
        assert_eq!(super::short_sha("0123456789abcdef"), "0123456");
    }
}
