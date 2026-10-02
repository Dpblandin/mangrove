//! Mangrove's reference tree and lazy repository context, rendered with a
//! variable-height virtual list so repository size does not set UI work per frame.
use gpui_kit::{
    assets::IconName,
    base::Button as NativeButton,
    component::{
        Icon, Sizable,
        input::{Input, InputEvent, InputState},
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
    cell::RefCell,
    cmp::Ordering,
    collections::{HashMap, HashSet},
    ops::Range,
    rc::Rc,
    sync::{Arc, OnceLock},
};

use crate::{
    format::{commit_date, format_count, short_sha},
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
    Other,
}

impl ReferenceKind {
    const ALL: [Self; 4] = [Self::Local, Self::Remote, Self::Tag, Self::Other];
    fn name(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Remote => "remote",
            Self::Tag => "tag",
            Self::Other => "other",
        }
    }
    fn title(self) -> &'static str {
        match self {
            Self::Local => "Branches locales",
            Self::Remote => "Branches distantes",
            Self::Tag => "Tags",
            Self::Other => "Autres références",
        }
    }
    fn icon(self) -> IconName {
        match self {
            Self::Local => IconName::GitBranch,
            Self::Remote => IconName::Globe,
            Self::Tag => IconName::Tag,
            Self::Other => IconName::Layers,
        }
    }
    fn can_solo(self) -> bool {
        matches!(self, Self::Local | Self::Remote)
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

#[derive(Default)]
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
    Worktree(usize),
    Stashes {
        count: Option<usize>,
        expanded: bool,
    },
    Stash(usize),
    Notice {
        text: String,
        error: bool,
    },
}

impl SidebarRow {
    fn height_hint(&self, font_offset: f32) -> Pixels {
        px(match self {
            Self::Group { .. } | Self::Worktrees { .. } | Self::Stashes { .. } => {
                (10. + font_offset) * 1.5 + 10.
            }
            Self::Folder { .. } | Self::Reference { .. } => 32.,
            Self::Spacer(height) => *height,
            Self::NoReferences => (11. + font_offset) * 1.5 + 40.,
            Self::ContextSeparator { margin_top } => margin_top + 8.,
            Self::Worktree(_) => 28.,
            Self::Stash(_) => (29. + 3. * font_offset) * 1.5 + 24.,
            Self::Notice { .. } => (9. + font_offset) * 1.5 + 16.,
        })
    }
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
    reset_filter_input: bool,
    palette: Palette,
    font_size_offset: f32,
    width: f32,
    menu: Option<(Entity<PopupMenu>, Point<Pixels>)>,
    menu_subscription: Option<Subscription>,
    row_bounds: Rc<RefCell<HashMap<String, Bounds<Pixels>>>>,
    reference_focus: HashMap<String, FocusHandle>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<SidebarEvent> for SidebarView {}

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
            self.list_state.scroll_to_reveal_item(index);
        }
        self.filter_input
            .update(cx, |input, cx| input.focus(window, cx));
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
                view.rebuild_reference_groups();
                view.rebuild_rows(cx);
            }
        });
        let bounds_subscription =
            cx.observe_window_bounds(window, |view, _, cx| view.close_menu(cx));
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
            reset_filter_input: false,
            palette: Palette::default(),
            font_size_offset: 1.,
            width: 226.,
            menu: None,
            menu_subscription: None,
            row_bounds: Rc::default(),
            reference_focus: HashMap::new(),
            _subscriptions: vec![subscription, bounds_subscription],
        }
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
        let names: HashSet<_> = self
            .references
            .iter()
            .map(|reference| reference.name.as_str())
            .collect();
        self.reference_focus
            .retain(|name, _| names.contains(name.as_str()));
        self.repository = Some(repository);
        self.context.invalidate(changed && !shared_worktrees);
        if different_path {
            if !shared_worktrees {
                self.reference_state.set_filter(String::new());
                self.reset_filter_input = true;
            }
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
        self.reference_focus.clear();
        self.reference_groups.clear();
        self.context.invalidate(true);
        self.selected_sha = None;
        self.selected_stash.clear();
        self.solo_refs.clear();
        self.reference_state.set_filter(String::new());
        self.reset_filter_input = true;
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
        self.row_bounds.borrow_mut().clear();
        let previous_rows = std::mem::take(&mut self.rows);
        if self.repository.is_none() {
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
        let worktree_indices = self
            .context
            .worktrees
            .data
            .as_ref()
            .map(|list| filtered_worktree_indices(list, &self.reference_state.filter));
        self.rows.push(SidebarRow::Worktrees {
            count: worktree_indices.as_ref().map(Vec::len),
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
            if let Some(indices) = worktree_indices {
                if indices.is_empty() && self.reference_state.searching() {
                    self.rows.push(SidebarRow::Notice {
                        text: "Aucun worktree trouvé.".into(),
                        error: false,
                    });
                }
                self.rows
                    .extend(indices.into_iter().map(SidebarRow::Worktree));
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
                    .extend((0..list.stashes.len()).map(SidebarRow::Stash));
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
        cx.notify();
    }

    fn close_menu(&mut self, cx: &mut Context<Self>) {
        if self.menu.take().is_some() {
            cx.notify();
        }
        self.menu_subscription = None;
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
        let original_focus = self
            .reference_focus
            .get(&name)
            .cloned()
            .or_else(|| window.focused(cx));
        let view = cx.weak_entity();
        let color = self.palette.text;
        let font_size = 11. + self.font_size_offset;
        let menu = PopupMenu::build(window, cx, move |menu, _, _| {
            menu.min_w(px(210.))
                .when_some(original_focus, |menu, focus| menu.action_context(focus))
                .item(
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

    fn render_group(
        &self,
        id: String,
        title: &'static str,
        count: Option<usize>,
        expanded: bool,
        icon_name: Option<IconName>,
    ) -> Stateful<Div> {
        let p = self.palette;
        div()
            .id(SharedString::from(id))
            .role(Role::Button)
            .focusable()
            .tab_index(0)
            .aria_label(title)
            .aria_expanded(expanded)
            .flex()
            .items_center()
            .gap(px(7.))
            .w_full()
            .h(px((10. + self.font_size_offset) * 1.5 + 10.))
            .px(px(15.))
            .pb(px(10.))
            .text_size(px(10. + self.font_size_offset))
            .text_color(rgb(p.muted))
            .font_weight(FontWeight::MEDIUM)
            .cursor_pointer()
            .focus_visible(|style| style.border_1().border_color(rgb(p.accent)))
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
            .when_some(count, |this, count| {
                this.child(
                    div()
                        .font_family(MONO)
                        .text_size(px(9. + self.font_size_offset))
                        .child(format_count(count)),
                )
            })
    }

    fn highlighted_label(&self, label: String, ranges: Vec<Range<usize>>) -> StyledText {
        let style = HighlightStyle {
            color: Some(rgb(self.palette.text).into()),
            background_color: Some(rgb(self.palette.accent).opacity(0.35).into()),
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
                    format!("reference-group-{}", kind.name()),
                    kind.title(),
                    Some(count),
                    !closed,
                    None,
                )
                .on_click(cx.listener(move |view, _, _, cx| {
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
                div()
                    .id(SharedString::from(format!(
                        "reference-folder-{}-{path}",
                        kind.name()
                    )))
                    .role(Role::Button)
                    .focusable()
                    .tab_index(0)
                    .aria_label(format!("Dossier {path}"))
                    .aria_expanded(!closed)
                    .tooltip(move |window, cx| Tooltip::new(tooltip_path.clone()).build(window, cx))
                    .flex()
                    .items_center()
                    .gap(px(7.))
                    .w_full()
                    .min_w_0()
                    .h(px(32.))
                    .pl(px(padding))
                    .pr(px(17.))
                    .text_size(px(11. + font_offset))
                    .text_color(rgb(p.text))
                    .cursor_pointer()
                    .hover(|style| style.bg(rgb(p.accent).opacity(0.1)))
                    .focus_visible(|style| style.border_1().border_color(rgb(p.accent)))
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
                            .text_size(px(9. + font_offset))
                            .text_color(rgb(p.muted))
                            .child(format_count(count)),
                    )
                    .on_click(cx.listener(move |view, _, _, cx| {
                        view.reference_state.toggle_folder(kind, path.clone());
                        view.rebuild_rows(cx);
                    }))
                    .into_any_element()
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
                let key_name = name.clone();
                let menu_name = name.clone();
                let row_bounds = self.row_bounds.clone();
                let bounds_key = name.clone();
                let tooltip_label = reference.label.clone();
                let reference_focus = self
                    .reference_focus
                    .entry(name.clone())
                    .or_insert_with(|| cx.focus_handle())
                    .clone();
                let button_width = self.width - 1. - if kind.can_solo() { 24. } else { 0. };
                let padding = (26. + depth as f32 * 14.).min(button_width * 0.35);
                let label = self.highlighted_label(label, highlights);
                let row = div()
                    .id(SharedString::from(format!("reference-{name}")))
                    .role(Role::Button)
                    .track_focus(&reference_focus)
                    .tab_index(0)
                    .aria_label(reference.label.clone())
                    .tooltip(move |window, cx| {
                        Tooltip::new(tooltip_label.clone()).build(window, cx)
                    })
                    .relative()
                    .flex()
                    .items_center()
                    .gap(px(9.))
                    .h(px(32.))
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
                    .focus_visible(|style| style.border_1().border_color(rgb(p.accent)))
                    .child(
                        canvas(
                            move |bounds, _, _| {
                                row_bounds.borrow_mut().insert(bounds_key.clone(), bounds);
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .size_full(),
                    )
                    .child(Icon::new(kind.icon()).with_size(px(13.)).opacity(0.8))
                    .child(div().min_w_0().flex_1().truncate().child(label))
                    .when(current, |this| {
                        this.child(Icon::new(IconName::Check).with_size(px(12.)).opacity(0.8))
                    })
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(SidebarEvent::NavigateReference(click_name.clone()))
                    }))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |view, event: &MouseDownEvent, window, cx| {
                            if kind.can_solo() {
                                view.open_branch_menu(
                                    menu_name.clone(),
                                    event.position,
                                    window,
                                    cx,
                                );
                                cx.stop_propagation();
                            }
                        }),
                    )
                    .on_key_down(cx.listener(move |view, event: &KeyDownEvent, window, cx| {
                        if kind.can_solo()
                            && ((event.keystroke.key == "f10" && event.keystroke.modifiers.shift)
                                || event.keystroke.key == "contextmenu")
                        {
                            let position = view
                                .row_bounds
                                .borrow()
                                .get(&key_name)
                                .map(|bounds| point(bounds.left(), bounds.bottom()))
                                .unwrap_or_else(|| window.mouse_position());
                            view.open_branch_menu(key_name.clone(), position, window, cx);
                            cx.stop_propagation();
                        }
                    }));
                div()
                    .id(SharedString::from(format!("reference-leaf-{name}")))
                    .group("reference-leaf")
                    .flex()
                    .items_center()
                    .w_full()
                    .min_w_0()
                    .child(row)
                    .when(kind.can_solo(), |this| {
                        this.child(
                            div()
                                .id(SharedString::from(format!("solo-{name}")))
                                .role(Role::Button)
                                .focusable()
                                .tab_index(0)
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
                                .when(reference_focus.is_focused(window), |this| this.opacity(1.))
                                .group_hover("reference-leaf", |style| style.opacity(1.))
                                .focus(|style| style.opacity(1.))
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
                                    move |_, _, _, cx| {
                                        cx.emit(SidebarEvent::ToggleSolo(name.clone()))
                                    }
                                })),
                        )
                    })
                    .into_any_element()
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
                    "worktrees-group".into(),
                    "Worktrees",
                    count,
                    expanded,
                    Some(IconName::FolderGit2),
                )
                .on_click(cx.listener(|view, _, _, cx| view.toggle_worktrees(cx)))
                .into_any_element(),
            SidebarRow::Stashes { count, expanded } => self
                .render_group("stashes-group".into(), "Stashes", count, expanded, None)
                .on_click(cx.listener(|view, _, _, cx| view.toggle_stashes(cx)))
                .into_any_element(),
            SidebarRow::Worktree(index) => {
                let Some(item) = self
                    .context
                    .worktrees
                    .data
                    .as_ref()
                    .and_then(|list| list.worktrees.get(index))
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
                let color = if current { p.success } else { p.text };
                NativeButton::new(SharedString::from(format!("worktree-{path}")))
                    .disabled(!available)
                    .role(Role::Button)
                    .when(available, |this| this.cursor_pointer())
                    .aria_label(label.clone())
                    .aria_description(description)
                    .tooltip(move |window, cx| Tooltip::new(tooltip_text.clone()).build(window, cx))
                    .flex()
                    .w_full()
                    .items_center()
                    .gap(px(8.))
                    .h(px(28.))
                    .pl(px(26.))
                    .pr(px(15.))
                    .text_size(px(11. + font_offset))
                    .text_color(rgb(color))
                    .opacity(if available { 1. } else { 0.7 })
                    .when(current, |this| this.bg(rgb(p.success).opacity(0.18)))
                    .hover(|style| {
                        style.bg(if current {
                            rgb(p.success).opacity(0.18)
                        } else {
                            rgb(p.accent).opacity(0.1)
                        })
                    })
                    .focus_visible(|style| style.border_1().border_color(rgb(p.accent)))
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
                            .truncate()
                            .font_weight(if current {
                                FontWeight::SEMIBOLD
                            } else {
                                FontWeight::MEDIUM
                            })
                            .child(self.highlighted_label(label, highlights)),
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
                    .on_click(cx.listener(move |_, _, _, cx| {
                        if available {
                            cx.emit(SidebarEvent::OpenWorktree(path.clone()));
                        }
                    }))
                    .into_any_element()
            }
            SidebarRow::Stash(index) => {
                let Some(item) = self
                    .context
                    .stashes
                    .data
                    .as_ref()
                    .and_then(|list| list.stashes.get(index))
                else {
                    return div().into_any_element();
                };
                let sha = item.sha.clone();
                let title = format!(
                    "stash@{{ {} }} · {}",
                    item.index,
                    item.message.lines().next().unwrap_or("")
                );
                let subtitle = format!("{} · {}", item.author, commit_date(item.time));
                let base = format!("Base {}", short_sha(&item.base_sha));
                let tooltip_message = item.message.clone();
                div()
                    .id(SharedString::from(format!("stash-{sha}")))
                    .role(Role::Button)
                    .focusable()
                    .tab_index(0)
                    .aria_label(item.message.clone())
                    .flex()
                    .items_start()
                    .gap(px(8.))
                    .px(px(12.))
                    .py(px(8.))
                    .tooltip(move |window, cx| {
                        Tooltip::new(tooltip_message.clone()).build(window, cx)
                    })
                    .rounded(RADIUS_MD)
                    .cursor_pointer()
                    .text_color(rgb(p.text))
                    .when(sha == self.selected_stash, |this| {
                        this.bg(rgb(p.accent).opacity(0.1))
                    })
                    .hover(|style| style.bg(rgb(p.accent).opacity(0.1)))
                    .focus_visible(|style| style.border_1().border_color(rgb(p.accent)))
                    .child(icon(IconName::Archive, 13., p.text))
                    .child(
                        div()
                            .min_w_0()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap(px(4.))
                            .child(
                                div()
                                    .truncate()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_size(px(11. + font_offset))
                                    .child(title),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(9. + font_offset))
                                    .text_color(rgb(p.muted))
                                    .child(subtitle),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_size(px(9. + font_offset))
                                    .text_color(rgb(p.muted))
                                    .child(base),
                            ),
                    )
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(SidebarEvent::SelectStash(sha.clone()))
                    }))
                    .into_any_element()
            }
            SidebarRow::Notice { text, error } => div()
                .px(px(12.))
                .py(px(8.))
                .text_size(px(9. + font_offset))
                .text_color(rgb(if error { p.danger } else { p.muted }))
                .child(text)
                .into_any_element(),
        }
    }
}

impl Render for SidebarView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.reset_filter_input {
            self.filter_input
                .update(cx, |input, cx| input.set_value("", window, cx));
            self.reset_filter_input = false;
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
                .w_full()
                .min_w_0()
                .text_size(px(10. + font_offset))
                .line_height(relative(1.5)),
            px((10. + font_offset) * 1.5),
        );
        let menu = self.menu.clone();
        let wheel_view = cx.weak_entity();
        let wheel_capture = self.scroll_handle.wheel_capture(move |changed, _, cx| {
            let _ = wheel_view.update(cx, |view, cx| {
                view.close_menu(cx);
                if changed {
                    cx.notify();
                }
            });
        });
        div().id("sidebar").relative().size_full().min_w_0().flex().flex_col()
            .font_family(SANS).line_height(relative(1.5)).bg(rgb(p.panel)).text_color(rgb(p.text))
            .border_r_1().border_color(rgb(p.border))
            .child(div().id("sidebar-heading").aria_label("ESPACE DE TRAVAIL")
                .pt(px(16.)).pb(px(12.)).px(px(if self.width < 210. { 14. } else { 22. }))
                .text_size(px(9. + font_offset)).text_color(rgb(p.muted))
                // GPUI has no letter-spacing property. Individual glyphs
                // reproduce the source's 0.1em uppercase heading tracking.
                .flex().gap(px((9. + font_offset) * 0.1))
                .children("ESPACE DE TRAVAIL".chars().map(|character|div().child(character.to_string()))))
            .when(self.repository.is_some(), |this| this
                .child(div().mx(px(15.)).mb(px(18.)).px(px(8.)).py(px(7.))
                    .flex().items_center().gap(px(7.)).border_1().border_color(rgb(p.border)).rounded(RADIUS_MD)
                    .child(icon(IconName::Search,13.,p.muted)).child(filter_input))
                .child(div().relative().flex_1().min_h_0().w_full().flex().flex_col()
                    .child(wheel_capture)
                    .child(list(self.list_state.clone(), move |index, window, cx| {
                        entity.update(cx, |view,cx| view.render_row(index,window,cx))
                    }).flex_1().min_h_0().w_full())
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
        (SidebarRow::Worktree(a), SidebarRow::Worktree(b))
        | (SidebarRow::Stash(a), SidebarRow::Stash(b)) => a == b,
        (SidebarRow::Notice { text: a, .. }, SidebarRow::Notice { text: b, .. }) => a == b,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ReferenceKind, ReferenceNode, ReferenceTreeState, SidebarRow, filtered_worktree_indices,
        flatten_tree, reference_order, reference_tree, search_highlight_ranges,
    };
    use crate::virtual_scroll::{RowGeometry, VirtualScrollHandle};
    use gpui_kit::{
        ListAlignment, ListState, ScrollDelta, component::scroll::ScrollbarHandle, point, px,
    };
    use mangrove_core::model::{Reference, WorktreeInfo, WorktreeList};
    use std::{cmp::Ordering, sync::Arc};

    fn reference(label: &str) -> Arc<Reference> {
        Arc::new(Reference {
            name: format!("refs/heads/{label}"),
            label: label.into(),
            kind: "local".into(),
            sha: "a".repeat(40),
        })
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
            SidebarRow::Worktree(0),
            SidebarRow::Stashes {
                count: Some(1),
                expanded: true,
            },
            SidebarRow::Stash(0),
            SidebarRow::Spacer(24.),
        ]);
        let list = ListState::new(rows.len(), ListAlignment::Top, px(200.));
        let handle = VirtualScrollHandle::measured(list.clone());
        handle.set_heights(rows.iter().map(|row| row.height_hint(1.)));
        // No window or rendered item is required to know the complete extent.
        assert_eq!(handle.content_size().height, px(320_230.5));
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
        handle.set_heights((0..10_000).map(|_| px(32.)));
        handle.set_offset(point(px(0.), px(-160_003.)));
        assert_eq!(list.logical_scroll_top().item_ix, 5_000);
        for _ in 0..6 {
            assert!(handle.scroll_wheel(ScrollDelta::Pixels(point(px(0.), px(-40.))), false));
        }
        assert_eq!(handle.offset().y, px(-160_243.));
        assert_eq!(list.logical_scroll_top().item_ix, 5_007);
        assert_eq!(list.logical_scroll_top().offset_in_item, px(19.));

        assert!(handle.scroll_wheel(ScrollDelta::Lines(point(0., 3.)), false));
        assert_eq!(handle.offset().y, px(-160_183.));
        assert!(!handle.scroll_wheel(ScrollDelta::Pixels(point(px(40.), px(0.))), false));
        assert_eq!(handle.offset().y, px(-160_183.));
        handle.scroll_wheel(ScrollDelta::Pixels(point(px(0.), px(1_000_000.))), false);
        assert_eq!(list.logical_scroll_top().item_ix, 0);
        assert_eq!(list.logical_scroll_top().offset_in_item, px(0.));
    }
}
