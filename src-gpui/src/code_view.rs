//! Read-only, virtualized code and patch viewer. Selection is expressed in
//! snapshot byte positions, so unmounted lines still participate in copying.
mod split;

use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    ops::Range,
    rc::Rc,
    sync::{
        Arc, LazyLock,
        atomic::{AtomicU64, Ordering},
    },
};

use accesskit::Role;
use gpui_kit::component::input::{Copy, SelectAll};
use gpui_kit::component::scroll::{Scrollbar, ScrollbarHandle};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::{assets::IconName, prelude::*, *};
use mangrove_core::model::{FileBlame, FileContent, FileDiff, Oversized, PreviewLimit};

use crate::{
    diff::{self, LineKind},
    format::{
        alt_shortcut_label, byte_size, byte_size_above, commit_date, compact_date, short_sha,
        shortcut_label,
    },
    preferences::{
        DEFAULT_DIFF_FONT_SIZE, DiffLayout, MAX_DIFF_FONT_SIZE, MIN_DIFF_FONT_SIZE, Preferences,
    },
    syntax::{self, Kind},
    theme::{MONO, Palette, RADIUS_MD, RADIUS_SM, SANS, icon, scrollbar_theme},
    virtual_scroll::{RowGeometry, VirtualScrollHandle},
};
use split::{SplitLayout, SplitRow};

#[derive(Clone, Debug)]
pub enum CodeViewEvent {
    NavigateCommit(String),
    FontSizeChanged(f32),
    LayoutChanged(DiffLayout),
    /// The diff switched between unified and side by side: the header's
    /// toggle shows another state.
    LayoutSwitched,
    Parent,
    /// Read the shown file again up to `PreviewLimit::MAX`.
    ShowLarge,
}

/// The button under the explanation of an empty view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StateAction {
    Parent,
    ShowLarge,
}

impl StateAction {
    fn label(self) -> &'static str {
        match self {
            Self::Parent => "Voir la version du premier parent",
            Self::ShowLarge => "Afficher quand même",
        }
    }

    fn event(self) -> CodeViewEvent {
        match self {
            Self::Parent => CodeViewEvent::Parent,
            Self::ShowLarge => CodeViewEvent::ShowLarge,
        }
    }
}

/// Why a file above its read's preview limit shows no contents, and whether
/// the highest limit can still show it.
fn oversized_state(oversized: Oversized, unread: &str) -> (String, Option<StateAction>) {
    let highest = PreviewLimit::MAX.bytes();
    if oversized.size > highest {
        return (
            format!(
                "Fichier de {} : {unread}. L’aperçu est limité à {}.",
                byte_size_above(oversized.size, highest),
                byte_size(highest)
            ),
            None,
        );
    }
    (
        format!(
            "Fichier de {}, au-delà de la limite d’aperçu de {} : {unread}.\nCette limite se règle dans Paramètres › Général.",
            byte_size_above(oversized.size, oversized.limit),
            byte_size(oversized.limit)
        ),
        Some(StateAction::ShowLarge),
    )
}

/// Why the blame of `content` is not read: blame keeps the background bound
/// whatever limit the preview was read with.
pub(crate) fn blame_unavailable(content: &FileContent) -> Option<String> {
    let (size, bound) = (content.size as u64, PreviewLimit::MIN.bytes());
    (matches!(content.kind.as_str(), "text" | "large") && size > bound).then(|| {
        format!(
            "Fichier de {} : le blame est disponible jusqu’à {}.",
            byte_size_above(size, bound),
            byte_size(bound)
        )
    })
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord)]
struct Position {
    row: usize,
    byte: usize,
}

#[derive(Clone, Copy, Debug)]
struct Selection {
    anchor: Position,
    head: Position,
}
impl Selection {
    fn ordered(self) -> (Position, Position) {
        if self.anchor <= self.head {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }
    fn range_in_row(self, row: usize, length: usize) -> Option<Range<usize>> {
        let (start, end) = self.ordered();
        if row < start.row || row > end.row || start == end {
            return None;
        }
        Some(
            (if row == start.row { start.byte } else { 0 })..(if row == end.row {
                end.byte
            } else {
                length
            }),
        )
    }
}

#[derive(Clone, Debug)]
struct DisplayLine {
    text: SharedString,
    /// Original byte positions mapped to display byte positions, only for tabs.
    mapping: Option<Vec<usize>>,
    original_end: usize,
    original_len: usize,
    content_end: usize,
    columns: usize,
}
// Bound expansion, shaping and glyph preparation even for a line made entirely
// of tabs. The omitted bytes remain in Row::original for selection and copying.
const MAX_DISPLAY_BYTES: usize = 16 * 1024;
const OMITTED_LINE: &str = " … [suite masquée ; sélectionner la ligne pour tout copier]";
static DESCRIPTION: LazyLock<SharedString> = LazyLock::new(|| {
    format!(
        "Texte sélectionnable. Flèches et Majuscule pour étendre la sélection, {} pour copier, {} pour tout sélectionner, {} et {} pour changer de bloc.",
        shortcut_label("C"),
        shortcut_label("A"),
        alt_shortcut_label("↑"),
        alt_shortcut_label("↓")
    )
    .into()
});

impl DisplayLine {
    fn new(text: Arc<str>) -> Self {
        let budget = MAX_DISPLAY_BYTES - OMITTED_LINE.len();
        let mut end = text.len().min(budget);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        if !text[..end].contains('\t') {
            let columns = text[..end].chars().count();
            let display = if end == text.len() {
                text.clone().into()
            } else {
                format!("{}{OMITTED_LINE}", &text[..end]).into()
            };
            return Self {
                text: display,
                mapping: None,
                original_end: end,
                original_len: text.len(),
                content_end: end,
                columns: columns
                    + if end < text.len() {
                        OMITTED_LINE.chars().count()
                    } else {
                        0
                    },
            };
        }
        let mut expanded = String::new();
        let mut map = vec![0];
        let mut column = 0;
        let mut original_end = 0;
        for (offset, ch) in text.char_indices() {
            let before = expanded.len();
            let bytes = if ch == '\t' {
                8 - column % 8
            } else {
                ch.len_utf8()
            };
            if before + bytes > budget {
                break;
            }
            if ch == '\t' {
                expanded.extend(std::iter::repeat_n(' ', bytes));
                column += bytes;
            } else {
                expanded.push(ch);
                column += 1;
            }
            map.extend(std::iter::repeat_n(before, ch.len_utf8() - 1));
            map.push(expanded.len());
            original_end = offset + ch.len_utf8();
        }
        let content_end = expanded.len();
        if original_end < text.len() {
            expanded.push_str(OMITTED_LINE);
            column += OMITTED_LINE.chars().count();
        }
        Self {
            text: expanded.into(),
            mapping: Some(map),
            original_end,
            original_len: text.len(),
            content_end,
            columns: column,
        }
    }
    fn truncated(&self) -> bool {
        self.original_end < self.original_len
    }
    fn display_byte(&self, original: usize) -> usize {
        if original > self.original_end {
            return self.text.len();
        }
        self.mapping
            .as_ref()
            .map_or(original, |map| map[original.min(map.len() - 1)])
    }
    fn original_byte(&self, display: usize) -> usize {
        if display > self.content_end {
            return self.original_len;
        }
        self.mapping.as_ref().map_or(display, |map| {
            let right = map.partition_point(|byte| *byte < display);
            if right == 0 {
                0
            } else if right >= map.len() {
                map.len() - 1
            } else if display - map[right - 1] < map[right] - display {
                right - 1
            } else {
                right
            }
        })
    }
}

#[derive(Clone, Debug)]
struct Row {
    kind: LineKind,
    old_number: String,
    new_number: String,
    original: Arc<str>,
    display: DisplayLine,
}
impl Row {
    fn new(kind: LineKind, old_number: String, new_number: String, text: &str) -> Self {
        let original: Arc<str> = text.into();
        let display = DisplayLine::new(original.clone());
        Self {
            kind,
            old_number,
            new_number,
            original,
            display,
        }
    }
}

/// Width of the column holding a diff line's sign.
const SIGN_WIDTH: f32 = 20.;
/// Space after the text of a diff line.
const DIFF_PADDING: f32 = 15.;
const DIVIDER_WIDTH: f32 = 1.;
/// Each side of a split diff shows at least this many characters.
const MIN_SPLIT_COLUMNS: f32 = 40.;

/// Why the diff shown cannot be laid out side by side.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SplitBlocker {
    NoText,
    OneSide,
    Narrow,
}

impl SplitBlocker {
    fn reason(self) -> &'static str {
        match self {
            Self::NoText => "Côte à côte indisponible : aucun diff texte à afficher",
            Self::OneSide => "Côte à côte indisponible : fichier ajouté ou supprimé",
            Self::Narrow => "Côte à côte indisponible : vue trop étroite",
        }
    }
}

struct CodeLineLayout {
    line: ShapedLine,
    bounds: Bounds<Pixels>,
}
impl CodeLineLayout {
    fn byte_at(&self, point: Point<Pixels>) -> usize {
        if point.y < self.bounds.top() {
            0
        } else if point.y > self.bounds.bottom() {
            self.line.len()
        } else {
            self.line.closest_index_for_x(point.x - self.bounds.left())
        }
    }
}

/// Keep full shaped geometry for hit testing and selection backgrounds, but send
/// only viewport glyphs to foreground paint. Preserve their positions and byte
/// indices, including fallback fonts and ligatures. Reordered runs stay intact.
fn visible_shaped_line(
    line: &ShapedLine,
    left: Pixels,
    right: Pixels,
    overhang: Pixels,
) -> ShapedLine {
    if left <= px(0.) && right >= line.width() {
        return line.clone();
    }
    let ordered = line
        .runs
        .iter()
        .flat_map(|run| &run.glyphs)
        .try_fold((px(f32::NEG_INFINITY), 0), |(x, byte), glyph| {
            (glyph.position.x >= x && glyph.index >= byte)
                .then_some((glyph.position.x, glyph.index))
        })
        .is_some();
    if !ordered {
        return line.clone();
    }
    // Include neighbors whose ink can extend beyond their advance positions.
    let margin = overhang;
    let runs = line
        .runs
        .iter()
        .filter_map(|run| {
            let start = run
                .glyphs
                .partition_point(|glyph| glyph.position.x < left - margin);
            let end = run
                .glyphs
                .partition_point(|glyph| glyph.position.x <= right + margin);
            (start < end).then(|| ShapedRun {
                font_id: run.font_id,
                glyphs: run.glyphs[start..end].to_vec(),
            })
        })
        .collect();
    let mut visible = line.clone();
    *visible = Arc::new(LineLayout {
        font_size: line.font_size,
        width: line.width(),
        ascent: line.ascent,
        descent: line.descent,
        runs,
        len: line.len(),
    });
    visible
}

/// Exact heights of the shown rows: hunk headers are taller, with a top
/// border after the first row.
fn row_geometry(
    hunks: impl IntoIterator<Item = bool>,
    font_size: f32,
    scale_factor: f32,
) -> RowGeometry {
    RowGeometry::from_tenths(
        hunks.into_iter().enumerate().map(|(index, hunk)| {
            f64::from(font_size) * 17.
                + if hunk {
                    100. + if index > 0 { 10. } else { 0. }
                } else {
                    0.
                }
        }),
        scale_factor,
    )
}

fn selected_rows_text(rows: &[Row], selection: Option<Selection>, final_newline: bool) -> String {
    let Some(selection) = selection else {
        return String::new();
    };
    let (start, end) = selection.ordered();
    if start == end {
        return String::new();
    }
    let mut text = String::new();
    for (index, row) in rows
        .iter()
        .enumerate()
        .take(end.row.saturating_add(1))
        .skip(start.row)
    {
        let range = selection
            .range_in_row(index, row.original.len())
            .unwrap_or(0..0);
        if index > start.row {
            text.push('\n');
        }
        text.push_str(&row.original[range]);
    }
    if final_newline
        && start == Position::default()
        && end.row + 1 == rows.len()
        && end.byte == rows[end.row].original.len()
    {
        text.push('\n');
    }
    text
}

/// Syntax tokens and changed words of each row, published together.
#[derive(Clone, Default)]
pub struct Highlights {
    pub tokens: syntax::Tokens,
    pub words: diff::Words,
    /// The paired rows of a prepared patch, which the side-by-side layout
    /// reuses instead of pairing them again on the UI thread.
    pub pairs: Option<diff::Pairs>,
}

impl Highlights {
    /// Pairs and changed words, then syntax, of a patch: the inspector and
    /// the file browser publish them all or none. `None` means cancellation.
    pub fn of_diff(path: &str, diff: &FileDiff, should_cancel: impl Fn() -> bool) -> Option<Self> {
        #[cfg(feature = "app-benchmark")]
        let words_start = std::time::Instant::now();
        let (kinds, texts) = diff::rows(diff);
        let pairs = diff::pairs(&kinds, &texts, &should_cancel)?;
        let words = diff::changed_words(&texts, &pairs, &should_cancel)?;
        #[cfg(feature = "app-benchmark")]
        crate::perf::record_operation("inline", words_start.elapsed());
        #[cfg(feature = "app-benchmark")]
        let syntax_start = std::time::Instant::now();
        let tokens = syntax::cached_diff_tokens_cancellable(path, diff, &should_cancel)?;
        #[cfg(feature = "app-benchmark")]
        crate::perf::record_operation("syntax", syntax_start.elapsed());
        Some(Self {
            tokens,
            words,
            pairs: Some(Arc::new(pairs)),
        })
    }
    pub fn same(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.tokens, &other.tokens)
            && Arc::ptr_eq(&self.words, &other.words)
            && self.pairs.as_ref().map(Arc::as_ptr) == other.pairs.as_ref().map(Arc::as_ptr)
    }
}

/// A header row per hunk, then a row per line: the indexing of `Highlights`.
fn diff_rows(diff: &FileDiff) -> Vec<Row> {
    diff.hunks
        .iter()
        .flat_map(|hunk| {
            std::iter::once(Row::new(
                LineKind::Hunk,
                String::new(),
                String::new(),
                hunk.header.trim_end(),
            ))
            .chain(hunk.lines.iter().map(|line| {
                Row::new(
                    LineKind::of(&line.kind),
                    line.old_line.map(|n| n.to_string()).unwrap_or_default(),
                    line.new_line.map(|n| n.to_string()).unwrap_or_default(),
                    diff::strip_line_ending(&line.content),
                )
            }))
        })
        .collect()
}

/// The row read at the top of the view, as a unified row: it is the same
/// line whether the diff is shown unified or side by side.
#[derive(Clone, Debug, PartialEq)]
pub struct ReadingPosition {
    /// Whether the position is in the diff, and the file path.
    source: (bool, String),
    row: usize,
    offset: Pixels,
}

pub struct CodeView {
    rows: Arc<Vec<Row>>,
    /// Shown instead of `rows` while the diff is laid out side by side.
    split: Option<SplitLayout>,
    layout: DiffLayout,
    /// The host shows this view centrally, where side by side is offered.
    split_host: bool,
    both_sides: bool,
    /// Whether two columns fit the last two-sided diff: it sets the width
    /// threshold of the next decision, whatever the files shown in between.
    split_fits: bool,
    /// The text of the layout toggle's tooltip, for its current state.
    layout_tooltip: Rc<RefCell<SharedString>>,
    /// Measured at each paint; unknown before the first one in a host.
    viewport_width: Rc<Cell<f32>>,
    scroll: VirtualScrollHandle,
    geometry_scale: f32,
    diff_source: Option<(Arc<FileDiff>, String)>,
    content_source: Option<Arc<FileContent>>,
    /// Reading position to apply once the same diff or file is shown again.
    restored_position: Option<ReadingPosition>,
    highlights: Highlights,
    generation: u64,
    live_generation: Arc<AtomicU64>,
    syntax_task: Option<Task<()>>,
    syntax_pending: bool,
    focus: FocusHandle,
    list: ListState,
    horizontal: ScrollHandle,
    layouts: Rc<RefCell<BTreeMap<usize, CodeLineLayout>>>,
    palette: Palette,
    font_size: f32,
    ui_offset: f32,
    text_width: f32,
    /// The widest text of the old and new sides, measured for a split diff.
    side_widths: [f32; 2],
    char_width: f32,
    number_width: f32,
    is_diff: bool,
    hunk_rows: Vec<usize>,
    selection: Option<Selection>,
    dragging: bool,
    drag_point: Option<Point<Pixels>>,
    active_line: usize,
    show_blame: bool,
    /// Replaces the file while blame is shown, when blame cannot run.
    blame_unavailable: Option<String>,
    blame: Option<Arc<FileBlame>>,
    blame_indices: Vec<Option<usize>>,
    notice: Option<String>,
    state: Option<String>,
    action: Option<StateAction>,
    /// While set, an empty view names what is being read.
    loading: Option<String>,
    error: Option<String>,
    ends_with_newline: bool,
}
impl EventEmitter<CodeViewEvent> for CodeView {}
impl Focusable for CodeView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl CodeView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let list = ListState::new(0, ListAlignment::Top, px(150.));
        let horizontal = ScrollHandle::new();
        let scroll =
            VirtualScrollHandle::exact(list.clone(), horizontal.clone(), RowGeometry::default());
        Self {
            rows: Arc::new(Vec::new()),
            split: None,
            layout: DiffLayout::default(),
            split_host: false,
            both_sides: false,
            split_fits: false,
            layout_tooltip: Rc::default(),
            viewport_width: Rc::new(Cell::new(0.)),
            scroll,
            geometry_scale: window.scale_factor(),
            diff_source: None,
            content_source: None,
            restored_position: None,
            highlights: Highlights::default(),
            generation: 0,
            live_generation: Arc::new(AtomicU64::new(0)),
            syntax_task: None,
            syntax_pending: false,
            focus: cx.focus_handle().tab_stop(true),
            list,
            horizontal,
            layouts: Rc::new(RefCell::new(BTreeMap::new())),
            palette: Palette::default(),
            font_size: DEFAULT_DIFF_FONT_SIZE,
            ui_offset: 1.,
            text_width: 0.,
            side_widths: [0.; 2],
            char_width: 7.,
            number_width: 32.,
            is_diff: false,
            hunk_rows: Vec::new(),
            selection: None,
            dragging: false,
            drag_point: None,
            active_line: 0,
            show_blame: false,
            blame_unavailable: None,
            blame: None,
            blame_indices: Vec::new(),
            notice: None,
            state: Some("Sélectionnez un fichier pour voir ses modifications.".into()),
            action: None,
            loading: None,
            error: None,
            ends_with_newline: false,
        }
    }

    fn invalidate_syntax(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.live_generation
            .store(self.generation, Ordering::Release);
    }
    fn reset(
        &mut self,
        rows: Vec<Row>,
        highlights: Highlights,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.invalidate_syntax();
        self.syntax_task = None;
        self.syntax_pending = false;
        self.rows = Arc::new(rows);
        self.diff_source = None;
        self.content_source = None;
        self.highlights = highlights;
        self.selection = None;
        self.dragging = false;
        self.drag_point = None;
        self.active_line = 0;
        self.state = None;
        self.action = None;
        self.blame_unavailable = None;
        self.notice = None;
        self.error = None;
        self.loading = None;
        self.blame = None;
        self.blame_indices.clear();
        self.geometry_scale = window.scale_factor();
        self.horizontal.set_offset(point(px(0.), px(0.)));
        self.both_sides = split::has_both_sides(self.rows.iter().map(|row| row.kind));
        self.measure(window);
        self.refresh_split_fit();
        self.lay_out(self.wants_split(), window);
        cx.notify();
    }
    fn item_count(&self) -> usize {
        self.split
            .as_ref()
            .map_or(self.rows.len(), |split| split.rows.len())
    }
    fn geometry(&self, scale_factor: f32) -> RowGeometry {
        match &self.split {
            Some(split) => row_geometry(
                split
                    .rows
                    .iter()
                    .map(|row| matches!(row, SplitRow::Hunk(_))),
                self.font_size,
                scale_factor,
            ),
            None => row_geometry(
                self.rows.iter().map(|row| row.kind == LineKind::Hunk),
                self.font_size,
                scale_factor,
            ),
        }
    }
    /// Shows the rows unified or side by side, from the top.
    fn lay_out(&mut self, split: bool, window: &Window) {
        self.split = split.then(|| {
            let kinds: Vec<_> = self.rows.iter().map(|row| row.kind).collect();
            // A patch shown before its preparation is paired here, with the
            // same function.
            let pairs = self.highlights.pairs.clone().unwrap_or_else(|| {
                let texts: Vec<_> = self.rows.iter().map(|row| &*row.original).collect();
                Arc::new(diff::pairs(&kinds, &texts, || false).unwrap_or_default())
            });
            SplitLayout::new(&kinds, &pairs)
        });
        if split {
            self.measure_sides(window);
        }
        self.hunk_rows = match &self.split {
            Some(split) => (0..split.rows.len())
                .filter(|&index| matches!(split.rows[index], SplitRow::Hunk(_)))
                .collect(),
            None => hunk_rows(&self.rows),
        };
        self.layouts.borrow_mut().clear();
        self.scroll.set_geometry(self.geometry(self.geometry_scale));
        self.list.reset(self.item_count());
    }
    fn wants_split(&self) -> bool {
        self.layout == DiffLayout::Split && self.split_host && self.split_blocker().is_none()
    }
    fn split_blocker(&self) -> Option<SplitBlocker> {
        if !self.is_diff || self.rows.is_empty() {
            Some(SplitBlocker::NoText)
        } else if !self.both_sides {
            Some(SplitBlocker::OneSide)
        } else if !self.split_fits {
            Some(SplitBlocker::Narrow)
        } else {
            None
        }
    }
    /// Decides again whether two columns fit, for a two-sided diff in a
    /// central view. The last decision sets the threshold.
    fn refresh_split_fit(&mut self) {
        if self.split_host && self.is_diff && self.both_sides {
            self.split_fits = split::fits(
                self.viewport_width.get(),
                self.split_minimum(),
                self.split_fits,
            );
        }
    }
    /// Switches between unified and side by side once the preference, the
    /// host or the width allows another layout, keeping the line read at the
    /// top. A selection does not survive the change.
    fn update_layout(&mut self, window: &Window, cx: &mut Context<Self>) {
        self.refresh_split_fit();
        let split = self.wants_split();
        if split == self.split.is_some() {
            return;
        }
        let anchor = self.anchor();
        let active = self.unified_row(self.active_line);
        self.selection = None;
        self.dragging = false;
        self.drag_point = None;
        self.lay_out(split, window);
        self.active_line = self.display_row(active);
        self.horizontal.set_offset(point(px(0.), px(0.)));
        self.scroll_to_anchor(anchor);
        cx.notify();
        cx.emit(CodeViewEvent::LayoutSwitched);
    }
    /// The unified row shown on a displayed row, and back.
    fn unified_row(&self, index: usize) -> usize {
        self.split
            .as_ref()
            .map_or(index, |split| split.unified_of(index))
    }
    fn display_row(&self, row: usize) -> usize {
        self.split.as_ref().map_or(row, |split| split.row_of(row))
    }
    /// The unified row at the top of the view, and how far it is scrolled.
    fn anchor(&self) -> (usize, Pixels) {
        let top = self.list.logical_scroll_top();
        (self.unified_row(top.item_ix), top.offset_in_item)
    }
    fn scroll_to_anchor(&mut self, (row, offset): (usize, Pixels)) {
        self.list.scroll_to(ListOffset {
            item_ix: self.display_row(row),
            offset_in_item: offset,
        });
    }
    fn measure(&mut self, window: &Window) {
        let largest = self
            .rows
            .iter()
            .flat_map(|row| {
                [
                    row.old_number.parse::<usize>().unwrap_or(0),
                    row.new_number.parse::<usize>().unwrap_or(0),
                ]
            })
            .max()
            .unwrap_or(1);
        let run = TextRun {
            len: 1,
            font: font(MONO),
            color: rgb(self.palette.text).into(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        self.char_width = window
            .text_system()
            .shape_line("0".into(), px(self.font_size), &[run], None)
            .width()
            .into();
        self.number_width = (largest.to_string().len() + 2) as f32 * self.char_width;
        self.text_width = self.widest(self.rows.iter(), window);
    }
    /// Each side of a split diff shows every line but the other side's.
    fn measure_sides(&mut self, window: &Window) {
        self.side_widths = [LineKind::Addition, LineKind::Deletion].map(|other| {
            self.widest(
                self.rows
                    .iter()
                    .filter(|row| row.kind != other && row.kind != LineKind::Hunk),
                window,
            )
        });
    }
    /// The shaped width of the line with the most columns.
    fn widest<'a>(&self, rows: impl Iterator<Item = &'a Row>, window: &Window) -> f32 {
        rows.max_by_key(|row| row.display.columns)
            .map_or(0., |row| {
                let run = TextRun {
                    len: row.display.text.len(),
                    font: font(MONO),
                    color: rgb(self.palette.text).into(),
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                };
                window
                    .text_system()
                    .shape_line(row.display.text.clone(), px(self.font_size), &[run], None)
                    .width()
                    .into()
            })
    }
    fn blame_width(&self) -> f32 {
        // Short SHA and compact date stay whole; the author takes the rest.
        self.font_size * 20. + 32.
    }
    /// Line numbers, diff signs and blame stay visible during horizontal scroll.
    fn gutter_width(&self) -> f32 {
        if self.is_diff {
            self.number_width * 2. + SIGN_WIDTH
        } else {
            self.number_width
                + 12.
                + if self.show_blame {
                    self.blame_width()
                } else {
                    0.
                }
        }
    }
    /// A split diff scrolls both sides together, as far as its widest side.
    fn content_width(&self) -> f32 {
        if self.split.is_some() {
            let viewport = self.viewport_width.get();
            let widest = self.side_widths[0].max(self.side_widths[1]);
            return viewport + (widest - self.side_text_width(viewport)).max(0.);
        }
        self.text_width + self.gutter_width() + if self.is_diff { DIFF_PADDING } else { 20. }
    }
    /// The text area of each side of a split view `viewport` wide.
    fn side_text_width(&self, viewport: f32) -> f32 {
        (viewport - DIVIDER_WIDTH) / 2. - self.number_width - SIGN_WIDTH - DIFF_PADDING
    }
    fn split_minimum(&self) -> f32 {
        2. * (self.number_width + SIGN_WIDTH + DIFF_PADDING + MIN_SPLIT_COLUMNS * self.char_width)
            + DIVIDER_WIDTH
    }
    fn append_line_notice(&mut self) {
        if self.rows.iter().any(|row| row.display.truncated()) {
            let notice = self.notice.get_or_insert_with(String::new);
            if !notice.is_empty() {
                notice.push(' ');
            }
            notice.push_str("Lignes longues : aperçu limité à 16 Kio après développement des tabulations. La sélection d’une ligne et Tout sélectionner copient son texte original complet.");
        }
    }
    fn row_height(&self, index: usize) -> Pixels {
        self.scroll
            .row_height(index)
            .expect("rendered row has geometry")
    }
    fn rebuild_row_geometry(&mut self, scale_factor: f32) {
        let top = self.list.logical_scroll_top();
        let old_height = (top.item_ix < self.item_count()).then(|| self.row_height(top.item_ix));
        self.geometry_scale = scale_factor;
        self.scroll.set_geometry(self.geometry(scale_factor));
        let scale = old_height.map_or(1., |old| {
            f32::from(self.row_height(top.item_ix)) / f32::from(old)
        });
        self.list.reset(self.item_count());
        self.list.scroll_to(ListOffset {
            item_ix: top.item_ix,
            offset_in_item: top.offset_in_item * scale,
        });
    }
    pub fn set_prepared_diff(
        &mut self,
        diff: Arc<FileDiff>,
        path: String,
        highlights: Highlights,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let unchanged = self
            .diff_source
            .as_ref()
            .is_some_and(|(source, source_path)| {
                Arc::ptr_eq(source, &diff) && source_path == &path
            });
        if !unchanged {
            self.is_diff = true;
            self.ends_with_newline = false;
            self.reset(diff_rows(&diff), highlights.clone(), window, cx);
            self.diff_source = Some((diff.clone(), path.clone()));
            self.apply_reading_position();
            if let Some(oversized) = diff.oversized {
                let (state, action) = oversized_state(oversized, "son diff n’est pas calculé");
                self.state = Some(state);
                self.action = action;
            } else if diff.binary {
                self.state=Some("Fichier binaire\nLe contenu de ce fichier ne peut pas être affiché sous forme de diff texte.".into());
            } else if diff.hunks.is_empty() && !diff.truncated {
                self.state=Some("Aucune différence de contenu texte. Le changement peut concerner le nom ou les permissions du fichier.".into());
            } else if diff.truncated {
                self.notice = Some("Aperçu limité à 2 000 lignes et 512 Kio de diff.".into());
            }
            self.append_line_notice();
        }
        self.invalidate_syntax();
        self.syntax_task = None;
        self.syntax_pending = false;
        if !self.highlights.same(&highlights) {
            self.highlights = highlights;
            cx.notify();
        }
    }
    pub fn set_content(
        &mut self,
        content: Arc<FileContent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self
            .content_source
            .as_ref()
            .is_some_and(|source| Arc::ptr_eq(source, &content))
        {
            return;
        }
        self.is_diff = false;
        self.ends_with_newline = content.ends_with_newline;
        let rows = content
            .lines
            .iter()
            .enumerate()
            .map(|(index, text)| {
                Row::new(
                    LineKind::Content,
                    String::new(),
                    (index + 1).to_string(),
                    text,
                )
            })
            .collect();
        self.reset(rows, Highlights::default(), window, cx);
        self.content_source = Some(content.clone());
        self.apply_reading_position();
        self.blame_unavailable = blame_unavailable(&content);
        self.state = if let Some(oversized) = content.oversized {
            let (state, action) = oversized_state(oversized, "son contenu n’est pas chargé");
            self.action = action;
            Some(state)
        } else {
            match content.kind.as_str() {
                "binary" => Some(
                    "Fichier binaire ou encodage non UTF-8 : aucun affichage texte ni blame."
                        .into(),
                ),
                "deleted" => {
                    self.action = content.parent_sha.is_some().then_some(StateAction::Parent);
                    Some("Ce fichier est absent à la révision sélectionnée.".into())
                }
                "submodule" => {
                    Some("Sous-module : son contenu appartient à un autre dépôt.".into())
                }
                "unsupported" => {
                    Some("Ce type d’objet Git ne peut pas être affiché comme un fichier.".into())
                }
                _ if content.lines.is_empty() && !content.truncated => {
                    Some("Fichier vide à cette révision.".into())
                }
                _ => None,
            }
        };
        if content.truncated && content.oversized.is_none() {
            self.notice = Some(if self.blame_unavailable.is_some() {
                "Aperçu limité à 2 000 lignes et 256 Kio de texte.".into()
            } else {
                "Aperçu limité à 2 000 lignes et 256 Kio de texte. Le blame concerne uniquement les lignes affichées.".into()
            });
        } else if content.size > 0 && !content.ends_with_newline {
            self.notice = Some("Pas de saut de ligne à la fin du fichier.".into());
        }
        self.append_line_notice();
        let generation = self.generation;
        let live_generation = self.live_generation.clone();
        self.syntax_pending = true;
        self.syntax_task = Some(cx.spawn(async move |this, cx| {
            let tokens = cx
                .background_executor()
                .spawn(async move {
                    syntax::cached_content_tokens_cancellable(&content.path, &content.lines, || {
                        live_generation.load(Ordering::Acquire) != generation
                    })
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.generation == generation
                    && let Some(tokens) = tokens
                {
                    view.highlights = Highlights {
                        tokens,
                        ..Highlights::default()
                    };
                    view.syntax_pending = false;
                    view.syntax_task = None;
                    cx.notify();
                }
            });
        }));
        cx.notify();
    }
    /// The shown diff or file and how far it was scrolled.
    pub fn reading_position(&self) -> Option<ReadingPosition> {
        let (row, offset) = self.anchor();
        Some(ReadingPosition {
            source: self.source_key()?,
            row,
            offset,
        })
    }
    /// Scrolls back to `position` when its diff or file is shown next.
    pub fn restore_reading_position(&mut self, position: ReadingPosition) {
        self.restored_position = Some(position);
    }
    fn source_key(&self) -> Option<(bool, String)> {
        match (&self.diff_source, &self.content_source) {
            (Some((_, path)), _) if self.is_diff => Some((true, path.clone())),
            (_, Some(content)) => Some((false, content.path.clone())),
            _ => None,
        }
    }
    /// The diff of a file can precede its content view; only another path
    /// discards the position.
    fn apply_reading_position(&mut self) {
        let Some(source) = self.source_key() else {
            return;
        };
        let Some(position) = self.restored_position.take() else {
            return;
        };
        if source == position.source {
            self.scroll_to_anchor((position.row, position.offset));
        } else if source.1 == position.source.1 {
            self.restored_position = Some(position);
        }
    }
    pub fn set_blame(&mut self, blame: Option<Arc<FileBlame>>, cx: &mut Context<Self>) {
        self.blame_indices = vec![None; self.rows.len()];
        if let Some(blame) = &blame {
            for (index, group) in blame.groups.iter().enumerate() {
                let start = group.start_line.saturating_sub(1).min(self.rows.len());
                let end = start.saturating_add(group.lines).min(self.rows.len());
                self.blame_indices[start..end].fill(Some(index));
            }
        }
        self.blame = blame;
        cx.notify();
    }
    pub fn set_show_blame(&mut self, value: bool, cx: &mut Context<Self>) {
        if self.show_blame != value {
            self.show_blame = value;
            cx.notify();
        }
    }
    pub fn set_theme(&mut self, palette: Palette, cx: &mut Context<Self>) {
        if self.palette != palette {
            self.palette = palette;
            cx.notify();
        }
    }
    pub fn set_preferences(&mut self, preferences: Preferences, cx: &mut Context<Self>) {
        let old = self.font_size;
        self.ui_offset = preferences.font_size_offset;
        self.layout = preferences.diff_layout;
        self.palette = Palette::for_theme(preferences.theme);
        self.font_size = if preferences.diff_font_size.is_finite() {
            preferences
                .diff_font_size
                .round()
                .clamp(MIN_DIFF_FONT_SIZE, MAX_DIFF_FONT_SIZE)
        } else {
            DEFAULT_DIFF_FONT_SIZE
        };
        if old != self.font_size {
            let ratio = self.font_size / old;
            self.text_width *= ratio;
            self.side_widths = self.side_widths.map(|width| width * ratio);
            self.char_width *= ratio;
            self.number_width *= ratio;
            self.rebuild_row_geometry(self.geometry_scale);
        }
        cx.notify();
    }
    /// Only central views offer the side-by-side layout. A width measured
    /// in another host does not decide the first layout in this one.
    pub fn set_split_host(&mut self, central: bool, cx: &mut Context<Self>) {
        if self.split_host != central {
            self.split_host = central;
            self.viewport_width.set(0.);
            cx.notify();
        }
    }
    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.invalidate_syntax();
        self.syntax_task = None;
        self.syntax_pending = false;
        self.rows = Arc::new(Vec::new());
        self.split = None;
        self.both_sides = false;
        self.scroll.set_geometry(RowGeometry::default());
        self.diff_source = None;
        self.content_source = None;
        self.highlights = Highlights::default();
        self.selection = None;
        self.dragging = false;
        self.drag_point = None;
        self.state = Some("Sélectionnez un fichier pour voir ses modifications.".into());
        self.error = None;
        self.loading = None;
        self.notice = None;
        self.action = None;
        self.blame_unavailable = None;
        self.blame = None;
        self.blame_indices.clear();
        self.hunk_rows.clear();
        self.text_width = 0.;
        self.ends_with_newline = false;
        self.list.reset(0);
        self.layouts.borrow_mut().clear();
        cx.notify();
    }
    /// `Some` dims the retained rows, or names the read when none are shown.
    pub fn set_loading(&mut self, value: Option<String>, cx: &mut Context<Self>) {
        if self.loading == value {
            return;
        }
        self.loading = value;
        cx.notify();
    }
    pub fn set_error(&mut self, error: Option<String>, cx: &mut Context<Self>) {
        if self.error == error {
            return;
        }
        self.error = error;
        cx.notify();
    }
    #[cfg(feature = "app-benchmark")]
    pub fn is_ready(&self) -> bool {
        self.loading.is_none() && !self.syntax_pending
    }
    #[cfg(feature = "app-benchmark")]
    pub fn is_split(&self) -> bool {
        self.split.is_some()
    }
    #[cfg(feature = "app-benchmark")]
    pub fn syntax_colored(&self) -> bool {
        self.highlights
            .tokens
            .iter()
            .flatten()
            .any(|token| token.kind != Kind::Plain)
    }
    #[cfg(feature = "app-benchmark")]
    pub fn scroll_by_pixels(&mut self, delta: f32, cx: &mut Context<Self>) {
        self.scroll.scroll_vertical_by(px(delta));
        cx.notify();
    }
    pub fn font_controls(&self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let palette = self.palette;
        let size = self.font_size;
        let button = |id: &'static str, name: IconName, enabled: bool, delta: f32| {
            let label = if delta < 0. {
                "Diminuer la taille du texte du diff"
            } else {
                "Augmenter la taille du texte du diff"
            };
            let change = move |view: &mut Self, cx: &mut Context<Self>| {
                if enabled {
                    cx.emit(CodeViewEvent::FontSizeChanged(
                        (view.font_size + delta).clamp(MIN_DIFF_FONT_SIZE, MAX_DIFF_FONT_SIZE),
                    ));
                }
            };
            icon_button(id, name, label, move || label.into(), enabled, palette)
                .on_click(cx.listener(move |view, _, _, cx| change(view, cx)))
                .on_key_down(cx.listener(move |view, event: &KeyDownEvent, _, cx| {
                    if is_activation(event) {
                        change(view, cx);
                        cx.stop_propagation();
                    }
                }))
        };
        div()
            .flex()
            .items_center()
            .gap(px(2.))
            .font_family(SANS)
            .child(button(
                "diff-font-minus",
                IconName::Minus,
                size > MIN_DIFF_FONT_SIZE,
                -1.,
            ))
            .child(
                div()
                    .min_w(px(36.))
                    .text_center()
                    .text_size(px(11. + self.ui_offset))
                    .text_color(rgb(palette.muted))
                    .child(format!("{size:.0} px")),
            )
            .child(button(
                "diff-font-plus",
                IconName::Plus,
                size < MAX_DIFF_FONT_SIZE,
                1.,
            ))
            .into_any_element()
    }
    /// Previous/next hunk buttons for a diff header, with their shortcuts.
    pub fn hunk_controls(&self, cx: &mut Context<Self>) -> AnyElement {
        let palette = self.palette;
        let enabled = !self.hunk_rows.is_empty();
        let button = |id: &'static str, name: IconName, label: &'static str, forward: bool| {
            let key = if forward { "↓" } else { "↑" };
            let tooltip = SharedString::from(format!("{label} ({})", alt_shortcut_label(key)));
            icon_button(id, name, label, move || tooltip.clone(), enabled, palette)
                .on_click(cx.listener(move |view, _, _, cx| view.jump_hunk(forward, cx)))
                .on_key_down(cx.listener(move |view, event: &KeyDownEvent, _, cx| {
                    if is_activation(event) {
                        view.jump_hunk(forward, cx);
                        cx.stop_propagation();
                    }
                }))
        };
        div()
            .flex()
            .items_center()
            .gap(px(2.))
            .child(button(
                "diff-hunk-previous",
                IconName::ChevronUp,
                "Bloc précédent",
                false,
            ))
            .child(button(
                "diff-hunk-next",
                IconName::ChevronDown,
                "Bloc suivant",
                true,
            ))
            .into_any_element()
    }
    /// Whether the diff is shown side by side, and else why it cannot be:
    /// the state of the header's toggle, `None` where it is hidden. Hosts
    /// build their header before this view renders, so the layout is
    /// brought up to date first.
    fn layout_state(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<(bool, Option<SplitBlocker>)> {
        if !self.split_host || !self.is_diff {
            return None;
        }
        self.update_layout(window, cx);
        let split = self.split.is_some();
        Some((split, if split { None } else { self.split_blocker() }))
    }
    /// The unified / side-by-side switch of a central diff header, disabled
    /// with its reason while this diff stays unified whatever the preference.
    pub fn layout_controls(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let (split, blocker) = self.layout_state(window, cx)?;
        let (name, label, target) = if split {
            (
                IconName::Rows2,
                "Afficher le diff unifié",
                DiffLayout::Unified,
            )
        } else {
            (
                IconName::Columns2,
                "Afficher le diff côte à côte",
                DiffLayout::Split,
            )
        };
        let enabled = blocker.is_none();
        // A shown tooltip keeps its view while hovered: it reads this text.
        *self.layout_tooltip.borrow_mut() = blocker.map_or(label, SplitBlocker::reason).into();
        let tooltip = self.layout_tooltip.clone();
        let change = move |cx: &mut Context<Self>| {
            if enabled {
                cx.emit(CodeViewEvent::LayoutChanged(target));
            }
        };
        Some(
            icon_button(
                "diff-layout",
                name,
                label,
                move || tooltip.borrow().clone(),
                enabled,
                self.palette,
            )
            .on_click(cx.listener(move |_, _, _, cx| change(cx)))
            .into_any_element(),
        )
    }
    /// Scroll the previous or next hunk header to the top and move the caret there.
    pub fn jump_hunk(&mut self, forward: bool, cx: &mut Context<Self>) {
        let viewport = self.list.viewport_bounds();
        let caret_visible = self
            .list
            .bounds_for_item(self.active_line)
            .is_some_and(|bounds| {
                bounds.bottom() > viewport.top() && bounds.top() < viewport.bottom()
            });
        let current = if caret_visible {
            self.active_line
        } else {
            self.list.logical_scroll_top().item_ix
        };
        let Some(row) = hunk_target(&self.hunk_rows, current, forward) else {
            return;
        };
        let caret = Position { row, byte: 0 };
        self.active_line = row;
        // A side-by-side diff has no caret: its reading line moves alone.
        self.selection = self.split.is_none().then_some(Selection {
            anchor: caret,
            head: caret,
        });
        let start = (0..row)
            .filter_map(|index| self.scroll.row_height(index))
            .fold(px(0.), |start, height| start + height);
        self.scroll.scroll_vertical_to(start);
        cx.notify();
    }

    fn position_from_layout(&self, index: usize, position: Point<Pixels>) -> Position {
        let byte = self
            .layouts
            .borrow()
            .get(&index)
            .map_or(0, |layout| layout.byte_at(position));
        let row = &self.rows[index];
        let mut byte = row.display.original_byte(byte).min(row.original.len());
        while !row.original.is_char_boundary(byte) {
            byte = byte.saturating_sub(1);
        }
        Position { row: index, byte }
    }
    fn begin_selection(
        &mut self,
        index: usize,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus, cx);
        self.active_line = index;
        if self.split.is_some() {
            cx.stop_propagation();
            cx.notify();
            return;
        }
        let position = self.position_from_layout(index, event.position);
        let row = &self.rows[index];
        if event.click_count >= 3 {
            self.selection = Some(Selection {
                anchor: Position {
                    row: index,
                    byte: 0,
                },
                head: Position {
                    row: index,
                    byte: row.original.len(),
                },
            });
        } else if event.click_count == 2 {
            let word = |ch: char| ch.is_alphanumeric() || ch == '_' || ch == '$';
            let start = row.original[..position.byte]
                .char_indices()
                .rev()
                .take_while(|(_, ch)| word(*ch))
                .last()
                .map_or(position.byte, |(offset, _)| offset);
            let end = position.byte
                + row.original[position.byte..]
                    .chars()
                    .take_while(|ch| word(*ch))
                    .map(char::len_utf8)
                    .sum::<usize>();
            self.selection = Some(Selection {
                anchor: Position {
                    row: index,
                    byte: start,
                },
                head: Position {
                    row: index,
                    byte: end,
                },
            });
        } else if event.modifiers.shift {
            self.selection = Some(Selection {
                anchor: self
                    .selection
                    .map_or(position, |selection| selection.anchor),
                head: position,
            });
        } else {
            self.selection = Some(Selection {
                anchor: position,
                head: position,
            });
        }
        self.dragging = true;
        self.drag_point = Some(event.position);
        cx.stop_propagation();
        cx.notify();
    }
    fn drag_selection(
        &mut self,
        event: &MouseMoveEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.dragging || self.rows.is_empty() {
            return;
        }
        if event.pressed_button != Some(MouseButton::Left) {
            self.dragging = false;
            self.drag_point = None;
            cx.notify();
            return;
        }
        self.drag_point = Some(event.position);
        let viewport = self.list.viewport_bounds();
        let point = event.position;
        let mut target = None;
        for index in self.layouts.borrow().keys() {
            if let Some(bounds) = self.list.bounds_for_item(*index)
                && point.y >= bounds.top()
                && point.y <= bounds.bottom()
            {
                target = Some(*index);
                break;
            }
        }
        if point.y < viewport.top() || point.y > viewport.bottom() {
            // Only the animation loop changes scroll position; event frequency
            // must not add a second source of scrolling during a drag.
            window.request_animation_frame();
            return;
        }
        if let Some(index) = target {
            let head = self.position_from_layout(index, point);
            if let Some(selection) = &mut self.selection
                && selection.head != head
            {
                selection.head = head;
                cx.notify();
            }
        }
    }
    fn autoscroll_selection(&mut self, window: &mut Window) {
        let Some(point) = self
            .drag_point
            .filter(|_| self.dragging && !self.rows.is_empty())
        else {
            return;
        };
        let viewport = self.list.viewport_bounds();
        let direction = if point.y < viewport.top() {
            -1.
        } else if point.y > viewport.bottom() {
            1.
        } else {
            return;
        };
        let top = self.list.logical_scroll_top();
        let handle = &self.scroll;
        let offset = handle.vertical_offset();
        let maximum = (handle.content_size().height - viewport.size.height).max(px(0.));
        if (direction < 0. && offset <= px(0.)) || (direction > 0. && offset >= maximum) {
            return;
        }
        let visible: Vec<_> = self
            .layouts
            .borrow()
            .keys()
            .copied()
            .filter(|index| {
                self.list.bounds_for_item(*index).is_some_and(|bounds| {
                    bounds.bottom() >= viewport.top() && bounds.top() <= viewport.bottom()
                })
            })
            .collect();
        let index = if direction < 0. {
            visible
                .first()
                .copied()
                .unwrap_or(top.item_ix)
                .saturating_sub(1)
        } else {
            visible
                .last()
                .copied()
                .unwrap_or(top.item_ix)
                .saturating_add(1)
                .min(self.rows.len() - 1)
        };
        self.scroll
            .scroll_vertical_by(px(direction * self.font_size * 1.7));
        if let Some(selection) = &mut self.selection {
            selection.head = Position {
                row: index,
                byte: if direction < 0. {
                    0
                } else {
                    self.rows[index].original.len()
                },
            };
        }
        // Frame requests exist only during an active drag beyond the viewport.
        window.request_animation_frame();
    }
    pub fn selected_text(&self) -> String {
        selected_rows_text(
            &self.rows,
            self.selection,
            !self.is_diff && self.ends_with_newline,
        )
    }
    fn copy_selection(&self, cx: &mut Context<Self>) {
        let text = self.selected_text();
        if !text.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
        cx.stop_propagation();
    }
    fn select_all(&mut self, cx: &mut Context<Self>) {
        if self.rows.is_empty() || self.split.is_some() {
            return;
        }
        let row = self.rows.len() - 1;
        self.selection = Some(Selection {
            anchor: Position::default(),
            head: Position {
                row,
                byte: self.rows[row].original.len(),
            },
        });
        cx.stop_propagation();
        cx.notify();
    }
    fn keydown(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let modifiers = event.keystroke.modifiers;
        let key = event.keystroke.key.as_str();
        if (modifiers.platform || modifiers.control) && key == "c" {
            self.copy_selection(cx);
            return;
        }
        if self.rows.is_empty() {
            return;
        }
        if (modifiers.platform || modifiers.control) && key == "a" {
            self.select_all(cx);
            return;
        }
        if let Some(forward) = hunk_shortcut(event) {
            self.jump_hunk(forward, cx);
            cx.stop_propagation();
            return;
        }
        if self.show_blame && matches!(key, "enter" | "space") && !modifiers.shift {
            if let Some(group) = self.blame_group(self.active_line) {
                cx.emit(CodeViewEvent::NavigateCommit(group.sha.clone()));
                cx.stop_propagation();
            }
            return;
        }
        let viewport_height: f32 = self.list.viewport_bounds().size.height.into();
        let page = (viewport_height / (self.font_size * 1.7)).floor() as usize;
        if self.split.is_some() {
            // Side-by-side rows have no caret: keys move the reading line.
            let (line, last) = (self.active_line, self.item_count() - 1);
            let jump = modifiers.platform || modifiers.control;
            let next = match key {
                "down" => (line + 1).min(last),
                "up" => line.saturating_sub(1),
                "pagedown" => (line + page.max(1)).min(last),
                "pageup" => line.saturating_sub(page.max(1)),
                "home" if jump => 0,
                "end" if jump => last,
                _ => return,
            };
            self.active_line = next;
            self.scroll.reveal_row(next);
            window.focus(&self.focus, cx);
            cx.stop_propagation();
            cx.notify();
            return;
        }
        let current = self.selection.map_or(
            Position {
                row: self.active_line,
                byte: 0,
            },
            |selection| selection.head,
        );
        let mut next = current;
        match key {
            "down" => next.row = (current.row + 1).min(self.rows.len() - 1),
            "up" => next.row = current.row.saturating_sub(1),
            "pagedown" => next.row = (current.row + page.max(1)).min(self.rows.len() - 1),
            "pageup" => next.row = current.row.saturating_sub(page.max(1)),
            "home" => {
                next.byte = 0;
                if modifiers.platform || modifiers.control {
                    next.row = 0;
                }
            }
            "end" => {
                if modifiers.platform || modifiers.control {
                    next.row = self.rows.len() - 1;
                }
                next.byte = self.rows[next.row].original.len();
            }
            "left" => {
                if next.byte > 0 {
                    next.byte = self.rows[next.row].original[..next.byte]
                        .char_indices()
                        .next_back()
                        .map_or(0, |(offset, _)| offset);
                } else if next.row > 0 {
                    next.row -= 1;
                    next.byte = self.rows[next.row].original.len();
                }
            }
            "right" => {
                if next.byte < self.rows[next.row].original.len() {
                    next.byte += self.rows[next.row].original[next.byte..]
                        .chars()
                        .next()
                        .unwrap()
                        .len_utf8();
                } else if next.row + 1 < self.rows.len() {
                    next.row += 1;
                    next.byte = 0;
                }
            }
            _ => return,
        }
        next.byte = next.byte.min(self.rows[next.row].original.len());
        while !self.rows[next.row].original.is_char_boundary(next.byte) {
            next.byte -= 1;
        }
        self.active_line = next.row;
        self.selection = if modifiers.shift {
            Some(Selection {
                anchor: self.selection.map_or(current, |selection| selection.anchor),
                head: next,
            })
        } else {
            Some(Selection {
                anchor: next,
                head: next,
            })
        };
        self.scroll.reveal_row(next.row);
        window.focus(&self.focus, cx);
        cx.stop_propagation();
        cx.notify();
    }
    fn blame_group(&self, row: usize) -> Option<&mangrove_core::model::BlameGroup> {
        let index = self.blame_indices.get(row).copied().flatten()?;
        self.blame.as_ref()?.groups.get(index)
    }
    /// Styles of a unified row: syntax colors, changed words and the
    /// selected bytes, which take precedence over the changed words.
    fn highlights(
        &self,
        index: usize,
        row: &Row,
        selection: Option<Range<usize>>,
    ) -> Vec<(Range<usize>, HighlightStyle)> {
        let tokens = self
            .highlights
            .tokens
            .get(index)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let words = self
            .highlights
            .words
            .get(index)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let word_background = match row.kind {
            LineKind::Addition => Some(self.palette.addition_word_bg),
            LineKind::Deletion => Some(self.palette.deletion_word_bg),
            _ => None,
        };
        let mut cuts = vec![0, row.display.original_end];
        for range in tokens.iter().map(|token| &token.range).chain(words) {
            cuts.extend([
                range.start.min(row.display.original_end),
                range.end.min(row.display.original_end),
            ]);
        }
        if let Some(selected) = &selection {
            cuts.extend([
                selected.start.min(row.display.original_end),
                selected.end.min(row.display.original_end),
            ]);
        }
        cuts.sort_unstable();
        cuts.dedup();
        let (mut token_index, mut word_index) = (0, 0);
        let mut result = Vec::with_capacity(cuts.len());
        for boundaries in cuts.windows(2) {
            let range = boundaries[0]..boundaries[1];
            if range.is_empty() {
                continue;
            }
            while token_index < tokens.len() && tokens[token_index].range.end <= range.start {
                token_index += 1;
            }
            let kind = tokens
                .get(token_index)
                .filter(|token| token.range.start <= range.start && token.range.end >= range.end)
                .map_or(Kind::Plain, |token| token.kind);
            while word_index < words.len() && words[word_index].end <= range.start {
                word_index += 1;
            }
            let changed = words
                .get(word_index)
                .is_some_and(|word| word.start <= range.start && word.end >= range.end);
            let selected = selection
                .as_ref()
                .is_some_and(|selected| selected.start <= range.start && selected.end >= range.end);
            let display_range =
                row.display.display_byte(range.start)..row.display.display_byte(range.end);
            if display_range.is_empty() {
                continue;
            }
            result.push((
                display_range,
                HighlightStyle {
                    color: if selected {
                        Some(rgb(self.palette.text).into())
                    } else {
                        kind.palette_index()
                            .map(|index| rgb(self.palette.syntax[index]).into())
                    },
                    background_color: if selected {
                        Some(rgb(self.palette.selection).into())
                    } else {
                        word_background
                            .filter(|_| changed)
                            .map(|color| rgb(color).into())
                    },
                    ..Default::default()
                },
            ));
        }
        if row.display.truncated() {
            let selected = selection.as_ref().is_some_and(|selection| {
                selection.end > row.display.original_end && selection.start < row.original.len()
            });
            result.push((
                row.display.content_end..row.display.text.len(),
                HighlightStyle {
                    color: Some(
                        rgb(if selected {
                            self.palette.text
                        } else {
                            self.palette.warning
                        })
                        .into(),
                    ),
                    background_color: selected.then(|| rgb(self.palette.selection).into()),
                    ..Default::default()
                },
            ));
        }
        result
    }
    /// The highlighted text of a unified row, shaped whole for hit testing
    /// under `layout` but painted only where it is visible.
    fn code_text(
        &self,
        index: usize,
        layout: Option<usize>,
        selection: Option<Range<usize>>,
        window: &Window,
    ) -> AnyElement {
        let row = &self.rows[index];
        let default_color = rgb(if row.kind == LineKind::Hunk {
            self.palette.accent
        } else {
            self.palette.text
        })
        .into();
        let runs = self
            .highlights(index, row, selection)
            .into_iter()
            .map(|(range, highlight)| TextRun {
                len: range.len(),
                font: font(MONO),
                color: highlight.color.unwrap_or(default_color),
                background_color: highlight.background_color,
                underline: None,
                strikethrough: None,
            })
            .collect::<Vec<_>>();
        let shaped = window.text_system().shape_line(
            row.display.text.clone(),
            px(self.font_size),
            &runs,
            None,
        );
        let width = shaped.width().ceil();
        let layouts = self.layouts.clone();
        let line_height = window.pixel_snap(px(self.font_size * 1.7));
        canvas(
            move |bounds, _, _| {
                if let Some(key) = layout {
                    layouts.borrow_mut().insert(
                        key,
                        CodeLineLayout {
                            line: shaped.clone(),
                            bounds,
                        },
                    );
                }
                shaped
            },
            move |bounds, shaped, window, cx| {
                let viewport = window.content_mask().bounds;
                let overhang = shaped
                    .runs
                    .iter()
                    .map(|run| {
                        window
                            .text_system()
                            .bounding_box(run.font_id, shaped.font_size)
                            .size
                            .width
                    })
                    .fold(shaped.font_size, Pixels::max);
                let visible = visible_shaped_line(
                    &shaped,
                    viewport.left() - bounds.left(),
                    viewport.right() - bounds.left(),
                    overhang,
                );
                let _ = shaped.paint_background(
                    bounds.origin,
                    line_height,
                    TextAlign::Left,
                    None,
                    window,
                    cx,
                );
                let _ = visible.paint(
                    bounds.origin,
                    line_height,
                    TextAlign::Left,
                    None,
                    window,
                    cx,
                );
            },
        )
        .w(width)
        .h_full()
        .flex_shrink_0()
        .into_any_element()
    }
    fn render_row(
        &self,
        index: usize,
        first_visible_row: usize,
        scroll_x: Pixels,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        match self.split.as_ref().map(|split| split.rows[index]) {
            None => self.render_line(index, index, first_visible_row, scroll_x, window, cx),
            Some(SplitRow::Hunk(row)) => {
                self.render_line(index, row, first_visible_row, scroll_x, window, cx)
            }
            Some(SplitRow::Pair { old, new }) => {
                self.render_pair(index, [old, new], scroll_x, window, cx)
            }
        }
    }
    /// A unified row, or a hunk header across a split diff, at `index`.
    fn render_line(
        &self,
        index: usize,
        row_index: usize,
        first_visible_row: usize,
        scroll_x: Pixels,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let row = &self.rows[row_index];
        let p = self.palette;
        let hunk = row.kind == LineKind::Hunk;
        let background = match row.kind {
            LineKind::Hunk => p.hunk_bg,
            LineKind::Addition => p.addition_bg,
            LineKind::Deletion => p.deletion_bg,
            _ if self.show_blame && index == self.active_line && self.focus.is_focused(window) => {
                p.surface
            }
            _ => p.reading_bg,
        };
        let selection = self
            .selection
            .and_then(|selection| selection.range_in_row(index, row.original.len()));
        let text = self.code_text(row_index, Some(index), selection, window);
        let gutter = |value: String| self.number_cell(row.kind, value);
        let mut line = div()
            .id(("code-row", index))
            .role(if self.show_blame && !self.is_diff {
                Role::ListBoxOption
            } else {
                Role::Paragraph
            })
            .when(window.is_a11y_active(), |line| {
                line.aria_label(if hunk {
                    row.original.to_string()
                } else {
                    format!(
                        "Ligne {} : {}",
                        if row.new_number.is_empty() {
                            &row.old_number
                        } else {
                            &row.new_number
                        },
                        row.original
                    )
                })
            })
            .when(self.show_blame && !self.is_diff, |line| {
                line.aria_selected(index == self.active_line)
                    .aria_position_in_set(index + 1)
                    .aria_size_of_set(self.rows.len())
            })
            .when(
                self.show_blame && !self.is_diff && index == self.active_line,
                |line| line.aria_active_descendant(),
            )
            .flex()
            .w_full()
            .min_w(px(self.content_width()))
            .h(self.row_height(index))
            .font_family(MONO)
            .text_size(px(self.font_size))
            .line_height(px(self.font_size * 1.7))
            .text_color(rgb(if hunk { p.accent } else { p.text }))
            .bg(rgb(background));
        let mut sticky = None;
        let mut header = None;
        if hunk {
            // The header text stays in view while the code scrolls beneath.
            line = line.relative().when(index > 0, |line| {
                line.border_t_1().border_color(rgb(p.border))
            });
            header = Some(
                self.pinned(scroll_x)
                    .flex()
                    .px(px(13.))
                    .py(px(5.))
                    .overflow_hidden(),
            );
        } else {
            // The gutter is painted last, over the code that scrolls beneath it.
            let mut cells = div()
                .absolute()
                .top_0()
                .bottom_0()
                .left(scroll_x)
                .flex()
                .bg(rgb(background))
                .block_mouse_except_scroll();
            if self.is_diff {
                cells = cells
                    .child(gutter(row.old_number.clone()))
                    .child(gutter(row.new_number.clone()))
                    .child(self.sign_cell(row.kind));
            } else {
                cells = cells
                    .children(
                        self.show_blame
                            .then(|| self.blame_cell(index, first_visible_row, cx)),
                    )
                    .child(gutter(row.new_number.clone()))
                    .child(div().w(px(12.)).flex_shrink_0());
            }
            line = line
                .relative()
                .child(div().w(px(self.gutter_width())).flex_shrink_0());
            sticky = Some(cells);
        }
        let code = div()
            .id(("code-text", index))
            .flex()
            .flex_1()
            .h_full()
            .pr(px(if self.is_diff { DIFF_PADDING } else { 20. }))
            .cursor(CursorStyle::IBeam)
            .child(text)
            .when(
                self.selection.is_some_and(|selection| {
                    let (start, end) = selection.ordered();
                    start.row <= index && index < end.row
                }),
                |view| {
                    view.child(
                        div()
                            .w(px(self.font_size * 0.6))
                            .h_full()
                            .bg(rgb(p.selection)),
                    )
                },
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |view, event, window, cx| {
                    view.begin_selection(index, event, window, cx)
                }),
            );
        match header {
            Some(header) => line.child(header.child(code)),
            None => line.child(code).children(sticky),
        }
        .into_any_element()
    }
    /// A box laid over the viewport, which the rows scroll beneath.
    fn pinned(&self, scroll_x: Pixels) -> Div {
        let viewport = self.viewport_width.get();
        div()
            .absolute()
            .top_0()
            .bottom_0()
            .left(scroll_x)
            .map(|pinned| {
                if viewport > 0. {
                    pinned.w(px(viewport))
                } else {
                    pinned.right_0()
                }
            })
    }
    /// A side-by-side row. Both cells stay in the viewport while their text
    /// scrolls horizontally beneath them by the same distance.
    fn render_pair(
        &self,
        index: usize,
        cells: [Option<usize>; 2],
        scroll_x: Pixels,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = self.palette;
        let window: &Window = window;
        let cell = |row_index: Option<usize>, old: bool| {
            let cell = div().flex().flex_1().min_w_0().h_full();
            let Some(row_index) = row_index else {
                return cell.bg(rgb(p.panel));
            };
            let row = &self.rows[row_index];
            let background = match row.kind {
                LineKind::Deletion => p.deletion_bg,
                LineKind::Addition => p.addition_bg,
                _ => p.reading_bg,
            };
            let number = if old {
                &row.old_number
            } else {
                &row.new_number
            };
            cell.bg(rgb(background))
                .child(self.number_cell(row.kind, number.clone()))
                .child(self.sign_cell(row.kind))
                .child(
                    div().flex_1().min_w_0().h_full().overflow_hidden().child(
                        div()
                            .flex()
                            .h_full()
                            .ml(-scroll_x)
                            .child(self.code_text(row_index, None, None, window)),
                    ),
                )
        };
        let columns = self
            .pinned(scroll_x)
            .flex()
            .child(cell(cells[0], true))
            .child(
                div()
                    .w(px(DIVIDER_WIDTH))
                    .h_full()
                    .flex_shrink_0()
                    .bg(rgb(p.border)),
            )
            .child(cell(cells[1], false));
        div()
            .id(("code-row", index))
            .role(Role::Paragraph)
            .when(window.is_a11y_active(), |line| {
                line.aria_label(self.pair_label(cells))
            })
            .relative()
            .w_full()
            .min_w(px(self.content_width()))
            .h(self.row_height(index))
            .font_family(MONO)
            .text_size(px(self.font_size))
            .line_height(px(self.font_size * 1.7))
            .text_color(rgb(p.text))
            .bg(rgb(p.reading_bg))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |view, _, window, cx| {
                    window.focus(&view.focus, cx);
                    view.active_line = index;
                    cx.notify();
                }),
            )
            .child(columns)
            .into_any_element()
    }
    /// A line number, tinted on added and deleted lines.
    fn number_cell(&self, kind: LineKind, number: String) -> Div {
        let p = self.palette;
        div()
            .w(px(self.number_width))
            .h_full()
            .flex_shrink_0()
            .border_r_1()
            .border_color(rgb(p.border))
            .pr(px(if self.is_diff { 5. } else { 8. }))
            .text_right()
            .map(|cell| match kind {
                LineKind::Addition => cell.bg(rgb(p.addition_gutter)).text_color(rgb(p.text)),
                LineKind::Deletion => cell.bg(rgb(p.deletion_gutter)).text_color(rgb(p.text)),
                _ => cell.text_color(rgb(p.muted)),
            })
            .child(number)
    }
    fn sign_cell(&self, kind: LineKind) -> Div {
        let p = self.palette;
        let (sign, color) = match kind {
            LineKind::Addition => ("+", p.success),
            LineKind::Deletion => ("−", p.danger),
            _ => (" ", p.muted),
        };
        div()
            .w(px(SIGN_WIDTH))
            .flex_shrink_0()
            .text_center()
            .text_color(rgb(color))
            .child(sign)
    }
    /// « Avant, ligne 12 : … ; Après, ligne 13 : … » for assistive technologies.
    fn pair_label(&self, cells: [Option<usize>; 2]) -> String {
        cells
            .into_iter()
            .zip([("Avant", true), ("Après", false)])
            .filter_map(|(cell, (side, old))| {
                let row = &self.rows[cell?];
                let number = if old {
                    &row.old_number
                } else {
                    &row.new_number
                };
                Some(if number.is_empty() {
                    format!("{side} : {}", row.original)
                } else {
                    format!("{side}, ligne {number} : {}", row.original)
                })
            })
            .collect::<Vec<_>>()
            .join(" ; ")
    }
    fn blame_cell(
        &self,
        index: usize,
        first_visible_row: usize,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let p = self.palette;
        let attribution = div()
            .id(("blame-line", index))
            .role(Role::Button)
            .flex()
            .w(px(self.blame_width()))
            .h_full()
            .flex_shrink_0()
            .items_center()
            .gap(px(8.))
            .px(px(8.))
            .bg(rgb(p.panel))
            .border_r_1()
            .border_color(rgb(p.border))
            .text_color(rgb(p.muted))
            .overflow_hidden();
        let Some(group) = self.blame_group(index) else {
            return attribution.child("—");
        };
        let sha = group.sha.clone();
        let title = format!(
            "{} · {}{}",
            group.author,
            commit_date(group.time),
            if group.boundary {
                " · Frontière du parcours"
            } else {
                ""
            }
        );
        let attribution = attribution
            .aria_label(format!(
                "Consulter le commit {} de {}",
                short_sha(&group.sha),
                title
            ))
            .cursor_pointer()
            .hover(move |view| view.bg(rgb(p.surface)).text_color(rgb(p.accent)))
            .when(group.boundary, |view| {
                view.border_l_2().border_color(rgb(p.warning))
            })
            .tooltip(move |window, cx| Tooltip::new(title.clone()).build(window, cx))
            .on_click(cx.listener(move |view, _, window, cx| {
                view.active_line = index;
                window.focus(&view.focus, cx);
                cx.emit(CodeViewEvent::NavigateCommit(sha.clone()));
            }));
        if group.start_line == index + 1 || first_visible_row == index {
            attribution
                .child(div().flex_shrink_0().child(short_sha(&group.sha)))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(rgb(p.text))
                        .child(group.author.clone()),
                )
                .child(div().flex_shrink_0().child(compact_date(group.time)))
        } else {
            attribution.child(div().opacity(0.35).child("│"))
        }
    }
}

impl Render for CodeView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.geometry_scale != window.scale_factor() {
            self.rebuild_row_geometry(window.scale_factor());
        }
        self.update_layout(window, cx);
        let p = self.palette;
        self.autoscroll_selection(window);
        let on_move = cx.listener(Self::drag_selection);
        let on_up = cx.listener(|view, event: &MouseUpEvent, _, cx| {
            if view.dragging && event.button == MouseButton::Left {
                view.dragging = false;
                view.drag_point = None;
                cx.notify();
            }
        });
        let mut root = div()
            .id("code-view")
            .role(if self.show_blame && !self.is_diff {
                Role::ListBox
            } else {
                Role::Document
            })
            .aria_label(if self.is_diff {
                "Modifications du fichier"
            } else if self.show_blame {
                "Blame du fichier"
            } else {
                "Contenu du fichier"
            })
            .aria_description(DESCRIPTION.clone())
            .flex()
            .flex_col()
            .size_full()
            .min_h_0()
            .min_w_0()
            .bg(rgb(p.reading_bg))
            .border_1()
            .border_color(rgb(p.reading_bg))
            .track_focus(&self.focus)
            .focus_visible(move |style| style.border_color(rgb(p.accent)))
            .on_action(cx.listener(|view, _: &Copy, _, cx| view.copy_selection(cx)))
            .on_action(cx.listener(|view, _: &SelectAll, _, cx| view.select_all(cx)))
            // A first Escape clears the selection; the next one closes the view.
            .on_action(cx.listener(|view, _: &crate::app::Escape, _, cx| {
                if view.selection.is_none() && !view.dragging {
                    cx.propagate();
                    return;
                }
                view.selection = None;
                view.dragging = false;
                view.drag_point = None;
                cx.notify();
            }))
            .on_key_down(cx.listener(Self::keydown))
            .child(
                canvas(
                    |_, _, _| (),
                    move |_, (), window, _| {
                        window.on_mouse_event(move |event: &MouseMoveEvent, phase, window, cx| {
                            if matches!(phase, DispatchPhase::Capture) {
                                on_move(event, window, cx);
                            }
                        });
                        window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
                            if matches!(phase, DispatchPhase::Capture) {
                                on_up(event, window, cx);
                            }
                        });
                    },
                )
                .absolute()
                .w(px(0.))
                .h(px(0.)),
            );
        let reading = self.loading.as_ref().filter(|_| self.rows.is_empty());
        let blame_unavailable = self
            .blame_unavailable
            .as_ref()
            .filter(|_| self.show_blame && !self.is_diff);
        // The action belongs to the state: an error, a read in progress or an
        // unavailable blame shows no button.
        let (message, action) = match self.error.as_ref().or(reading).or(blame_unavailable) {
            Some(message) => (Some(message), None),
            None => (self.state.as_ref(), self.action),
        };
        if let Some(state) = message {
            root = root.child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .items_center()
                    .justify_center()
                    .gap(px(10.))
                    .p(px(22.))
                    .text_center()
                    .font_family(SANS)
                    .text_size(px(11. + self.ui_offset))
                    .text_color(rgb(if self.error.is_some() {
                        p.danger
                    } else {
                        p.muted
                    }))
                    .child(icon(
                        if self.error.is_some() {
                            IconName::CircleAlert
                        } else {
                            IconName::FileText
                        },
                        22.,
                        if self.error.is_some() {
                            p.danger
                        } else {
                            p.muted
                        },
                    ))
                    .child(div().max_w(px(300.)).child(state.clone()))
                    .when_some(action, |view, action| {
                        view.child(
                            div()
                                .id("code-state-action")
                                .role(Role::Button)
                                .focusable()
                                .tab_index(0)
                                .aria_label(action.label())
                                .px(px(12.))
                                .py(px(7.))
                                .border_1()
                                .border_color(rgb(p.border))
                                .rounded(RADIUS_MD)
                                .bg(rgb(p.surface))
                                .text_color(rgb(p.text))
                                .cursor_pointer()
                                .focus_visible(move |style| style.border_color(rgb(p.accent)))
                                .child(action.label())
                                .on_click(cx.listener(move |_, _, _, cx| cx.emit(action.event())))
                                .on_key_down(cx.listener(move |_, event: &KeyDownEvent, _, cx| {
                                    if is_activation(event) {
                                        cx.emit(action.event());
                                        cx.stop_propagation();
                                    }
                                })),
                        )
                    }),
            );
        } else {
            // Keep only the active viewport's text geometry; selection's byte
            // positions do not depend on these short-lived layouts.
            self.layouts.borrow_mut().clear();
            let entity = cx.entity();
            // ListState is mutably borrowed by GPUI while it measures/render rows.
            // Capture the scroll position here, before entering that callback.
            let first_visible_row = self.list.logical_scroll_top().item_ix;
            let scroll_x = -self.horizontal.offset().x;
            let wheel_view = cx.weak_entity();
            let wheel_capture = self.scroll.wheel_capture(move |changed, _, cx| {
                if changed {
                    let _ = wheel_view.update(cx, |_, cx| cx.notify());
                }
            });
            // The layout depends on the viewport width, known once painted:
            // a new width renders the view again with it.
            let viewport_width = self.viewport_width.clone();
            let measure = canvas(
                move |bounds, window, _| {
                    let width = f32::from(bounds.size.width);
                    if viewport_width.replace(width) != width {
                        window.request_animation_frame();
                    }
                },
                |_, _, _, _| {},
            )
            .absolute()
            .inset_0();
            let code = list(self.list.clone(), move |index, window, cx| {
                entity.update(cx, |view, cx| {
                    view.render_row(index, first_visible_row, scroll_x, window, cx)
                })
            })
            .w_full()
            .min_w(px(self.content_width()))
            .h_full();
            let viewport = div()
                .id("code-scroll-viewport")
                .relative()
                .flex_1()
                .min_h_0()
                .min_w_0()
                .w_full()
                .overflow_hidden()
                .child(measure)
                .child(wheel_capture)
                .child(
                    div()
                        .id("code-horizontal-scroll")
                        .size_full()
                        .overflow_x_scroll()
                        .track_scroll(&self.horizontal)
                        .child(code),
                )
                // These overlays are siblings of the scrolled content:
                // the vertical track stays at the visible right edge when
                // long lines are scrolled horizontally.
                .child(
                    div().absolute().inset_0().child(scrollbar_theme(
                        Scrollbar::new(&self.scroll)
                            .id("code-scrollbars")
                            .viewport_from_layout(),
                    )),
                );
            let notice = self.notice.as_ref().map(|notice| {
                div()
                    .flex()
                    .flex_shrink_0()
                    .w_full()
                    .min_w_0()
                    .items_start()
                    .gap(px(8.))
                    .px(px(17.))
                    .py(px(13.))
                    .font_family(SANS)
                    .text_size(px(11. + self.ui_offset))
                    .line_height(relative(1.625))
                    .text_color(rgb(p.warning))
                    .child(
                        icon(IconName::CircleAlert, 14., p.warning)
                            .flex_shrink_0()
                            .mt(px(2.)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .whitespace_normal()
                            .child(notice.clone()),
                    )
            });
            root = if self.rows.is_empty() {
                root.children(notice).child(viewport)
            } else {
                root.child(viewport).children(notice)
            };
        }
        if self.loading.is_some() {
            root = root.opacity(0.65);
        }
        root
    }
}

fn is_activation(event: &KeyDownEvent) -> bool {
    matches!(event.keystroke.key.as_str(), "enter" | "space")
}

/// Alt+↓ moves to the next hunk and Alt+↑ to the previous one.
pub fn hunk_shortcut(event: &KeyDownEvent) -> Option<bool> {
    let modifiers = event.keystroke.modifiers;
    if !modifiers.alt || modifiers.platform || modifiers.control || modifiers.shift {
        return None;
    }
    match event.keystroke.key.as_str() {
        "down" => Some(true),
        "up" => Some(false),
        _ => None,
    }
}

fn hunk_rows(rows: &[Row]) -> Vec<usize> {
    rows.iter()
        .enumerate()
        .filter_map(|(index, row)| (row.kind == LineKind::Hunk).then_some(index))
        .collect()
}

/// First hunk header after `current`, or last one before it.
fn hunk_target(hunks: &[usize], current: usize, forward: bool) -> Option<usize> {
    if forward {
        hunks.iter().copied().find(|row| *row > current)
    } else {
        hunks.iter().copied().rev().find(|row| *row < current)
    }
}

/// `tooltip` is read whenever a shown tooltip renders, so it follows the
/// button's state while the pointer stays over it.
fn icon_button(
    id: &'static str,
    name: IconName,
    label: &'static str,
    tooltip: impl Fn() -> SharedString + 'static,
    enabled: bool,
    palette: Palette,
) -> Stateful<Div> {
    let tooltip = Rc::new(tooltip);
    div()
        .id(id)
        .role(Role::Button)
        .tab_index(0)
        .aria_label(label)
        .flex()
        .items_center()
        .justify_center()
        .w(px(26.))
        .h(px(26.))
        .rounded(RADIUS_SM)
        .border_1()
        .border_color(rgb(palette.panel))
        .cursor_pointer()
        .when(enabled, |view| {
            view.hover(move |style| style.bg(rgb(palette.hover)))
        })
        .when(!enabled, |view| view.opacity(0.4))
        .focus_visible(move |style| style.border_color(rgb(palette.accent)))
        .child(icon(name, 14., palette.muted))
        .tooltip(move |window, cx| {
            let tooltip = tooltip.clone();
            Tooltip::element(move |_, _| tooltip()).build(window, cx)
        })
}

#[cfg(test)]
mod tests {
    use super::{
        CodeLineLayout, DisplayLine, MAX_DISPLAY_BYTES, OMITTED_LINE, Position, Row, Selection,
        StateAction, blame_unavailable, hunk_rows, hunk_target, oversized_state, row_geometry,
        selected_rows_text, visible_shaped_line,
    };
    use crate::{
        diff::LineKind,
        virtual_scroll::{RowGeometry, VirtualScrollHandle},
    };
    use gpui_kit::component::scroll::ScrollbarHandle;
    use gpui_kit::{
        Bounds, FontId, GlyphId, LineLayout, ListAlignment, ListState, ScrollHandle, ShapedGlyph,
        ShapedLine, ShapedRun, point, px, size,
    };
    use mangrove_core::model::{FileContent, Oversized, PreviewLimit};
    use std::sync::Arc;

    #[test]
    fn oversized_files_name_their_size_and_limit_and_offer_the_highest_one() {
        let (state, action) = oversized_state(
            Oversized {
                size: 5_666_173,
                limit: PreviewLimit::MIN.bytes(),
            },
            "son diff n’est pas calculé",
        );
        assert_eq!(
            state,
            "Fichier de 5,4\u{a0}Mio, au-delà de la limite d’aperçu de 2\u{a0}Mio : son diff n’est pas calculé.\nCette limite se règle dans Paramètres › Général."
        );
        assert_eq!(action, Some(StateAction::ShowLarge));
        // The highest limit still admits a file of exactly its size.
        let at_highest = Oversized {
            size: PreviewLimit::MAX.bytes(),
            limit: PreviewLimit::MIN.bytes(),
        };
        assert_eq!(
            oversized_state(at_highest, "").1,
            Some(StateAction::ShowLarge)
        );
        let (state, action) = oversized_state(
            Oversized {
                size: PreviewLimit::MAX.bytes() + 1,
                limit: PreviewLimit::MAX.bytes(),
            },
            "son contenu n’est pas chargé",
        );
        assert_eq!(
            state,
            "Fichier de 65\u{a0}Mio : son contenu n’est pas chargé. L’aperçu est limité à 64\u{a0}Mio."
        );
        assert_eq!(action, None);
    }

    #[test]
    fn blame_is_unavailable_above_the_background_bound_whatever_the_preview_read() {
        let content = |kind: &str, size: usize| FileContent {
            sha: String::new(),
            path: String::new(),
            file_key: String::new(),
            kind: kind.into(),
            size,
            lines: Vec::new(),
            truncated: false,
            ends_with_newline: true,
            parent_sha: None,
            oversized: None,
        };
        assert_eq!(blame_unavailable(&content("text", 2 << 20)), None);
        assert_eq!(
            blame_unavailable(&content("text", 3 << 20)).as_deref(),
            Some("Fichier de 3\u{a0}Mio : le blame est disponible jusqu’à 2\u{a0}Mio.")
        );
        assert!(blame_unavailable(&content("large", 20 << 20)).is_some());
        assert_eq!(blame_unavailable(&content("binary", 20 << 20)), None);
    }

    fn row(kind: LineKind) -> Row {
        Row::new(kind, String::new(), String::new(), "line")
    }

    fn geometry(rows: &[Row], font_size: f32, scale: f32) -> RowGeometry {
        row_geometry(
            rows.iter().map(|row| row.kind == LineKind::Hunk),
            font_size,
            scale,
        )
    }

    fn near(value: gpui_kit::Pixels, expected: f32) {
        assert!((f32::from(value) - expected).abs() < 0.01);
    }

    #[test]
    fn unmeasured_hunks_keep_exact_scroll_targets_after_reset_and_zoom() {
        let rows = [
            row(LineKind::Hunk),
            row(LineKind::Context),
            row(LineKind::Hunk),
            row(LineKind::Addition),
        ];
        let list = ListState::new(rows.len(), ListAlignment::Top, px(150.));
        let handle =
            VirtualScrollHandle::exact(list.clone(), ScrollHandle::new(), geometry(&rows, 11., 1.));
        near(handle.content_size().height, 96.);
        handle.set_offset(point(px(0.), px(-60.)));
        assert_eq!(list.logical_scroll_top().item_ix, 2);
        near(list.logical_scroll_top().offset_in_item, 13.);
        near(handle.offset().y, -60.);
        handle.set_offset(point(px(0.), px(-1_000.)));
        assert_eq!(list.logical_scroll_top().item_ix, rows.len());
        near(handle.offset().y, -96.);

        // First layout/width changes drop GPUI's height hints. The source
        // geometry and scrollbar target still work with no measured rows.
        list.reset(rows.len());
        handle.set_offset(point(px(0.), px(-37.7)));
        assert_eq!(list.logical_scroll_top().item_ix, 1);
        near(list.logical_scroll_top().offset_in_item, 8.7);

        handle.set_geometry(geometry(&rows, 20., 1.));
        let zoomed = handle;
        near(zoomed.content_size().height, 157.);
        zoomed.set_offset(point(px(0.), px(-100.)));
        assert_eq!(list.logical_scroll_top().item_ix, 2);
        near(list.logical_scroll_top().offset_in_item, 22.);
        near(zoomed.offset().y, -100.);
    }

    #[test]
    fn scroll_targets_clamp_and_keep_content_blame_rows_uniform() {
        let offsets = geometry(&[LineKind::Content; 3].map(row), 11., 1.);
        let target = offsets.list_offset(px(19.), px(0.));
        assert_eq!(target.item_ix, 1);
        near(target.offset_in_item, 0.);
        let end = offsets.list_offset(px(1_000.), px(20.));
        assert_eq!(end.item_ix, 1);
        near(end.offset_in_item, 17.);
        for (position, height) in [(-10., 20.), (1_000., 100.)] {
            let target = offsets.list_offset(px(position), px(height));
            assert_eq!(target.item_ix, 0);
            near(target.offset_in_item, 0.);
        }
        let empty = RowGeometry::default().list_offset(px(100.), px(0.));
        assert_eq!(empty.item_ix, 0);
        near(empty.offset_in_item, 0.);
    }

    #[test]
    fn physical_row_boundaries_keep_css_height_at_one_and_two_times_scale() {
        let rows: Vec<_> = (0..1_000).map(|_| row(LineKind::Content)).collect();
        for scale in [1., 2.] {
            for font_size in [9., 11., 20.] {
                let offsets = geometry(&rows, font_size, scale);
                near(offsets.total(), font_size * 1_700.);
                // Every rendered row's explicit height is a whole physical
                // pixel, so GPUI's layout snapping leaves it unchanged.
                for index in 0..rows.len() {
                    let physical = f32::from(offsets.height(index).unwrap()) * scale;
                    assert_eq!(physical.fract(), 0.);
                }
            }
            let mixed = geometry(
                &[
                    row(LineKind::Hunk),
                    row(LineKind::Context),
                    row(LineKind::Hunk),
                    row(LineKind::Addition),
                ],
                11.,
                scale,
            );
            near(mixed.total(), 96.);
            let last_viewport = mixed.list_offset(px(1_000.), px(30.));
            assert_eq!(last_viewport.item_ix, 2);
            near(
                last_viewport.offset_in_item,
                if scale == 1. { 19. } else { 18.5 },
            );
            let normal = mixed.list_offset(px(40.), px(30.));
            assert_eq!(normal.item_ix, 1);
            near(normal.offset_in_item, if scale == 1. { 11. } else { 11.5 });
        }
    }

    #[test]
    fn wheel_after_far_drag_moves_exact_pixels_without_measured_rows() {
        let rows: Vec<_> = (0..2_000).map(|_| row(LineKind::Content)).collect();
        for scale in [1., 2.] {
            let offsets = geometry(&rows, 11., scale);
            let list = ListState::new(rows.len(), ListAlignment::Top, px(150.));
            let handle = VirtualScrollHandle::exact(list, ScrollHandle::new(), offsets.clone());
            handle.scroll_vertical_to(offsets.start(1_500));
            let before = handle.vertical_offset();
            for _ in 0..6 {
                handle.scroll_vertical_by(px(40.));
            }
            near(handle.vertical_offset() - before, 240.);
        }
    }
    #[test]
    fn hunk_navigation_moves_between_headers_and_stops_at_the_ends() {
        let rows = [
            LineKind::Hunk,
            LineKind::Context,
            LineKind::Addition,
            LineKind::Hunk,
            LineKind::Deletion,
            LineKind::Hunk,
            LineKind::Context,
        ]
        .map(row);
        let hunks = hunk_rows(&rows);
        assert_eq!(hunks, [0, 3, 5]);
        assert_eq!(hunk_target(&hunks, 0, true), Some(3));
        assert_eq!(hunk_target(&hunks, 1, true), Some(3));
        assert_eq!(hunk_target(&hunks, 3, true), Some(5));
        assert_eq!(hunk_target(&hunks, 5, true), None);
        assert_eq!(hunk_target(&hunks, 6, false), Some(5));
        // Inside a hunk, « previous » first returns to its own header.
        assert_eq!(hunk_target(&hunks, 4, false), Some(3));
        assert_eq!(hunk_target(&hunks, 0, false), None);
        assert_eq!(hunk_target(&[], 2, true), None);
    }
    #[test]
    fn selection_across_virtual_rows_excludes_gutters() {
        let selection = Selection {
            anchor: Position { row: 1, byte: 2 },
            head: Position { row: 3, byte: 1 },
        };
        assert_eq!(selection.range_in_row(0, 9), None);
        assert_eq!(selection.range_in_row(1, 9), Some(2..9));
        assert_eq!(selection.range_in_row(2, 9), Some(0..9));
        assert_eq!(selection.range_in_row(3, 9), Some(0..1));
    }
    #[test]
    fn tabs_render_with_columns_but_copy_keeps_original_bytes() {
        let line = DisplayLine::new("\télève\tx".into());
        assert_eq!(line.text.as_ref(), "        élève   x");
        for byte in [0, 1, 3, 4, 6, 7, 8, 9, 10] {
            if "\télève\tx".is_char_boundary(byte) {
                assert_eq!(line.original_byte(line.display_byte(byte)), byte);
            }
        }
    }

    #[test]
    fn long_utf8_and_tab_lines_bound_expansion_and_keep_original_copy() {
        for original in [
            "x".repeat(400 * 1024),
            "\t".repeat(512 * 1024),
            "\télève🦀".repeat(50_000),
        ] {
            let row = Row::new(LineKind::Content, String::new(), "1".into(), &original);
            let line = &row.display;
            assert!(line.truncated());
            assert!(line.text.len() <= MAX_DISPLAY_BYTES);
            assert!(line.text.ends_with(OMITTED_LINE));
            assert!(original.is_char_boundary(line.original_end));
            assert!(
                line.mapping
                    .as_ref()
                    .is_none_or(|map| map.len() <= MAX_DISPLAY_BYTES + 1)
            );
            for (byte, _) in original[..line.original_end].char_indices() {
                assert_eq!(line.original_byte(line.display_byte(byte)), byte);
            }
            assert_eq!(line.original_byte(line.content_end), line.original_end);
            assert_eq!(line.original_byte(line.text.len()), original.len());
            assert_eq!(line.display_byte(original.len()), line.text.len());
            let selection = Selection {
                anchor: Position::default(),
                head: Position {
                    row: 0,
                    byte: original.len(),
                },
            };
            assert_eq!(
                selected_rows_text(std::slice::from_ref(&row), Some(selection), false),
                original
            );
            assert_eq!(
                selected_rows_text(&[row], Some(selection), true),
                format!("{original}\n")
            );
        }
    }

    #[test]
    fn bounded_utf8_prefix_and_cross_row_selection_copy_hidden_bytes() {
        let original = "🦀".repeat(100_000);
        let first = Row::new(LineKind::Deletion, "1".into(), String::new(), &original);
        let second = Row::new(LineKind::Addition, String::new(), "1".into(), "\télève");
        let start = first.display.original_end;
        let rows = [first, second];
        let selection = Selection {
            anchor: Position {
                row: 0,
                byte: start,
            },
            head: Position { row: 1, byte: 3 },
        };
        assert_eq!(
            selected_rows_text(&rows, Some(selection), false),
            format!("{}\n\té", &original[start..])
        );
    }

    #[test]
    fn plain_lines_share_their_original_allocation() {
        let original: Arc<str> = "a normal line long enough to use shared heap storage".into();
        let line = DisplayLine::new(original.clone());
        assert_eq!(line.text.as_ptr(), original.as_ptr());
        assert!(!line.truncated());
    }

    fn shaped_fixture(text: &str) -> ShapedLine {
        let glyphs: Vec<_> = text
            .char_indices()
            .enumerate()
            .map(|(index, (byte, _))| ShapedGlyph {
                id: GlyphId(index as u32),
                position: point(px(index as f32 * 6.), px(0.)),
                index: byte,
                is_emoji: false,
            })
            .collect();
        let mut line = ShapedLine::default();
        *line = Arc::new(LineLayout {
            font_size: px(11.),
            width: px(glyphs.len() as f32 * 6.),
            ascent: px(9.),
            descent: px(2.),
            runs: vec![ShapedRun {
                font_id: FontId(0),
                glyphs,
            }],
            len: text.len(),
        });
        line.text = text.to_owned().into();
        line
    }

    #[test]
    fn horizontal_culling_preserves_utf8_glyphs_positions_and_scrolled_hit_testing() {
        let display = DisplayLine::new("\télève🦀xyz".repeat(500).into());
        let shaped = shaped_fixture(&display.text);
        let visible = visible_shaped_line(&shaped, px(1_200.), px(1_500.), px(12.));
        assert_eq!(visible.len(), shaped.len());
        assert_eq!(visible.width(), shaped.width());
        assert!(visible.runs[0].glyphs.len() < 60);
        for glyph in &visible.runs[0].glyphs {
            assert!(display.text.is_char_boundary(glyph.index));
            near(glyph.position.x, f32::from(shaped.x_for_index(glyph.index)));
            assert!(glyph.position.x >= px(1_188.) && glyph.position.x <= px(1_512.));
        }
        let layout = CodeLineLayout {
            line: shaped,
            bounds: Bounds::new(point(px(-1_000.), px(300.)), size(px(10_000.), px(19.))),
        };
        // Screen coordinates retain the full line's scrolled origin, independent
        // of the much smaller foreground glyph set.
        let byte = layout.byte_at(point(px(200.), px(310.)));
        assert_eq!(layout.line.x_for_index(byte), px(1_200.));
        assert_eq!(layout.byte_at(point(px(200.), px(299.))), 0);
        assert_eq!(layout.byte_at(point(px(200.), px(320.))), layout.line.len());
        let original = display.original_byte(byte);
        assert_eq!(display.display_byte(original), byte);
    }

    #[test]
    fn reordered_glyphs_keep_the_full_bounded_layout() {
        let mut shaped = shaped_fixture("abcאבג");
        let mut layout = LineLayout {
            font_size: shaped.font_size,
            width: shaped.width(),
            ascent: shaped.ascent,
            descent: shaped.descent,
            runs: shaped.runs.clone(),
            len: shaped.len(),
        };
        layout.runs[0].glyphs[3].index = 7;
        layout.runs[0].glyphs[4].index = 5;
        *shaped = Arc::new(layout);
        let visible = visible_shaped_line(&shaped, px(25.), px(30.), px(1.));
        assert_eq!(visible.runs[0].glyphs.len(), shaped.runs[0].glyphs.len());
    }

    fn edited_patch() -> mangrove_core::model::FileDiff {
        use mangrove_core::model::{DiffHunk, DiffLine, FileDiff};
        let line = |kind: &str, content: &str| DiffLine {
            kind: kind.into(),
            old_line: Some(1),
            new_line: Some(1),
            content: content.into(),
        };
        FileDiff {
            binary: false,
            oversized: None,
            truncated: false,
            additions: 2,
            deletions: 2,
            hunks: vec![
                DiffHunk {
                    header: "@@ -1,2 +1,2 @@".into(),
                    lines: vec![
                        line("deletion", "\tlet total = 1;\n"),
                        line("annotation", "\\ No newline at end of file"),
                        line("addition", "\tlet total = 2;\n"),
                        line("context", "fn main() {}\n"),
                    ],
                },
                DiffHunk {
                    header: "@@ -9 +9 @@".into(),
                    lines: vec![
                        line("deletion", "let a = old;\n"),
                        line("addition", "let a = new;\n"),
                    ],
                },
            ],
        }
    }

    #[test]
    fn highlights_index_the_rows_of_a_patch_and_leave_other_rows_plain() {
        let diff = edited_patch();
        let rows = super::diff_rows(&diff);
        let highlights = super::Highlights::of_diff("a.rs", &diff, || false).unwrap();
        let (words, tokens) = (&highlights.words, &highlights.tokens);
        assert_eq!((words.len(), tokens.len()), (rows.len(), rows.len()));
        for (row, ranges) in rows.iter().zip(words.iter()) {
            let changed = matches!(row.kind, LineKind::Addition | LineKind::Deletion);
            assert_eq!(!ranges.is_empty(), changed, "{:?}", row.kind);
        }
        // The pairs the side-by-side layout reuses, by row.
        assert_eq!(highlights.pairs.as_deref(), Some(&vec![(1, 3), (6, 7)]));
    }

    #[gpui_kit::test]
    fn changed_words_paint_under_their_tab_columns_and_give_way_to_the_selection(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use super::{CodeView, Highlights};
        use gpui_kit::{AppContext, Hsla, WindowOptions, rgb};
        cx.update(gpui_kit::init);
        let view = cx.update(|cx| {
            gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                cx.new(|cx| CodeView::new(window, cx))
            })
            .unwrap()
            .1
        });
        let diff = Arc::new(edited_patch());
        let highlights = Highlights::of_diff("a.txt", &diff, || false).unwrap();
        cx.update_window(cx.windows()[0], |_, window, cx| {
            view.update(cx, |view, cx| {
                view.set_prepared_diff(diff, "a.rs".into(), highlights, window, cx)
            })
        })
        .unwrap();
        view.update(cx, |view, _| {
            let background = |view: &CodeView, index: usize, display: std::ops::Range<usize>| {
                let row = &view.rows[index];
                let selection = view
                    .selection
                    .and_then(|selection| selection.range_in_row(index, row.original.len()));
                view.highlights(index, row, selection)
                    .into_iter()
                    .find(|(range, _)| *range == display)
                    .and_then(|(_, style)| style.background_color)
            };
            let color = |value: u32| -> Option<Hsla> { Some(rgb(value).into()) };
            // « 1 » is byte 13 of the deletion, column 20 after its tab.
            assert_eq!(view.rows[1].original.find('1'), Some(13));
            assert_eq!(
                background(view, 1, 20..21),
                color(view.palette.deletion_word_bg)
            );
            assert_eq!(
                background(view, 3, 20..21),
                color(view.palette.addition_word_bg)
            );
            assert_eq!(background(view, 1, 0..20), None);
            view.selection = Some(Selection {
                anchor: Position { row: 1, byte: 0 },
                head: Position { row: 1, byte: 16 },
            });
            assert_eq!(background(view, 1, 20..21), color(view.palette.selection));
        });
    }

    #[gpui_kit::test]
    fn a_restored_reading_position_applies_only_to_its_own_diff(cx: &mut gpui_kit::TestAppContext) {
        use super::{CodeView, Highlights};
        use gpui_kit::{AppContext, WindowOptions, px};
        use mangrove_core::model::{DiffHunk, DiffLine, FileDiff};
        use std::sync::Arc;
        let diff = || {
            Arc::new(FileDiff {
                binary: false,
                oversized: None,
                truncated: false,
                additions: 400,
                deletions: 0,
                hunks: vec![DiffHunk {
                    header: "@@ -0,0 +1,400 @@".into(),
                    lines: (1..=400)
                        .map(|line| DiffLine {
                            kind: "addition".into(),
                            old_line: None,
                            new_line: Some(line),
                            content: format!("line {line}"),
                        })
                        .collect(),
                }],
            })
        };
        cx.update(gpui_kit::init);
        let view = cx.update(|cx| {
            gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                cx.new(|cx| CodeView::new(window, cx))
            })
            .unwrap()
            .1
        });
        let show = |cx: &mut gpui_kit::TestAppContext, path: &'static str| {
            cx.update_window(cx.windows()[0], |_, window, cx| {
                view.update(cx, |view, cx| {
                    view.set_prepared_diff(diff(), path.into(), Highlights::default(), window, cx)
                })
            })
            .unwrap();
        };
        let top = |cx: &mut gpui_kit::TestAppContext| {
            cx.read(|cx| view.read(cx).scroll.vertical_offset())
        };
        show(cx, "a.rs");
        let (position, read) = view.update(cx, |view, _| {
            view.scroll.scroll_vertical_to(px(900.));
            (
                view.reading_position().unwrap(),
                view.scroll.vertical_offset(),
            )
        });
        assert!(read > px(0.));

        // Another file shown first drops the position instead of moving it.
        view.update(cx, |view, _| {
            view.restore_reading_position(position.clone())
        });
        show(cx, "b.rs");
        assert_eq!(top(cx), px(0.));
        show(cx, "a.rs");
        assert_eq!(top(cx), px(0.));

        view.update(cx, |view, _| {
            view.restore_reading_position(position.clone())
        });
        show(cx, "a.rs");
        assert_eq!(top(cx), read);
    }

    #[gpui_kit::test]
    fn switching_layouts_keeps_the_line_read_and_leaves_one_sided_diffs_unified(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use super::{CodeView, Highlights, SplitBlocker};
        use crate::preferences::{DiffLayout, Preferences};
        use gpui_kit::{AppContext, ListOffset, WindowOptions, px};
        use mangrove_core::model::{DiffHunk, DiffLine, FileDiff};
        let line = |kind: &str, content: String| DiffLine {
            kind: kind.into(),
            old_line: Some(1),
            new_line: Some(1),
            content,
        };
        // 200 blocks of two edited pairs, each followed by a context line:
        // unified row 1 + 5n is a deletion shown on split row 1 + 3n.
        let edited = Arc::new(FileDiff {
            binary: false,
            oversized: None,
            truncated: false,
            additions: 400,
            deletions: 400,
            hunks: vec![DiffHunk {
                header: "@@ -1,600 +1,600 @@".into(),
                lines: (0..200)
                    .flat_map(|n| {
                        [
                            line("deletion", format!("let first_{n} = {n};")),
                            line("deletion", format!("let second_{n} = {n};")),
                            line("addition", format!("let first_{n} = {};", n + 1)),
                            line("addition", format!("let second_{n} = {};", n + 1)),
                            line("context", format!("// block {n}")),
                        ]
                    })
                    .collect(),
            }],
        });
        let added = Arc::new(FileDiff {
            hunks: vec![DiffHunk {
                header: "@@ -0,0 +1,2 @@".into(),
                lines: vec![
                    line("addition", "new".into()),
                    line("addition", "file".into()),
                ],
            }],
            ..FileDiff::default()
        });
        cx.update(gpui_kit::init);
        let view = cx.update(|cx| {
            gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                cx.new(|cx| CodeView::new(window, cx))
            })
            .unwrap()
            .1
        });
        let show = |cx: &mut gpui_kit::TestAppContext, diff: &Arc<FileDiff>| {
            let diff = diff.clone();
            cx.update_window(cx.windows()[0], |_, window, cx| {
                view.update(cx, |view, cx| {
                    view.set_prepared_diff(diff, "a.rs".into(), Highlights::default(), window, cx)
                })
            })
            .unwrap();
        };
        let update = |cx: &mut gpui_kit::TestAppContext,
                      change: &dyn Fn(
            &mut CodeView,
            &mut gpui_kit::Window,
            &mut gpui_kit::Context<CodeView>,
        )| {
            cx.update_window(cx.windows()[0], |_, window, cx| {
                view.update(cx, |view, cx| change(view, window, cx))
            })
            .unwrap();
        };
        let prefer = |view: &mut CodeView,
                      layout,
                      window: &mut gpui_kit::Window,
                      cx: &mut gpui_kit::Context<CodeView>| {
            view.set_preferences(
                Preferences {
                    diff_layout: layout,
                    ..Preferences::default()
                },
                cx,
            );
            view.update_layout(window, cx);
        };
        show(cx, &edited);
        update(cx, &|view, window, cx| {
            view.set_split_host(true, cx);
            view.list.scroll_to(ListOffset {
                item_ix: 301,
                offset_in_item: px(4.),
            });
            view.selection = Some(Selection {
                anchor: Position { row: 301, byte: 0 },
                head: Position { row: 302, byte: 3 },
            });
            prefer(view, DiffLayout::Split, window, cx);
            assert_eq!(view.item_count(), 601);
            assert_eq!(view.list.logical_scroll_top().item_ix, 181);
            near(view.list.logical_scroll_top().offset_in_item, 4.);
            assert_eq!(view.anchor().0, 301);
            assert!(view.selection.is_none());
            assert_eq!(view.hunk_rows, [0]);
            prefer(view, DiffLayout::Unified, window, cx);
            assert_eq!(view.item_count(), 1_001);
            assert_eq!(view.list.logical_scroll_top().item_ix, 301);
            // The compact view keeps the unified layout.
            view.set_split_host(false, cx);
            prefer(view, DiffLayout::Split, window, cx);
            assert!(view.split.is_none());
            view.set_split_host(true, cx);
            view.update_layout(window, cx);
            assert!(view.split.is_some());
        });
        // A file added from nothing has a single side to show.
        show(cx, &added);
        view.update(cx, |view, _| {
            assert!(view.split.is_none());
            assert_eq!(view.split_blocker(), Some(SplitBlocker::OneSide));
        });
        show(cx, &edited);
        view.update(cx, |view, _| assert!(view.split.is_some()));
    }

    /// A code view in a test window, showing `diff` centrally with the
    /// side-by-side preference.
    fn split_view(
        cx: &mut gpui_kit::TestAppContext,
        diff: Arc<mangrove_core::model::FileDiff>,
    ) -> gpui_kit::Entity<super::CodeView> {
        use super::{CodeView, Highlights};
        use crate::preferences::{DiffLayout, Preferences};
        use gpui_kit::{AppContext, WindowOptions};
        cx.update(gpui_kit::init);
        let view = cx.update(|cx| {
            gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                cx.new(|cx| CodeView::new(window, cx))
            })
            .unwrap()
            .1
        });
        in_window(cx, &view, |view, window, cx| {
            view.set_split_host(true, cx);
            view.set_preferences(
                Preferences {
                    diff_layout: DiffLayout::Split,
                    ..Preferences::default()
                },
                cx,
            );
            view.set_prepared_diff(diff, "a.rs".into(), Highlights::default(), window, cx);
        });
        view
    }

    fn in_window<R>(
        cx: &mut gpui_kit::TestAppContext,
        view: &gpui_kit::Entity<super::CodeView>,
        change: impl FnOnce(
            &mut super::CodeView,
            &mut gpui_kit::Window,
            &mut gpui_kit::Context<super::CodeView>,
        ) -> R,
    ) -> R {
        use gpui_kit::AppContext;
        cx.update_window(cx.windows()[0], |_, window, cx| {
            view.update(cx, |view, cx| change(view, window, cx))
        })
        .unwrap()
    }

    /// 20 numbered context lines around an edited pair, then an added file.
    fn numbered_patches() -> (
        Arc<mangrove_core::model::FileDiff>,
        Arc<mangrove_core::model::FileDiff>,
    ) {
        use mangrove_core::model::{DiffHunk, DiffLine, FileDiff};
        let line = |kind: &str, number: u32, content: String| DiffLine {
            kind: kind.into(),
            old_line: (kind != "addition").then_some(number),
            new_line: (kind != "deletion").then_some(number),
            content,
        };
        let edited = FileDiff {
            hunks: vec![DiffHunk {
                header: "@@ -10,41 +10,41 @@".into(),
                lines: (10..30)
                    .map(|n| line("context", n, format!("context line {n}")))
                    .chain([
                        line("deletion", 30, "let value = 1;".into()),
                        line("addition", 30, "let value = 2;".into()),
                    ])
                    .chain((31..50).map(|n| line("context", n, format!("context line {n}"))))
                    .collect(),
            }],
            ..FileDiff::default()
        };
        let added = FileDiff {
            hunks: vec![DiffHunk {
                header: "@@ -0,0 +10,2 @@".into(),
                lines: (10..12)
                    .map(|n| line("addition", n, format!("added line {n}")))
                    .collect(),
            }],
            ..FileDiff::default()
        };
        (Arc::new(edited), Arc::new(added))
    }

    #[gpui_kit::test]
    fn the_header_toggle_follows_the_preference_and_the_width_at_once(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use super::{CodeViewEvent, SplitBlocker};
        use crate::preferences::{DiffLayout, Preferences};
        let (edited, _) = numbered_patches();
        let view = split_view(cx, edited);
        let switches = std::rc::Rc::new(std::cell::Cell::new(0));
        let counted = switches.clone();
        let _subscription = cx.update(|cx| {
            cx.subscribe(&view, move |_, event: &CodeViewEvent, _| {
                if matches!(event, CodeViewEvent::LayoutSwitched) {
                    counted.set(counted.get() + 1);
                }
            })
        });
        // The toggle's state, and the text its tooltip reads, even while it
        // stays shown under the pointer.
        let state = |cx: &mut gpui_kit::TestAppContext| {
            in_window(cx, &view, |view, window, cx| {
                let state = view.layout_state(window, cx);
                view.layout_controls(window, cx);
                (state, view.layout_tooltip.borrow().to_string())
            })
        };
        let unified = "Afficher le diff unifié".to_owned();
        assert_eq!(state(cx), (Some((true, None)), unified.clone()));
        // A preference changed from the palette shows on the next header.
        in_window(cx, &view, |view, _, cx| {
            view.set_preferences(Preferences::default(), cx)
        });
        assert_eq!(
            state(cx),
            (Some((false, None)), "Afficher le diff côte à côte".into())
        );
        in_window(cx, &view, |view, _, cx| {
            view.set_preferences(
                Preferences {
                    diff_layout: DiffLayout::Split,
                    ..Preferences::default()
                },
                cx,
            )
        });
        assert_eq!(state(cx), (Some((true, None)), unified.clone()));
        // A narrow view falls back to unified, with the toggle disabled.
        let minimum = view.read_with(cx, |view, _| view.split_minimum());
        resize_window(cx, &view, minimum - 10.);
        assert_eq!(
            state(cx),
            (
                Some((false, Some(SplitBlocker::Narrow))),
                SplitBlocker::Narrow.reason().into()
            )
        );
        resize_window(cx, &view, minimum + 60.);
        assert_eq!(state(cx), (Some((true, None)), unified));
        cx.run_until_parked();
        assert_eq!(switches.get(), 4);
    }

    /// Resizes the test window, then lets the view measure itself and lay
    /// out again on its next frame.
    fn resize_window(
        cx: &mut gpui_kit::TestAppContext,
        view: &gpui_kit::Entity<super::CodeView>,
        width: f32,
    ) -> bool {
        use gpui_kit::{AppContext, px, size};
        let window = cx.windows()[0];
        cx.simulate_window_resize(window, size(px(width), px(800.)));
        for _ in 0..3 {
            cx.run_until_parked();
            cx.update_window(window, |_, window, cx| window.simulate_next_frame(cx))
                .unwrap();
        }
        cx.run_until_parked();
        view.read_with(cx, |view, _| view.split.is_some())
    }

    #[gpui_kit::test]
    fn the_width_decision_survives_files_shown_unified_for_another_reason(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        let (edited, added) = numbered_patches();
        let view = split_view(cx, edited.clone());
        // Before its first paint, a view's width is unknown and does not decide.
        assert!(view.read_with(cx, |view, _| view.split.is_some()));
        // The layout of a newly shown file, before any later frame.
        let show = |cx: &mut gpui_kit::TestAppContext,
                    diff: Arc<mangrove_core::model::FileDiff>| {
            in_window(cx, &view, |view, window, cx| {
                view.set_prepared_diff(
                    diff,
                    "a.rs".into(),
                    super::Highlights::default(),
                    window,
                    cx,
                );
                view.split.is_some()
            })
        };
        // The view takes the window's width but its border: about 705 px are
        // needed for two columns of 40 characters, 737 to enter them.
        let minimum = view.read_with(cx, |view, _| view.split_minimum());
        assert!((700. ..720.).contains(&minimum), "{minimum}");
        let resize = |cx: &mut gpui_kit::TestAppContext, view_width: f32| {
            resize_window(cx, &view, view_width + 2.)
        };
        assert!(resize(cx, minimum + 40.));
        let measured = view.read_with(cx, |view, _| view.viewport_width.get());
        assert!((measured - minimum - 40.).abs() < 1., "{measured}");
        // Leaving needs to fall below the minimum; entering needs 32 px more.
        assert!(resize(cx, minimum + 10.));
        assert!(!resize(cx, minimum - 10.));
        assert!(!resize(cx, minimum + 10.));
        assert!(resize(cx, minimum + 40.));
        // Inside that band, an added file between two edited ones does not
        // reset the decision of the first.
        assert!(resize(cx, minimum + 10.));
        assert!(!show(cx, added));
        assert!(show(cx, edited));
        // A known narrow width keeps the next file unified from its first
        // layout, without flashing side by side.
        assert!(!resize(cx, minimum - 10.));
        assert!(!show(cx, numbered_patches().0));
    }

    #[gpui_kit::test]
    fn a_reading_position_restored_in_a_split_diff_finds_its_line(
        cx: &mut gpui_kit::TestAppContext,
    ) {
        use gpui_kit::{ListOffset, px};
        let view = split_view(cx, numbered_patches().0);
        let position = in_window(cx, &view, |view, _, _| {
            assert!(view.split.is_some());
            // Split row 21 shows the edited pair, unified rows 21 and 22:
            // later split rows show the unified row after them.
            view.list.scroll_to(ListOffset {
                item_ix: 30,
                offset_in_item: px(3.),
            });
            view.reading_position().unwrap()
        });
        assert_eq!((position.row, position.offset), (31, px(3.)));
        // Shown again unified, then side by side, the same line is on top.
        let show = |cx: &mut gpui_kit::TestAppContext, layout| {
            let (diff, position) = (numbered_patches().0, position.clone());
            in_window(cx, &view, |view, window, cx| {
                view.set_preferences(
                    crate::preferences::Preferences {
                        diff_layout: layout,
                        ..Default::default()
                    },
                    cx,
                );
                view.restore_reading_position(position);
                view.set_prepared_diff(
                    diff,
                    "a.rs".into(),
                    super::Highlights::default(),
                    window,
                    cx,
                );
                let top = view.list.logical_scroll_top();
                (top.item_ix, top.offset_in_item)
            })
        };
        assert_eq!(
            show(cx, crate::preferences::DiffLayout::Unified),
            (31, px(3.))
        );
        assert_eq!(
            show(cx, crate::preferences::DiffLayout::Split),
            (30, px(3.))
        );
    }
}
