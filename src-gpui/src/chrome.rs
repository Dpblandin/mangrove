//! Application chrome. The parent owns repositories, tasks and tab workspaces.
use crate::{
    format::display_path,
    integrations::{IntegrationsEvent, IntegrationsView},
    preferences::{Pane, Preferences, RecentRepository, profile_store},
    recent_repositories::{RecentMetadata, RecentNamesRequest},
    settings::{SettingsEvent, SettingsView},
    theme::{Palette, RADIUS_MD, RADIUS_SM, ThemeId, icon, mix, scrollbar_theme},
};
use gpui_kit::{
    assets::IconName,
    base::Button as NativeButton,
    component::{
        Disableable, Sizable,
        button::*,
        input::{Input, InputEvent, InputState},
        scroll::Scrollbar,
        tooltip::Tooltip,
    },
    prelude::*,
    *,
};
use mangrove_remote::{Profile, ProfileStore, Profiles};
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
    BrowseRepositories,
    CloseRepositoryBrowser,
    ManageIntegrations,
    ShowSettings,
    BackToWorkspace,
    Fetch,
    DismissError,
    PreferencesChanged,
    ProfileChanged,
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
    profiles: Profiles,
    profile_store: Option<ProfileStore>,
    integrations: Entity<IntegrationsView>,
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
    repository_browser: bool,
    separator_focus: [FocusHandle; 2],
    resize: Option<Resize>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ChromeEvent> for Chrome {}

impl Chrome {
    pub fn new(preferences: Preferences, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (profiles, profile_store, profile_error) = match profile_store()
            .and_then(|store| store.load().map(|profiles| (store, profiles)))
        {
            Ok((store, profiles)) => (profiles, Some(store), None),
            Err(error) => (Profiles::default(), None, Some(error.to_string())),
        };
        let integrations = cx.new(|cx| {
            IntegrationsView::new(
                profiles
                    .active()
                    .cloned()
                    .unwrap_or_else(|| Profile::new("personal", "Personnel")),
                preferences.palette(),
                preferences.font_size_offset,
                window,
                cx,
            )
        });
        integrations.update(cx, |view, cx| {
            view.set_profiles(profiles.profiles.clone(), window, cx);
            if let Some(error) = profile_error.clone() {
                view.set_error(error, cx);
            }
        });
        let recent_filter =
            cx.new(|cx| InputState::new(window, cx).placeholder("Rechercher un dépôt récent…"));
        let settings = cx.new(|cx| {
            SettingsView::new(
                preferences.auto_fetch_interval,
                preferences.theme,
                preferences.show_image_previews,
                preferences.palette(),
                preferences.font_size_offset,
                window,
                cx,
            )
        });
        settings.update(cx, |view, cx| {
            view.set_integrations(integrations.clone(), cx)
        });
        let subscriptions = vec![
            cx.subscribe_in(&integrations, window, |this, _, event, window, cx| {
                this.integration_event(event, window, cx)
            }),
            cx.subscribe(&recent_filter, |_, _, _: &InputEvent, cx| cx.notify()),
            cx.subscribe(&settings, |this, _, event, cx| match event {
                SettingsEvent::Back => cx.emit(ChromeEvent::BackToWorkspace),
                SettingsEvent::ThemeChanged(theme) => {
                    this.preferences.theme = *theme;
                    this.preferences_changed(cx);
                }
                SettingsEvent::IntervalChanged(value) => {
                    this.preferences.auto_fetch_interval = *value;
                    this.preferences_changed(cx);
                }
                SettingsEvent::ImagePreviewsChanged(value) => {
                    this.preferences.show_image_previews = *value;
                    this.preferences_changed(cx);
                }
            }),
        ];
        let applied_theme = preferences.theme;
        Self {
            preferences,
            profiles,
            profile_store,
            integrations,
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
            error: profile_error,
            recent_filter,
            welcome_scroll: ScrollHandle::default(),
            repository_browser: false,
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
        let update = self
            .recent_metadata
            .apply(request, &mut self.preferences.recent, names);
        if update.names_changed {
            self.save_preferences(cx);
        }
        if update.names_changed || update.locations_changed {
            cx.notify();
        }
    }
    pub fn remove_recent(&mut self, path: &str, cx: &mut Context<Self>) {
        for path in self.recent_metadata.equivalent_paths(path) {
            self.recent_metadata.invalidate(&path);
            self.preferences.remove_recent(&path);
        }
        self.preferences_changed(cx);
    }
    pub fn begin_repository_picker(&mut self, cx: &mut Context<Self>) -> bool {
        if self.busy
            || self.choosing
            || self.resize.is_some()
            || self.integrations.read(cx).choosing_path()
        {
            return false;
        }
        self.choosing = true;
        cx.notify();
        true
    }
    pub fn choosing_repository(&self) -> bool {
        self.choosing
    }
    pub fn can_show_settings(&self, cx: &App) -> bool {
        !self.choosing && self.resize.is_none() && !self.integrations.read(cx).choosing_path()
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
    pub fn select_theme(&mut self, theme: ThemeId, window: &mut Window, cx: &mut Context<Self>) {
        self.preferences.theme = theme;
        self.settings.update(cx, |settings, cx| {
            settings.set_selected_theme(theme, window, cx)
        });
        self.preferences_changed(cx);
    }
    fn preferences_changed(&mut self, cx: &mut Context<Self>) {
        if self.applied_theme != self.preferences.theme {
            self.preferences.apply_palette(cx);
            self.applied_theme = self.preferences.theme;
        }
        self.save_preferences(cx);
        self.integrations.update(cx, |view, cx| {
            view.set_theme(
                self.preferences.palette(),
                self.preferences.font_size_offset,
                cx,
            )
        });
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
    fn save_preferences(&mut self, cx: &mut Context<Self>) {
        if self
            .preferences
            .save_for_profile(&self.profiles.active_profile_id)
            .is_err()
        {
            self.integrations.update(cx, |view, cx| {
                view.set_error(
                    "Impossible d’enregistrer les préférences du profil.".into(),
                    cx,
                )
            });
        }
    }
    fn integration_event(
        &mut self,
        event: &IntegrationsEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut profiles = self.profiles.clone();
        match event {
            IntegrationsEvent::OpenIntegrationSettings => {
                cx.emit(ChromeEvent::ManageIntegrations);
                return;
            }
            IntegrationsEvent::OpenLocalFavorite(path) => {
                cx.emit(ChromeEvent::OpenRepository(path.clone()));
                return;
            }
            IntegrationsEvent::ProfileSelected(id) => {
                self.request_profile_switch(id, window, cx);
                return;
            }
            IntegrationsEvent::ProfileCreated(name) => {
                let mut index = 1;
                while profiles
                    .profiles
                    .iter()
                    .any(|profile| profile.id == format!("profile-{index}"))
                {
                    index += 1;
                }
                let profile = Profile::new(format!("profile-{index}"), name.clone());
                profiles.profiles.push(profile);
            }
            IntegrationsEvent::ProfileRenamed { profile_id, name } => {
                if profiles.rename(profile_id, name).is_err() {
                    self.profile_save_failed(
                        event,
                        "Choisissez un nom de profil de 128 caractères au plus.".into(),
                        window,
                        cx,
                    );
                    return;
                }
            }
            IntegrationsEvent::ProfileDeleted(id) => {
                if profiles.remove(id).is_err() {
                    self.profile_save_failed(
                        event,
                        "Le profil actif et le dernier profil ne peuvent pas être supprimés."
                            .into(),
                        window,
                        cx,
                    );
                    return;
                }
            }
            IntegrationsEvent::AccountAdded {
                profile_id,
                account,
            } => {
                if let Some(profile) = profiles
                    .profiles
                    .iter_mut()
                    .find(|profile| profile.id == *profile_id)
                {
                    profile
                        .accounts
                        .retain(|existing| existing.id != account.id);
                    profile.accounts.push(account.clone());
                }
            }
            IntegrationsEvent::AccountRemoved {
                profile_id,
                account_id,
            } => {
                if let Some(profile) = profiles
                    .profiles
                    .iter_mut()
                    .find(|profile| profile.id == *profile_id)
                {
                    profile.accounts.retain(|account| account.id != *account_id);
                }
            }
            IntegrationsEvent::FavoritesChanged(favorites) => {
                if let Some(profile) = profiles
                    .profiles
                    .iter_mut()
                    .find(|profile| profile.id == profiles.active_profile_id)
                {
                    profile.favorites = favorites.clone();
                }
            }
            IntegrationsEvent::SshChanged(choice) => {
                if let Some(profile) = profiles
                    .profiles
                    .iter_mut()
                    .find(|profile| profile.id == profiles.active_profile_id)
                {
                    profile.ssh = choice.clone();
                }
            }
        }
        self.commit_profiles(event, profiles, window, cx);
    }
    /// Switching profiles closes the repository tabs, so ask first.
    fn request_profile_switch(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let open = self.tabs.iter().filter(|tab| tab.path.is_some()).count();
        let Some(profile) = self
            .profiles
            .profiles
            .iter()
            .find(|profile| profile.id == id)
        else {
            return;
        };
        if open == 0 || self.profiles.active_profile_id == id {
            return self.select_profile(id, window, cx);
        }
        let detail = if open == 1 {
            "L’onglet de dépôt ouvert sera fermé.".to_string()
        } else {
            format!("Les {open} onglets de dépôts ouverts seront fermés.")
        };
        let answer = window.prompt(
            PromptLevel::Warning,
            &format!("Passer au profil « {} » ?", profile.name),
            Some(&detail),
            &[
                PromptButton::ok("Changer de profil"),
                PromptButton::cancel("Annuler"),
            ],
            cx,
        );
        let id = id.to_string();
        cx.spawn_in(window, async move |this, cx| {
            if answer.await == Ok(0) {
                let _ = cx.update(|window, cx| {
                    this.update(cx, |chrome, cx| chrome.select_profile(&id, window, cx))
                });
            }
        })
        .detach();
    }
    fn select_profile(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.profiles.active_profile_id == id {
            return;
        }
        self.save_preferences(cx);
        let mut profiles = self.profiles.clone();
        profiles.active_profile_id = id.to_string();
        self.commit_profiles(
            &IntegrationsEvent::ProfileSelected(id.to_string()),
            profiles,
            window,
            cx,
        );
    }
    fn commit_profiles(
        &mut self,
        event: &IntegrationsEvent,
        profiles: Profiles,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(store) = &self.profile_store else {
            self.profile_save_failed(event, "Les profils ne peuvent pas être enregistrés. Vérifiez leur fichier de configuration.".into(), window, cx);
            cx.notify();
            return;
        };
        if let Err(error) = store.save(&profiles) {
            self.profile_save_failed(event, error.to_string(), window, cx);
            cx.notify();
            return;
        }
        if let IntegrationsEvent::ProfileDeleted(id) = event {
            // A later profile may reuse this identifier.
            let _ = Preferences::remove_for_profile(id);
        }
        let changed_profile = profiles.active_profile_id != self.profiles.active_profile_id;
        self.profiles = profiles;
        if changed_profile {
            self.preferences = Preferences::load_for_profile(&self.profiles.active_profile_id);
            self.recent_metadata = RecentMetadata::default();
            self.preferences.apply_palette(cx);
            self.applied_theme = self.preferences.theme;
            self.settings.update(cx, |view, cx| {
                view.set_interval(self.preferences.auto_fetch_interval, window, cx);
                view.set_selected_theme(self.preferences.theme, window, cx);
            });
        }
        if let Some(profile) = self.profiles.active().cloned() {
            self.integrations.update(cx, |view, cx| {
                view.set_profiles(self.profiles.profiles.clone(), window, cx);
                view.set_profile(profile, window, cx);
            });
        }
        if changed_profile {
            self.preferences_changed(cx);
            cx.emit(ChromeEvent::ProfileChanged);
        }
        cx.notify();
    }
    fn profile_save_failed(
        &mut self,
        event: &IntegrationsEvent,
        message: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let message = if matches!(event, IntegrationsEvent::ProfileDeleted(_)) {
            format!(
                "Les identifiants du profil ont été retirés du coffre, mais le profil n’a pas été supprimé : {message}"
            )
        } else {
            message
        };
        self.integrations.update(cx, |view, cx| {
            if let IntegrationsEvent::AccountAdded {
                profile_id,
                account,
            } = event
            {
                view.rollback_new_account(profile_id, &account.id, cx);
            }
            view.set_profiles(self.profiles.profiles.clone(), window, cx);
            if let Some(profile) = self.profiles.active().cloned() {
                view.set_profile(profile, window, cx);
            }
            view.set_error(message, cx);
        });
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
                                    .rounded(RADIUS_SM)
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
                    .rounded(RADIUS_SM)
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
                    .child(format!(
                        "Espace local · {}",
                        self.profiles
                            .active()
                            .map(|profile| profile.name.as_str())
                            .unwrap_or("Personnel")
                    ))
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
                                if this.can_show_settings(cx) {
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
            .h(px(40.))
            .flex_none()
            .flex()
            .items_center()
            .px(px(if self.preferences.left_width < 210. {
                14.
            } else {
                20.
            }))
            .border_t_1()
            .border_color(rgb(p.border))
            .text_size(px(9. + offset))
            .text_color(rgb(p.muted))
            .child(format!("Version {}", env!("CARGO_PKG_VERSION")))
            .into_any_element()
    }
    pub fn repository_browser_open(&self) -> bool {
        self.repository_browser
    }

    pub fn open_repository_browser(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.repository_browser = true;
        self.integrations
            .update(cx, |view, cx| view.open_repositories(window, cx));
        cx.notify();
    }

    pub fn close_repository_browser(&mut self, cx: &mut Context<Self>) {
        if !self.repository_browser {
            return;
        }
        self.repository_browser = false;
        self.integrations
            .update(cx, |view, cx| view.close_repositories(cx));
        cx.notify();
    }

    pub fn render_repository_browser(
        &mut self,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = self.preferences.palette();
        let offset = self.preferences.font_size_offset;
        div()
            .id("remote-repository-browser")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .bg(rgb(p.bg))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(16.))
                    .px(px(24.))
                    .py(px(16.))
                    .bg(rgb(p.panel))
                    .border_b_1()
                    .border_color(rgb(p.border))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(12.))
                            .child(icon(IconName::Globe, 20., p.accent))
                            .child(
                                div()
                                    .text_size(px(16. + offset))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("Dépôts distants"),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(12.))
                            .child(
                                NativeButton::new("remote-manage-accounts")
                                    .px(px(12.))
                                    .py(px(7.))
                                    .text_size(px(12. + offset))
                                    .rounded(RADIUS_MD)
                                    .text_color(rgb(p.accent))
                                    .cursor_pointer()
                                    .hover(move |style| style.bg(rgb(p.hover)))
                                    .focus_visible(move |style| {
                                        style.border_1().border_color(rgb(p.accent))
                                    })
                                    .accessibility_label("Gérer les connexions")
                                    .child("Gérer les connexions")
                                    .on_click(cx.listener(|_, _, _, cx| {
                                        cx.emit(ChromeEvent::ManageIntegrations)
                                    })),
                            )
                            .child(
                                NativeButton::new("remote-browser-close")
                                    .size(px(32.))
                                    .rounded(RADIUS_SM)
                                    .cursor_pointer()
                                    .hover(move |style| style.bg(rgb(p.hover)))
                                    .focus_visible(move |style| {
                                        style.border_1().border_color(rgb(p.accent))
                                    })
                                    .accessibility_label("Fermer les dépôts distants")
                                    .tooltip(|window, cx| {
                                        Tooltip::new("Fermer (Échap)").build(window, cx)
                                    })
                                    .child(icon(IconName::X, 18., p.muted))
                                    .on_click(cx.listener(|_, _, _, cx| {
                                        cx.emit(ChromeEvent::CloseRepositoryBrowser)
                                    })),
                            ),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .child(self.integrations.clone()),
            )
            .into_any_element()
    }

    fn render_recent_repository(
        &self,
        entry: &RecentRepository,
        label: &str,
        worktree: bool,
        removable: bool,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let p = self.preferences.palette();
        let offset = self.preferences.font_size_offset;
        let path = entry.path.clone();
        let remove = path.clone();
        let tooltip = format!("{}\n{}", entry.name, entry.path);
        div()
            .id(SharedString::from(format!("recent:{path}")))
            .flex()
            .items_center()
            .rounded(RADIUS_MD)
            .border_1()
            .border_color(rgb(p.border))
            .bg(rgb(if worktree { p.panel } else { p.surface }))
            .overflow_hidden()
            .hover(move |view| view.bg(rgb(mix(p.text, p.surface, 0.06))))
            .child(
                NativeButton::new(SharedString::from(format!("recent-open:{path}")))
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
                    .tooltip(move |window, cx| Tooltip::new(tooltip.clone()).build(window, cx))
                    .focus_visible(move |style| style.bg(rgb(mix(p.text, p.surface, 0.06))))
                    .styles(|styles| styles.disabled(|style| style.opacity(0.5)))
                    .disabled(self.busy)
                    .child(icon(
                        if worktree {
                            IconName::GitBranch
                        } else {
                            IconName::FolderGit2
                        },
                        16.,
                        p.accent,
                    ))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(11. + offset))
                            .child(
                                div()
                                    .truncate()
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(rgb(p.accent))
                                    .child(label.to_owned()),
                            )
                            .child(
                                div()
                                    .mt(px(4.))
                                    .w_full()
                                    .truncate()
                                    .text_size(px(9. + offset))
                                    .text_color(rgb(p.muted))
                                    .child(display_path(&path)),
                            ),
                    )
                    .child(icon(IconName::ChevronRight, 14., p.accent))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if !this.busy {
                            cx.emit(ChromeEvent::OpenRepository(path.clone()))
                        }
                    })),
            )
            .when(removable, |view| {
                view.child(
                    NativeButton::new(SharedString::from(format!("recent-remove:{remove}")))
                        .w(px(28.))
                        .h(px(28.))
                        .mr(px(6.))
                        .rounded(RADIUS_SM)
                        .flex_shrink_0()
                        .text_color(rgb(p.muted))
                        .cursor_pointer()
                        .hover(move |style| style.text_color(rgb(p.danger)))
                        .focus_visible(move |style| style.border_1().border_color(rgb(p.accent)))
                        .styles(|styles| styles.disabled(|style| style.opacity(0.5)))
                        .child(gpui_kit::component::Icon::new(IconName::X).with_size(px(14.)))
                        .tooltip(|window, cx| {
                            Tooltip::new("Retirer des dépôts récents").build(window, cx)
                        })
                        .accessibility_label(format!("Retirer {} des dépôts récents", entry.name))
                        .disabled(self.busy)
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.remove_recent(&remove, cx)),
                        ),
                )
            })
            .into_any_element()
    }

    pub fn render_welcome(&mut self, _: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let p = self.preferences.palette();
        let offset = self.preferences.font_size_offset;
        let query = self.recent_filter.read(cx).value().trim().to_lowercase();
        let recent_focus = self.recent_filter.read(cx).focus_handle(cx);
        let recent = self
            .recent_metadata
            .groups(&self.preferences.recent, &query);
        let content = div()
            .id("welcome-content")
            .w_full()
            .h_auto()
            .min_h_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .items_center()
            .justify_start()
            .pt(px(54.))
            .px(px(28.))
            .pb(px(32.))
            .bg(rgb(p.bg))
            .text_left()
            .child(
                div()
                    .w_full()
                    .max_w(px(660.))
                    .mb(px(24.))
                    .child(
                        div()
                            .text_size(px(24. + offset))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Dépôts"),
                    )
                    .child(
                        div()
                            .mt(px(6.))
                            .text_size(px(12. + offset))
                            .text_color(rgb(p.muted))
                            .child(format!(
                                "Profil {}",
                                self.profiles
                                    .active()
                                    .map_or("Personnel", |profile| profile.name.as_str())
                            )),
                    ),
            )
            .child(
                div()
                    .w_full()
                    .max_w(px(660.))
                    .flex()
                    .flex_wrap()
                    .gap(px(10.))
                    .child(
                        NativeButton::new("welcome-open")
                            .px(px(17.))
                            .py(px(12.))
                            .border_1()
                            .border_color(rgb(p.accent_border))
                            .bg(rgb(p.accent))
                            .text_color(rgb(p.on_accent))
                            .rounded(RADIUS_MD)
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
                            .child("Ouvrir")
                            .disabled(self.busy || self.choosing)
                            .on_click(
                                cx.listener(|_, _, _, cx| cx.emit(ChromeEvent::PickRepository)),
                            ),
                    )
                    .child(
                        NativeButton::new("welcome-remotes")
                            .px(px(17.))
                            .py(px(12.))
                            .border_1()
                            .border_color(rgb(p.border))
                            .bg(rgb(p.panel))
                            .rounded(RADIUS_MD)
                            .text_size(px(12. + offset))
                            .font_weight(FontWeight::MEDIUM)
                            .gap(px(9.))
                            .cursor_pointer()
                            .accessibility_label("Cloner ou consulter un dépôt distant")
                            .hover(move |style| style.bg(rgb(p.hover)))
                            .focus_visible(move |style| style.border_color(rgb(p.accent)))
                            .styles(|styles| styles.disabled(|style| style.opacity(0.5)))
                            .disabled(self.busy || self.choosing)
                            .child(icon(IconName::Globe, 17., p.muted))
                            .child("Cloner")
                            .on_click(
                                cx.listener(|_, _, _, cx| cx.emit(ChromeEvent::BrowseRepositories)),
                            ),
                    ),
            )
            .child(
                div()
                    .w_full()
                    .max_w(px(660.))
                    .mt(px(15.))
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .text_size(px(9. + offset))
                    .text_color(rgb(p.muted))
                    .child("Ouvrir un dépôt local")
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
                        .w(px(660.))
                        .max_w_full()
                        .mt(px(28.))
                        .text_left()
                        .child(
                            div()
                                .mb(px(8.))
                                .text_size(px(12. + offset))
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
                                .rounded(RADIUS_MD)
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
                        .children(recent.iter().map(|group| {
                            div()
                                .mb(px(12.))
                                .child(self.render_recent_repository(
                                    &group.repository,
                                    &group.repository.name,
                                    false,
                                    group.removable,
                                    cx,
                                ))
                                .when(!group.worktrees.is_empty(), |view| {
                                    let prefix = format!("{} · ", group.repository.name);
                                    view.child(
                                        div()
                                            .mt(px(6.))
                                            .ml(px(20.))
                                            .pl(px(14.))
                                            .border_l_1()
                                            .border_color(rgb(p.border))
                                            .flex()
                                            .flex_col()
                                            .gap(px(6.))
                                            .children(group.worktrees.iter().map(|entry| {
                                                self.render_recent_repository(
                                                    entry,
                                                    entry
                                                        .name
                                                        .strip_prefix(&prefix)
                                                        .unwrap_or(&entry.name),
                                                    true,
                                                    true,
                                                    cx,
                                                )
                                            })),
                                    )
                                })
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
                .rounded(RADIUS_MD)
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
        .rounded(RADIUS_SM)
        .py(px(2.))
        .px(px(5.))
        .text_size(px(9. + offset))
        .text_color(rgb(p.muted))
        .child(label.to_string())
        .into_any_element()
}
