//! Searchable commands carry their context so a queued click cannot target a
//! different repository or file. Only visible rows are rendered.
use crate::{
    file_browser::FileView,
    history::fold,
    preferences::Pane,
    settings::SettingsSection,
    theme::{Palette, RADIUS_LG, RADIUS_SM, icon, scrollbar_theme},
};
use accesskit::Role;
use gpui_kit::{
    assets::IconName,
    component::{
        input::{Enter, Escape as InputEscape, Input, InputEvent, InputState, MoveDown, MoveUp},
        scroll::Scrollbar,
    },
    prelude::*,
    *,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    OpenRepository,
    NewTab,
    CloseTab(u64),
    ActivateTab(u64),
    CycleTab(isize),
    OpenRecent(String),
    RemoveRecent(String),
    BrowseRepositories,
    History,
    Search,
    WorkingTree,
    Refresh,
    Fetch,
    References,
    Worktrees,
    Stashes,
    OpenWorktree(String),
    InspectStash(String),
    ToggleSolo(String),
    ClearSolo,
    FileView(FileView),
    CloseFile,
    Compact(bool),
    CopySha(String),
    NavigateCommit(String),
    Settings(Option<SettingsSection>),
    Theme(crate::theme::ThemeId),
    Zoom(Option<i32>),
    DiffZoom(i32),
    ResetPane(Pane),
    ToggleImagePreviews,
    ToggleLargePreviews,
    ToggleAutoFetch,
    DismissError,
    Quit,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandEntry {
    pub command: Command,
    pub label: String,
    pub category: &'static str,
    pub detail: String,
    pub shortcut: Option<String>,
    pub disabled: Option<String>,
    pub active: bool,
    pub scope: Option<[u64; 3]>,
    search_label: String,
    search_terms: String,
}

impl CommandEntry {
    pub fn new(
        command: Command,
        label: impl Into<String>,
        category: &'static str,
        keywords: &str,
    ) -> Self {
        let label = label.into();
        Self {
            command,
            search_label: fold(&label),
            search_terms: fold(&format!("{category} {keywords}")),
            label,
            category,
            detail: String::new(),
            shortcut: None,
            disabled: None,
            active: false,
            scope: None,
        }
    }
    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = detail.into();
        self.search_terms.push(' ');
        self.search_terms.push_str(&fold(&self.detail));
        self
    }
    /// Label from `format::shortcut_label`, or a named key such as « Échap ».
    pub fn shortcut(mut self, label: impl Into<String>) -> Self {
        self.shortcut = Some(label.into());
        self
    }
    pub fn unavailable(mut self, reason: Option<&str>) -> Self {
        self.disabled = reason.map(str::to_owned);
        self
    }
    pub fn active(mut self, active: bool) -> Self {
        self.active = active;
        self
    }
    pub fn scoped(mut self, scope: [u64; 3]) -> Self {
        self.scope = Some(scope);
        self
    }
    fn score(&self, query: &str, words: &[&str]) -> Option<usize> {
        if !words
            .iter()
            .all(|word| self.search_label.contains(word) || self.search_terms.contains(word))
        {
            return None;
        }
        Some(if self.search_label == query {
            0
        } else if self.search_label.starts_with(query) {
            1
        } else if self.search_label.contains(query) {
            2
        } else if words.iter().all(|word| self.search_label.contains(word)) {
            3
        } else {
            4
        })
    }
    /// Compare with a fresh catalogue immediately before executing an event.
    pub fn is_current(&self, current: &[Self]) -> bool {
        self.disabled.is_none() && current.iter().any(|entry| entry == self)
    }
}

#[derive(Default)]
struct Results {
    entries: Vec<CommandEntry>,
    matches: Vec<usize>,
    selected: usize,
}
impl Results {
    fn filter(&mut self, query: &str) {
        let query = fold(query.trim());
        let words: Vec<_> = query.split_whitespace().collect();
        let mut matches: Vec<_> = self
            .entries
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| entry.score(&query, &words).map(|score| (score, index)))
            .collect();
        matches.sort_by_key(|&(score, index)| (score, index));
        self.matches = matches.into_iter().map(|(_, index)| index).collect();
        self.selected = 0;
    }
    fn update(&mut self, entries: Vec<CommandEntry>, query: &str) -> bool {
        if self.entries == entries {
            return false;
        }
        let selected = self.selected().map(|entry| entry.command.clone());
        self.entries = entries;
        self.filter(query);
        if let Some(index) = self
            .matches
            .iter()
            .position(|&index| Some(&self.entries[index].command) == selected.as_ref())
        {
            self.selected = index;
        }
        true
    }
    fn selected(&self) -> Option<&CommandEntry> {
        self.matches
            .get(self.selected)
            .and_then(|&index| self.entries.get(index))
    }
    fn navigate(&mut self, direction: isize) {
        if !self.matches.is_empty() {
            self.selected = self
                .selected
                .saturating_add_signed(direction)
                .min(self.matches.len() - 1);
        }
    }
}

pub enum PaletteEvent {
    Close,
    Execute(Box<CommandEntry>),
}

pub struct CommandPalette {
    input: Entity<InputState>,
    results: Results,
    scroll: UniformListScrollHandle,
    palette: Palette,
    font_offset: f32,
    _subscription: Subscription,
}
impl EventEmitter<PaletteEvent> for CommandPalette {}
impl CommandPalette {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Rechercher une commande…"));
        let subscription = cx.subscribe(&input, |view, input, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                view.results.filter(input.read(cx).value().as_ref());
                view.reveal_selection();
                cx.notify();
            }
        });
        Self {
            input,
            results: Results::default(),
            scroll: UniformListScrollHandle::new(),
            palette: Palette::default(),
            font_offset: 1.,
            _subscription: subscription,
        }
    }
    pub fn open(
        &mut self,
        entries: Vec<CommandEntry>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.results.entries = entries;
        self.results.filter("");
        self.reveal_selection();
        self.input.update(cx, |input, cx| {
            input.set_value("", window, cx);
            input.focus(window, cx);
        });
        cx.notify();
    }
    pub fn sync(
        &mut self,
        entries: Vec<CommandEntry>,
        palette: Palette,
        font_offset: f32,
        cx: &mut Context<Self>,
    ) {
        let changed = self
            .results
            .update(entries, self.input.read(cx).value().as_ref());
        if changed || self.palette != palette || self.font_offset != font_offset {
            self.palette = palette;
            self.font_offset = font_offset;
            self.reveal_selection();
            cx.notify();
        }
    }
    fn reveal_selection(&self) {
        self.scroll
            .scroll_to_item(self.results.selected, ScrollStrategy::Nearest);
    }
    fn move_selection(&mut self, direction: isize, cx: &mut Context<Self>) {
        self.results.navigate(direction);
        self.reveal_selection();
        cx.stop_propagation();
        cx.notify();
    }
    fn render_command(&self, index: usize, row_height: f32, cx: &mut Context<Self>) -> AnyElement {
        let p = self.palette;
        let offset = self.font_offset;
        let entry = self.results.entries[self.results.matches[index]].clone();
        let selected = index == self.results.selected;
        let enabled = entry.disabled.is_none();
        let description = entry.disabled.as_ref().unwrap_or(&entry.detail).clone();
        div()
            .id(("palette-command", index))
            .role(Role::ListBoxOption)
            .aria_label(entry.label.clone())
            .aria_description(if entry.active {
                format!("Actif. {description}")
            } else {
                description.clone()
            })
            .aria_selected(selected)
            .aria_position_in_set(index + 1)
            .aria_size_of_set(self.results.matches.len())
            .when(selected, |row| row.aria_active_descendant())
            .a11y_synthetic_children(move |tree| {
                if !enabled {
                    tree.parent_node().set_disabled();
                }
            })
            .h(px(row_height))
            .w_full()
            .px(px(16.))
            .flex()
            .items_center()
            .gap(px(10.))
            .bg(rgb(if selected { p.selection } else { p.panel }))
            .when(enabled, |row| {
                row.cursor_pointer()
                    .hover(move |style| style.bg(rgb(p.hover)))
            })
            .text_color(rgb(if enabled { p.text } else { p.muted }))
            .child(icon(
                if entry.active {
                    IconName::Check
                } else {
                    IconName::ChevronRight
                },
                14.,
                if entry.active { p.accent } else { p.muted },
            ))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .truncate()
                            .text_size(px(12. + offset))
                            .child(entry.label.clone()),
                    )
                    .child(
                        div()
                            .truncate()
                            .text_size(px(11. + offset))
                            .text_color(rgb(p.muted))
                            .child(if description.is_empty() {
                                entry.category.to_owned()
                            } else {
                                description
                            }),
                    ),
            )
            .children(
                entry
                    .shortcut
                    .clone()
                    .map(|shortcut| key_hint(shortcut, p, offset)),
            )
            .on_click(cx.listener(move |view, _, _, cx| {
                view.results.selected = index;
                if enabled {
                    cx.emit(PaletteEvent::Execute(Box::new(entry.clone())));
                }
                cx.stop_propagation();
                cx.notify();
            }))
            .into_any_element()
    }
    fn execute(&self, cx: &mut Context<Self>) {
        if let Some(entry) = self
            .results
            .selected()
            .filter(|entry| entry.disabled.is_none())
        {
            cx.emit(PaletteEvent::Execute(Box::new(entry.clone())));
        }
    }
    fn key_down(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = &event.keystroke;
        if key.modifiers.platform || key.modifiers.control || key.modifiers.alt {
            return;
        }
        match key.key.as_str() {
            "up" => self.results.navigate(-1),
            "down" => self.results.navigate(1),
            "enter" => self.execute(cx),
            "escape" => cx.emit(PaletteEvent::Close),
            "tab" => {
                // Keep keyboard focus in the search field; results use an
                // active descendant rather than competing tab stops.
                self.input.update(cx, |input, cx| input.focus(window, cx));
            }
            _ => return,
        }
        self.reveal_selection();
        cx.stop_propagation();
        cx.notify();
    }
}
impl Render for CommandPalette {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = self.palette;
        let offset = self.font_offset;
        let size = window.viewport_size();
        let height = (f32::from(size.height) - 155.).clamp(64., 390.);
        let row_height = 48. + offset.max(0.) * 2.;
        div()
            .id("command-palette-overlay")
            .absolute()
            .inset_0()
            .occlude()
            .capture_key_down(cx.listener(Self::key_down))
            .capture_action(cx.listener(|view, _: &MoveUp, _, cx| view.move_selection(-1, cx)))
            .capture_action(cx.listener(|view, _: &MoveDown, _, cx| view.move_selection(1, cx)))
            .capture_action(cx.listener(|view, _: &Enter, _, cx| {
                view.execute(cx);
                cx.stop_propagation();
            }))
            .capture_action(cx.listener(|_, _: &InputEscape, _, cx| {
                cx.emit(PaletteEvent::Close);
                cx.stop_propagation();
            }))
            .child(
                div()
                    .id("command-palette-backdrop")
                    .absolute()
                    .inset_0()
                    .bg(rgb(0).opacity(0.58))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|_, _, _, cx| {
                            cx.emit(PaletteEvent::Close);
                            cx.stop_propagation();
                        }),
                    ),
            )
            .child(
                div()
                    .id("command-palette")
                    .role(Role::Dialog)
                    .aria_label("Palette de commandes")
                    .a11y_synthetic_children(|tree| tree.parent_node().set_modal())
                    .relative()
                    .mt(px((f32::from(size.height) * 0.10).clamp(12., 64.)))
                    .mx_auto()
                    .w(px((f32::from(size.width) - 32.).clamp(0., 640.)))
                    .max_h(px(f32::from(size.height) - 24.))
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .occlude()
                    .rounded(RADIUS_LG)
                    .border_1()
                    .border_color(rgb(p.border))
                    .bg(rgb(p.panel))
                    .shadow_lg()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(12.))
                            .px(px(16.))
                            .py(px(12.))
                            .border_b_1()
                            .border_color(rgb(p.accent))
                            .child(icon(IconName::Search, 18., p.muted))
                            .child(
                                Input::new(&self.input)
                                    .id("command-palette-search")
                                    .aria_label("Rechercher une commande")
                                    .appearance(false)
                                    .bordered(false)
                                    .focus_bordered(false)
                                    .flex_1()
                                    .min_w_0()
                                    .text_size(px(14. + offset)),
                            )
                            .child(key_hint("Échap", p, offset)),
                    )
                    .child(
                        div()
                            .id("command-palette-results")
                            .role(Role::ListBox)
                            .aria_label("Commandes")
                            .relative()
                            .h(px(height.min(
                                (self.results.matches.len().max(1) as f32) * row_height,
                            )))
                            .min_h_0()
                            .flex()
                            .flex_col()
                            .when(self.results.matches.is_empty(), |view| {
                                view.child(
                                    div()
                                        .p(px(20.))
                                        .text_size(px(13. + offset))
                                        .text_color(rgb(p.muted))
                                        .child("Aucune commande trouvée"),
                                )
                            })
                            .when(!self.results.matches.is_empty(), |view| {
                                view.child(
                                    uniform_list(
                                        "command-palette-list",
                                        self.results.matches.len(),
                                        cx.processor(
                                            move |view, range: std::ops::Range<usize>, _, cx| {
                                                range
                                                    .map(|index| {
                                                        view.render_command(index, row_height, cx)
                                                    })
                                                    .collect()
                                            },
                                        ),
                                    )
                                    .flex_1()
                                    .min_h_0()
                                    .track_scroll(&self.scroll),
                                )
                                .child(scrollbar_theme(
                                    Scrollbar::vertical(&self.scroll)
                                        .id("command-palette-scrollbar"),
                                ))
                            }),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(16.))
                            .px(px(16.))
                            .py(px(8.))
                            .border_t_1()
                            .border_color(rgb(p.border))
                            .text_size(px(11. + offset))
                            .text_color(rgb(p.muted))
                            .children([("↑↓", "Naviguer"), ("Entrée", "Exécuter")].map(
                                |(key, action)| {
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(6.))
                                        .child(key_hint(key, p, offset))
                                        .child(action)
                                },
                            )),
                    ),
            )
    }
}

/// Keyboard key or shortcut, as built by `format::shortcut_label`.
pub fn key_hint(label: impl Into<SharedString>, p: Palette, offset: f32) -> Div {
    div()
        .flex_shrink_0()
        .px(px(5.))
        .rounded(RADIUS_SM)
        .border_1()
        .border_color(rgb(p.border))
        .text_size(px(11. + offset))
        .text_color(rgb(p.muted))
        .child(label.into())
}

#[cfg(test)]
mod tests {
    use super::{Command, CommandEntry, Results};
    fn entry(label: &str, keywords: &str) -> CommandEntry {
        CommandEntry::new(Command::OpenRecent(label.into()), label, "Dépôts", keywords)
    }
    #[test]
    fn partial_search_folds_case_and_precomposed_or_combining_accents() {
        let mut results = Results {
            entries: vec![
                entry("Paramètres généraux", "settings"),
                entry("Thème Ardoise", "theme"),
            ],
            ..Default::default()
        };
        for query in ["PARAMETRES", "parame\u{301}tres", " settings "] {
            results.filter(query);
            assert_eq!(results.matches, [0]);
        }
        results.filter("THEME ardo");
        assert_eq!(results.matches, [1]);
    }
    #[test]
    fn ranks_labels_before_aliases_and_requires_every_word() {
        let mut results = Results {
            entries: vec![
                entry("Autre action", "theme"),
                entry("Choisir le thème", ""),
                entry("Thème", ""),
            ],
            ..Default::default()
        };
        results.filter("theme");
        assert_eq!(results.matches, [2, 1, 0]);
        results.filter("theme absent");
        assert!(results.selected().is_none());
        results.navigate(1);
        assert!(results.selected().is_none());
    }
    #[test]
    fn filtering_and_context_updates_keep_selection_valid() {
        let entries = vec![entry("Premier", ""), entry("Second", "")];
        let mut results = Results::default();
        results.update(entries.clone(), "");
        results.navigate(20);
        assert_eq!(results.selected().unwrap().label, "Second");
        results.update(vec![entries[1].clone()], "");
        assert_eq!(results.selected, 0);
        results.filter("premier");
        assert!(results.selected().is_none());
        results.update(entries, "premier");
        assert_eq!(results.selected().unwrap().label, "Premier");
        results.filter("");
        results.navigate(-20);
        assert_eq!(results.selected, 0);
    }
    #[test]
    fn rejects_queued_commands_after_context_change_or_disable() {
        let original = entry("Diff", "").scoped([1, 2, 3]);
        assert!(original.is_current(std::slice::from_ref(&original)));
        for scope in [[2, 2, 3], [1, 3, 3], [1, 2, 4]] {
            assert!(!original.is_current(&[original.clone().scoped(scope)]));
        }
        let disabled = original.clone().unavailable(Some("Chargement"));
        assert!(!original.is_current(std::slice::from_ref(&disabled)));
        assert!(!disabled.is_current(std::slice::from_ref(&disabled)));
        assert!(!original.is_current(&[]));
    }
}
