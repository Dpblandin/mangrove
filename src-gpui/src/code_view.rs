//! Read-only, virtualized code and patch viewer. Selection is expressed in
//! snapshot byte positions, so unmounted lines still participate in copying.
use std::{cell::RefCell, collections::BTreeMap, ops::Range, rc::Rc, sync::Arc};

use accesskit::Role;
use gpui_kit::component::input::{Copy, SelectAll};
use gpui_kit::component::scroll::{Scrollbar, ScrollbarHandle};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::{assets::IconName, prelude::*, *};
use mangrove_core::model::{FileBlame, FileContent, FileDiff};

use crate::{
    diff,
    format::{commit_date, short_sha},
    preferences::Preferences,
    syntax::{self, Kind, Token},
    theme::{MONO, Palette, SANS, icon, scrollbar_theme},
    virtual_scroll::{RowGeometry, VirtualScrollHandle},
};

#[derive(Clone, Debug)]
pub enum CodeViewEvent {
    NavigateCommit(String),
    FontSizeChanged(f32),
    Parent,
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
}
impl DisplayLine {
    fn new(text: &str) -> Self {
        if !text.contains('\t') {
            return Self {
                text: text.to_owned().into(),
                mapping: None,
            };
        }
        let mut expanded = String::new();
        let mut map = vec![0; text.len() + 1];
        let mut column = 0;
        for (offset, ch) in text.char_indices() {
            let before = expanded.len();
            map[offset] = before;
            if ch == '\t' {
                let spaces = 8 - column % 8;
                expanded.extend(std::iter::repeat_n(' ', spaces));
                column += spaces;
            } else {
                expanded.push(ch);
                column += 1;
            }
            for entry in map.iter_mut().take(offset + ch.len_utf8()).skip(offset + 1) {
                *entry = before;
            }
            map[offset + ch.len_utf8()] = expanded.len();
        }
        Self {
            text: expanded.into(),
            mapping: Some(map),
        }
    }
    fn display_byte(&self, original: usize) -> usize {
        self.mapping
            .as_ref()
            .map_or(original, |map| map[original.min(map.len() - 1)])
    }
    fn original_byte(&self, display: usize) -> usize {
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
    kind: String,
    old_number: String,
    new_number: String,
    original: Arc<str>,
    display: DisplayLine,
}
impl From<&diff::DiffRow> for Row {
    fn from(row: &diff::DiffRow) -> Self {
        Self {
            kind: row.kind.clone(),
            old_number: row.old_number.clone(),
            new_number: row.new_number.clone(),
            original: row.text.clone().into(),
            display: DisplayLine::new(&row.text),
        }
    }
}
impl Row {
    fn height_tenths(&self, index: usize, font_size: f32) -> f64 {
        f64::from(font_size) * 17.
            + if self.kind == "hunk" {
                100. + if index > 0 { 10. } else { 0. }
            } else {
                0.
            }
    }
}

fn row_geometry(rows: &[Row], font_size: f32, scale_factor: f32) -> RowGeometry {
    RowGeometry::from_tenths(
        rows.iter()
            .enumerate()
            .map(|(index, row)| row.height_tenths(index, font_size)),
        scale_factor,
    )
}

pub struct CodeView {
    rows: Arc<Vec<Row>>,
    scroll: VirtualScrollHandle,
    geometry_scale: f32,
    diff_source: Option<(Arc<FileDiff>, String)>,
    content_source: Option<Arc<FileContent>>,
    tokens: Arc<Vec<Vec<Token>>>,
    generation: u64,
    syntax_task: Option<Task<()>>,
    syntax_pending: bool,
    focus: FocusHandle,
    list: ListState,
    horizontal: ScrollHandle,
    layouts: Rc<RefCell<BTreeMap<usize, TextLayout>>>,
    palette: Palette,
    font_size: f32,
    ui_offset: f32,
    content_width: f32,
    number_width: f32,
    is_diff: bool,
    selection: Option<Selection>,
    dragging: bool,
    drag_point: Option<Point<Pixels>>,
    active_line: usize,
    show_blame: bool,
    blame: Option<Arc<FileBlame>>,
    blame_indices: Vec<Option<usize>>,
    notice: Option<String>,
    state: Option<String>,
    parent_available: bool,
    loading: bool,
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
            scroll,
            geometry_scale: window.scale_factor(),
            diff_source: None,
            content_source: None,
            tokens: Arc::new(Vec::new()),
            generation: 0,
            syntax_task: None,
            syntax_pending: false,
            focus: cx.focus_handle(),
            list,
            horizontal,
            layouts: Rc::new(RefCell::new(BTreeMap::new())),
            palette: Palette::default(),
            font_size: 11.,
            ui_offset: 1.,
            content_width: 0.,
            number_width: 32.,
            is_diff: false,
            selection: None,
            dragging: false,
            drag_point: None,
            active_line: 0,
            show_blame: false,
            blame: None,
            blame_indices: Vec::new(),
            notice: None,
            state: Some("Sélectionnez un fichier pour voir ses modifications.".into()),
            parent_available: false,
            loading: false,
            error: None,
            ends_with_newline: false,
        }
    }

    fn reset(&mut self, rows: Vec<Row>, window: &mut Window, cx: &mut Context<Self>) {
        self.generation = self.generation.wrapping_add(1);
        self.syntax_task = None;
        self.syntax_pending = false;
        self.rows = Arc::new(rows);
        self.diff_source = None;
        self.content_source = None;
        self.tokens = Arc::new(Vec::new());
        self.selection = None;
        self.dragging = false;
        self.drag_point = None;
        self.active_line = 0;
        self.state = None;
        self.parent_available = false;
        self.notice = None;
        self.error = None;
        self.loading = false;
        self.blame = None;
        self.blame_indices.clear();
        self.layouts.borrow_mut().clear();
        self.geometry_scale = window.scale_factor();
        self.scroll.set_geometry(row_geometry(
            &self.rows,
            self.font_size,
            self.geometry_scale,
        ));
        self.list.reset(self.rows.len());
        self.horizontal.set_offset(point(px(0.), px(0.)));
        self.measure(window);
        cx.notify();
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
        let char_width: f32 = window
            .text_system()
            .shape_line("0".into(), px(self.font_size), &[run], None)
            .width()
            .into();
        self.number_width = (largest.to_string().len() + 2) as f32 * char_width;
        let widest = self
            .rows
            .iter()
            .max_by_key(|row| row.display.text.chars().count());
        let text_width = widest.map_or(0., |row| {
            let run = TextRun {
                len: row.display.text.len(),
                font: font(MONO),
                color: rgb(self.palette.text).into(),
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let width: f32 = window
                .text_system()
                .shape_line(row.display.text.clone(), px(self.font_size), &[run], None)
                .width()
                .into();
            width
        });
        self.content_width = text_width
            + if self.is_diff {
                self.number_width * 2. + 35.
            } else {
                self.number_width + 32.
            };
        if self.show_blame && !self.is_diff {
            self.content_width += 240.;
        }
    }
    fn row_height(&self, index: usize) -> Pixels {
        self.scroll
            .row_height(index)
            .expect("rendered row has geometry")
    }
    fn rebuild_row_geometry(&mut self, scale_factor: f32) {
        let top = self.list.logical_scroll_top();
        let old_height = self
            .rows
            .get(top.item_ix)
            .map(|_| self.row_height(top.item_ix));
        self.geometry_scale = scale_factor;
        self.scroll
            .set_geometry(row_geometry(&self.rows, self.font_size, scale_factor));
        let scale = old_height.map_or(1., |old| {
            f32::from(self.row_height(top.item_ix)) / f32::from(old)
        });
        self.list.reset(self.rows.len());
        self.list.scroll_to(ListOffset {
            item_ix: top.item_ix,
            offset_in_item: top.offset_in_item * scale,
        });
    }
    pub fn set_diff(
        &mut self,
        diff: Arc<FileDiff>,
        path: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.apply_diff(diff, path, None, window, cx);
    }
    pub fn set_prepared_diff(
        &mut self,
        diff: Arc<FileDiff>,
        path: String,
        tokens: syntax::Tokens,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.apply_diff(diff, path, Some(tokens), window, cx);
    }
    fn apply_diff(
        &mut self,
        diff: Arc<FileDiff>,
        path: String,
        prepared: Option<syntax::Tokens>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let unchanged = self
            .diff_source
            .as_ref()
            .is_some_and(|(source, source_path)| {
                Arc::ptr_eq(source, &diff) && source_path == &path
            });
        if unchanged && prepared.is_none() {
            return;
        }
        if !unchanged {
            self.is_diff = true;
            self.ends_with_newline = false;
            let rows = diff::rows(Some(&diff)).iter().map(Row::from).collect();
            self.reset(rows, window, cx);
            self.diff_source = Some((diff.clone(), path.clone()));
            if diff.binary {
                self.state=Some("Fichier binaire\nLe contenu de ce fichier ne peut pas être affiché sous forme de diff texte.".into());
            } else if diff.hunks.is_empty() && !diff.truncated {
                self.state=Some("Aucune différence de contenu texte. Le changement peut concerner le nom ou les permissions du fichier.".into());
            }
            if diff.truncated {
                self.notice=Some("Aperçu limité pour préserver la réactivité. Le fichier est volumineux ou le diff dépasse la limite d’affichage.".into());
            }
        }
        if let Some(tokens) = prepared {
            self.generation = self.generation.wrapping_add(1);
            self.syntax_task = None;
            self.syntax_pending = false;
            if !Arc::ptr_eq(&self.tokens, &tokens) {
                self.tokens = tokens;
                cx.notify();
            }
            return;
        }
        let generation = self.generation;
        self.syntax_pending = true;
        self.syntax_task = Some(cx.spawn(async move |this, cx| {
            let tokens = cx
                .background_executor()
                .spawn(async move { syntax::cached_diff_tokens(&path, &diff) })
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.generation == generation {
                    view.tokens = tokens;
                    view.syntax_pending = false;
                    view.syntax_task = None;
                    cx.notify();
                }
            });
        }));
        cx.notify();
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
            .map(|(index, text)| Row {
                kind: "content".into(),
                old_number: String::new(),
                new_number: (index + 1).to_string(),
                original: text.clone().into(),
                display: DisplayLine::new(text),
            })
            .collect();
        self.reset(rows, window, cx);
        self.content_source = Some(content.clone());
        self.state = match content.kind.as_str() {
            "binary" => Some(
                "Fichier binaire ou encodage non UTF-8 : aucun affichage texte ni blame.".into(),
            ),
            "deleted" => {
                self.parent_available = content.parent_sha.is_some();
                Some("Ce fichier est absent à la révision sélectionnée.".into())
            }
            "submodule" => Some("Sous-module : son contenu appartient à un autre dépôt.".into()),
            "large" => Some(
                "Fichier supérieur à 2 Mio : son contenu et son blame ne sont pas chargés.".into(),
            ),
            "unsupported" => {
                Some("Ce type d’objet Git ne peut pas être affiché comme un fichier.".into())
            }
            _ if content.lines.is_empty() && !content.truncated => {
                Some("Fichier vide à cette révision.".into())
            }
            _ => None,
        };
        if content.truncated {
            self.notice=Some("Aperçu limité à 2 000 lignes et 256 Kio de texte. Le blame concerne uniquement les lignes affichées.".into());
        } else if content.size > 0 && !content.ends_with_newline {
            self.notice = Some("Pas de saut de ligne à la fin du fichier.".into());
        }
        let generation = self.generation;
        self.syntax_pending = true;
        self.syntax_task = Some(cx.spawn(async move |this, cx| {
            let tokens = cx
                .background_executor()
                .spawn(async move { syntax::cached_content_tokens(&content.path, &content.lines) })
                .await;
            let _ = this.update(cx, |view, cx| {
                if view.generation == generation {
                    view.tokens = tokens;
                    view.syntax_pending = false;
                    view.syntax_task = None;
                    cx.notify();
                }
            });
        }));
        cx.notify();
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
            self.content_width += if value { 240. } else { -240. };
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
        self.palette = Palette::for_theme(preferences.theme);
        self.font_size = if preferences.diff_font_size.is_finite() {
            preferences.diff_font_size.round().clamp(9., 20.)
        } else {
            11.
        };
        if old != self.font_size {
            let fixed = if self.is_diff {
                35.
            } else {
                32. + if self.show_blame { 240. } else { 0. }
            };
            self.content_width = (self.content_width - fixed) * self.font_size / old + fixed;
            self.number_width *= self.font_size / old;
            self.rebuild_row_geometry(self.geometry_scale);
        }
        cx.notify();
    }
    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.generation = self.generation.wrapping_add(1);
        self.syntax_task = None;
        self.syntax_pending = false;
        self.rows = Arc::new(Vec::new());
        self.scroll.set_geometry(RowGeometry::default());
        self.diff_source = None;
        self.content_source = None;
        self.tokens = Arc::new(Vec::new());
        self.selection = None;
        self.dragging = false;
        self.drag_point = None;
        self.state = Some("Sélectionnez un fichier pour voir ses modifications.".into());
        self.error = None;
        self.loading = false;
        self.notice = None;
        self.parent_available = false;
        self.blame = None;
        self.blame_indices.clear();
        self.content_width = 0.;
        self.ends_with_newline = false;
        self.list.reset(0);
        self.layouts.borrow_mut().clear();
        cx.notify();
    }
    pub fn set_loading(&mut self, value: bool, cx: &mut Context<Self>) {
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
        !self.loading && !self.syntax_pending
    }
    #[cfg(feature = "app-benchmark")]
    pub fn syntax_colored(&self) -> bool {
        self.tokens
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
            div()
                .id(id)
                .role(Role::Button)
                .focusable()
                .tab_index(0)
                .aria_label(if delta < 0. {
                    "Diminuer la taille du texte du diff"
                } else {
                    "Augmenter la taille du texte du diff"
                })
                .flex()
                .items_center()
                .justify_center()
                .w(px(26.))
                .h(px(26.))
                .rounded(px(4.))
                .cursor_pointer()
                .when(enabled, |view| {
                    view.hover(move |style| style.bg(rgb(palette.hover)))
                })
                .when(!enabled, |view| view.opacity(0.4))
                .child(icon(name, 14., palette.muted))
                .tooltip(move |window, cx| {
                    Tooltip::new(if delta < 0. {
                        "Diminuer la taille du texte du diff"
                    } else {
                        "Augmenter la taille du texte du diff"
                    })
                    .build(window, cx)
                })
                .on_click(cx.listener(move |view, _, _, cx| {
                    if enabled {
                        cx.emit(CodeViewEvent::FontSizeChanged(
                            (view.font_size + delta).clamp(9., 20.),
                        ));
                    }
                }))
                .on_key_down(cx.listener(move |view, event: &KeyDownEvent, _, cx| {
                    if enabled && matches!(event.keystroke.key.as_str(), "enter" | "space") {
                        cx.emit(CodeViewEvent::FontSizeChanged(
                            (view.font_size + delta).clamp(9., 20.),
                        ));
                        cx.stop_propagation();
                    }
                }))
        };
        div()
            .flex()
            .items_center()
            .gap(px(2.))
            .font_family(SANS)
            .child(button("diff-font-minus", IconName::Minus, size > 9., -1.))
            .child(
                div()
                    .min_w(px(32.))
                    .text_center()
                    .text_size(px(10. + self.ui_offset))
                    .text_color(rgb(palette.muted))
                    .child(format!("{size:.0} px")),
            )
            .child(button("diff-font-plus", IconName::Plus, size < 20., 1.))
            .into_any_element()
    }

    fn position_from_layout(&self, index: usize, position: Point<Pixels>) -> Position {
        let byte = self.layouts.borrow().get(&index).map_or(0, |layout| {
            layout
                .index_for_position(position)
                .unwrap_or_else(|closest| closest)
        });
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
        let position = self.position_from_layout(index, event.position);
        self.active_line = index;
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
        _window: &mut Window,
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
        if target.is_none() {
            let top = self
                .list
                .logical_scroll_top()
                .item_ix
                .min(self.rows.len() - 1);
            if point.y < viewport.top() {
                self.scroll.scroll_vertical_by(px(-self.font_size * 1.7));
                target = Some(top.saturating_sub(1));
            } else if point.y > viewport.bottom() {
                self.scroll.scroll_vertical_by(px(self.font_size * 1.7));
                target = Some(
                    self.layouts
                        .borrow()
                        .keys()
                        .next_back()
                        .copied()
                        .unwrap_or(top)
                        .saturating_add(1)
                        .min(self.rows.len() - 1),
                );
            }
        }
        if let Some(index) = target {
            let head = self.position_from_layout(index, point);
            if let Some(selection) = &mut self.selection {
                selection.head = head;
            }
            cx.notify();
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
        let Some(selection) = self.selection else {
            return String::new();
        };
        let (start, end) = selection.ordered();
        if start == end {
            return String::new();
        }
        let mut text = String::new();
        for index in start.row..=end.row.min(self.rows.len().saturating_sub(1)) {
            let row = &self.rows[index];
            let range = selection
                .range_in_row(index, row.original.len())
                .unwrap_or(0..0);
            if index > start.row {
                text.push('\n');
            }
            text.push_str(&row.original[range]);
        }
        if !self.is_diff
            && self.ends_with_newline
            && start == Position::default()
            && end.row + 1 == self.rows.len()
            && end.byte == self.rows[end.row].original.len()
        {
            text.push('\n');
        }
        text
    }
    fn copy_selection(&self, cx: &mut Context<Self>) {
        let text = self.selected_text();
        if !text.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
        cx.stop_propagation();
    }
    fn select_all(&mut self, cx: &mut Context<Self>) {
        if self.rows.is_empty() {
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
        if key == "escape" {
            self.selection = None;
            self.dragging = false;
            self.drag_point = None;
            cx.notify();
            return;
        }
        if self.show_blame && matches!(key, "enter" | "space") && !modifiers.shift {
            if let Some(group) = self.blame_group(self.active_line) {
                cx.emit(CodeViewEvent::NavigateCommit(group.sha.clone()));
                cx.stop_propagation();
            }
            return;
        }
        let current = self.selection.map_or(
            Position {
                row: self.active_line,
                byte: 0,
            },
            |selection| selection.head,
        );
        let viewport_height: f32 = self.list.viewport_bounds().size.height.into();
        let page = (viewport_height / (self.font_size * 1.7)).floor() as usize;
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
    fn highlights(&self, index: usize, row: &Row) -> Vec<(Range<usize>, HighlightStyle)> {
        let selection = self
            .selection
            .and_then(|selection| selection.range_in_row(index, row.original.len()));
        let tokens = self.tokens.get(index).map(Vec::as_slice).unwrap_or(&[]);
        let mut cuts = vec![0, row.original.len()];
        for token in tokens {
            cuts.extend([token.range.start, token.range.end]);
        }
        if let Some(selected) = &selection {
            cuts.extend([selected.start, selected.end]);
        }
        cuts.sort_unstable();
        cuts.dedup();
        let mut token_index = 0;
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
            let selected = selection
                .as_ref()
                .is_some_and(|selected| selected.start <= range.start && selected.end >= range.end);
            result.push((
                row.display.display_byte(range.start)..row.display.display_byte(range.end),
                HighlightStyle {
                    color: if selected {
                        Some(rgb(self.palette.text).into())
                    } else {
                        kind.palette_index()
                            .map(|index| rgb(self.palette.syntax[index]).into())
                    },
                    background_color: selected.then(|| rgb(self.palette.selection).into()),
                    ..Default::default()
                },
            ));
        }
        result
    }
    fn render_row(
        &self,
        index: usize,
        first_visible_row: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let row = &self.rows[index];
        let p = self.palette;
        let hunk = row.kind == "hunk";
        let text =
            StyledText::new(row.display.text.clone()).with_highlights(self.highlights(index, row));
        self.layouts
            .borrow_mut()
            .insert(index, text.layout().clone());
        let gutter = |value: String| {
            div()
                .w(px(self.number_width))
                .h_full()
                .flex_shrink_0()
                .border_r_1()
                .border_color(rgb(p.border))
                .pr(px(if self.is_diff { 5. } else { 8. }))
                .text_right()
                .text_color(rgb(
                    if matches!(row.kind.as_str(), "addition" | "deletion") {
                        p.text
                    } else {
                        p.muted
                    },
                ))
                .child(value)
        };
        let mut line = div()
            .id(("code-row", index))
            .role(if self.show_blame && !self.is_diff {
                Role::ListBoxOption
            } else {
                Role::Paragraph
            })
            .aria_label(if hunk {
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
            .min_w(px(self.content_width))
            .h(self.row_height(index))
            .font_family(MONO)
            .text_size(px(self.font_size))
            .line_height(px(self.font_size * 1.7))
            .text_color(rgb(if hunk { p.accent } else { p.text }))
            .bg(rgb(match row.kind.as_str() {
                "hunk" => p.surface,
                "addition" => p.addition_bg,
                "deletion" => p.deletion_bg,
                _ if self.show_blame
                    && index == self.active_line
                    && self.focus.is_focused(window) =>
                {
                    p.surface
                }
                _ => p.reading_bg,
            }));
        if hunk {
            line = line.px(px(13.)).py(px(5.)).when(index > 0, |line| {
                line.border_t_1().border_color(rgb(p.border))
            });
        } else if self.is_diff {
            let (sign, color) = match row.kind.as_str() {
                "addition" => ("+", p.success),
                "deletion" => ("−", p.danger),
                _ => (" ", p.muted),
            };
            line = line
                .child(gutter(row.old_number.clone()))
                .child(gutter(row.new_number.clone()))
                .child(
                    div()
                        .w(px(20.))
                        .flex_shrink_0()
                        .text_center()
                        .text_color(rgb(color))
                        .child(sign),
                );
        } else {
            if self.show_blame {
                let mut attribution = div()
                    .id(("blame-line", index))
                    .role(Role::Button)
                    .flex()
                    .w(px(240.))
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
                if let Some(group) = self.blame_group(index) {
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
                    attribution = attribution
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
                        let date = commit_date(group.time);
                        attribution = attribution
                            .child(short_sha(&group.sha))
                            .child(
                                div()
                                    .max_w(px(90.))
                                    .text_ellipsis()
                                    .text_color(rgb(p.text))
                                    .child(group.author.clone()),
                            )
                            .child(div().text_ellipsis().child(date));
                    } else {
                        attribution = attribution.child(div().opacity(0.35).child("│"));
                    }
                } else {
                    attribution = attribution.child("—");
                }
                line = line.child(attribution);
            }
            line = line
                .child(gutter(row.new_number.clone()))
                .child(div().w(px(12.)).flex_shrink_0());
        }
        let code = div()
            .id(("code-text", index))
            .flex()
            .flex_1()
            .h_full()
            .pr(px(if self.is_diff { 15. } else { 20. }))
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
        line.child(code).into_any_element()
    }
}

impl Render for CodeView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.geometry_scale != window.scale_factor() {
            self.rebuild_row_geometry(window.scale_factor());
        }
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
            .role(if self.show_blame&&!self.is_diff {Role::ListBox} else {Role::Document})
            .aria_label(if self.is_diff {"Modifications du fichier"} else if self.show_blame {"Blame du fichier"} else {"Contenu du fichier"})
            .aria_description("Texte sélectionnable. Flèches et Majuscule pour étendre la sélection, ⌘C pour copier, ⌘A pour tout sélectionner.")
            .flex()
            .flex_col()
            .size_full()
            .min_h_0()
            .min_w_0()
            .bg(rgb(p.reading_bg))
            .track_focus(&self.focus)
            .on_action(cx.listener(|view,_:&Copy,_,cx|view.copy_selection(cx)))
            .on_action(cx.listener(|view,_:&SelectAll,_,cx|view.select_all(cx)))
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
        if let Some(state) = self.error.as_ref().or(self.state.as_ref()) {
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
                    .when(self.parent_available, |view| {
                        view.child(
                            div()
                                .id("code-parent")
                                .role(Role::Button)
                                .focusable()
                                .tab_index(0)
                                .aria_label("Voir la version du premier parent")
                                .px(px(12.))
                                .py(px(7.))
                                .border_1()
                                .border_color(rgb(p.border))
                                .rounded(px(4.))
                                .bg(rgb(p.surface))
                                .text_color(rgb(p.text))
                                .cursor_pointer()
                                .child("Voir la version du premier parent")
                                .on_click(cx.listener(|_, _, _, cx| cx.emit(CodeViewEvent::Parent)))
                                .on_key_down(cx.listener(|_, event: &KeyDownEvent, _, cx| {
                                    if matches!(event.keystroke.key.as_str(), "enter" | "space") {
                                        cx.emit(CodeViewEvent::Parent);
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
            let wheel_view = cx.weak_entity();
            let wheel_capture = self.scroll.wheel_capture(move |changed, _, cx| {
                if changed {
                    let _ = wheel_view.update(cx, |_, cx| cx.notify());
                }
            });
            let code = list(self.list.clone(), move |index, window, cx| {
                entity.update(cx, |view, cx| {
                    view.render_row(index, first_visible_row, window, cx)
                })
            })
            .w_full()
            .min_w(px(self.content_width))
            .h_full();
            root = root.child(
                div()
                    .id("code-scroll-viewport")
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .w_full()
                    .overflow_hidden()
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
                    ),
            );
            if let Some(notice) = &self.notice {
                root = root.child(
                    div()
                        .flex()
                        .flex_shrink_0()
                        .items_start()
                        .gap(px(8.))
                        .px(px(17.))
                        .py(px(13.))
                        .font_family(SANS)
                        .text_size(px(10. + self.ui_offset))
                        .text_color(rgb(p.warning))
                        .child(icon(IconName::CircleAlert, 14., p.warning))
                        .child(notice.clone()),
                );
            }
        }
        if self.loading {
            root = root.opacity(0.65);
        }
        root
    }
}

#[cfg(test)]
mod tests {
    use super::{DisplayLine, Position, Row, Selection, row_geometry};
    use crate::virtual_scroll::VirtualScrollHandle;
    use gpui_kit::component::scroll::ScrollbarHandle;
    use gpui_kit::{ListAlignment, ListState, ScrollHandle, point, px};

    fn row(kind: &str) -> Row {
        Row {
            kind: kind.into(),
            old_number: String::new(),
            new_number: String::new(),
            original: "line".into(),
            display: DisplayLine::new("line"),
        }
    }

    fn near(value: gpui_kit::Pixels, expected: f32) {
        assert!((f32::from(value) - expected).abs() < 0.01);
    }

    #[test]
    fn unmeasured_hunks_keep_exact_scroll_targets_after_reset_and_zoom() {
        let rows = [row("hunk"), row("context"), row("hunk"), row("addition")];
        let list = ListState::new(rows.len(), ListAlignment::Top, px(150.));
        let handle = VirtualScrollHandle::exact(
            list.clone(),
            ScrollHandle::new(),
            row_geometry(&rows, 11., 1.),
        );
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

        handle.set_geometry(row_geometry(&rows, 20., 1.));
        let zoomed = handle;
        near(zoomed.content_size().height, 157.);
        zoomed.set_offset(point(px(0.), px(-100.)));
        assert_eq!(list.logical_scroll_top().item_ix, 2);
        near(list.logical_scroll_top().offset_in_item, 22.);
        near(zoomed.offset().y, -100.);
    }

    #[test]
    fn scroll_targets_clamp_and_keep_content_blame_rows_uniform() {
        let offsets = row_geometry(&[row("content"), row("content"), row("content")], 11., 1.);
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
        let empty = crate::virtual_scroll::RowGeometry::default().list_offset(px(100.), px(0.));
        assert_eq!(empty.item_ix, 0);
        near(empty.offset_in_item, 0.);
    }

    #[test]
    fn physical_row_boundaries_keep_css_height_at_one_and_two_times_scale() {
        let rows: Vec<_> = (0..1_000).map(|_| row("content")).collect();
        for scale in [1., 2.] {
            for font_size in [9., 11., 20.] {
                let offsets = row_geometry(&rows, font_size, scale);
                near(offsets.total(), font_size * 1_700.);
                // Every rendered row's explicit height is a whole physical
                // pixel, so GPUI's layout snapping leaves it unchanged.
                for index in 0..rows.len() {
                    let physical = f32::from(offsets.height(index).unwrap()) * scale;
                    assert_eq!(physical.fract(), 0.);
                }
            }
            let mixed = row_geometry(
                &[row("hunk"), row("context"), row("hunk"), row("addition")],
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
        let rows: Vec<_> = (0..2_000).map(|_| row("content")).collect();
        for scale in [1., 2.] {
            let offsets = row_geometry(&rows, 11., scale);
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
        let line = DisplayLine::new("\télève\tx");
        assert_eq!(line.text.as_ref(), "        élève   x");
        for byte in [0, 1, 3, 4, 6, 7, 8, 9, 10] {
            if "\télève\tx".is_char_boundary(byte) {
                assert_eq!(line.original_byte(line.display_byte(byte)), byte);
            }
        }
    }
}
