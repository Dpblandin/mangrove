//! Application chrome. The parent owns repositories, tasks and tab workspaces.
use crate::{
    preferences::{Pane, Preferences, RecentRepository},
    recent_repositories::{RecentMetadata, RecentNamesRequest},
    settings::{SettingsEvent, SettingsView},
    theme::{Palette, ThemeId, icon, mix, scrollbar_theme},
};
use gpui_kit::{
    assets::IconName,
    base::Button as NativeButton,
    component::{
        Disableable, IndexPath, Sizable,
        button::*,
        input::{Input, InputEvent, InputState},
        scroll::Scrollbar,
        searchable_list::SearchableVec,
        select::{Select, SelectEvent, SelectState},
        tooltip::Tooltip,
    },
    prelude::*,
    *,
};
use std::{collections::HashMap, time::Duration};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChromeTab {
    pub id: u64,
    pub name: String,
    pub path: Option<String>,
}

#[derive(Clone, Debug)]
pub enum ChromeEvent {
    OpenRepository(String),
    PickRepository,
    ActivateTab(u64),
    CloseTab(u64),
    NewTab,
    ShowSettings,
    BackToWorkspace,
    Fetch,
    DismissError,
    PreferencesChanged,
    PaneResized(Pane, f32),
}

#[derive(Clone)]
struct PaneDrag {
    pane: Pane,
}
struct DragGhost;
impl Render for DragGhost {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().size(px(0.))
    }
}

struct Resize {
    pane: Pane,
    x: Pixels,
    before: (f32, f32),
    width: f32,
    available: f32,
}

pub struct Chrome {
    pub preferences: Preferences,
    recent_metadata: RecentMetadata,
    applied_theme: ThemeId,
    pub settings: Entity<SettingsView>,
    tabs: Vec<ChromeTab>,
    active_tab: Option<u64>,
    tab_focus: HashMap<u64, FocusHandle>,
    name: Option<String>,
    has_remotes: bool,
    busy: bool,
    busy_visible: bool,
    busy_task: Option<Task<()>>,
    choosing: bool,
    fetching: bool,
    error: Option<String>,
    recent_filter: Entity<InputState>,
    welcome_scroll: ScrollHandle,
    theme_select: Entity<SelectState<SearchableVec<&'static str>>>,
    separator_focus: [FocusHandle; 2],
    resize: Option<Resize>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ChromeEvent> for Chrome {}

impl Chrome {
    pub fn new(preferences: Preferences, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let recent_filter =
            cx.new(|cx| InputState::new(window, cx).placeholder("Rechercher un dépôt récent…"));
        let initial_theme = ThemeId::ALL
            .iter()
            .position(|id| *id == preferences.theme)
            .unwrap_or(0);
        let theme_select = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(ThemeId::ALL.iter().map(|id| id.label()).collect::<Vec<_>>()),
                Some(IndexPath::new(initial_theme)),
                window,
                cx,
            )
        });
        let settings = cx.new(|cx| {
            SettingsView::new(
                preferences.auto_fetch_interval,
                preferences.palette(),
                preferences.font_size_offset,
                window,
                cx,
            )
        });
        let subscriptions = vec![
            cx.subscribe(&recent_filter, |_, _, _: &InputEvent, cx| cx.notify()),
            cx.subscribe(&theme_select, |this, _, event, cx| {
                if let SelectEvent::Confirm(Some(label)) = event
                    && let Some(theme) = ThemeId::ALL.iter().find(|id| id.label() == *label)
                {
                    this.preferences.theme = *theme;
                    this.preferences_changed(cx);
                }
            }),
            cx.subscribe(&settings, |this, _, event, cx| match event {
                SettingsEvent::Back => cx.emit(ChromeEvent::BackToWorkspace),
                SettingsEvent::IntervalChanged(value) => {
                    this.preferences.auto_fetch_interval = *value;
                    this.preferences_changed(cx);
                }
            }),
        ];
        let applied_theme = preferences.theme;
        Self {
            preferences,
            recent_metadata: RecentMetadata::default(),
            applied_theme,
            settings,
            tabs: Vec::new(),
            active_tab: None,
            tab_focus: HashMap::new(),
            name: None,
            has_remotes: false,
            busy: false,
            busy_visible: false,
            busy_task: None,
            choosing: false,
            fetching: false,
            error: None,
            recent_filter,
            welcome_scroll: ScrollHandle::default(),
            theme_select,
            separator_focus: [cx.focus_handle(), cx.focus_handle()],
            resize: None,
            _subscriptions: subscriptions,
        }
    }
    pub fn set_tabs(&mut self, tabs: Vec<ChromeTab>, active: Option<u64>, cx: &mut Context<Self>) {
        if self.tabs == tabs && self.active_tab == active {
            return;
        }
        self.tab_focus
            .retain(|id, _| tabs.iter().any(|tab| tab.id == *id));
        for tab in &tabs {
            self.tab_focus
                .entry(tab.id)
                .or_insert_with(|| cx.focus_handle());
        }
        self.tabs = tabs;
        self.active_tab = active;
        cx.notify();
    }
    pub fn set_context(
        &mut self,
        name: Option<String>,
        has_remotes: bool,
        busy: bool,
        error: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if self.name == name
            && self.has_remotes == has_remotes
            && self.busy == busy
            && self.error == error
        {
            return;
        }
        if self.busy != busy {
            self.busy_visible = false;
            self.busy_task = None;
            if busy {
                self.busy_task = Some(cx.spawn(async move |this, cx| {
                    cx.background_executor()
                        .timer(Duration::from_millis(500))
                        .await;
                    let _ = this.update(cx, |this, cx| {
                        if this.busy {
                            this.busy_visible = true;
                            cx.notify();
                        }
                    });
                }));
            }
        }
        self.name = name;
        self.has_remotes = has_remotes;
        self.busy = busy;
        self.error = error;
        cx.notify();
    }
    pub fn set_fetching(&mut self, fetching: bool, cx: &mut Context<Self>) {
        if self.fetching == fetching {
            return;
        }
        self.fetching = fetching;
        cx.notify();
    }
    pub fn remember_repository(&mut self, path: String, name: String, cx: &mut Context<Self>) {
        self.recent_metadata.invalidate(&path);
        if self
            .preferences
            .recent
            .first()
            .is_some_and(|entry| entry.path == path && entry.name == name)
        {
            return;
        }
        self.preferences
            .remember_repository(RecentRepository { path, name });
        self.preferences_changed(cx);
    }
    pub fn begin_recent_metadata(&mut self) -> Option<RecentNamesRequest> {
        self.recent_metadata.begin(&self.preferences.recent)
    }
    pub fn apply_recent_metadata(
        &mut self,
        request: RecentNamesRequest,
        names: &[mangrove_core::model::RepositoryName],
        cx: &mut Context<Self>,
    ) {
        if self
            .recent_metadata
            .apply(request, &mut self.preferences.recent, names)
        {
            let _ = self.preferences.save();
            cx.notify();
        }
    }
    fn remove_recent(&mut self, path: &str, cx: &mut Context<Self>) {
        self.recent_metadata.invalidate(path);
        self.preferences.remove_recent(path);
        self.preferences_changed(cx);
    }
    pub fn begin_repository_picker(&mut self, cx: &mut Context<Self>) -> bool {
        if self.busy || self.choosing || self.resize.is_some() {
            return false;
        }
        self.choosing = true;
        cx.notify();
        true
    }
    pub fn choosing_repository(&self) -> bool {
        self.choosing
    }
    pub fn finish_repository_picker(&mut self, cx: &mut Context<Self>) {
        self.choosing = false;
        cx.notify();
    }
    pub fn prefs_changed(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.preferences_changed(cx);
    }
    pub fn zoom(&mut self, delta: Option<i32>, cx: &mut Context<Self>) {
        self.preferences.font_shortcut(delta);
        self.preferences_changed(cx);
    }
    fn preferences_changed(&mut self, cx: &mut Context<Self>) {
        if self.applied_theme != self.preferences.theme {
            self.preferences.apply_palette(cx);
            self.applied_theme = self.preferences.theme;
        }
        let _ = self.preferences.save();
        self.settings.update(cx, |settings, cx| {
            settings.set_theme(
                self.preferences.palette(),
                self.preferences.font_size_offset,
                cx,
            )
        });
        cx.emit(ChromeEvent::PreferencesChanged);
        cx.notify();
    }
    fn tab_key(
        &mut self,
        id: u64,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let key = &event.keystroke;
        if key.modifiers.alt || key.modifiers.control || key.modifiers.platform {
            return;
        }
        let Some(index) = self.tabs.iter().position(|tab| tab.id == id) else {
            return;
        };
        let target = match key.key.as_str() {
            "left" => (index + self.tabs.len() - 1) % self.tabs.len(),
            "right" => (index + 1) % self.tabs.len(),
            "home" => 0,
            "end" => self.tabs.len() - 1,
            _ => return,
        };
        let id = self.tabs[target].id;
        if let Some(focus) = self.tab_focus.get(&id) {
            window.focus(focus, cx);
        }
        cx.emit(ChromeEvent::ActivateTab(id));
        cx.stop_propagation();
    }
    pub fn render_tabs(&mut self, _: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let p = self.preferences.palette();
        let offset = self.preferences.font_size_offset;
        div()
            .id("repository-tabs")
            .flex()
            .flex_none()
            .min_h(px(38.))
            .h(px(38.))
            .px(px(12.))
            .bg(rgb(p.bg))
            .border_b_1()
            .border_color(rgb(p.border))
            .child(
                div()
                    .id("tab-strip")
                    .flex()
                    .min_w_0()
                    .overflow_x_scroll()
                    .children(self.tabs.iter().map(|tab| {
                        let id = tab.id;
                        let active = Some(id) == self.active_tab;
                        div()
                            .id(("repository-tab", id))
                            .flex()
                            .flex_none()
                            .items_center()
                            .min_w(px(100.))
                            .max_w(px(260.))
                            .relative()
                            .border_r_1()
                            .border_color(rgb(p.border))
                            .bg(rgb(if active { p.surface } else { p.bg }))
                            .text_color(rgb(if active { p.text } else { p.muted }))
                            .hover(move |view| {
                                view.bg(rgb(mix(
                                    p.text,
                                    if active { p.surface } else { p.bg },
                                    0.06,
                                )))
                                .text_color(rgb(p.text))
                            })
                            .when(active, |view| {
                                view.child(
                                    div()
                                        .absolute()
                                        .top_0()
                                        .left_0()
                                        .right_0()
                                        .h(px(2.))
                                        .bg(rgb(p.accent)),
                                )
                            })
                            .child(
                                div()
                                    .id(("tab-select", id))
                                    .track_focus(&self.tab_focus[&id])
                                    .flex()
                                    .flex_1()
                                    .min_w_0()
                                    .items_center()
                                    .gap(px(7.))
                                    .pl(px(12.))
                                    .pr(px(8.))
                                    .h_full()
                                    .text_size(px(12. + offset))
                                    .font_weight(FontWeight::MEDIUM)
                                    .cursor_pointer()
                                    .child(icon(
                                        if tab.path.is_some() {
                                            IconName::GitBranch
                                        } else {
                                            IconName::FolderOpen
                                        },
                                        14.,
                                        if active { p.text } else { p.muted },
                                    ))
                                    .child(div().truncate().child(tab.name.clone()))
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        cx.emit(ChromeEvent::ActivateTab(id))
                                    }))
                                    .on_key_down(cx.listener(move |this, event, window, cx| {
                                        this.tab_key(id, event, window, cx)
                                    })),
                            )
                            .child(
                                Button::new(("tab-close", id))
                                    .ghost()
                                    .w(px(24.))
                                    .h(px(26.))
                                    .mr(px(5.))
                                    .rounded(px(3.))
                                    .icon(icon(IconName::X, 13., p.muted))
                                    .accessibility_label(format!("Fermer {}", tab.name))
                                    .tooltip(format!("Fermer {}", tab.name))
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        cx.stop_propagation();
                                        cx.emit(ChromeEvent::CloseTab(id));
                                    })),
                            )
                    })),
            )
            .child(
                Button::new("tab-new")
                    .ghost()
                    .w(px(32.))
                    .h(px(30.))
                    .ml(px(5.))
                    .mt(px(4.))
                    .rounded(px(3.))
                    .icon(icon(IconName::Plus, 17., p.muted))
                    .accessibility_label("Nouvel onglet")
                    .tooltip("Nouvel onglet")
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(ChromeEvent::NewTab))),
            )
            .into_any_element()
    }
    pub fn render_topbar(&mut self, _: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let p = self.preferences.palette();
        let offset = self.preferences.font_size_offset;
        let name = self
            .name
            .clone()
            .or_else(|| {
                self.busy
                    .then(|| {
                        self.tabs
                            .iter()
                            .find(|tab| Some(tab.id) == self.active_tab)
                            .map(|tab| tab.name.clone())
                    })
                    .flatten()
            })
            .unwrap_or_else(|| "Bienvenue".into());
        div()
            .h(px(59.))
            .flex_none()
            .flex()
            .items_center()
            .justify_between()
            .px(px(23.))
            .border_b_1()
            .border_color(rgb(p.border))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .min_w_0()
                    .text_size(px(11. + offset))
                    .text_color(rgb(p.muted))
                    .child("Espace local")
                    .child(icon(IconName::ChevronRight, 12., p.muted))
                    .child(
                        div()
                            .truncate()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(rgb(p.text))
                            .child(name),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(22.))
                    .when(self.busy_visible, |view| {
                        view.child(icon(IconName::LoaderCircle, 16., p.accent))
                    })
                    .child(
                        Select::new(&self.theme_select)
                            .w(px(158.))
                            .min_w(px(158.))
                            .h(px(29.))
                            .max_w(px(190.))
                            .px(px(8.))
                            .py(px(4.))
                            .border_1()
                            .border_color(rgb(p.border))
                            .rounded(px(5.))
                            .bg(rgb(p.surface))
                            .text_color(rgb(p.text))
                            .text_size(px(11. + offset))
                            .font_weight(FontWeight::NORMAL)
                            .accessibility_label("Thème de l’interface"),
                    )
                    .when(self.has_remotes, |view| {
                        view.child(
                            Button::new("fetch")
                                .ghost()
                                .w(px(28.))
                                .h(px(28.))
                                .icon(icon(IconName::Download, 17., p.muted))
                                .disabled(self.busy || self.fetching)
                                .accessibility_label("Récupérer les références distantes")
                                .tooltip(if self.fetching {
                                    "Fetch en cours…"
                                } else {
                                    "Récupérer les références de tous les remotes"
                                })
                                .on_click(cx.listener(|_, _, _, cx| cx.emit(ChromeEvent::Fetch))),
                        )
                    })
                    .child(
                        Button::new("settings")
                            .ghost()
                            .w(px(28.))
                            .h(px(28.))
                            .icon(icon(IconName::Settings, 17., p.muted))
                            .accessibility_label("Paramètres")
                            .tooltip("Paramètres · ⌘ ,")
                            .on_click(cx.listener(|this, _, _, cx| {
                                if !this.choosing && this.resize.is_none() {
                                    cx.emit(ChromeEvent::ShowSettings)
                                }
                            })),
                    )
                    .child(
                        Button::new("open-repository")
                            .ghost()
                            .w(px(28.))
                            .h(px(28.))
                            .icon(icon(IconName::FolderOpen, 17., p.muted))
                            .disabled(self.busy || self.choosing)
                            .accessibility_label("Ouvrir un dépôt")
                            .tooltip("Ouvrir un dépôt · ⌘ O")
                            .on_click(
                                cx.listener(|_, _, _, cx| cx.emit(ChromeEvent::PickRepository)),
                            ),
                    ),
            )
            .into_any_element()
    }
    pub fn render_empty_sidebar(&self) -> AnyElement {
        let p = self.preferences.palette();
        let offset = self.preferences.font_size_offset;
        div()
            .flex()
            .flex_col()
            .text_color(rgb(p.muted))
            .child(
                div()
                    .px(px(22.))
                    .pt(px(16.))
                    .pb(px(12.))
                    .text_size(px(9. + offset))
                    .child("ESPACE DE TRAVAIL"),
            )
            .child(
                div()
                    .mt(px(20.))
                    .flex()
                    .flex_col()
                    .items_center()
                    .text_center()
                    .child(
                        div()
                            .mx(px(20.))
                            .self_stretch()
                            .h(px(1.))
                            .mb(px(52.))
                            .bg(rgb(p.border)),
                    )
                    .child(icon(IconName::GitBranch, 22., p.muted))
                    .child(
                        div()
                            .mt(px(12.))
                            .text_size(px(11. + offset))
                            .line_height(px((11. + offset) * 1.8))
                            .child("Les branches et les tags\nde votre dépôt apparaîtront ici."),
                    ),
            )
            .into_any_element()
    }
    pub fn render_sidebar_footer(&self) -> AnyElement {
        let p = self.preferences.palette();
        let offset = self.preferences.font_size_offset;
        div()
            .mt_auto()
            .h(px(68.))
            .flex_none()
            .flex()
            .items_center()
            .gap(px(10.))
            .px(px(if self.preferences.left_width < 210. {
                14.
            } else {
                20.
            }))
            .border_t_1()
            .border_color(rgb(p.border))
            .child(
                div()
                    .size(px(29.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(7.))
                    .border_1()
                    .border_color(rgb(p.border))
                    .bg(rgb(p.surface))
                    .child(icon(IconName::FolderGit2, 17., p.accent)),
            )
            .child(
                div()
                    .flex_1()
                    .text_size(px(11. + offset))
                    .child("Espace local")
                    .child(
                        div()
                            .mt(px(4.))
                            .text_size(px(9. + offset))
                            .text_color(rgb(p.muted))
                            .child(format!("Version {}", env!("CARGO_PKG_VERSION"))),
                    ),
            )
            .into_any_element()
    }
    pub fn render_welcome(&mut self, _: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let p = self.preferences.palette();
        let offset = self.preferences.font_size_offset;
        let query = self.recent_filter.read(cx).value().trim().to_lowercase();
        let recent_focus = self.recent_filter.read(cx).focus_handle(cx);
        let recent: Vec<_> = self
            .preferences
            .recent
            .iter()
            .filter(|entry| {
                format!("{} {}", entry.name, entry.path)
                    .to_lowercase()
                    .contains(&query)
            })
            .cloned()
            .collect();
        let content = div()
            .id("welcome-content")
            .w_full()
            .h_auto()
            .min_h_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .pt(px(35.))
            .px(px(40.))
            .pb(px(25.))
            .bg(rgb(p.bg))
            .text_center()
            .child(
                div()
                    .p(px(13.))
                    .rounded(px(24.))
                    .border_1()
                    .border_color(rgb(p.accent_border))
                    .bg(rgb(p.accent_soft))
                    .mb(px(24.))
                    .child(logo()),
            )
            .child(
                div()
                    .mb(px(28.))
                    .text_size(px(30. + offset))
                    .font_weight(FontWeight(530.))
                    .line_height(px((30. + offset) * 1.12))
                    .child("Mangrove"),
            )
            .child(
                NativeButton::new("welcome-open")
                    .px(px(17.))
                    .py(px(12.))
                    .border_1()
                    .border_color(rgb(p.accent_border))
                    .bg(rgb(p.accent))
                    .text_color(rgb(p.on_accent))
                    .rounded(px(7.))
                    .text_size(px(12. + offset))
                    .font_weight(FontWeight::SEMIBOLD)
                    .line_height(relative(1.5))
                    .gap(px(9.))
                    .cursor_pointer()
                    .accessibility_label("Ouvrir un dépôt local")
                    .hover(move |style| style.bg(rgb(mix(p.accent, p.text, 0.85))))
                    .focus_visible(move |style| style.border_color(rgb(p.accent)))
                    .styles(|styles| styles.disabled(|style| style.opacity(0.5)))
                    .child(icon(IconName::FolderOpen, 17., p.on_accent))
                    .child("Ouvrir un dépôt local")
                    .child(
                        div()
                            .ml(px(14.))
                            .child(icon(IconName::ArrowRight, 15., p.on_accent)),
                    )
                    .disabled(self.busy || self.choosing)
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(ChromeEvent::PickRepository))),
            )
            .child(
                div()
                    .mt(px(15.))
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .text_size(px(9. + offset))
                    .text_color(rgb(p.muted))
                    .child("ou")
                    .child(kbd(
                        if cfg!(target_os = "macos") {
                            "⌘ O"
                        } else {
                            "Ctrl O"
                        },
                        p,
                        offset,
                    )),
            )
            .when_some(self.error.clone(), |view, error| {
                view.child(
                    div()
                        .mt(px(15.))
                        .max_w(px(450.))
                        .text_size(px(11. + offset))
                        .text_color(rgb(p.danger))
                        .child(error),
                )
            })
            .when(!self.preferences.recent.is_empty(), |view| {
                view.child(
                    div()
                        .w(px(340.))
                        .max_w_full()
                        .mt(px(28.))
                        .text_left()
                        .child(
                            div()
                                .mb(px(8.))
                                .text_size(px(9. + offset))
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(rgb(p.muted))
                                .child("Récemment ouverts"),
                        )
                        .child(
                            div()
                                .id("recent-search")
                                .track_focus(&recent_focus)
                                .flex()
                                .items_center()
                                .gap(px(8.))
                                .min_h(px(36.))
                                .mb(px(8.))
                                .px(px(10.))
                                .rounded(px(4.))
                                .border_1()
                                .border_color(rgb(p.border))
                                .bg(rgb(p.bg))
                                .text_color(rgb(p.muted))
                                .focus(move |style| style.border_color(rgb(p.accent)))
                                .child(icon(IconName::Search, 14., p.muted))
                                .child(
                                    Input::new(&self.recent_filter)
                                        .appearance(false)
                                        .bordered(false)
                                        .h_auto()
                                        .min_w_0()
                                        .flex_1()
                                        .px(px(0.))
                                        .py(px(8.))
                                        .text_size(px(12. + offset))
                                        .font_weight(FontWeight::NORMAL)
                                        .line_height(px(16. + offset * 2.))
                                        .text_color(rgb(p.text))
                                        .aria_label("Rechercher un dépôt récent"),
                                ),
                        )
                        .children(recent.iter().map(|entry| {
                            let path = entry.path.clone();
                            let remove = path.clone();
                            let tooltip = format!("{}\n{}", entry.name, entry.path);
                            div()
                                .id(SharedString::from(format!("recent:{path}")))
                                .flex()
                                .items_center()
                                .mb(px(6.))
                                .rounded(px(6.))
                                .border_1()
                                .border_color(rgb(p.border))
                                .bg(rgb(p.surface))
                                .overflow_hidden()
                                .hover(move |view| view.bg(rgb(mix(p.text, p.surface, 0.06))))
                                .child(
                                    NativeButton::new(SharedString::from(format!(
                                        "recent-open:{path}"
                                    )))
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .items_center()
                                    .gap(px(10.))
                                    .p(px(10.))
                                    .line_height(relative(1.5))
                                    .font_weight(FontWeight::NORMAL)
                                    .text_left()
                                    .cursor_pointer()
                                    .accessibility_label(format!("Ouvrir {}", entry.name))
                                    .tooltip(move |window, cx| {
                                        Tooltip::new(tooltip.clone()).build(window, cx)
                                    })
                                    .focus_visible(move |style| {
                                        style.bg(rgb(mix(p.text, p.surface, 0.06)))
                                    })
                                    .styles(|styles| styles.disabled(|style| style.opacity(0.5)))
                                    .disabled(self.busy)
                                    .child(icon(IconName::FolderGit2, 16., p.accent))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .text_size(px(11. + offset))
                                            .child(div().truncate().child(entry.name.clone()))
                                            .child(
                                                div()
                                                    .mt(px(4.))
                                                    .max_w(px(270.))
                                                    .truncate()
                                                    .text_size(px(9. + offset))
                                                    .text_color(rgb(p.muted))
                                                    .child(path.clone()),
                                            ),
                                    )
                                    .child(icon(IconName::ChevronRight, 14., p.accent))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if !this.busy {
                                            cx.emit(ChromeEvent::OpenRepository(path.clone()))
                                        }
                                    })),
                                )
                                .child(
                                    NativeButton::new(SharedString::from(format!(
                                        "recent-remove:{remove}"
                                    )))
                                    .w(px(28.))
                                    .h(px(28.))
                                    .mr(px(6.))
                                    .rounded(px(4.))
                                    .flex_shrink_0()
                                    .text_color(rgb(p.muted))
                                    .cursor_pointer()
                                    .hover(move |style| style.text_color(rgb(p.danger)))
                                    .focus_visible(move |style| {
                                        style.border_1().border_color(rgb(p.accent))
                                    })
                                    .styles(|styles| styles.disabled(|style| style.opacity(0.5)))
                                    .child(
                                        gpui_kit::component::Icon::new(IconName::X)
                                            .with_size(px(14.)),
                                    )
                                    .tooltip(|window, cx| {
                                        Tooltip::new("Retirer des dépôts récents").build(window, cx)
                                    })
                                    .accessibility_label(format!(
                                        "Retirer {} des dépôts récents",
                                        entry.name
                                    ))
                                    .disabled(self.busy)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.remove_recent(&remove, cx)
                                    })),
                                )
                        }))
                        .when(recent.is_empty(), |view| {
                            view.child(
                                div()
                                    .py(px(8.))
                                    .text_size(px(12. + offset))
                                    .text_color(rgb(p.muted))
                                    .child("Aucun dépôt récent trouvé."),
                            )
                        }),
                )
            });
        div()
            .id("welcome")
            .relative()
            .flex_1()
            .min_h_0()
            .overflow_hidden()
            .child(
                div()
                    .id("welcome-scroll-area")
                    .size_full()
                    .flex()
                    .flex_col()
                    .overflow_y_scroll()
                    .track_scroll(&self.welcome_scroll)
                    .child(content),
            )
            .child(scrollbar_theme(
                Scrollbar::vertical(&self.welcome_scroll).id("welcome-scrollbar"),
            ))
            .into_any_element()
    }
    /// Place in the relative workspace content, beside history and inspector.
    pub fn render_error(
        &mut self,
        details_width: f32,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if self.name.is_none() || self.busy {
            return None;
        }
        let error = self.error.clone()?;
        let p = self.preferences.palette();
        Some(
            div()
                .id("workspace-error")
                .absolute()
                .bottom(px(43.))
                .left(px(20.))
                .right(px(details_width + 20.))
                .flex()
                .items_center()
                .justify_between()
                .gap(px(15.))
                .text_size(px(11. + self.preferences.font_size_offset))
                .line_height(px(18.))
                .px(px(12.))
                .py(px(8.))
                .rounded(px(6.))
                .border_1()
                .border_color(rgb(mix(p.danger, p.border, 0.45)))
                .bg(rgb(mix(p.danger, p.bg, 0.10)))
                .text_color(rgb(p.danger))
                .shadow(vec![BoxShadow {
                    color: rgba(0x00000055).into(),
                    offset: point(px(0.), px(6.)),
                    blur_radius: px(24.),
                    spread_radius: px(0.),
                    inset: false,
                }])
                .child(div().flex_1().min_w_0().child(error))
                .child(
                    Button::new("dismiss-workspace-error")
                        .ghost()
                        .w(px(28.))
                        .h(px(28.))
                        .icon(icon(IconName::X, 14., p.muted))
                        .accessibility_label("Fermer le message")
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(ChromeEvent::DismissError))),
                )
                .into_any_element(),
        )
    }
    pub fn render_divider(
        &mut self,
        pane: Pane,
        available: f32,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = self.preferences.palette();
        let ix = if pane == Pane::Left { 0 } else { 1 };
        let layout = self.preferences.pane_layout(available);
        let width = if pane == Pane::Left {
            layout.left
        } else {
            layout.right
        };
        let dragging = self
            .resize
            .as_ref()
            .is_some_and(|resize| resize.pane == pane);
        let drag_view = cx.entity();
        div()
            .id(if pane == Pane::Left {
                "left-divider"
            } else {
                "right-divider"
            })
            .track_focus(&self.separator_focus[ix])
            .w(px(6.))
            .flex_none()
            .h_full()
            .relative()
            .occlude()
            .cursor(CursorStyle::ResizeLeftRight)
            .bg(rgb(p.bg))
            .child(
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .left(px(2.5))
                    .w(px(1.))
                    .bg(rgb(if dragging { p.accent } else { p.border })),
            )
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |this, event: &MouseDownEvent, window, cx| {
                    window.focus(&this.separator_focus[ix], cx);
                    if event.click_count == 2 {
                        this.preferences.reset_pane(pane);
                        this.preferences_changed(cx);
                        cx.emit(ChromeEvent::PaneResized(
                            pane,
                            if pane == Pane::Left { 226. } else { 390. },
                        ));
                    } else {
                        this.resize = Some(Resize {
                            pane,
                            x: event.position.x,
                            before: (this.preferences.left_width, this.preferences.right_width),
                            width,
                            available,
                        });
                    }
                }),
            )
            .on_drag(PaneDrag { pane }, move |_, _, window, cx| {
                // GPUI starts the drag in bubble phase, after on_drag_move's
                // capture listener. Apply its first movement here as well.
                let x = window.mouse_position().x;
                drag_view.update(cx, |this, cx| this.move_resize(pane, x, cx));
                cx.new(|_| DragGhost)
            })
            .on_drag_move(
                cx.listener(move |this, event: &DragMoveEvent<PaneDrag>, _, cx| {
                    if event.drag(cx).pane != pane {
                        return;
                    }
                    this.move_resize(pane, event.event.position.x, cx);
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.finish_resize(false, cx)),
            )
            .on_mouse_up_out(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| this.finish_resize(false, cx)),
            )
            .on_key_down(cx.listener(move |this, event: &KeyDownEvent, window, cx| {
                let key = event.keystroke.key.as_str();
                if key == "escape" && this.resize.is_some() {
                    this.finish_resize(true, cx);
                    cx.stop_active_drag(window);
                    cx.stop_propagation();
                    return;
                }
                let layout = this.preferences.pane_layout(available);
                let step = if event.keystroke.modifiers.shift {
                    50.
                } else {
                    10.
                };
                let direction = if pane == Pane::Left { 1. } else { -1. };
                let width = if pane == Pane::Left {
                    layout.left
                } else {
                    layout.right
                };
                let next = match key {
                    "left" => width - step * direction,
                    "right" => width + step * direction,
                    "home" => {
                        if pane == Pane::Left {
                            layout.left_min
                        } else {
                            layout.right_min
                        }
                    }
                    "end" => {
                        if pane == Pane::Left {
                            layout.left_max
                        } else {
                            layout.right_max
                        }
                    }
                    _ => return,
                };
                this.preferences.resize_pane(pane, next, available);
                this.preferences_changed(cx);
                cx.emit(ChromeEvent::PaneResized(
                    pane,
                    if pane == Pane::Left {
                        this.preferences.left_width
                    } else {
                        this.preferences.right_width
                    },
                ));
                cx.stop_propagation();
            }))
            .into_any_element()
    }
    fn move_resize(&mut self, pane: Pane, x: Pixels, cx: &mut Context<Self>) {
        let Some(resize) = self.resize.as_ref().filter(|resize| resize.pane == pane) else {
            return;
        };
        let direction = if pane == Pane::Left { 1. } else { -1. };
        let next = resize.width + f32::from(x - resize.x) * direction;
        self.preferences.resize_pane(pane, next, resize.available);
        let applied = if pane == Pane::Left {
            self.preferences.left_width
        } else {
            self.preferences.right_width
        };
        cx.emit(ChromeEvent::PaneResized(pane, applied));
        cx.notify();
    }
    fn finish_resize(&mut self, cancelled: bool, cx: &mut Context<Self>) {
        let Some(resize) = self.resize.take() else {
            return;
        };
        if cancelled {
            self.preferences.left_width = resize.before.0;
            self.preferences.right_width = resize.before.1;
        }
        self.preferences_changed(cx);
        cx.emit(ChromeEvent::PaneResized(
            resize.pane,
            if resize.pane == Pane::Left {
                self.preferences.left_width
            } else {
                self.preferences.right_width
            },
        ));
    }
}

impl Render for Chrome {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.render_tabs(window, cx)
    }
}

fn kbd(label: &str, p: Palette, offset: f32) -> AnyElement {
    div()
        .border_1()
        .border_color(rgb(p.border))
        .rounded(px(4.))
        .py(px(2.))
        .px(px(5.))
        .text_size(px(9. + offset))
        .text_color(rgb(p.muted))
        .child(label.to_string())
        .into_any_element()
}

fn logo() -> AnyElement {
    canvas(
        |_, _, _| (),
        |bounds, _, window, _| {
            let unit = f32::from(bounds.size.width) / 64.;
            let position = |x, y| point(bounds.left() + px(x * unit), bounds.top() + px(y * unit));
            window.paint_quad(fill(bounds, rgb(0x173a33)).corner_radii(px(18. * unit)));
            // Separate segments plus endpoint circles reproduce the SVG's
            // round caps and joins without adding a second SVG asset source.
            let mut path = PathBuilder::stroke(px(5. * unit));
            for ((x1, y1), (x2, y2)) in [
                ((17., 46.), (17., 23.)),
                ((17., 23.), (32., 37.)),
                ((32., 37.), (47., 23.)),
                ((47., 23.), (47., 46.)),
                ((32., 37.), (32., 17.)),
            ] {
                path.move_to(position(x1, y1));
                path.line_to(position(x2, y2));
            }
            if let Ok(path) = path.build() {
                window.paint_path(path, rgb(0x78e4bb));
            }
            for (x, y) in [
                (17., 46.),
                (17., 23.),
                (32., 37.),
                (47., 23.),
                (47., 46.),
                (32., 17.),
            ] {
                window.paint_quad(
                    fill(
                        Bounds::new(
                            position(x - 2.5, y - 2.5),
                            size(px(5. * unit), px(5. * unit)),
                        ),
                        rgb(0x78e4bb),
                    )
                    .corner_radii(px(2.5 * unit)),
                );
            }
            window.paint_quad(
                fill(
                    Bounds::new(position(27., 10.), size(px(10. * unit), px(10. * unit))),
                    rgb(0x78e4bb),
                )
                .corner_radii(px(5. * unit)),
            );
        },
    )
    .size(px(56.))
    .into_any_element()
}
