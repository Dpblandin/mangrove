//! Mangrove's reference tree and lazy repository context, rendered with a
//! variable-height virtual list so repository size does not set UI work per frame.
use gpui_kit::{
    assets::IconName,
    base::Button as NativeButton,
    component::{
        Icon, Sizable,
        input::{Input, InputEvent, InputState, MoveDown},
        menu::{PopupMenu, PopupMenuItem},
        scroll::Scrollbar,
        tooltip::Tooltip,
    },
    prelude::*,
    *,
};
use icu_collator::{
    Collator, CollatorBorrowed, CollatorPreferences,
    options::{CollatorOptions, Strength},
    preferences::CollationNumericOrdering,
};
use mangrove_core::{
    Workspace,
    model::{Reference, RepositoryInfo, WorktreeInfo, WorktreeList},
};
use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
    ops::Range,
    sync::{Arc, OnceLock},
};

use crate::{
    app::Escape,
    format::{commit_date, compact_date, count_label, display_path, format_count, short_sha},
    repository_context::RepositoryContextState,
    theme::{MONO, Palette, RADIUS_MD, RADIUS_SM, SANS, icon, scrollbar_theme},
    virtual_scroll::VirtualScrollHandle,
};

/// Navigation remains owned by the workspace. Reference names are full Git
/// names (`refs/heads/main`), worktree paths are absolute, stash values are SHAs.
#[derive(Clone, Debug)]
pub enum SidebarEvent {
    NavigateReference(String),
    ToggleSolo(String),
    OpenWorktree(String),
    SelectStash(String),
    StashUnavailable { repository_id: String, sha: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum ReferenceKind {
    Local,
    Remote,
    Tag,
}

impl ReferenceKind {
    const ALL: [Self; 3] = [Self::Local, Self::Remote, Self::Tag];
    fn name(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Remote => "remote",
            Self::Tag => "tag",
        }
    }
    fn title(self) -> &'static str {
        match self {
            Self::Local => "Branches locales",
            Self::Remote => "Branches distantes",
            Self::Tag => "Tags",
        }
    }
    fn icon(self) -> IconName {
        match self {
            Self::Local => IconName::GitBranch,
            Self::Remote => IconName::Globe,
            Self::Tag => IconName::Tag,
        }
    }
    fn can_solo(self) -> bool {
        matches!(self, Self::Local | Self::Remote)
    }
    fn count_label(self, count: usize) -> String {
        match self {
            Self::Local | Self::Remote => count_label(count, "branche", "branches"),
            Self::Tag => count_label(count, "tag", "tags"),
        }
    }
}

#[derive(Clone, Debug)]
enum ReferenceNode {
    Folder {
        path: String,
        label: String,
        highlights: Vec<Range<usize>>,
        count: usize,
        children: Vec<Self>,
    },
    Reference {
        label: String,
        highlights: Vec<Range<usize>>,
        reference: Arc<Reference>,
    },
}

impl ReferenceNode {
    fn label(&self) -> &str {
        match self {
            Self::Folder { label, .. } | Self::Reference { label, .. } => label,
        }
    }
}

#[derive(Default)]
struct FolderBuilder {
    count: usize,
    highlights: Vec<Range<usize>>,
    folders: Vec<(String, Self)>,
    folder_indices: HashMap<String, usize>,
    references: Vec<ReferenceNode>,
}

fn reference_tree(
    references: impl Iterator<Item = Arc<Reference>>,
    filter: &str,
) -> Vec<ReferenceNode> {
    let mut root = FolderBuilder::default();
    for reference in references {
        let segments: Vec<_> = reference.label.split('/').collect();
        let highlights = search_highlight_ranges(&reference.label, filter);
        let mut offset = 0;
        let mut folder = &mut root;
        for segment in &segments[..segments.len().saturating_sub(1)] {
            let index = if let Some(index) = folder.folder_indices.get(*segment) {
                *index
            } else {
                let index = folder.folders.len();
                folder.folder_indices.insert((*segment).to_owned(), index);
                folder
                    .folders
                    .push(((*segment).to_owned(), FolderBuilder::default()));
                index
            };
            folder = &mut folder.folders[index].1;
            folder.count += 1;
            folder.highlights.extend(visible_highlight_ranges(
                &highlights,
                offset..offset + segment.len(),
            ));
            offset += segment.len() + 1;
        }
        let label = segments.last().copied().unwrap_or("").to_owned();
        folder.references.push(ReferenceNode::Reference {
            highlights: visible_highlight_ranges(&highlights, offset..reference.label.len()),
            label,
            reference,
        });
    }
    fn children(folder: FolderBuilder, parent: &str) -> Vec<ReferenceNode> {
        let mut nodes: Vec<_> = folder
            .folders
            .into_iter()
            .map(|(label, mut folder)| {
                let path = if parent.is_empty() {
                    label.clone()
                } else {
                    format!("{parent}/{label}")
                };
                let count = folder.count;
                let highlights = merged_highlight_ranges(std::mem::take(&mut folder.highlights));
                ReferenceNode::Folder {
                    children: children(folder, &path),
                    count,
                    path,
                    label,
                    highlights,
                }
            })
            .collect();
        nodes.extend(folder.references);
        nodes.sort_by(|a, b| reference_order(a.label(), b.label()));
        nodes
    }
    children(root, "")
}

// The source uses Intl.Collator("fr", { numeric: true, sensitivity: "base" }).
// Keep a shared ICU collator so sorting never allocates folded copies of names.
fn reference_order(a: &str, b: &str) -> Ordering {
    static COLLATOR: OnceLock<CollatorBorrowed<'static>> = OnceLock::new();
    COLLATOR
        .get_or_init(|| {
            let locale = "fr".parse().expect("valid French locale");
            let mut preferences = CollatorPreferences::from_locale_strict(&locale)
                .expect("supported French collation preferences");
            preferences.numeric_ordering = Some(CollationNumericOrdering::True);
            let mut options = CollatorOptions::default();
            options.strength = Some(Strength::Primary);
            Collator::try_new(preferences, options).expect("compiled French collation data")
        })
        .compare(a, b)
}

fn worktree_label(item: &WorktreeInfo) -> String {
    if item.detached {
        item.head
            .as_ref()
            .map(|sha| format!("{} · {}", item.head_name, short_sha(sha)))
            .unwrap_or_else(|| item.head_name.clone())
    } else {
        item.head_name.clone()
    }
}

fn filtered_worktree_indices(list: &WorktreeList, filter: &str) -> Vec<usize> {
    let filter = filter.trim().to_lowercase();
    list.worktrees
        .iter()
        .enumerate()
        .filter(|(_, item)| {
            filter.is_empty() || worktree_label(item).to_lowercase().contains(&filter)
        })
        .map(|(index, _)| index)
        .collect()
}

/// Splits Git's « WIP on main: » or « On main: » prefix from a stash message.
/// Reference names cannot contain a colon, so the first one ends the branch.
fn stash_summary(message: &str) -> (Option<&str>, &str) {
    let line = message.lines().next().unwrap_or("").trim();
    line.strip_prefix("WIP on ")
        .or_else(|| line.strip_prefix("On "))
        .and_then(|rest| rest.split_once(':'))
        .map_or((None, line), |(branch, text)| (Some(branch), text.trim()))
}

fn stash_name(index: usize) -> String {
    format!("stash@{{{index}}}")
}

/// The date precedes the author, whose name is truncated first.
fn stash_details(branch: Option<&str>, date: &str, author: &str) -> String {
    branch
        .into_iter()
        .chain([date, author])
        .collect::<Vec<_>>()
        .join(" · ")
}

fn search_highlight_ranges(text: &str, filter: &str) -> Vec<Range<usize>> {
    let filter = filter.trim().to_lowercase();
    if filter.is_empty() {
        return Vec::new();
    }
    let folded = text.to_lowercase();
    let matches: Vec<_> = folded
        .match_indices(&filter)
        .map(|(start, matched)| start..start + matched.len())
        .collect();
    if matches.is_empty() {
        return Vec::new();
    }
    // Lowercasing can change UTF-8 lengths (İ, Ⱥ). Map folded byte offsets back
    // to complete source characters before passing ranges to GPUI.
    let mut source_ranges = Vec::with_capacity(folded.len());
    for (start, character) in text.char_indices() {
        let folded_len = character.to_lowercase().map(char::len_utf8).sum();
        source_ranges.extend(std::iter::repeat_n(
            start..start + character.len_utf8(),
            folded_len,
        ));
    }
    merged_highlight_ranges(
        matches
            .into_iter()
            .map(|matched| source_ranges[matched.start].start..source_ranges[matched.end - 1].end)
            .collect(),
    )
}

// Project full-name matches onto the segment displayed in a tree row. Folder
// highlights come from actual descendant matches, including cross-segment ones.
fn visible_highlight_ranges(matches: &[Range<usize>], visible: Range<usize>) -> Vec<Range<usize>> {
    matches
        .iter()
        .filter_map(|matched| {
            let start = matched.start.max(visible.start);
            let end = matched.end.min(visible.end);
            (start < end).then(|| start - visible.start..end - visible.start)
        })
        .collect()
}

fn merged_highlight_ranges(mut matches: Vec<Range<usize>>) -> Vec<Range<usize>> {
    matches.sort_by_key(|range| range.start);
    let mut ranges: Vec<Range<usize>> = Vec::new();
    for range in matches {
        if let Some(previous) = ranges
            .last_mut()
            .filter(|previous| range.start <= previous.end)
        {
            previous.end = previous.end.max(range.end);
        } else {
            ranges.push(range);
        }
    }
    ranges
}

#[derive(Clone, Debug, Default, PartialEq)]
struct ReferenceTreeState {
    filter: String,
    collapsed_groups: HashSet<ReferenceKind>,
    collapsed_folders: HashSet<(ReferenceKind, String)>,
    search_groups: HashSet<ReferenceKind>,
    search_folders: HashSet<(ReferenceKind, String)>,
}

struct ReferenceGroup {
    kind: ReferenceKind,
    count: usize,
    tree: Vec<ReferenceNode>,
}

impl ReferenceTreeState {
    fn searching(&self) -> bool {
        !self.filter.trim().is_empty()
    }
    fn set_filter(&mut self, value: String) {
        if self.filter != value {
            self.filter = value;
            self.search_groups.clear();
            self.search_folders.clear();
        }
    }
    fn group_closed(&self, kind: ReferenceKind) -> bool {
        if self.searching() {
            self.search_groups.contains(&kind)
        } else {
            self.collapsed_groups.contains(&kind)
        }
    }
    fn folder_closed(&self, kind: ReferenceKind, path: &str) -> bool {
        let key = (kind, path.to_owned());
        if self.searching() {
            self.search_folders.contains(&key)
        } else {
            self.collapsed_folders.contains(&key)
        }
    }
    fn toggle_group(&mut self, kind: ReferenceKind) {
        let closed = if self.searching() {
            &mut self.search_groups
        } else {
            &mut self.collapsed_groups
        };
        if !closed.remove(&kind) {
            closed.insert(kind);
        }
    }
    fn toggle_folder(&mut self, kind: ReferenceKind, path: String) {
        let closed = if self.searching() {
            &mut self.search_folders
        } else {
            &mut self.collapsed_folders
        };
        let key = (kind, path);
        if !closed.remove(&key) {
            closed.insert(key);
        }
    }
}

#[derive(Clone)]
enum SidebarRow {
    Group {
        kind: ReferenceKind,
        count: usize,
        closed: bool,
    },
    Folder {
        kind: ReferenceKind,
        path: String,
        label: String,
        highlights: Vec<Range<usize>>,
        count: usize,
        depth: usize,
        closed: bool,
    },
    Reference {
        kind: ReferenceKind,
        label: String,
        highlights: Vec<Range<usize>>,
        reference: Arc<Reference>,
        depth: usize,
    },
    Spacer(f32),
    NoReferences,
    ContextSeparator {
        margin_top: f32,
    },
    Worktrees {
        count: Option<usize>,
        expanded: bool,
    },
    /// Context entries keep their list index for rendering and their path or
    /// SHA as identity, since a refresh can shift the indices.
    Worktree {
        index: usize,
        path: String,
    },
    Stashes {
        count: Option<usize>,
        expanded: bool,
    },
    Stash {
        index: usize,
        sha: String,
    },
    Notice {
        text: String,
        error: bool,
    },
}

/// Matches the history rows.
const ROW_HEIGHT: f32 = 28.;

fn section_height(font_offset: f32) -> f32 {
    (11. + font_offset) * 1.5 + 10.
}

/// Worktrees and stashes show a second, muted line.
fn context_item_height(font_offset: f32) -> f32 {
    (11. + font_offset) * 3. + 8.
}

impl SidebarRow {
    fn height_hint(&self, font_offset: f32) -> Pixels {
        px(match self {
            Self::Group { .. } | Self::Worktrees { .. } | Self::Stashes { .. } => {
                section_height(font_offset)
            }
            Self::Folder { .. } | Self::Reference { .. } => ROW_HEIGHT,
            Self::Spacer(height) => *height,
            Self::NoReferences => (11. + font_offset) * 1.5 + 40.,
            Self::ContextSeparator { margin_top } => margin_top + 8.,
            Self::Worktree { .. } | Self::Stash { .. } => context_item_height(font_offset),
            Self::Notice { .. } => (11. + font_offset) * 1.5 + 16.,
        })
    }

    /// Tree depth of a keyboard-navigable row. Spacers, separators and
    /// notices are skipped.
    fn level(&self) -> Option<usize> {
        match self {
            Self::Group { .. } | Self::Worktrees { .. } | Self::Stashes { .. } => Some(0),
            Self::Folder { depth, .. } | Self::Reference { depth, .. } => Some(depth + 1),
            Self::Worktree { .. } | Self::Stash { .. } => Some(1),
            Self::Spacer(_)
            | Self::NoReferences
            | Self::ContextSeparator { .. }
            | Self::Notice { .. } => None,
        }
    }

    fn navigable(&self) -> bool {
        self.level().is_some()
    }

    fn expanded(&self) -> Option<bool> {
        match self {
            Self::Group { closed, .. } | Self::Folder { closed, .. } => Some(!closed),
            Self::Worktrees { expanded, .. } | Self::Stashes { expanded, .. } => Some(*expanded),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TreeKey {
    Previous,
    Next,
    Collapse,
    Expand,
    First,
    Last,
}

impl TreeKey {
    fn from_key(key: &str) -> Option<Self> {
        Some(match key {
            "up" => Self::Previous,
            "down" => Self::Next,
            "left" => Self::Collapse,
            "right" => Self::Expand,
            "home" => Self::First,
            "end" => Self::Last,
            _ => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TreeMove {
    Focus(usize),
    Toggle(usize),
}

/// Keyboard movement over the flattened tree: arrows move between visible
/// rows, ← collapses or reaches the parent, → expands or reaches the first child.
fn tree_move(rows: &[SidebarRow], active: Option<usize>, key: TreeKey) -> Option<TreeMove> {
    let navigable = |index: &usize| rows[*index].navigable();
    let first = || (0..rows.len()).find(navigable);
    let last = || (0..rows.len()).rev().find(navigable);
    let Some((active, level)) = active.and_then(|index| Some((index, rows.get(index)?.level()?)))
    else {
        return if key == TreeKey::Last {
            last()
        } else {
            first()
        }
        .map(TreeMove::Focus);
    };
    match key {
        TreeKey::Previous => (0..active).rev().find(navigable).map(TreeMove::Focus),
        TreeKey::Next => (active + 1..rows.len())
            .find(navigable)
            .map(TreeMove::Focus),
        TreeKey::First => first().map(TreeMove::Focus),
        TreeKey::Last => last().map(TreeMove::Focus),
        TreeKey::Collapse if rows[active].expanded() == Some(true) => {
            Some(TreeMove::Toggle(active))
        }
        TreeKey::Collapse => (0..active)
            .rev()
            .find(|index| rows[*index].level().is_some_and(|parent| parent < level))
            .map(TreeMove::Focus),
        TreeKey::Expand => match rows[active].expanded()? {
            false => Some(TreeMove::Toggle(active)),
            true => (active + 1..rows.len())
                .find(navigable)
                .filter(|index| rows[*index].level() > Some(level))
                .map(TreeMove::Focus),
        },
    }
}

/// Scroll position, filter and collapsed sections and folders of the
/// navigation tree. Opaque, so a tab can keep it and give it back to
/// `restore_view_state`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SidebarViewState {
    scroll_top: Pixels,
    references: ReferenceTreeState,
    worktrees_expanded: bool,
    stashes_expanded: bool,
}

pub struct SidebarView {
    pub(crate) engine: Arc<Workspace>,
    pub(crate) repository: Option<Arc<RepositoryInfo>>,
    pub(crate) context: RepositoryContextState,
    pub(crate) selected_stash: String,
    selected_sha: Option<String>,
    solo_refs: HashSet<String>,
    reference_state: ReferenceTreeState,
    references: Vec<Arc<Reference>>,
    reference_shas: HashSet<String>,
    reference_groups: Vec<ReferenceGroup>,
    rows: Vec<SidebarRow>,
    list_state: ListState,
    scroll_handle: VirtualScrollHandle,
    filter_input: Entity<InputState>,
    sync_filter_input: bool,
    palette: Palette,
    font_size_offset: f32,
    width: f32,
    menu: Option<(Entity<PopupMenu>, Point<Pixels>)>,
    menu_subscription: Option<Subscription>,
    window: AnyWindowHandle,
    /// The tree is one tab stop; its active row is announced as the focused
    /// descendant, so virtualised rows never own the keyboard focus.
    tree_focus: FocusHandle,
    active_row: Option<usize>,
    /// Tooltips keep the bounds of the row they were opened on. A new row
    /// identity after each wheel scroll drops them.
    scroll_epoch: usize,
    /// A restored offset waits for the expanded worktree and stash lists that
    /// complete the tree height. Any input in the sidebar drops it.
    restored_scroll_top: Option<Pixels>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<SidebarEvent> for SidebarView {}

impl SidebarView {
    pub fn view_state(&self) -> SidebarViewState {
        SidebarViewState {
            scroll_top: self
                .restored_scroll_top
                .unwrap_or_else(|| self.scroll_handle.vertical_offset()),
            references: self.reference_state.clone(),
            worktrees_expanded: self.context.worktrees_expanded,
            stashes_expanded: self.context.stashes_expanded,
        }
    }

    /// Call after `set_repository`, so the restored offset applies to its rows.
    pub fn restore_view_state(&mut self, state: SidebarViewState, cx: &mut Context<Self>) {
        let filter_changed = self.reference_state.filter != state.references.filter;
        self.set_reference_state(state.references);
        if filter_changed {
            self.rebuild_reference_groups();
        }
        self.context.worktrees_expanded = state.worktrees_expanded;
        self.context.stashes_expanded = state.stashes_expanded;
        self.restored_scroll_top = Some(state.scroll_top);
        self.load_expanded_context(cx);
        self.rebuild_rows(cx);
    }

    fn set_reference_state(&mut self, state: ReferenceTreeState) {
        self.reference_state = state;
        self.sync_filter_input = true;
    }
}

impl SidebarView {
    pub fn focus_references(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_menu(cx);
        self.filter_input
            .update(cx, |input, cx| input.focus(window, cx));
    }
    pub fn show_context(&mut self, worktrees: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.close_menu(cx);
        if worktrees && !self.context.worktrees_expanded {
            self.toggle_worktrees(cx);
        } else if !worktrees && !self.context.stashes_expanded {
            self.toggle_stashes(cx);
        }
        let index = self.rows.iter().position(|row| {
            if worktrees {
                matches!(row, SidebarRow::Worktrees { .. })
            } else {
                matches!(row, SidebarRow::Stashes { .. })
            }
        });
        if let Some(index) = index {
            self.set_active_row(index, cx);
        }
        self.tree_focus.focus(window, cx);
        cx.notify();
    }
    pub fn new(engine: Arc<Workspace>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder("Filtrer les références et worktrees")
        });
        let subscription = cx.subscribe(&filter_input, |view, input, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                view.reference_state
                    .set_filter(input.read(cx).value().to_string());
                // ↓ from the filter then starts at the first result.
                view.active_row = None;
                view.rebuild_reference_groups();
                view.rebuild_rows(cx);
            }
        });
        let bounds_subscription =
            cx.observe_window_bounds(window, |view, _, cx| view.close_menu(cx));
        let tree_focus = cx.focus_handle().tab_stop(true);
        let focus_subscription = cx.on_focus(&tree_focus, window, |view, _, cx| {
            view.ensure_active_row(cx)
        });
        let list_state = ListState::new(0, ListAlignment::Top, px(200.));
        let view = cx.weak_entity();
        list_state.set_scroll_handler(move |_, _, cx| {
            let _ = view.update(cx, |view, cx| view.close_menu(cx));
        });
        let scroll_handle = VirtualScrollHandle::measured(list_state.clone());
        Self {
            engine,
            repository: None,
            context: RepositoryContextState::default(),
            selected_stash: String::new(),
            selected_sha: None,
            solo_refs: HashSet::new(),
            reference_state: ReferenceTreeState::default(),
            references: Vec::new(),
            reference_shas: HashSet::new(),
            reference_groups: Vec::new(),
            rows: Vec::new(),
            list_state,
            scroll_handle,
            filter_input,
            sync_filter_input: false,
            palette: Palette::default(),
            font_size_offset: 1.,
            width: 226.,
            menu: None,
            menu_subscription: None,
            window: window.window_handle(),
            tree_focus,
            active_row: None,
            scroll_epoch: 0,
            restored_scroll_top: None,
            _subscriptions: vec![subscription, bounds_subscription, focus_subscription],
        }
    }

    /// Tabbing or clicking into the tree starts on the first row in view.
    fn ensure_active_row(&mut self, cx: &mut Context<Self>) {
        if self
            .active_row
            .and_then(|index| self.rows.get(index))
            .is_some_and(SidebarRow::navigable)
        {
            return;
        }
        let top = self.list_state.logical_scroll_top().item_ix;
        self.active_row = (top..self.rows.len())
            .chain(0..top.min(self.rows.len()))
            .find(|&index| self.rows[index].navigable());
        if self.active_row.is_some() {
            cx.notify();
        }
    }

    fn set_active_row(&mut self, index: usize, cx: &mut Context<Self>) {
        self.active_row = Some(index);
        self.restored_scroll_top = None;
        self.scroll_handle.reveal_row(index);
        cx.notify();
    }

    fn focus_tree(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_menu(cx);
        if let Some(index) = self
            .active_row
            .or_else(|| self.rows.iter().position(SidebarRow::navigable))
        {
            self.set_active_row(index, cx);
        }
        self.tree_focus.focus(window, cx);
    }

    fn tree_focus_visible(&self, window: &Window) -> bool {
        self.tree_focus.is_focused(window) && window.last_input_was_keyboard()
    }

    fn tree_key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let keystroke = &event.keystroke;
        let modifiers = keystroke.modifiers;
        if modifiers.platform || modifiers.control || modifiers.alt {
            return;
        }
        match keystroke.key.as_str() {
            "enter" | "space" => {
                if let Some(index) = self.active_row {
                    self.activate_row(index, cx);
                }
            }
            "contextmenu" | "f10" if keystroke.key == "contextmenu" || modifiers.shift => {
                if let Some(index) = self.active_row
                    && let Some(SidebarRow::Reference {
                        kind, reference, ..
                    }) = self.rows.get(index)
                    && kind.can_solo()
                {
                    let name = reference.name.clone();
                    // The row may have scrolled away since it was painted.
                    self.set_active_row(index, cx);
                    let bottom: Pixels = (0..=index)
                        .filter_map(|row| self.scroll_handle.row_height(row))
                        .sum();
                    let viewport = self.list_state.viewport_bounds();
                    let position = point(
                        viewport.left(),
                        viewport.top() + bottom - self.scroll_handle.vertical_offset(),
                    );
                    self.open_branch_menu(name, position, window, cx);
                }
            }
            key => {
                let Some(key) = TreeKey::from_key(key) else {
                    return;
                };
                match tree_move(&self.rows, self.active_row, key) {
                    Some(TreeMove::Focus(index)) => self.set_active_row(index, cx),
                    Some(TreeMove::Toggle(index)) => {
                        self.active_row = Some(index);
                        self.toggle_row(index, cx);
                        if let Some(index) = self.active_row {
                            self.scroll_handle.reveal_row(index);
                        }
                    }
                    None if key == TreeKey::Previous => self.focus_references(window, cx),
                    None => {}
                }
            }
        }
        cx.stop_propagation();
    }

    fn toggle_row(&mut self, index: usize, cx: &mut Context<Self>) {
        match self.rows.get(index) {
            Some(SidebarRow::Group { kind, .. }) => {
                self.reference_state.toggle_group(*kind);
                self.rebuild_rows(cx);
            }
            Some(SidebarRow::Folder { kind, path, .. }) => {
                self.reference_state.toggle_folder(*kind, path.clone());
                self.rebuild_rows(cx);
            }
            Some(SidebarRow::Worktrees { .. }) => self.toggle_worktrees(cx),
            Some(SidebarRow::Stashes { .. }) => self.toggle_stashes(cx),
            _ => {}
        }
    }

    /// Enter and Space do what a click on the row does.
    fn activate_row(&mut self, index: usize, cx: &mut Context<Self>) {
        let event = match self.rows.get(index) {
            Some(SidebarRow::Reference { reference, .. }) => {
                SidebarEvent::NavigateReference(reference.name.clone())
            }
            Some(SidebarRow::Worktree { index, .. }) => {
                let Some(item) = self
                    .context
                    .worktrees
                    .data
                    .as_ref()
                    .and_then(|list| list.worktrees.get(*index))
                    .filter(|item| item.available)
                else {
                    return;
                };
                SidebarEvent::OpenWorktree(item.path.clone())
            }
            Some(SidebarRow::Stash { sha, .. }) => SidebarEvent::SelectStash(sha.clone()),
            _ => return self.toggle_row(index, cx),
        };
        cx.emit(event);
    }

    pub fn set_theme(&mut self, palette: Palette, font_size_offset: f32, cx: &mut Context<Self>) {
        if self.palette == palette && self.font_size_offset == font_size_offset {
            return;
        }
        self.palette = palette;
        self.font_size_offset = font_size_offset;
        self.invalidate_row_measurements();
        self.close_menu(cx);
        cx.notify();
    }

    pub fn set_width(&mut self, width: f32, cx: &mut Context<Self>) {
        if (self.width - width).abs() < 0.5 {
            return;
        }
        self.width = width;
        self.invalidate_row_measurements();
        self.close_menu(cx);
        cx.notify();
    }

    fn shares_worktree_list(&self, path: &str) -> bool {
        self.repository.is_some()
            && self
                .context
                .worktrees
                .data
                .as_ref()
                .is_some_and(|list| list.worktrees.iter().any(|item| item.path == path))
    }

    /// Linked worktrees share this navigation tree. Keep its rows and scroll
    /// anchor while the new session and its lazy context are being loaded.
    pub fn prepare_repository_open(&mut self, path: &str, cx: &mut Context<Self>) {
        if self.shares_worktree_list(path) {
            self.context.invalidate(false);
            self.close_menu(cx);
        } else {
            self.clear_repository(cx);
        }
    }

    pub fn set_repository(&mut self, repository: Arc<RepositoryInfo>, cx: &mut Context<Self>) {
        let shared_worktrees = self.shares_worktree_list(&repository.path);
        let changed = self
            .repository
            .as_ref()
            .is_none_or(|previous| previous.id != repository.id);
        let different_path = self
            .repository
            .as_ref()
            .is_none_or(|previous| previous.path != repository.path);
        self.solo_refs = repository.solo_refs.iter().cloned().collect();
        self.references = repository
            .references
            .iter()
            .cloned()
            .map(Arc::new)
            .collect();
        self.reference_shas = self
            .references
            .iter()
            .map(|reference| reference.sha.clone())
            .collect();
        self.repository = Some(repository);
        self.context.invalidate(changed && !shared_worktrees);
        if different_path {
            if !shared_worktrees {
                self.set_reference_state(ReferenceTreeState::default());
            }
            self.restored_scroll_top = None;
            self.selected_sha = None;
            self.selected_stash.clear();
        }
        self.rebuild_reference_groups();
        self.load_expanded_context(cx);
        self.rebuild_rows(cx);
    }

    /// Restore the empty/welcome sidebar when a tab has no repository.
    pub fn clear_repository(&mut self, cx: &mut Context<Self>) {
        self.repository = None;
        self.references.clear();
        self.reference_shas.clear();
        self.reference_groups.clear();
        self.context.invalidate(true);
        self.selected_sha = None;
        self.selected_stash.clear();
        self.solo_refs.clear();
        self.set_reference_state(ReferenceTreeState::default());
        self.rebuild_rows(cx);
    }

    pub fn set_selection(
        &mut self,
        selected_sha: Option<String>,
        selected_stash: String,
        _working_tree: bool,
        cx: &mut Context<Self>,
    ) {
        if self.selected_sha == selected_sha && self.selected_stash == selected_stash {
            return;
        }
        let reference_highlight_changed = self
            .selected_sha
            .as_ref()
            .is_some_and(|sha| self.reference_shas.contains(sha))
            || selected_sha
                .as_ref()
                .is_some_and(|sha| self.reference_shas.contains(sha));
        let stash_highlight_changed = self.selected_stash != selected_stash;
        self.selected_sha = selected_sha;
        self.selected_stash = selected_stash;
        if reference_highlight_changed || stash_highlight_changed {
            cx.notify();
        }
    }

    fn rebuild_reference_groups(&mut self) {
        let filter = self.reference_state.filter.trim().to_lowercase();
        self.reference_groups = ReferenceKind::ALL
            .into_iter()
            .filter_map(|kind| {
                let references: Vec<_> = self
                    .references
                    .iter()
                    .filter(|reference| {
                        reference.kind == kind.name()
                            && (filter.is_empty()
                                || reference.label.to_lowercase().contains(&filter))
                    })
                    .cloned()
                    .collect();
                (!references.is_empty()).then(|| ReferenceGroup {
                    kind,
                    count: references.len(),
                    tree: reference_tree(references.into_iter(), &filter),
                })
            })
            .collect();
    }

    fn invalidate_row_measurements(&mut self) {
        let scroll = self.list_state.logical_scroll_top();
        self.list_state.reset(self.rows.len());
        self.list_state.scroll_to(scroll);
        self.scroll_handle.set_heights(
            self.rows
                .iter()
                .map(|row| row.height_hint(self.font_size_offset)),
        );
    }

    pub(crate) fn rebuild_rows(&mut self, cx: &mut Context<Self>) {
        self.close_menu(cx);
        let previous_rows = std::mem::take(&mut self.rows);
        if self.repository.is_none() {
            self.active_row = None;
            self.list_state.reset(0);
            self.scroll_handle.set_heights([]);
            cx.notify();
            return;
        }
        for group in &self.reference_groups {
            let kind = group.kind;
            let closed = self.reference_state.group_closed(kind);
            self.rows.push(SidebarRow::Group {
                kind,
                count: group.count,
                closed,
            });
            if !closed {
                flatten_tree(&group.tree, kind, 0, &self.reference_state, &mut self.rows);
            }
            self.rows.push(SidebarRow::Spacer(19.));
        }
        if self.reference_groups.is_empty() {
            self.rows.push(SidebarRow::NoReferences);
        }
        self.rows.push(SidebarRow::ContextSeparator {
            margin_top: if self.reference_groups.is_empty() {
                12.
            } else {
                0.
            },
        });
        let worktree_rows = self.context.worktrees.data.as_ref().map(|list| {
            filtered_worktree_indices(list, &self.reference_state.filter)
                .into_iter()
                .map(|index| SidebarRow::Worktree {
                    index,
                    path: list.worktrees[index].path.clone(),
                })
                .collect::<Vec<_>>()
        });
        self.rows.push(SidebarRow::Worktrees {
            count: worktree_rows.as_ref().map(Vec::len),
            expanded: self.context.worktrees_expanded,
        });
        if self.context.worktrees_expanded {
            if let Some(error) = &self.context.worktrees.error {
                self.rows.push(SidebarRow::Notice {
                    text: error.clone(),
                    error: true,
                });
            } else if self.context.worktrees.loading && self.context.worktrees.data.is_none() {
                self.rows.push(SidebarRow::Notice {
                    text: "Lecture des worktrees…".into(),
                    error: false,
                });
            }
            if let Some(rows) = worktree_rows {
                if rows.is_empty() && self.reference_state.searching() {
                    self.rows.push(SidebarRow::Notice {
                        text: "Aucun worktree trouvé.".into(),
                        error: false,
                    });
                }
                self.rows.extend(rows);
                if self
                    .context
                    .worktrees
                    .data
                    .as_ref()
                    .is_some_and(|list| list.truncated)
                {
                    self.rows.push(SidebarRow::Notice {
                        text: "Les 256 premiers worktrees sont pris en compte.".into(),
                        error: false,
                    });
                }
            }
        }
        self.rows.push(SidebarRow::Stashes {
            count: self
                .context
                .stashes
                .data
                .as_ref()
                .map(|list| list.stashes.len()),
            expanded: self.context.stashes_expanded,
        });
        if self.context.stashes_expanded {
            if let Some(error) = &self.context.stashes.error {
                self.rows.push(SidebarRow::Notice {
                    text: error.clone(),
                    error: true,
                });
            } else if self.context.stashes.loading && self.context.stashes.data.is_none() {
                self.rows.push(SidebarRow::Notice {
                    text: "Lecture des stashes…".into(),
                    error: false,
                });
            } else if self
                .context
                .stashes
                .data
                .as_ref()
                .is_some_and(|list| list.stashes.is_empty())
            {
                self.rows.push(SidebarRow::Notice {
                    text: "Aucun stash dans ce dépôt.".into(),
                    error: false,
                });
            }
            if let Some(list) = &self.context.stashes.data {
                self.rows
                    .extend(list.stashes.iter().enumerate().map(|(index, stash)| {
                        SidebarRow::Stash {
                            index,
                            sha: stash.sha.clone(),
                        }
                    }));
                if list.truncated {
                    self.rows.push(SidebarRow::Notice {
                        text: "Les 128 stashes les plus récents sont affichés.".into(),
                        error: false,
                    });
                }
            }
        }
        self.rows.push(SidebarRow::Spacer(24.));
        // Keep unchanged list measurements and scroll anchors across expansion
        // and asynchronous reads, just as the source's scrolling <nav> does.
        let prefix = previous_rows
            .iter()
            .zip(&self.rows)
            .take_while(|(left, right)| same_row_identity(left, right))
            .count();
        let suffix = previous_rows[prefix..]
            .iter()
            .rev()
            .zip(self.rows[prefix..].iter().rev())
            .take_while(|(left, right)| same_row_identity(left, right))
            .count();
        let removed_end = previous_rows.len() - suffix;
        let inserted_count = self.rows.len() - prefix - suffix;
        if removed_end != prefix || inserted_count != 0 {
            self.scroll_handle.splice(
                prefix..removed_end,
                self.rows[prefix..self.rows.len() - suffix]
                    .iter()
                    .map(|row| row.height_hint(self.font_size_offset)),
            );
        }
        self.active_row = self
            .active_row
            .and_then(|index| follow_row(&previous_rows, &self.rows, index, prefix, suffix));
        if let Some(top) = self.restored_scroll_top {
            self.scroll_handle.scroll_vertical_to(top);
            if !self.context.expanded_lists_loading() {
                self.restored_scroll_top = None;
            }
        }
        cx.notify();
    }

    fn close_menu(&mut self, cx: &mut Context<Self>) {
        self.menu_subscription = None;
        let Some((menu, _)) = self.menu.take() else {
            return;
        };
        // Most callers have no window or run inside its update, and menu items
        // close the menu from its own update, so it is read once that update
        // ends. Before the next frame drops a focused menu, give the keyboard
        // back to the tree.
        let tree_focus = self.tree_focus.clone();
        let window = self.window;
        cx.defer(move |cx| {
            let _ = window.update(cx, |_, window, cx| {
                if menu.focus_handle(cx).contains_focused(window, cx) {
                    tree_focus.focus(window, cx);
                }
            });
        });
        cx.notify();
    }

    fn open_branch_menu(
        &mut self,
        name: String,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(reference) = self
            .references
            .iter()
            .find(|reference| reference.name == name)
        else {
            return;
        };
        if reference.kind != "local" && reference.kind != "remote" {
            return;
        }
        let label = if self.solo_refs.contains(&name) {
            "Retirer du Solo"
        } else if self.solo_refs.is_empty() {
            "Solo"
        } else {
            "Ajouter au Solo"
        };
        let tree_focus = self.tree_focus.clone();
        let view = cx.weak_entity();
        let color = self.palette.text;
        let font_size = 11. + self.font_size_offset;
        let menu = PopupMenu::build(window, cx, move |menu, _, _| {
            menu.min_w(px(210.)).action_context(tree_focus).item(
                PopupMenuItem::element(move |_, _| {
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .px(px(2.))
                        .py(px(8.))
                        .text_size(px(font_size))
                        .line_height(relative(1.5))
                        .font_weight(FontWeight::NORMAL)
                        .child(icon(IconName::Focus, 14., color))
                        .child(label)
                })
                .on_click(move |_, _, cx| {
                    let _ = view.update(cx, |view, cx| {
                        // A refreshed reference cannot retain a stale popup action.
                        if view
                            .references
                            .iter()
                            .any(|reference| reference.name == name)
                        {
                            cx.emit(SidebarEvent::ToggleSolo(name.clone()));
                        }
                        view.close_menu(cx);
                    });
                }),
            )
        });
        window.focus(&menu.focus_handle(cx), cx);
        self.menu_subscription =
            Some(cx.subscribe(&menu, |view, _, _: &DismissEvent, cx| view.close_menu(cx)));
        self.menu = Some((menu, position));
        cx.notify();
    }

    /// Tree semantics and the keyboard focus ring shared by navigable rows.
    fn tree_item(&self, row: Stateful<Div>, index: usize, window: &Window) -> Stateful<Div> {
        let active = self.active_row == Some(index);
        let level = self.rows.get(index).and_then(SidebarRow::level);
        let accent = self.palette.accent;
        row.role(Role::TreeItem)
            .when_some(level, |row, level| row.aria_level(level + 1))
            .when(active, |row| row.aria_active_descendant())
            .when(active && self.tree_focus_visible(window), |row| {
                row.relative().child(
                    div()
                        .absolute()
                        .inset_0()
                        .border_1()
                        .border_color(rgb(accent)),
                )
            })
    }

    /// Two-line worktree and stash entries.
    fn context_item(&self, id: String) -> Stateful<Div> {
        div()
            .id(SharedString::from(id))
            .flex()
            .w_full()
            .min_w_0()
            .items_center()
            .gap(px(8.))
            .h(px(context_item_height(self.font_size_offset)))
            .pl(px(26.))
            .pr(px(15.))
            .text_size(px(11. + self.font_size_offset))
    }

    /// Section titles are unique and double as element ids.
    fn render_group(
        &self,
        index: usize,
        title: &'static str,
        count: Option<(usize, String)>,
        expanded: bool,
        icon_name: Option<IconName>,
        window: &Window,
    ) -> Stateful<Div> {
        let p = self.palette;
        let font_offset = self.font_size_offset;
        let row = div()
            .id(title)
            .aria_label(title)
            .when_some(count.as_ref(), |this, (_, label)| {
                this.aria_description(label.clone())
            })
            .aria_expanded(expanded)
            .flex()
            .items_center()
            .gap(px(7.))
            .w_full()
            .h(px(section_height(font_offset)))
            .px(px(15.))
            .text_size(px(11. + font_offset))
            .text_color(rgb(p.muted))
            .font_weight(FontWeight::MEDIUM)
            .cursor_pointer()
            .child(icon(
                if expanded {
                    IconName::ChevronDown
                } else {
                    IconName::ChevronRight
                },
                12.,
                p.muted,
            ))
            .when_some(icon_name, |this, name| this.child(icon(name, 13., p.muted)))
            .child(div().flex_1().child(title))
            .when_some(count, |this, (count, _)| {
                this.child(div().font_family(MONO).child(format_count(count)))
            });
        self.tree_item(row, index, window)
    }

    fn highlighted_label(&self, label: String, ranges: Vec<Range<usize>>) -> StyledText {
        let style = HighlightStyle {
            color: Some(rgb(self.palette.on_accent).into()),
            background_color: Some(rgb(self.palette.accent).into()),
            ..Default::default()
        };
        StyledText::new(label).with_highlights(ranges.into_iter().map(|range| (range, style)))
    }

    fn render_row(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let Some(row) = self.rows.get(index).cloned() else {
            return div().into_any_element();
        };
        let p = self.palette;
        let font_offset = self.font_size_offset;
        match row {
            SidebarRow::Group {
                kind,
                count,
                closed,
            } => self
                .render_group(
                    index,
                    kind.title(),
                    Some((count, kind.count_label(count))),
                    !closed,
                    None,
                    window,
                )
                .on_click(cx.listener(move |view, _, _, cx| {
                    view.active_row = Some(index);
                    view.reference_state.toggle_group(kind);
                    view.rebuild_rows(cx);
                }))
                .into_any_element(),
            SidebarRow::Folder {
                kind,
                path,
                label,
                highlights,
                count,
                depth,
                closed,
            } => {
                let tooltip_path = path.clone();
                let padding = (12. + depth as f32 * 14.).min((self.width * 0.35 - 14.).max(0.));
                let label = self.highlighted_label(label, highlights);
                let row = div()
                    .id(SharedString::from(format!(
                        "reference-folder-{}-{path}",
                        kind.name()
                    )))
                    .aria_label(format!("Dossier {path}"))
                    .aria_description(kind.count_label(count))
                    .aria_expanded(!closed)
                    .tooltip(move |window, cx| Tooltip::new(tooltip_path.clone()).build(window, cx))
                    .flex()
                    .items_center()
                    .gap(px(7.))
                    .w_full()
                    .min_w_0()
                    .h(px(ROW_HEIGHT))
                    .pl(px(padding))
                    .pr(px(17.))
                    .text_size(px(11. + font_offset))
                    .text_color(rgb(p.text))
                    .cursor_pointer()
                    .hover(|style| style.bg(rgb(p.accent).opacity(0.1)))
                    .child(
                        Icon::new(if closed {
                            IconName::ChevronRight
                        } else {
                            IconName::ChevronDown
                        })
                        .with_size(px(12.))
                        .opacity(0.8),
                    )
                    .child(
                        Icon::new(if closed {
                            IconName::Folder
                        } else {
                            IconName::FolderOpen
                        })
                        .with_size(px(13.))
                        .opacity(0.8),
                    )
                    .child(div().min_w_0().flex_1().truncate().child(label))
                    .child(
                        div()
                            .flex_shrink_0()
                            .font_family(MONO)
                            .text_color(rgb(p.muted))
                            .child(format_count(count)),
                    )
                    .on_click(cx.listener(move |view, _, _, cx| {
                        view.active_row = Some(index);
                        view.reference_state.toggle_folder(kind, path.clone());
                        view.rebuild_rows(cx);
                    }));
                self.tree_item(row, index, window).into_any_element()
            }
            SidebarRow::Reference {
                kind,
                label,
                highlights,
                reference,
                depth,
            } => {
                let current = self.repository.as_ref().is_some_and(|repository| {
                    kind == ReferenceKind::Local
                        && reference.name == format!("refs/heads/{}", repository.head_name)
                });
                let highlighted = self.selected_sha.as_ref() == Some(&reference.sha);
                let soloed = self.solo_refs.contains(&reference.name);
                let color = if current {
                    p.accent
                } else if soloed {
                    p.warning
                } else {
                    p.muted
                };
                let name = reference.name.clone();
                let click_name = name.clone();
                let menu_name = name.clone();
                let tooltip_label = reference.label.clone();
                let show_solo =
                    self.active_row == Some(index) && self.tree_focus.is_focused(window);
                let button_width = self.width - 1. - if kind.can_solo() { 24. } else { 0. };
                let padding = (26. + depth as f32 * 14.).min(button_width * 0.35);
                let label = self.highlighted_label(label, highlights);
                let row = div()
                    .id(SharedString::from(format!("reference-{name}")))
                    .tooltip(move |window, cx| {
                        Tooltip::new(tooltip_label.clone()).build(window, cx)
                    })
                    .relative()
                    .flex()
                    .items_center()
                    .gap(px(9.))
                    .h(px(ROW_HEIGHT))
                    .min_w_0()
                    .flex_1()
                    .pl(px(padding))
                    .pr(px(17.))
                    .text_size(px(11. + font_offset))
                    .text_color(rgb(color))
                    .cursor_pointer()
                    .when(highlighted, |this| this.bg(rgb(p.selection)))
                    .when(soloed && !highlighted, |this| {
                        this.bg(rgb(p.warning).opacity(0.12))
                    })
                    .hover(|style| {
                        style
                            .bg(if highlighted {
                                rgb(p.selection)
                            } else {
                                rgb(p.accent).opacity(0.1)
                            })
                            .text_color(rgb(if current { p.accent } else { p.text }))
                    })
                    .child(Icon::new(kind.icon()).with_size(px(13.)).opacity(0.8))
                    .child(div().min_w_0().flex_1().truncate().child(label))
                    .when(current, |this| {
                        this.child(Icon::new(IconName::Check).with_size(px(12.)).opacity(0.8))
                    })
                    .on_click(cx.listener(move |view, _, _, cx| {
                        view.active_row = Some(index);
                        cx.emit(SidebarEvent::NavigateReference(click_name.clone()));
                        cx.notify();
                    }))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |view, event: &MouseDownEvent, window, cx| {
                            if kind.can_solo() {
                                view.active_row = Some(index);
                                view.open_branch_menu(
                                    menu_name.clone(),
                                    event.position,
                                    window,
                                    cx,
                                );
                                cx.stop_propagation();
                            }
                        }),
                    );
                let leaf = div()
                    .id(SharedString::from(format!("reference-leaf-{name}")))
                    .aria_label(reference.label.clone())
                    .aria_selected(highlighted)
                    .when(current, |this| this.aria_description("Branche courante"))
                    .group("reference-leaf")
                    .flex()
                    .items_center()
                    .w_full()
                    .min_w_0()
                    .child(row)
                    .when(kind.can_solo(), |this| {
                        // Shift+F10 or the Menu key reach Solo from the keyboard.
                        this.child(
                            div()
                                .id(SharedString::from(format!("solo-{name}")))
                                .role(Role::Button)
                                .aria_label(format!(
                                    "{} : {}",
                                    if soloed { "Retirer du Solo" } else { "Solo" },
                                    reference.label
                                ))
                                .tooltip(move |window, cx| {
                                    Tooltip::new(if soloed {
                                        "Retirer du Solo"
                                    } else {
                                        "Solo / ajouter au Solo"
                                    })
                                    .build(window, cx)
                                })
                                .aria_toggled(if soloed {
                                    Toggled::True
                                } else {
                                    Toggled::False
                                })
                                .flex()
                                .items_center()
                                .justify_center()
                                .w(px(24.))
                                .h(px(26.))
                                .rounded(RADIUS_SM)
                                .cursor_pointer()
                                .text_color(rgb(p.muted))
                                .opacity(0.)
                                .when(show_solo, |this| this.opacity(1.))
                                .group_hover("reference-leaf", |style| style.opacity(1.))
                                .when(soloed, |this| {
                                    this.text_color(rgb(p.warning))
                                        .bg(rgb(p.warning).opacity(0.12))
                                })
                                .hover(|style| {
                                    style
                                        .bg(rgb(p.warning).opacity(0.12))
                                        .text_color(rgb(p.warning))
                                })
                                .child(Icon::new(IconName::Focus).with_size(px(13.)))
                                .on_click(cx.listener({
                                    let name = name.clone();
                                    move |view, _, _, cx| {
                                        view.active_row = Some(index);
                                        cx.emit(SidebarEvent::ToggleSolo(name.clone()))
                                    }
                                })),
                        )
                    });
                self.tree_item(leaf, index, window).into_any_element()
            }
            SidebarRow::Spacer(height) => div().h(px(height)).into_any_element(),
            SidebarRow::NoReferences => div()
                .p(px(20.))
                .text_center()
                .text_color(rgb(p.muted))
                .text_size(px(11. + font_offset))
                .child("Aucune référence trouvée.")
                .into_any_element(),
            SidebarRow::ContextSeparator { margin_top } => div()
                .mt(px(margin_top))
                .pt(px(7.))
                .border_t_1()
                .border_color(rgb(p.border))
                .into_any_element(),
            SidebarRow::Worktrees { count, expanded } => self
                .render_group(
                    index,
                    "Worktrees",
                    count.map(|count| (count, count_label(count, "worktree", "worktrees"))),
                    expanded,
                    Some(IconName::FolderGit2),
                    window,
                )
                .on_click(cx.listener(move |view, _, _, cx| {
                    view.active_row = Some(index);
                    view.toggle_worktrees(cx);
                }))
                .into_any_element(),
            SidebarRow::Stashes { count, expanded } => self
                .render_group(
                    index,
                    "Stashes",
                    count.map(|count| (count, count_label(count, "stash", "stashes"))),
                    expanded,
                    None,
                    window,
                )
                .on_click(cx.listener(move |view, _, _, cx| {
                    view.active_row = Some(index);
                    view.toggle_stashes(cx);
                }))
                .into_any_element(),
            SidebarRow::Worktree {
                index: item_index, ..
            } => {
                let Some(item) = self
                    .context
                    .worktrees
                    .data
                    .as_ref()
                    .and_then(|list| list.worktrees.get(item_index))
                else {
                    return div().into_any_element();
                };
                let label = worktree_label(item);
                let highlights = search_highlight_ranges(&label, &self.reference_state.filter);
                let path = item.path.clone();
                let current = self
                    .repository
                    .as_ref()
                    .is_some_and(|repository| repository.path == item.path);
                let available = item.available;
                let description = format!(
                    "{}\n{}{}{}{}",
                    item.path,
                    if item.primary {
                        "Worktree principal"
                    } else {
                        "Worktree lié"
                    },
                    if current { "\nOuvert ici" } else { "" },
                    item.locked
                        .as_ref()
                        .map(|reason| format!("\nVerrouillé : {reason}"))
                        .unwrap_or_default(),
                    item.reason
                        .as_ref()
                        .map(|reason| format!("\n{reason}"))
                        .unwrap_or_default()
                );
                let tooltip_text = format!("{label}\n{description}");
                // The current worktree shares the current branch's accent.
                let color = if current { p.accent } else { p.text };
                let row = self
                    .context_item(format!("worktree-{path}"))
                    .aria_label(label.clone())
                    .aria_description(description)
                    .tooltip(move |window, cx| Tooltip::new(tooltip_text.clone()).build(window, cx))
                    .text_color(rgb(color))
                    .opacity(if available { 1. } else { 0.7 })
                    .when(available, |this| {
                        this.cursor_pointer()
                            .hover(|style| style.bg(rgb(p.accent).opacity(0.1)))
                    })
                    .child(icon(
                        if item.primary {
                            IconName::House
                        } else {
                            IconName::FolderGit2
                        },
                        13.,
                        color,
                    ))
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .truncate()
                                    .font_weight(if current {
                                        FontWeight::SEMIBOLD
                                    } else {
                                        FontWeight::MEDIUM
                                    })
                                    .child(self.highlighted_label(label, highlights)),
                            )
                            .child(
                                // The end of the path names the worktree.
                                div()
                                    .overflow_hidden()
                                    .whitespace_nowrap()
                                    .text_ellipsis_start()
                                    .text_color(rgb(p.muted))
                                    .child(display_path(&item.path)),
                            ),
                    )
                    .when(!item.available, |this| {
                        this.child(icon(IconName::TriangleAlert, 12., p.warning))
                    })
                    .when(item.locked.is_some(), |this| {
                        this.child(icon(IconName::LockKeyhole, 12., p.warning))
                    })
                    .when(current, |this| {
                        this.child(icon(IconName::Check, 12., color))
                    })
                    .on_click(cx.listener(move |view, _, _, cx| {
                        view.active_row = Some(index);
                        if available {
                            cx.emit(SidebarEvent::OpenWorktree(path.clone()));
                        }
                        cx.notify();
                    }));
                self.tree_item(row, index, window).into_any_element()
            }
            SidebarRow::Stash {
                index: item_index, ..
            } => {
                let Some(item) = self
                    .context
                    .stashes
                    .data
                    .as_ref()
                    .and_then(|list| list.stashes.get(item_index))
                else {
                    return div().into_any_element();
                };
                let sha = item.sha.clone();
                let selected = sha == self.selected_stash;
                let name = stash_name(item.index);
                let (branch, message) = stash_summary(&item.message);
                let details = stash_details(branch, &compact_date(item.time), &item.author);
                let description = format!(
                    "{}\nBase {}",
                    stash_details(branch, &commit_date(item.time), &item.author),
                    short_sha(&item.base_sha)
                );
                let tooltip_text = format!("{name} · {message}\n{description}");
                let message = message.to_owned();
                let row = self
                    .context_item(format!("stash-{sha}"))
                    .aria_label(format!("{name} : {message}"))
                    .aria_description(description)
                    .aria_selected(selected)
                    .tooltip(move |window, cx| Tooltip::new(tooltip_text.clone()).build(window, cx))
                    .text_color(rgb(p.text))
                    .cursor_pointer()
                    .when(selected, |this| this.bg(rgb(p.selection)))
                    .hover(|style| {
                        style.bg(if selected {
                            rgb(p.selection)
                        } else {
                            rgb(p.accent).opacity(0.1)
                        })
                    })
                    .child(icon(IconName::Archive, 13., p.text))
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .flex()
                                    .min_w_0()
                                    .gap(px(6.))
                                    .child(
                                        div()
                                            .flex_shrink_0()
                                            .font_weight(FontWeight::MEDIUM)
                                            .child(name),
                                    )
                                    .child(div().min_w_0().flex_1().truncate().child(message)),
                            )
                            .child(div().truncate().text_color(rgb(p.muted)).child(details)),
                    )
                    .on_click(cx.listener(move |view, _, _, cx| {
                        view.active_row = Some(index);
                        cx.emit(SidebarEvent::SelectStash(sha.clone()));
                        cx.notify();
                    }));
                self.tree_item(row, index, window).into_any_element()
            }
            SidebarRow::Notice { text, error } => div()
                .px(px(12.))
                .py(px(8.))
                .text_size(px(11. + font_offset))
                .text_color(rgb(if error { p.danger } else { p.muted }))
                .child(text)
                .into_any_element(),
        }
    }
}

impl Render for SidebarView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.sync_filter_input {
            let filter = self.reference_state.filter.clone();
            self.filter_input
                .update(cx, |input, cx| input.set_value(filter, window, cx));
            self.sync_filter_input = false;
        }
        let p = self.palette;
        let font_offset = self.font_size_offset;
        let entity = cx.entity();
        let filter_input = Styled::h(
            Input::new(&self.filter_input)
                .id("reference-filter")
                .aria_label("Filtrer les références et worktrees")
                .appearance(false)
                .bordered(false)
                .focus_bordered(false)
                .p_0()
                .flex_1()
                .min_w_0()
                .text_size(px(11. + font_offset))
                .line_height(relative(1.5)),
            px((11. + font_offset) * 1.5),
        );
        let menu = self.menu.clone();
        let wheel_view = cx.weak_entity();
        let wheel_capture = self.scroll_handle.wheel_capture(move |changed, _, cx| {
            let _ = wheel_view.update(cx, |view, cx| {
                view.restored_scroll_top = None;
                view.close_menu(cx);
                if changed {
                    view.scroll_epoch = view.scroll_epoch.wrapping_add(1);
                    cx.notify();
                }
            });
        });
        div().id("sidebar").relative().size_full().min_w_0().flex().flex_col()
            .capture_any_mouse_down(cx.listener(|view, _, _, _| view.restored_scroll_top = None))
            .capture_key_down(cx.listener(|view, _, _, _| view.restored_scroll_top = None))
            .font_family(SANS).line_height(relative(1.5)).bg(rgb(p.panel)).text_color(rgb(p.text))
            .border_r_1().border_color(rgb(p.border))
            .child(div().id("sidebar-heading").aria_label("ESPACE DE TRAVAIL")
                .pt(px(16.)).pb(px(12.)).px(px(if self.width < 210. { 14. } else { 22. }))
                .text_size(px(11. + font_offset)).text_color(rgb(p.muted))
                // GPUI has no letter-spacing property. Individual glyphs
                // reproduce the source's 0.1em uppercase heading tracking.
                .flex().gap(px((11. + font_offset) * 0.1))
                .children("ESPACE DE TRAVAIL".chars().map(|character|div().child(character.to_string()))))
            .when(self.repository.is_some(), |this| this
                .child(div().mx(px(15.)).mb(px(18.)).px(px(8.)).py(px(7.))
                    .flex().items_center().gap(px(7.)).border_1().border_color(rgb(p.border)).rounded(RADIUS_MD)
                    .capture_action(cx.listener(|view, _: &MoveDown, window, cx| {
                        view.focus_tree(window, cx);
                        cx.stop_propagation();
                    }))
                    .child(icon(IconName::Search,13.,p.muted)).child(filter_input)
                    .when(!self.reference_state.filter.is_empty(), |this| this.child(
                        NativeButton::new("clear-reference-filter")
                            .accessibility_label("Effacer la recherche")
                            .size(px(16.))
                            .flex_shrink_0()
                            .rounded(RADIUS_SM)
                            .cursor_pointer()
                            .hover(|style| style.bg(rgb(p.accent).opacity(0.1)))
                            .focus_visible(|style| style.border_1().border_color(rgb(p.accent)))
                            .tooltip(|window, cx| Tooltip::new("Effacer la recherche").build(window, cx))
                            .child(icon(IconName::X, 12., p.muted))
                            .on_click(cx.listener(|view, _, window, cx| {
                                view.filter_input.update(cx, |input, cx| {
                                    input.set_value("", window, cx);
                                    input.focus(window, cx);
                                });
                                view.reference_state.set_filter(String::new());
                                view.rebuild_reference_groups();
                                view.rebuild_rows(cx);
                            })))))
                .child(div().id("sidebar-tree").role(Role::Tree).aria_label("Références, worktrees et stashes")
                    .track_focus(&self.tree_focus).tab_index(0)
                    .on_key_down(cx.listener(Self::tree_key_down))
                    .on_action(cx.listener(|view, _: &Escape, window, cx| view.focus_references(window, cx)))
                    .relative().flex_1().min_h_0().w_full().flex().flex_col()
                    .child(wheel_capture)
                    .child(div().id(("sidebar-rows", self.scroll_epoch)).flex_1().min_h_0().w_full().flex().flex_col()
                        .child(list(self.list_state.clone(), move |index, window, cx| {
                            entity.update(cx, |view,cx| view.render_row(index,window,cx))
                        }).flex_1().min_h_0().w_full()))
                    .child(scrollbar_theme(Scrollbar::vertical(&self.scroll_handle)
                        .id("sidebar-scrollbar")))))
            .when(self.repository.is_none(), |this| this.child(div().flex_1().min_h_0()
                .child(div().mt(px(20.)).flex().flex_col().items_center().text_color(rgb(p.muted))
                    .child(div().self_stretch().h(px(1.)).mx(px(20.)).mb(px(52.)).bg(rgb(p.border)))
                    .child(icon(IconName::GitBranch,22.,p.muted))
                    .child(div().text_size(px(11. + font_offset)).line_height(relative(1.8)).text_center().child("Les branches et les tags\nde votre dépôt apparaîtront ici.")))))
            .when_some(menu, |this, (menu,position)| {
                // PopupMenu handles escape, outside click, keyboard traversal,
                // and returning focus to the branch that opened the menu.
                this.child(deferred(anchored().child(div().w(window.bounds().size.width).h(window.bounds().size.height)
                    .on_scroll_wheel(cx.listener(|view, _, _, cx| view.close_menu(cx)))
                    .child(anchored().position(position).snap_to_window_with_margin(px(8.)).anchor(Anchor::TopLeft).child(menu)))
                    ).with_priority(100))
            })
    }
}

fn flatten_tree(
    tree: &[ReferenceNode],
    kind: ReferenceKind,
    depth: usize,
    state: &ReferenceTreeState,
    rows: &mut Vec<SidebarRow>,
) {
    for node in tree {
        match node {
            ReferenceNode::Folder {
                path,
                label,
                highlights,
                count,
                children,
            } => {
                let closed = state.folder_closed(kind, path);
                rows.push(SidebarRow::Folder {
                    kind,
                    path: path.clone(),
                    label: label.clone(),
                    highlights: highlights.clone(),
                    count: *count,
                    depth,
                    closed,
                });
                if !closed {
                    flatten_tree(children, kind, depth + 1, state, rows);
                }
            }
            ReferenceNode::Reference {
                label,
                highlights,
                reference,
            } => rows.push(SidebarRow::Reference {
                kind,
                label: label.clone(),
                highlights: highlights.clone(),
                reference: reference.clone(),
                depth,
            }),
        }
    }
}

fn same_row_identity(left: &SidebarRow, right: &SidebarRow) -> bool {
    match (left, right) {
        (SidebarRow::Group { kind: a, .. }, SidebarRow::Group { kind: b, .. }) => a == b,
        (
            SidebarRow::Folder {
                kind: a, path: ap, ..
            },
            SidebarRow::Folder {
                kind: b, path: bp, ..
            },
        ) => a == b && ap == bp,
        (
            SidebarRow::Reference { reference: a, .. },
            SidebarRow::Reference { reference: b, .. },
        ) => a.name == b.name,
        (SidebarRow::Spacer(a), SidebarRow::Spacer(b)) => a == b,
        (
            SidebarRow::ContextSeparator { margin_top: a },
            SidebarRow::ContextSeparator { margin_top: b },
        ) => a == b,
        (SidebarRow::NoReferences, SidebarRow::NoReferences)
        | (SidebarRow::Worktrees { .. }, SidebarRow::Worktrees { .. })
        | (SidebarRow::Stashes { .. }, SidebarRow::Stashes { .. }) => true,
        (SidebarRow::Worktree { path: a, .. }, SidebarRow::Worktree { path: b, .. })
        | (SidebarRow::Stash { sha: a, .. }, SidebarRow::Stash { sha: b, .. }) => a == b,
        (SidebarRow::Notice { text: a, .. }, SidebarRow::Notice { text: b, .. }) => a == b,
        _ => false,
    }
}

/// Follows a row across a rebuild that changed only the rows between the
/// shared `prefix` and `suffix`. A vanished row hands over to the nearest
/// navigable row above it, usually its parent.
fn follow_row(
    previous: &[SidebarRow],
    rows: &[SidebarRow],
    index: usize,
    prefix: usize,
    suffix: usize,
) -> Option<usize> {
    let row = previous.get(index)?;
    if index < prefix {
        return Some(index);
    }
    if index >= previous.len() - suffix {
        return Some(rows.len() - (previous.len() - index));
    }
    rows[prefix..rows.len() - suffix]
        .iter()
        .position(|candidate| same_row_identity(row, candidate))
        .map(|offset| prefix + offset)
        .or_else(|| {
            (0..index.min(rows.len()))
                .rev()
                .find(|&candidate| rows[candidate].navigable())
        })
        .or_else(|| rows.iter().position(SidebarRow::navigable))
}

#[cfg(test)]
mod tests {
    use super::{
        ROW_HEIGHT, ReferenceKind, ReferenceNode, ReferenceTreeState, SidebarEvent, SidebarRow,
        SidebarView, SidebarViewState, TreeKey, TreeMove, filtered_worktree_indices, flatten_tree,
        follow_row, reference_order, reference_tree, search_highlight_ranges, stash_details,
        stash_name, stash_summary, tree_move,
    };
    use crate::{
        app::Escape,
        virtual_scroll::{RowGeometry, VirtualScrollHandle},
    };
    use gpui_kit::{
        AnyWindowHandle, AppContext, Bounds, Entity, Focusable, InputEvent as _, KeyBinding,
        ListAlignment, ListState, MouseMoveEvent, ScrollDelta, ScrollWheelEvent, TestAppContext,
        WindowBounds, WindowOptions, component::scroll::ScrollbarHandle, point, px, size,
        test::TestWindowExt,
    };
    use mangrove_core::{
        Workspace,
        model::{IndexProgress, Reference, RepositoryInfo, WorktreeInfo, WorktreeList},
    };
    use std::{cell::RefCell, cmp::Ordering, path::Path, rc::Rc, sync::Arc};
    use tempfile::TempDir;

    fn reference(label: &str) -> Arc<Reference> {
        Arc::new(Reference {
            name: format!("refs/heads/{label}"),
            label: label.into(),
            kind: "local".into(),
            sha: "a".repeat(40),
        })
    }

    fn reference_row(label: &str, depth: usize) -> SidebarRow {
        SidebarRow::Reference {
            kind: ReferenceKind::Local,
            label: label.rsplit('/').next().unwrap().into(),
            highlights: Vec::new(),
            reference: reference(label),
            depth,
        }
    }

    fn folder_row(path: &str, depth: usize, closed: bool) -> SidebarRow {
        SidebarRow::Folder {
            kind: ReferenceKind::Local,
            path: path.into(),
            label: path.rsplit('/').next().unwrap().into(),
            highlights: Vec::new(),
            count: 1,
            depth,
            closed,
        }
    }

    fn notice(text: &str) -> SidebarRow {
        SidebarRow::Notice {
            text: text.into(),
            error: false,
        }
    }

    /// Indices: 0 group, 1 folder, 2 feature/a, 3 main, 6 worktrees,
    /// 8 worktree, 9 stashes; the others are not navigable.
    fn navigation_rows() -> Vec<SidebarRow> {
        vec![
            SidebarRow::Group {
                kind: ReferenceKind::Local,
                count: 2,
                closed: false,
            },
            folder_row("feature", 0, false),
            reference_row("feature/a", 1),
            reference_row("main", 0),
            SidebarRow::Spacer(19.),
            SidebarRow::ContextSeparator { margin_top: 0. },
            SidebarRow::Worktrees {
                count: Some(1),
                expanded: true,
            },
            notice("Lecture des worktrees…"),
            SidebarRow::Worktree {
                index: 0,
                path: "/main".into(),
            },
            SidebarRow::Stashes {
                count: Some(0),
                expanded: true,
            },
            notice("Aucun stash dans ce dépôt."),
            SidebarRow::Spacer(24.),
        ]
    }

    #[test]
    fn arrows_visit_only_visible_navigable_rows_in_order() {
        let rows = navigation_rows();
        let step = |active, key| tree_move(&rows, active, key);
        assert_eq!(step(None, TreeKey::Next), Some(TreeMove::Focus(0)));
        assert_eq!(step(None, TreeKey::Previous), Some(TreeMove::Focus(0)));
        assert_eq!(step(None, TreeKey::Last), Some(TreeMove::Focus(9)));
        // A stale index beyond the rows behaves as no active row.
        assert_eq!(step(Some(40), TreeKey::Next), Some(TreeMove::Focus(0)));
        assert_eq!(step(Some(4), TreeKey::Next), Some(TreeMove::Focus(0)));
        for (from, to) in [(0, 1), (1, 2), (2, 3), (3, 6), (6, 8), (8, 9)] {
            assert_eq!(step(Some(from), TreeKey::Next), Some(TreeMove::Focus(to)));
            assert_eq!(
                step(Some(to), TreeKey::Previous),
                Some(TreeMove::Focus(from))
            );
        }
        assert_eq!(step(Some(9), TreeKey::Next), None);
        assert_eq!(step(Some(0), TreeKey::Previous), None);
        assert_eq!(step(Some(8), TreeKey::First), Some(TreeMove::Focus(0)));
        assert_eq!(step(Some(0), TreeKey::Last), Some(TreeMove::Focus(9)));
    }

    #[test]
    fn left_collapses_or_reaches_the_parent_and_right_expands_or_descends() {
        let mut rows = navigation_rows();
        let step = |rows: &[SidebarRow], active, key| tree_move(rows, Some(active), key);
        assert_eq!(step(&rows, 0, TreeKey::Collapse), Some(TreeMove::Toggle(0)));
        assert_eq!(step(&rows, 1, TreeKey::Collapse), Some(TreeMove::Toggle(1)));
        assert_eq!(step(&rows, 2, TreeKey::Collapse), Some(TreeMove::Focus(1)));
        assert_eq!(step(&rows, 3, TreeKey::Collapse), Some(TreeMove::Focus(0)));
        assert_eq!(step(&rows, 8, TreeKey::Collapse), Some(TreeMove::Focus(6)));
        assert_eq!(step(&rows, 0, TreeKey::Expand), Some(TreeMove::Focus(1)));
        assert_eq!(step(&rows, 1, TreeKey::Expand), Some(TreeMove::Focus(2)));
        // Notices are skipped; an open section without entries has no child.
        assert_eq!(step(&rows, 6, TreeKey::Expand), Some(TreeMove::Focus(8)));
        assert_eq!(step(&rows, 9, TreeKey::Expand), None);
        assert_eq!(step(&rows, 2, TreeKey::Expand), None);
        assert_eq!(step(&rows, 8, TreeKey::Expand), None);

        rows[1] = folder_row("feature", 0, true);
        rows.remove(2);
        assert_eq!(step(&rows, 1, TreeKey::Expand), Some(TreeMove::Toggle(1)));
        assert_eq!(step(&rows, 1, TreeKey::Collapse), Some(TreeMove::Focus(0)));
        rows[8] = SidebarRow::Stashes {
            count: None,
            expanded: false,
        };
        assert_eq!(step(&rows, 8, TreeKey::Expand), Some(TreeMove::Toggle(8)));
        // A collapsed top-level section has no parent to reach.
        assert_eq!(step(&rows, 8, TreeKey::Collapse), None);
    }

    #[test]
    fn active_row_follows_its_identity_or_falls_back_to_its_parent() {
        let previous = navigation_rows();
        let mut rows = previous.clone();
        rows[1] = folder_row("feature", 0, true);
        rows.remove(2);
        // Rows 0..2 are shared, as are the last nine.
        assert_eq!(follow_row(&previous, &rows, 1, 2, 9), Some(1));
        assert_eq!(follow_row(&previous, &rows, 8, 2, 9), Some(7));
        assert_eq!(follow_row(&previous, &rows, 2, 2, 9), Some(1));

        let mut reordered = previous.clone();
        reordered.swap(2, 3);
        assert_eq!(follow_row(&previous, &reordered, 2, 2, 8), Some(3));
        assert_eq!(follow_row(&previous, &[], 2, 0, 0), None);
        assert_eq!(follow_row(&previous, &rows, 40, 2, 9), None);
    }

    #[test]
    fn stash_rows_separate_git_prefixes_from_messages() {
        assert_eq!(
            stash_summary("On main: WIP essai de graphe compact"),
            (Some("main"), "WIP essai de graphe compact")
        );
        assert_eq!(
            stash_summary("WIP on feature/ui: 1a2b3c4 Fix layout\n\nBody"),
            (Some("feature/ui"), "1a2b3c4 Fix layout")
        );
        assert_eq!(
            stash_summary("WIP on (no branch): 1a2b3c4 Detached"),
            (Some("(no branch)"), "1a2b3c4 Detached")
        );
        assert_eq!(stash_summary("On main:"), (Some("main"), ""));
        assert_eq!(stash_summary("autostash"), (None, "autostash"));
        assert_eq!(
            stash_summary("Once upon a time"),
            (None, "Once upon a time")
        );
        assert_eq!(stash_summary(""), (None, ""));
        assert_eq!(stash_name(0), "stash@{0}");
        assert_eq!(stash_name(12), "stash@{12}");
        assert_eq!(
            stash_details(Some("main"), "9 oct. 14:32", "Ada"),
            "main · 9 oct. 14:32 · Ada"
        );
        assert_eq!(
            stash_details(None, "9 oct. 14:32", "Ada"),
            "9 oct. 14:32 · Ada"
        );
    }

    #[test]
    fn dense_rows_keep_text_inside_their_height_at_every_zoom() {
        for offset in [0., 1., 5.] {
            let line = (11. + offset) * 1.5;
            for row in navigation_rows() {
                let height = f32::from(row.height_hint(offset));
                match row {
                    SidebarRow::Folder { .. } | SidebarRow::Reference { .. } => {
                        assert_eq!(height, ROW_HEIGHT);
                        assert!(line <= height);
                    }
                    SidebarRow::Worktree { .. } | SidebarRow::Stash { .. } => {
                        assert!(2. * line <= height)
                    }
                    SidebarRow::Group { .. }
                    | SidebarRow::Worktrees { .. }
                    | SidebarRow::Stashes { .. } => assert!(line <= height),
                    _ => {}
                }
            }
        }
    }

    /// A sidebar-sized window, so long trees scroll.
    fn sidebar_window(
        cx: &mut TestAppContext,
        engine: Arc<Workspace>,
    ) -> (AnyWindowHandle, Entity<SidebarView>) {
        cx.update(gpui_kit::init);
        cx.update(|cx| cx.bind_keys([KeyBinding::new("escape", Escape, None)]));
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                point(px(0.), px(0.)),
                size(px(260.), px(600.)),
            ))),
            ..Default::default()
        };
        cx.update(|cx| {
            gpui_kit::open_window(options, cx, |window, cx| {
                cx.new(|cx| SidebarView::new(engine, window, cx))
            })
        })
        .unwrap()
    }

    fn repository_info(path: &str, references: Vec<Reference>) -> RepositoryInfo {
        RepositoryInfo {
            id: path.into(),
            name: path.into(),
            path: path.into(),
            head: None,
            head_name: "main".into(),
            references,
            solo_refs: Vec::new(),
            remotes: Vec::new(),
            index: IndexProgress {
                index_run: String::new(),
                indexed: 0,
                complete: true,
                error: None,
            },
        }
    }

    fn open_sidebar_with(
        cx: &mut TestAppContext,
        references: Vec<Reference>,
    ) -> (TempDir, AnyWindowHandle, Entity<SidebarView>) {
        let cache = tempfile::tempdir().unwrap();
        let engine = Arc::new(Workspace::new(cache.path().join("cache")).unwrap());
        let (window, view) = sidebar_window(cx, engine);
        let info = repository_info("/repository", references);
        view.update(cx, |view, cx| view.set_repository(Arc::new(info), cx));
        (cache, window, view)
    }

    fn open_sidebar(cx: &mut TestAppContext) -> (TempDir, AnyWindowHandle, Entity<SidebarView>) {
        let references = ["feature/a", "feature/b", "main"]
            .into_iter()
            .map(|label| (*reference(label)).clone())
            .chain([Reference {
                name: "refs/tags/v1".into(),
                label: "v1".into(),
                kind: "tag".into(),
                sha: "b".repeat(40),
            }])
            .collect();
        open_sidebar_with(cx, references)
    }

    fn press(cx: &mut TestAppContext, window: AnyWindowHandle, key: &str) {
        cx.update_window(window, |_, window, cx| window.press(key, cx))
            .unwrap();
    }

    fn row_label(row: Option<&SidebarRow>) -> String {
        match row {
            Some(SidebarRow::Group { kind, .. }) => kind.title().into(),
            Some(SidebarRow::Folder { path, .. }) => format!("{path}/"),
            Some(SidebarRow::Reference { reference, .. }) => reference.label.clone(),
            Some(SidebarRow::Worktrees { .. }) => "Worktrees".into(),
            Some(SidebarRow::Stashes { .. }) => "Stashes".into(),
            Some(SidebarRow::Worktree { path, .. }) => path.clone(),
            Some(SidebarRow::Stash { sha, .. }) => sha.clone(),
            other => format!("{:?}", other.map(SidebarRow::level)),
        }
    }

    fn active_label(cx: &mut TestAppContext, view: &Entity<SidebarView>) -> String {
        view.read_with(cx, |view, _| {
            row_label(view.active_row.and_then(|index| view.rows.get(index)))
        })
    }

    fn render(cx: &mut TestAppContext, window: AnyWindowHandle) {
        cx.update_window(window, |_, window, cx| window.render_frame(cx))
            .unwrap();
    }

    #[gpui_kit::test]
    fn keyboard_walks_the_tree_from_the_filter_and_back(cx: &mut TestAppContext) {
        let (_cache, window, view) = open_sidebar(cx);
        let events = Rc::new(RefCell::new(Vec::new()));
        let captured = events.clone();
        let _subscription = cx.update(|cx| {
            cx.subscribe(&view, move |_, event: &SidebarEvent, _| {
                captured.borrow_mut().push(format!("{event:?}"));
            })
        });
        cx.update_window(window, |_, window, cx| {
            view.update(cx, |view, cx| view.focus_references(window, cx));
        })
        .unwrap();
        let focus = |cx: &mut TestAppContext| {
            cx.update_window(window, |_, window, cx| {
                let view = view.read(cx);
                (
                    view.tree_focus.is_focused(window),
                    view.filter_input
                        .read(cx)
                        .focus_handle(cx)
                        .is_focused(window),
                )
            })
            .unwrap()
        };
        assert_eq!(focus(cx), (false, true));

        press(cx, window, "down");
        assert_eq!(focus(cx), (true, false));
        assert_eq!(active_label(cx, &view), "Branches locales");
        press(cx, window, "down");
        assert_eq!(active_label(cx, &view), "feature/");
        press(cx, window, "left");
        assert!(view.read_with(cx, |view, _| {
            view.reference_state
                .folder_closed(ReferenceKind::Local, "feature")
        }));
        assert_eq!(active_label(cx, &view), "feature/");
        press(cx, window, "down");
        assert_eq!(active_label(cx, &view), "main");
        press(cx, window, "up");
        press(cx, window, "right");
        press(cx, window, "right");
        assert_eq!(active_label(cx, &view), "feature/a");
        press(cx, window, "enter");
        press(cx, window, "left");
        assert_eq!(active_label(cx, &view), "feature/");
        press(cx, window, "end");
        assert_eq!(active_label(cx, &view), "Stashes");
        press(cx, window, "up");
        assert_eq!(active_label(cx, &view), "Worktrees");
        press(cx, window, "home");
        assert_eq!(active_label(cx, &view), "Branches locales");
        assert_eq!(
            events.borrow().as_slice(),
            [format!(
                "{:?}",
                SidebarEvent::NavigateReference("refs/heads/feature/a".into())
            )]
        );

        press(cx, window, "escape");
        assert_eq!(focus(cx), (false, true));
        // Leaving and re-entering the tree keeps its place.
        press(cx, window, "down");
        assert_eq!(active_label(cx, &view), "Branches locales");
        press(cx, window, "up");
        assert_eq!(focus(cx), (false, true));
    }

    fn signature() -> git2::Signature<'static> {
        git2::Signature::new(
            "Test",
            "test@example.test",
            &git2::Time::new(1_700_000_000, 0),
        )
        .unwrap()
    }

    /// Stashes `text` as a change of the committed file and returns its SHA.
    fn stash(path: &Path, text: &str) -> String {
        let mut repository = git2::Repository::open(path).unwrap();
        std::fs::write(path.join("file.txt"), text).unwrap();
        repository
            .stash_save(&signature(), text, None)
            .unwrap()
            .to_string()
    }

    /// A one-commit repository with `count` stashes, the newest last.
    fn stash_repository(count: usize) -> (TempDir, Arc<Workspace>, RepositoryInfo, Vec<String>) {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("repository");
        let repository = git2::Repository::init(&path).unwrap();
        std::fs::write(path.join("file.txt"), "base").unwrap();
        let mut index = repository.index().unwrap();
        index.add_path(Path::new("file.txt")).unwrap();
        index.write().unwrap();
        let tree = repository.find_tree(index.write_tree().unwrap()).unwrap();
        repository
            .commit(Some("HEAD"), &signature(), &signature(), "base", &tree, &[])
            .unwrap();
        let shas = (0..count)
            .map(|index| stash(&path, &format!("change {index}")))
            .collect();
        let engine = Arc::new(Workspace::new(root.path().join("cache")).unwrap());
        let info = engine.open_repository(path.to_str().unwrap()).unwrap();
        (root, engine, info, shas)
    }

    #[gpui_kit::test]
    fn active_stash_keeps_its_sha_when_a_new_stash_shifts_the_list(cx: &mut TestAppContext) {
        let (_root, engine, info, shas) = stash_repository(2);
        let (window, view) = sidebar_window(cx, engine.clone());
        view.update(cx, |view, cx| {
            view.set_repository(Arc::new(info.clone()), cx)
        });
        cx.update_window(window, |_, window, cx| {
            view.update(cx, |view, cx| view.show_context(false, window, cx));
        })
        .unwrap();
        cx.run_until_parked();
        press(cx, window, "down");
        press(cx, window, "down");
        // stash@{1} is the older of the two.
        assert_eq!(active_label(cx, &view), shas[0]);

        stash(Path::new(&info.path), "change 2");
        let info = engine.open_repository(&info.path).unwrap();
        view.update(cx, |view, cx| view.set_repository(Arc::new(info), cx));
        cx.run_until_parked();
        assert_eq!(
            view.read_with(cx, |view, _| view
                .context
                .stashes
                .data
                .as_ref()
                .unwrap()
                .stashes
                .len()),
            3
        );
        assert_eq!(active_label(cx, &view), shas[0]);
        let events = Rc::new(RefCell::new(Vec::new()));
        let captured = events.clone();
        let _subscription = cx.update(|cx| {
            cx.subscribe(&view, move |_, event: &SidebarEvent, _| {
                captured.borrow_mut().push(format!("{event:?}"));
            })
        });
        press(cx, window, "enter");
        assert_eq!(
            events.borrow().as_slice(),
            [format!("{:?}", SidebarEvent::SelectStash(shas[0].clone()))]
        );
    }

    #[gpui_kit::test]
    fn view_state_restores_collapsed_sections_and_scroll(cx: &mut TestAppContext) {
        let (_cache, window, view) = open_sidebar(cx);
        cx.update_window(window, |_, window, cx| window.render_frame(cx))
            .unwrap();
        let saved = view.update(cx, |view, cx| {
            view.reference_state.toggle_group(ReferenceKind::Tag);
            view.reference_state
                .toggle_folder(ReferenceKind::Local, "feature".into());
            view.rebuild_rows(cx);
            view.view_state()
        });
        view.update(cx, |view, cx| {
            view.reference_state.toggle_group(ReferenceKind::Tag);
            view.rebuild_rows(cx);
            assert_ne!(view.view_state(), saved);
            view.restore_view_state(saved.clone(), cx);
            assert_eq!(view.view_state(), saved);
            assert!(view.reference_state.group_closed(ReferenceKind::Tag));
            assert!(
                view.reference_state
                    .folder_closed(ReferenceKind::Local, "feature")
            );
        });
    }

    #[gpui_kit::test]
    fn view_state_restores_the_filter_its_scroll_offset_belongs_to(cx: &mut TestAppContext) {
        let references = (0..40)
            .flat_map(|index| [format!("feature/f{index:02}"), format!("zeta/z{index:02}")])
            .map(|label| (*reference(&label)).clone())
            .collect();
        let (_cache, window, view) = open_sidebar_with(cx, references);
        cx.update_window(window, |_, window, cx| {
            view.update(cx, |view, cx| view.focus_references(window, cx));
            window.input("zeta/", cx);
        })
        .unwrap();
        view.update(cx, |view, _| {
            view.scroll_handle.scroll_vertical_to(px(500.))
        });
        render(cx, window);
        let top_label = |cx: &mut TestAppContext| {
            view.read_with(cx, |view, _| {
                row_label(view.rows.get(view.list_state.logical_scroll_top().item_ix))
            })
        };
        let top = top_label(cx);
        assert!(top.starts_with("zeta/"), "{top}");
        let saved = view.read_with(cx, |view, _| view.view_state());

        // Leave the tab, then come back to it.
        view.update(cx, |view, cx| {
            let info = view.repository.clone().unwrap();
            view.clear_repository(cx);
            view.prepare_repository_open(&info.path, cx);
            view.set_repository(info, cx);
            view.restore_view_state(saved, cx);
        });
        render(cx, window);
        view.read_with(cx, |view, cx| {
            assert_eq!(view.filter_input.read(cx).value(), "zeta/");
            assert_eq!(view.scroll_handle.vertical_offset(), px(500.));
        });
        assert_eq!(top_label(cx), top);
    }

    #[gpui_kit::test]
    fn another_repository_starts_with_open_groups_and_folders(cx: &mut TestAppContext) {
        let (_cache, _window, view) = open_sidebar(cx);
        view.update(cx, |view, cx| {
            let closed = |view: &SidebarView| {
                (
                    view.reference_state.group_closed(ReferenceKind::Tag),
                    view.reference_state
                        .folder_closed(ReferenceKind::Local, "feature"),
                )
            };
            view.reference_state.toggle_group(ReferenceKind::Tag);
            view.reference_state
                .toggle_folder(ReferenceKind::Local, "feature".into());
            let info = view.repository.clone().unwrap();
            view.set_repository(info.clone(), cx);
            assert_eq!(closed(view), (true, true), "a refresh keeps them");

            // A new tab opens another repository.
            view.clear_repository(cx);
            let other = repository_info("/other", info.references.clone());
            view.prepare_repository_open(&other.path, cx);
            view.set_repository(Arc::new(other), cx);
            assert_eq!(closed(view), (false, false));
        });
    }

    /// Restores an offset beyond the tree height while the expanded stash
    /// list is still being read.
    fn restore_while_stashes_load(
        cx: &mut TestAppContext,
    ) -> (
        TempDir,
        AnyWindowHandle,
        Entity<SidebarView>,
        SidebarViewState,
    ) {
        let (root, engine, info, _) = stash_repository(40);
        let (window, view) = sidebar_window(cx, engine);
        cx.update_window(window, |_, window, cx| {
            view.update(cx, |view, cx| view.set_repository(Arc::new(info), cx));
            window.render_frame(cx);
        })
        .unwrap();
        let state = SidebarViewState {
            scroll_top: px(900.),
            stashes_expanded: true,
            ..Default::default()
        };
        view.update(cx, |view, cx| {
            view.restore_view_state(state.clone(), cx);
            assert!(view.context.stashes.loading);
            assert_eq!(view.scroll_handle.vertical_offset(), px(0.));
        });
        (root, window, view, state)
    }

    #[gpui_kit::test]
    fn restored_scroll_waits_for_the_expanded_stash_list(cx: &mut TestAppContext) {
        let (_root, window, view, state) = restore_while_stashes_load(cx);
        // Leaving the tab again meanwhile keeps the offset to restore.
        assert_eq!(view.read_with(cx, |view, _| view.view_state()), state);
        cx.run_until_parked();
        render(cx, window);
        view.read_with(cx, |view, _| {
            assert_eq!(
                view.context.stashes.data.as_ref().unwrap().stashes.len(),
                40
            );
            assert_eq!(view.scroll_handle.vertical_offset(), px(900.));
            assert_eq!(view.restored_scroll_top, None);
        });
    }

    #[gpui_kit::test]
    fn scrolling_before_the_lists_are_read_drops_the_restored_offset(cx: &mut TestAppContext) {
        let (_root, window, view, _) = restore_while_stashes_load(cx);
        let position = view.read_with(cx, |view, _| view.list_state.viewport_bounds().center());
        cx.update_window(window, |_, window, cx| {
            window.dispatch_event(
                MouseMoveEvent {
                    position,
                    pressed_button: None,
                    modifiers: Default::default(),
                }
                .to_platform_input(),
                cx,
            );
            window.render_frame(cx);
            window.dispatch_event(
                ScrollWheelEvent {
                    position,
                    delta: ScrollDelta::Pixels(point(px(0.), px(-40.))),
                    ..Default::default()
                }
                .to_platform_input(),
                cx,
            );
        })
        .unwrap();
        assert_eq!(view.read_with(cx, |view, _| view.restored_scroll_top), None);
        cx.run_until_parked();
        render(cx, window);
        view.read_with(cx, |view, _| {
            assert_eq!(
                view.context.stashes.data.as_ref().unwrap().stashes.len(),
                40
            );
            assert_eq!(view.scroll_handle.vertical_offset(), px(0.));
        });
    }

    /// Leaves the keyboard on branch b02, scrolled out of view, then opens its
    /// menu with Shift+F10.
    fn open_menu_of_scrolled_away_row(
        cx: &mut TestAppContext,
    ) -> (TempDir, AnyWindowHandle, Entity<SidebarView>) {
        let references = (0..40)
            .map(|index| (*reference(&format!("b{index:02}"))).clone())
            .collect();
        let (cache, window, view) = open_sidebar_with(cx, references);
        cx.update_window(window, |_, window, cx| {
            view.update(cx, |view, cx| view.focus_tree(window, cx));
        })
        .unwrap();
        for _ in 0..3 {
            press(cx, window, "down");
        }
        assert_eq!(active_label(cx, &view), "b02");
        view.update(cx, |view, _| {
            view.scroll_handle.scroll_vertical_to(px(800.))
        });
        render(cx, window);
        press(cx, window, "shift-f10");
        let menu_focused = cx
            .update_window(window, |_, window, cx| {
                let (menu, _) = view.read(cx).menu.clone().unwrap();
                menu.focus_handle(cx).is_focused(window)
            })
            .unwrap();
        assert!(menu_focused);
        (cache, window, view)
    }

    #[gpui_kit::test]
    fn keyboard_branch_menu_opens_below_the_active_row_where_it_is_now(cx: &mut TestAppContext) {
        let (_cache, window, view) = open_menu_of_scrolled_away_row(cx);
        render(cx, window);
        view.read_with(cx, |view, _| {
            let index = view.active_row.unwrap();
            let bounds = view.list_state.bounds_for_item(index).unwrap();
            assert!(view.list_state.viewport_bounds().contains(&bounds.origin));
            assert_eq!(view.menu.as_ref().unwrap().1, bounds.bottom_left());
        });
    }

    #[gpui_kit::test]
    fn choosing_solo_in_the_branch_menu_closes_it_and_returns_focus_to_the_tree(
        cx: &mut TestAppContext,
    ) {
        let (_cache, window, view) = open_menu_of_scrolled_away_row(cx);
        let events = Rc::new(RefCell::new(Vec::new()));
        let captured = events.clone();
        let _subscription = cx.update(|cx| {
            cx.subscribe(&view, move |_, event: &SidebarEvent, _| {
                captured.borrow_mut().push(format!("{event:?}"));
            })
        });
        render(cx, window);
        press(cx, window, "down");
        render(cx, window);
        press(cx, window, "enter");
        render(cx, window);
        assert_eq!(
            events.borrow().as_slice(),
            [format!(
                "{:?}",
                SidebarEvent::ToggleSolo("refs/heads/b02".into())
            )]
        );
        let tree_focused = cx
            .update_window(window, |_, window, cx| {
                let view = view.read(cx);
                view.menu.is_none() && view.tree_focus.is_focused(window)
            })
            .unwrap();
        assert!(tree_focused);
    }

    #[gpui_kit::test]
    fn closing_the_branch_menu_from_outside_returns_focus_to_the_tree(cx: &mut TestAppContext) {
        let (_cache, window, view) = open_menu_of_scrolled_away_row(cx);
        view.update(cx, |view, cx| view.set_width(300., cx));
        render(cx, window);
        let tree_focused = cx
            .update_window(window, |_, window, cx| {
                let view = view.read(cx);
                view.menu.is_none() && view.tree_focus.is_focused(window)
            })
            .unwrap();
        assert!(tree_focused);
        assert_eq!(active_label(cx, &view), "b02");
    }

    #[test]
    fn worktree_search_keeps_navigation_indices_and_matches_detached_heads() {
        let worktree = |path: &str, head_name: &str, detached, available| WorktreeInfo {
            name: path.into(),
            path: path.into(),
            primary: path == "/main",
            current: path == "/main",
            available,
            head: Some("d0646df0123456789012345678901234567890123".into()),
            head_name: head_name.into(),
            detached,
            locked: None,
            reason: None,
        };
        let list = WorktreeList {
            worktrees: vec![
                worktree("/main", "main", false, true),
                worktree("/review", "t3code/review", false, true),
                worktree("/feature", "feature/ui", false, true),
                worktree("/detached", "HEAD détachée", true, true),
                worktree("/missing", "t3code/missing", false, false),
            ],
            truncated: false,
        };
        let indices = filtered_worktree_indices(&list, "  T3CODE/  ");
        let paths: Vec<_> = indices
            .iter()
            .map(|&index| list.worktrees[index].path.as_str())
            .collect();
        assert_eq!(paths, ["/review", "/missing"]);
        assert_eq!(filtered_worktree_indices(&list, "D0646DF"), [3]);
        assert_eq!(filtered_worktree_indices(&list, "détachée"), [3]);
        assert!(filtered_worktree_indices(&list, "absent").is_empty());
        assert_eq!(filtered_worktree_indices(&list, "  "), [0, 1, 2, 3, 4]);
    }

    #[test]
    fn search_highlights_follow_matches_across_tree_segments() {
        let cases = [
            ("t3code/review", "t3code/", vec![vec![0..6], vec![]]),
            (
                "feature/ui/work",
                "ui/wo",
                vec![vec![], vec![0..2], vec![0..2]],
            ),
            (
                "foo/baz/foo/bar",
                "foo/bar",
                vec![vec![], vec![], vec![0..3], vec![0..3]],
            ),
        ];
        for (name, filter, expected) in cases {
            let tree = reference_tree([reference(name)].into_iter(), filter);
            let mut rows = Vec::new();
            flatten_tree(
                &tree,
                ReferenceKind::Local,
                0,
                &ReferenceTreeState::default(),
                &mut rows,
            );
            let highlights: Vec<_> = rows
                .into_iter()
                .filter_map(|row| match row {
                    SidebarRow::Folder { highlights, .. }
                    | SidebarRow::Reference { highlights, .. } => Some(highlights),
                    _ => None,
                })
                .collect();
            assert_eq!(highlights, expected, "{name} filtered by {filter}");
        }
        let tree = reference_tree(
            [reference("t3code/review"), reference("t3code/fix")].into_iter(),
            "t3code/",
        );
        assert!(
            matches!(&tree[0], ReferenceNode::Folder { highlights, .. } if matches!(highlights.as_slice(), [range] if *range == (0..6)))
        );
    }

    #[test]
    fn search_highlights_preserve_original_utf8_and_all_occurrences() {
        let text = "İ/Ⱥ/École-éCOLE";
        let ranges = search_highlight_ranges(text, "  ÉCOLE  ");
        let matched: Vec<_> = ranges.into_iter().map(|range| &text[range]).collect();
        assert_eq!(matched, ["École", "éCOLE"]);
        let range = search_highlight_ranges(text, "i").pop().unwrap();
        assert_eq!(&text[range], "İ");
        let range = search_highlight_ranges(text, "ⱥ").pop().unwrap();
        assert_eq!(&text[range], "Ⱥ");
        assert!(search_highlight_ranges(text, "  ").is_empty());
        assert!(search_highlight_ranges(text, "absent").is_empty());
    }

    #[test]
    fn search_reveals_folders_without_changing_saved_collapse_state() {
        let mut state = ReferenceTreeState::default();
        state.toggle_group(ReferenceKind::Local);
        state.toggle_folder(ReferenceKind::Local, "feature".into());
        state.set_filter("work".into());
        assert!(!state.group_closed(ReferenceKind::Local));
        assert!(!state.folder_closed(ReferenceKind::Local, "feature"));
        state.toggle_folder(ReferenceKind::Local, "feature".into());
        assert!(state.folder_closed(ReferenceKind::Local, "feature"));
        state.set_filter("other".into());
        assert!(!state.folder_closed(ReferenceKind::Local, "feature"));
        state.set_filter("  ".into());
        assert!(state.group_closed(ReferenceKind::Local));
        assert!(state.folder_closed(ReferenceKind::Local, "feature"));
    }

    #[test]
    fn nested_counts_and_closed_rows_preserve_reference_identity() {
        let tree = reference_tree(
            [
                reference("feature/ui/work"),
                reference("feature/ui/design"),
                reference("feature/backend"),
                reference("main"),
            ]
            .into_iter(),
            "",
        );
        assert!(matches!(&tree[0],ReferenceNode::Folder { path,count:3,.. } if path == "feature"));
        let mut state = ReferenceTreeState::default();
        state.toggle_folder(ReferenceKind::Local, "feature/ui".into());
        let mut rows = Vec::new();
        flatten_tree(&tree, ReferenceKind::Local, 0, &state, &mut rows);
        assert_eq!(rows.len(), 4);
        assert!(rows.iter().any(|row| matches!(row,SidebarRow::Reference { reference,.. } if reference.name == "refs/heads/feature/backend")));
        assert!(!rows.iter().any(|row| matches!(row,SidebarRow::Reference { reference,.. } if reference.name == "refs/heads/feature/ui/work")));
    }

    #[test]
    fn numeric_reference_order_is_case_and_accent_insensitive() {
        assert_eq!(reference_order("release2", "release10"), Ordering::Less);
        assert_eq!(reference_order("ÉTAPE2", "etape2"), Ordering::Equal);
        assert_eq!(reference_order("release02", "release2"), Ordering::Equal);
        assert_eq!(
            reference_order("release100000000000000000000", "release9"),
            Ordering::Greater
        );
        for (left, right) in [
            ("œuvre", "oeuvre"),
            ("Æther", "aether"),
            ("école", "e\u{301}cole"),
            ("release２", "release2"),
        ] {
            assert_eq!(reference_order(left, right), Ordering::Equal);
        }
        for (left, right) in [("_fix", "-fix"), ("-fix", "fix"), ("fix", "fix2")] {
            assert_eq!(reference_order(left, right), Ordering::Less);
        }
    }

    #[test]
    fn collation_equal_labels_keep_folder_then_reference_insertion_order() {
        let tree = reference_tree(
            [
                reference("ecole"),
                reference("École/a"),
                reference("école/b"),
            ]
            .into_iter(),
            "",
        );
        assert!(matches!(&tree[0], ReferenceNode::Folder { label, .. } if label == "École"));
        assert!(matches!(&tree[1], ReferenceNode::Folder { label, .. } if label == "école"));
        assert!(matches!(&tree[2], ReferenceNode::Reference { label, .. } if label == "ecole"));
    }

    #[test]
    fn scrollbar_covers_large_unmeasured_trees_with_variable_context_rows() {
        let mut rows = vec![SidebarRow::Group {
            kind: ReferenceKind::Local,
            count: 10_000,
            closed: false,
        }];
        let reference = reference("main");
        rows.extend((0..10_000).map(|_| SidebarRow::Reference {
            kind: ReferenceKind::Local,
            label: "main".into(),
            highlights: Vec::new(),
            reference: reference.clone(),
            depth: 0,
        }));
        rows.extend([
            SidebarRow::Spacer(19.),
            SidebarRow::ContextSeparator { margin_top: 0. },
            SidebarRow::Worktrees {
                count: Some(1),
                expanded: true,
            },
            SidebarRow::Worktree {
                index: 0,
                path: "/main".into(),
            },
            SidebarRow::Stashes {
                count: Some(1),
                expanded: true,
            },
            SidebarRow::Stash {
                index: 0,
                sha: "a".repeat(40),
            },
            SidebarRow::Spacer(24.),
        ]);
        let list = ListState::new(rows.len(), ListAlignment::Top, px(200.));
        let handle = VirtualScrollHandle::measured(list.clone());
        handle.set_heights(rows.iter().map(|row| row.height_hint(1.)));
        // No window or rendered item is required to know the complete extent.
        // 28 px sections, 10 000 × 28 px references, spacers, separator and
        // two 44 px context items.
        assert_eq!(handle.content_size().height, px(280_223.));
        handle.set_offset(point(px(0.), -handle.content_size().height));
        assert_eq!(list.logical_scroll_top().item_ix, rows.len());

        let metrics = RowGeometry::from_heights(rows.iter().map(|row| row.height_hint(1.)));
        let bottom = metrics.total() - px(600.);
        let final_viewport = metrics.list_offset(bottom, px(600.));
        assert!(final_viewport.item_ix > 9_980);
        assert_eq!(
            metrics.start(final_viewport.item_ix) + final_viewport.offset_in_item,
            bottom
        );
    }

    #[test]
    fn wheel_after_drag_into_unmeasured_rows_moves_only_the_requested_pixels() {
        let list = ListState::new(10_000, ListAlignment::Top, px(200.));
        let handle = VirtualScrollHandle::measured(list.clone());
        handle.set_heights((0..10_000).map(|_| px(ROW_HEIGHT)));
        handle.set_offset(point(px(0.), px(-140_003.)));
        assert_eq!(list.logical_scroll_top().item_ix, 5_000);
        for _ in 0..6 {
            assert!(handle.scroll_wheel(ScrollDelta::Pixels(point(px(0.), px(-40.))), false));
        }
        assert_eq!(handle.offset().y, px(-140_243.));
        assert_eq!(list.logical_scroll_top().item_ix, 5_008);
        assert_eq!(list.logical_scroll_top().offset_in_item, px(19.));

        assert!(handle.scroll_wheel(ScrollDelta::Lines(point(0., 3.)), false));
        assert_eq!(handle.offset().y, px(-140_183.));
        assert!(!handle.scroll_wheel(ScrollDelta::Pixels(point(px(40.), px(0.))), false));
        assert_eq!(handle.offset().y, px(-140_183.));
        handle.scroll_wheel(ScrollDelta::Pixels(point(px(0.), px(1_000_000.))), false);
        assert_eq!(list.logical_scroll_top().item_ix, 0);
        assert_eq!(list.logical_scroll_top().offset_in_item, px(0.));
    }
}
