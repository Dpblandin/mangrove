//! The single active workspace coordinates native feature views and Git effects.
#[cfg(feature = "app-benchmark")]
use crate::perf::{Configuration, PerfAction, PerfSession, PerfSnapshot};
use crate::{
    branch_tracking::TrackingState,
    chrome::{Chrome, ChromeEvent, Notice},
    command_palette::{CommandPalette, PaletteEvent},
    history_view::{HistoryEvent, HistoryView},
    inspector::{InspectorEvent, InspectorView, InspectorViewState},
    preferences::{Pane, Preferences},
    repository_open::{OpenIntent, RepositoryOpenLane},
    settings::SettingsView,
    sidebar::{SidebarEvent, SidebarView, SidebarViewState},
    tabs::Tabs,
    theme::SANS,
    view_cache::render_cached_view,
};
use gpui_kit::{component::input::Escape as InputEscape, prelude::*, *};
use mangrove_core::{Workspace, model::RepositoryInfo};
#[cfg(feature = "app-benchmark")]
use std::time::Instant;
use std::{collections::HashMap, sync::Arc};

mod commands;
mod fetch;

use fetch::FetchFeedback;

actions!(
    mangrove,
    [
        Quit,
        OpenRepository,
        NewTab,
        CloseTab,
        NextTab,
        PreviousTab,
        ShowSettings,
        ShowCommandPalette,
        Escape,
        Search,
        ZoomIn,
        ZoomOut,
        ZoomReset,
        MinimizeWindow,
        ZoomWindow
    ]
);

const ROOT_CONTEXT: &str = "Mangrove";
/// Unshifted characters of the French AZERTY digit row, from 1 to 9.
const AZERTY_DIGITS: [&str; 9] = ["&", "é", "\"", "'", "(", "§", "è", "!", "ç"];

/// Cmd/Ctrl+1 to 8 select a tab position, 9 the last tab.
#[derive(Clone, PartialEq, Action)]
#[action(namespace = mangrove, no_json)]
pub struct SelectTab {
    pub position: usize,
}

pub fn bind_keys(cx: &mut App) {
    use gpui_kit::component::input::{Copy, Cut, Paste, SelectAll};
    for modifier in ["cmd", "ctrl"] {
        cx.bind_keys([
            KeyBinding::new(&format!("{modifier}-q"), Quit, None),
            KeyBinding::new(&format!("{modifier}-o"), OpenRepository, None),
            KeyBinding::new(&format!("{modifier}-t"), NewTab, None),
            KeyBinding::new(&format!("{modifier}-w"), CloseTab, None),
            KeyBinding::new(&format!("{modifier}-,"), ShowSettings, None),
            KeyBinding::new(&format!("{modifier}-p"), ShowCommandPalette, None),
            KeyBinding::new(&format!("{modifier}-k"), Search, None),
            KeyBinding::new(&format!("{modifier}-f"), Search, None),
            KeyBinding::new(&format!("{modifier}-="), ZoomIn, None),
            KeyBinding::new(&format!("{modifier}-+"), ZoomIn, None),
            KeyBinding::new(&format!("{modifier}--"), ZoomOut, None),
            KeyBinding::new(&format!("{modifier}-0"), ZoomReset, None),
        ]);
        cx.bind_keys((1..=9).map(|position| {
            KeyBinding::new(
                &format!("{modifier}-{position}"),
                SelectTab { position },
                None,
            )
        }));
        // AZERTY keeps digits behind Shift: the same physical keys report
        // their unshifted characters. Digit bindings come first for menus.
        cx.bind_keys(AZERTY_DIGITS.iter().zip(1..).map(|(key, position)| {
            KeyBinding::new(&format!("{modifier}-{key}"), SelectTab { position }, None)
        }));
        cx.bind_keys([KeyBinding::new(&format!("{modifier}-à"), ZoomReset, None)]);
    }
    cx.bind_keys([
        // Menus show the first binding; macOS cannot display a Tab key equivalent.
        KeyBinding::new("cmd-alt-right", NextTab, None),
        KeyBinding::new("cmd-alt-left", PreviousTab, None),
        KeyBinding::new("ctrl-tab", NextTab, None),
        KeyBinding::new("ctrl-shift-tab", PreviousTab, None),
        // A binding without context outranks every focused element. Scoped to
        // the root, Escape first reaches fields, menus and lists that use it.
        KeyBinding::new("escape", Escape, Some(ROOT_CONTEXT)),
        KeyBinding::new("cmd-m", MinimizeWindow, None),
    ]);
    cx.set_menus([
        Menu::new("Mangrove").items([
            MenuItem::action("Paramètres…", ShowSettings),
            MenuItem::separator(),
            MenuItem::action("Quitter Mangrove", Quit),
        ]),
        Menu::new("Fichier").items([
            MenuItem::action("Ouvrir un dépôt…", OpenRepository),
            MenuItem::action("Nouvel onglet", NewTab),
            MenuItem::action("Fermer l’onglet", CloseTab),
        ]),
        Menu::new("Édition").items([
            MenuItem::action("Couper", Cut),
            MenuItem::action("Copier", Copy),
            MenuItem::action("Coller", Paste),
            MenuItem::separator(),
            MenuItem::action("Tout sélectionner", SelectAll),
        ]),
        Menu::new("Affichage").items([
            MenuItem::action("Palette de commandes…", ShowCommandPalette),
            MenuItem::separator(),
            MenuItem::action("Rechercher des commits", Search),
            MenuItem::separator(),
            MenuItem::action("Agrandir le texte", ZoomIn),
            MenuItem::action("Réduire le texte", ZoomOut),
            MenuItem::action("Taille du texte par défaut", ZoomReset),
        ]),
        // GPUI hands only a menu named "Window" to macOS, which then adds its
        // own window items; this localized menu provides them itself.
        Menu::new("Fenêtre").items([
            MenuItem::action("Placer dans le Dock", MinimizeWindow),
            MenuItem::action("Zoom", ZoomWindow),
            MenuItem::separator(),
            MenuItem::action("Onglet suivant", NextTab),
            MenuItem::action("Onglet précédent", PreviousTab),
            MenuItem::separator(),
            MenuItem::action("Premier onglet", SelectTab { position: 1 }),
            MenuItem::action("Dernier onglet", SelectTab { position: 9 }),
        ]),
    ]);
}

/// Where a background tab was left; restored when its repository reopens.
struct TabState {
    selection: TabSelection,
    query: String,
    solo: Vec<String>,
    sidebar: SidebarViewState,
    inspector: InspectorViewState,
}

enum TabSelection {
    Commit(String),
    Stash(String),
    WorkingTree,
    None,
}

pub struct Mangrove {
    engine: Arc<Workspace>,
    repository: Option<Arc<RepositoryInfo>>,
    history: Entity<HistoryView>,
    sidebar: Entity<SidebarView>,
    tracking: Entity<TrackingState>,
    inspector: Entity<InspectorView>,
    chrome: Entity<Chrome>,
    tabs: Tabs,
    tab_states: HashMap<u64, TabState>,
    restoring: Option<(u64, TabState)>,
    settings: bool,
    /// The settings were opened over the clone screen, which they return to.
    settings_from_clone: bool,
    opening: bool,
    fetching: bool,
    error: Option<String>,
    fetch_error: Option<Notice>,
    fetch_feedback: FetchFeedback,
    generation: u64,
    refresh_queued: bool,
    pending_navigation: Option<String>,
    focus: FocusHandle,
    command_palette: Entity<CommandPalette>,
    palette_open: bool,
    palette_previous_focus: Option<FocusHandle>,
    open_task: Option<Task<()>>,
    open_lane: RepositoryOpenLane,
    fetch_task: Option<Task<()>>,
    fetch_timer: Option<Task<()>>,
    picker_task: Option<Task<()>>,
    recent_task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
    #[cfg(feature = "app-benchmark")]
    perf: Option<PerfSession>,
    #[cfg(feature = "app-benchmark")]
    perf_task: Option<Task<()>>,
}

impl Mangrove {
    pub fn new(
        engine: Arc<Workspace>,
        preferences: Preferences,
        path: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let palette = preferences.palette();
        let sidebar = cx.new(|cx| SidebarView::new(engine.clone(), window, cx));
        let tracking = sidebar.read(cx).tracking();
        let history = cx.new(|cx| {
            HistoryView::new(
                engine.clone(),
                palette,
                preferences.font_size_offset,
                tracking.clone(),
                window,
                cx,
            )
        });
        let inspector = cx.new(|cx| InspectorView::new(engine.clone(), window, cx));
        let chrome = cx.new(|cx| Chrome::new(preferences, window, cx));
        let command_palette = cx.new(|cx| CommandPalette::new(window, cx));
        let this = cx.weak_entity();
        // Closing the window during a clone asks first, like leaving its screen.
        window.on_window_should_close(cx, move |window, cx| {
            this.update(cx, |view, cx| {
                !view.confirm_leave_clone(window, cx, |_, window, _| window.remove_window())
            })
            .unwrap_or(true)
        });
        let subscriptions = vec![
            cx.subscribe_in(
                &command_palette,
                window,
                |view, _, event, window, cx| match event {
                    PaletteEvent::Close => view.close_palette(window, cx),
                    PaletteEvent::Execute(entry) => view.execute_command(entry, window, cx),
                },
            ),
            cx.observe(&inspector, |view, _, cx| view.sync_palette(cx)),
            cx.observe(&sidebar, |view, _, cx| view.sync_palette(cx)),
            cx.observe(&chrome, |view, _, cx| view.sync_palette(cx)),
            cx.subscribe_in(&history, window, |this, _, event, window, cx| {
                this.history_event(event, window, cx)
            }),
            cx.subscribe_in(&sidebar, window, |this, _, event, window, cx| {
                this.sidebar_event(event, window, cx)
            }),
            cx.subscribe_in(&inspector, window, |this, _, event, window, cx| {
                this.inspector_event(event, window, cx)
            }),
            cx.subscribe_in(&chrome, window, |this, _, event, window, cx| {
                this.chrome_event(event, window, cx)
            }),
            cx.observe_window_activation(window, |this, window, cx| {
                if window.is_window_active() && !this.chrome.read(cx).choosing_repository() {
                    #[cfg(feature = "app-benchmark")]
                    if this.perf.is_some() {
                        return;
                    }
                    if this.repository.is_some() {
                        this.refresh(window, cx);
                    } else {
                        this.refresh_recent(cx);
                    }
                }
            }),
            // Closed tabs, settings and dialogs leave no focused element behind.
            cx.on_focus_lost(window, |this, window, cx| this.restore_focus(window, cx)),
        ];
        let mut this = Self {
            engine,
            repository: None,
            history,
            sidebar,
            tracking,
            inspector,
            chrome,
            tabs: Tabs::default(),
            tab_states: HashMap::new(),
            restoring: None,
            settings: false,
            settings_from_clone: false,
            opening: false,
            fetching: false,
            error: None,
            fetch_error: None,
            fetch_feedback: FetchFeedback::default(),
            generation: 0,
            refresh_queued: false,
            pending_navigation: None,
            focus: cx.focus_handle(),
            command_palette,
            palette_open: false,
            palette_previous_focus: None,
            open_task: None,
            open_lane: RepositoryOpenLane::default(),
            fetch_task: None,
            fetch_timer: None,
            picker_task: None,
            recent_task: None,
            _subscriptions: subscriptions,
            #[cfg(feature = "app-benchmark")]
            perf: None,
            #[cfg(feature = "app-benchmark")]
            perf_task: None,
        };
        this.focus.focus(window, cx);
        this.apply_preferences(window, cx);
        if let Some(path) = path {
            this.open(path, false, None, window, cx);
        } else {
            this.tabs.create();
            this.sync_chrome(cx);
            this.refresh_recent(cx);
        }
        this
    }

    #[cfg(feature = "app-benchmark")]
    pub fn new_with_benchmark(
        engine: Arc<Workspace>,
        preferences: Preferences,
        path: Option<String>,
        started: Instant,
        benchmark: Option<Configuration>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self::new(engine, preferences, path, window, cx);
        this.perf = match benchmark
            .map(|configuration| PerfSession::new(configuration, started))
            .transpose()
        {
            Ok(perf) => perf,
            Err(error) => {
                eprintln!("{error}");
                cx.quit();
                None
            }
        };
        this.schedule_perf(window, cx);
        this
    }
    fn sync_chrome(&mut self, cx: &mut Context<Self>) {
        let error = self
            .error
            .as_deref()
            .or_else(|| self.history.read(cx).error())
            .map(Notice::plain)
            .or_else(|| self.fetch_error.clone());
        let name = self
            .repository
            .as_ref()
            .map(|repo| repo.name.clone())
            .or_else(|| {
                self.opening
                    .then(|| self.tabs.active_tab().map(|tab| tab.name.clone()))
                    .flatten()
            });
        self.chrome.update(cx, |chrome, cx| {
            chrome.set_tabs(self.tabs.entries.clone(), self.tabs.active, cx);
            chrome.set_context(
                name,
                self.repository
                    .as_ref()
                    .is_some_and(|r| !r.remotes.is_empty()),
                self.opening,
                error,
                cx,
            );
            chrome.set_fetching(self.fetching, cx);
        });
        cx.notify();
    }
    fn apply_preferences(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let p = self.chrome.read(cx).preferences.clone();
        self.history.update(cx, |view, cx| {
            view.set_theme(p.palette(), p.font_size_offset, cx)
        });
        self.sidebar.update(cx, |view, cx| {
            view.set_theme(p.palette(), p.font_size_offset, cx)
        });
        self.inspector.update(cx, |view, cx| {
            view.set_theme(p.palette(), cx);
            view.set_preferences(p, cx);
        });
        self.schedule_fetch(window, cx);
        cx.notify();
    }
    fn clear_workspace(&mut self, cx: &mut Context<Self>) {
        self.generation += 1;
        self.open_lane.invalidate_pending();
        self.fetch_timer = None;
        self.fetch_error = None;
        self.fetch_feedback.reset_backoff();
        self.opening = false;
        self.refresh_queued = false;
        self.repository = None;
        self.pending_navigation = None;
        self.history.update(cx, |view, cx| view.clear(cx));
        self.sidebar
            .update(cx, |view, cx| view.clear_repository(cx));
        self.tracking.update(cx, |tracking, cx| tracking.clear(cx));
        self.inspector
            .update(cx, |view, cx| view.clear_repository(cx));
        self.sync_chrome(cx);
    }
    fn leave_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.settings {
            self.chrome
                .read(cx)
                .settings
                .clone()
                .update(cx, |view, cx| view.leave(window, cx));
        }
        self.settings = false;
        self.settings_from_clone = false;
    }
    /// Shows the settings in place of the workspace or of the clone screen.
    fn enter_settings(
        &mut self,
        show: impl FnOnce(&mut SettingsView, &mut Window, &mut Context<SettingsView>),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let from_clone = self.chrome.read(cx).repository_browser_open();
        self.chrome
            .update(cx, |chrome, cx| chrome.close_repository_browser(cx));
        self.chrome
            .read(cx)
            .settings
            .clone()
            .update(cx, |view, cx| {
                view.set_return_to_clone(from_clone, cx);
                show(view, window, cx);
            });
        self.settings = true;
        self.settings_from_clone = from_clone;
        cx.notify();
    }

    fn escape(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.palette_open {
            self.close_palette(window, cx);
        } else if self.chrome.read(cx).repository_browser_open() {
            self.chrome_event(&ChromeEvent::CloseRepositoryBrowser, window, cx);
        } else if self.settings {
            self.chrome_event(&ChromeEvent::BackToWorkspace, window, cx);
        } else if self.inspector.read(cx).central_active() {
            self.inspector
                .update(cx, |inspector, cx| inspector.close_workspace(cx));
        }
    }

    /// Defers `retry` behind a confirmation when it would cancel a running clone.
    fn confirm_leave_clone(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        retry: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) -> bool {
        let this = cx.entity().downgrade();
        self.chrome.update(cx, |chrome, cx| {
            chrome.confirm_leave_repository_browser(window, cx, move |window, cx| {
                let _ = this.update(cx, |view, cx| retry(view, window, cx));
            })
        })
    }

    fn open(
        &mut self,
        path: String,
        refresh: bool,
        solo: Option<Vec<String>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !refresh {
            let retry = (path.clone(), solo.clone());
            if self.confirm_leave_clone(window, cx, move |view, window, cx| {
                view.open(retry.0, false, retry.1, window, cx)
            }) {
                return;
            }
            self.chrome
                .update(cx, |chrome, cx| chrome.close_repository_browser(cx));
        }
        if refresh && self.opening && solo.is_none() {
            self.refresh_queued = true;
            return;
        }
        if !refresh || solo.is_some() {
            self.refresh_queued = false;
        }
        let previous = self.tabs.active;
        if !refresh {
            self.remember_tab_state(cx);
        }
        let attempt = self.tabs.begin_open(path.clone());
        if !refresh {
            // Opening a path already shown in another tab switches to it.
            self.restoring = match self.tabs.active {
                Some(id) if self.tabs.active != previous => {
                    self.tab_states.remove(&id).map(|state| (id, state))
                }
                _ => None,
            };
            if let Some(id) = previous.filter(|id| self.tabs.active == Some(*id)) {
                self.tab_states.remove(&id);
            }
        }
        let restored_solo = self
            .restoring
            .as_ref()
            .filter(|_| !refresh)
            .map(|(_, state)| state.solo.clone());
        let solo = solo.or(restored_solo).unwrap_or_else(|| {
            if refresh {
                self.repository
                    .as_ref()
                    .map(|repo| repo.solo_refs.clone())
                    .unwrap_or_default()
            } else {
                Vec::new()
            }
        });
        let same_path = self
            .repository
            .as_ref()
            .is_some_and(|repo| repo.path == path);
        self.generation += 1;
        let generation = self.generation;
        if !refresh {
            self.error = None;
        }
        self.opening = true;
        self.fetch_timer = None;
        if !refresh {
            self.leave_settings(window, cx);
        }
        if !same_path {
            self.repository = None;
            self.fetch_error = None;
            self.fetch_feedback.reset_backoff();
            self.history.update(cx, |view, cx| view.clear(cx));
            self.sidebar
                .update(cx, |view, cx| view.prepare_repository_open(&path, cx));
            self.tracking.update(cx, |tracking, cx| tracking.clear(cx));
            self.inspector
                .update(cx, |view, cx| view.clear_repository(cx));
        }
        self.sync_chrome(cx);
        if let Some(intent) = self.open_lane.submit(OpenIntent {
            path,
            solo,
            refresh,
            generation,
            attempt,
            navigate_to: self.pending_navigation.take().filter(|_| refresh),
        }) {
            self.start_open(intent, window, cx);
        }
    }
    fn start_open(&mut self, intent: OpenIntent, window: &mut Window, cx: &mut Context<Self>) {
        let OpenIntent {
            path,
            solo,
            refresh,
            generation,
            attempt,
            navigate_to,
        } = intent;
        let engine = self.engine.clone();
        self.open_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { engine.open_repository_with_solo(&path, &solo) })
                .await;
            let _ = cx.update(|window, cx| {
                let _ = this.update(cx, |view, cx| {
                    // The synchronous native read has ended. Invalidation never
                    // drops this task before it can release its physical lane.
                    view.open_task = None;
                    let next = view.open_lane.finished(generation);
                    if view.generation != generation || !view.tabs.is_current(&attempt) {
                        if let Some(next) = next {
                            view.start_open(next, window, cx);
                        }
                        return;
                    }
                    view.opening = false;
                    match result {
                        Ok(info) => {
                            let info = Arc::new(info);
                            let same_session = view
                                .repository
                                .as_ref()
                                .is_some_and(|old| old.id == info.id);
                            view.tabs
                                .complete_open(attempt, info.path.clone(), info.name.clone());
                            view.chrome.update(cx, |chrome, cx| {
                                chrome.remember_repository(info.path.clone(), info.name.clone(), cx)
                            });
                            view.repository = Some(info.clone());
                            view.sidebar
                                .update(cx, |sidebar, cx| sidebar.set_repository(info.clone(), cx));
                            view.tracking.update(cx, |tracking, cx| {
                                tracking.set_repository(info.clone(), cx)
                            });
                            view.inspector.update(cx, |inspector, cx| {
                                inspector.set_repository(info.clone(), cx);
                                if refresh && same_session {
                                    inspector.refresh_status(cx);
                                }
                            });
                            view.history
                                .update(cx, |history, cx| history.set_repository(info, window, cx));
                            let restored = view
                                .restoring
                                .take()
                                .filter(|(id, _)| view.tabs.active == Some(*id));
                            if let Some((_, state)) = restored {
                                view.restore_tab_state(state, window, cx);
                            }
                            if let Some(sha) = navigate_to {
                                view.history
                                    .update(cx, |history, cx| history.jump_to(sha, window, cx));
                            }
                            view.sync_selection(cx);
                            view.error = None;
                            if view.focus.is_focused(window) {
                                view.restore_focus(window, cx);
                            }
                        }
                        Err(error) => {
                            view.restoring = None;
                            view.error = Some(error);
                            if view.repository.is_none() {
                                view.sidebar
                                    .update(cx, |sidebar, cx| sidebar.clear_repository(cx));
                                view.tracking.update(cx, |tracking, cx| tracking.clear(cx));
                            }
                            if !refresh {
                                view.tabs.reject_open(attempt);
                            }
                        }
                    }
                    view.sync_chrome(cx);
                    view.schedule_fetch(window, cx);
                    if view.refresh_queued {
                        view.refresh_queued = false;
                        view.refresh(window, cx);
                    }
                });
            });
        }));
    }
    fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(repo) = self.repository.clone() {
            self.open(repo.path.clone(), true, None, window, cx);
        }
    }
    /// Activating a tab, even the current one, shows its content and focus.
    fn activate_tab(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        if self.confirm_leave_clone(window, cx, move |view, window, cx| {
            view.activate_tab(id, window, cx)
        }) {
            return;
        }
        self.chrome
            .update(cx, |chrome, cx| chrome.close_repository_browser(cx));
        self.leave_settings(window, cx);
        if self.tabs.active != Some(id) {
            self.remember_tab_state(cx);
            let path = self.tabs.activate(id);
            self.clear_workspace(cx);
            if let Some(path) = path {
                self.reopen_active_tab(path, window, cx);
            } else {
                self.refresh_recent(cx);
            }
        }
        self.restore_focus(window, cx);
        cx.notify();
    }
    fn cycle_tab(&mut self, step: isize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(id) = self.tabs.neighbor(step) {
            self.activate_tab(id, window, cx);
        }
    }
    fn close_tab(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let active = self.tabs.active == Some(id);
        if active
            && self.confirm_leave_clone(window, cx, move |view, window, cx| {
                view.close_tab(id, window, cx)
            })
        {
            return;
        }
        if active {
            self.chrome
                .update(cx, |chrome, cx| chrome.close_repository_browser(cx));
        }
        self.tabs.close(id);
        self.tab_states.remove(&id);
        // The home screen always has its own tab, like a new window.
        if self.tabs.entries.is_empty() {
            self.tabs.create();
        }
        if active {
            let path = self.tabs.active_tab().and_then(|tab| tab.path.clone());
            self.clear_workspace(cx);
            self.restore_focus(window, cx);
            if let Some(path) = path {
                self.reopen_active_tab(path, window, cx);
            } else {
                self.refresh_recent(cx);
            }
        }
        self.sync_chrome(cx);
    }
    fn new_tab(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.confirm_leave_clone(window, cx, |view, window, cx| view.new_tab(window, cx)) {
            return;
        }
        self.chrome
            .update(cx, |chrome, cx| chrome.close_repository_browser(cx));
        self.remember_tab_state(cx);
        self.tabs.create();
        self.leave_settings(window, cx);
        self.clear_workspace(cx);
        self.restore_focus(window, cx);
        self.refresh_recent(cx);
    }
    /// Keeps the active tab's place before another tab replaces the views.
    fn remember_tab_state(&mut self, cx: &App) {
        let Some(id) = self.tabs.active else {
            return;
        };
        // A tab left before its reopening finished keeps the state it awaited.
        if let Some((restoring, state)) = self.restoring.take()
            && restoring == id
        {
            self.tab_states.insert(id, state);
            return;
        }
        let Some(repository) = self.repository.as_ref() else {
            return;
        };
        let history = self.history.read(cx);
        let inspector_view = self.inspector.read(cx);
        let inspector = inspector_view.view_state(cx);
        let selection = if let Some(sha) = inspector_view.selected_stash() {
            TabSelection::Stash(sha.to_owned())
        } else if inspector_view.is_working_tree() {
            TabSelection::WorkingTree
        } else {
            // The history forgets its commit while a restored one is looked up.
            Some(history.selected_sha().to_owned())
                .filter(|sha| !sha.is_empty())
                .or_else(|| inspector.sha().map(str::to_owned))
                .map_or(TabSelection::None, TabSelection::Commit)
        };
        let state = TabState {
            selection,
            query: history.query().to_owned(),
            solo: repository.solo_refs.clone(),
            sidebar: self.sidebar.read(cx).view_state(),
            inspector,
        };
        self.tab_states.insert(id, state);
    }
    /// Reloads the active tab's repository, back where the tab was left.
    fn reopen_active_tab(&mut self, path: String, window: &mut Window, cx: &mut Context<Self>) {
        self.restoring = self
            .tabs
            .active
            .and_then(|id| Some((id, self.tab_states.remove(&id)?)));
        let solo = self.restoring.as_ref().map(|(_, state)| state.solo.clone());
        self.open(path, true, solo, window, cx);
    }
    fn restore_tab_state(&mut self, state: TabState, window: &mut Window, cx: &mut Context<Self>) {
        self.sidebar.update(cx, |sidebar, cx| {
            sidebar.restore_view_state(state.sidebar, cx)
        });
        self.inspector.update(cx, |inspector, _| {
            inspector.restore_view_state(state.inspector)
        });
        let TabSelection::Commit(sha) = state.selection else {
            self.history
                .update(cx, |history, cx| history.set_query(state.query, cx));
            match state.selection {
                TabSelection::Stash(sha) => {
                    self.sidebar_event(&SidebarEvent::SelectStash(sha), window, cx)
                }
                // Without local changes, the status read returns to a commit.
                TabSelection::WorkingTree => {
                    self.history.update(cx, |view, cx| view.clear_selection(cx));
                    self.inspector
                        .update(cx, |view, cx| view.select_working_tree(cx));
                    self.sync_selection(cx);
                }
                TabSelection::Commit(_) | TabSelection::None => {}
            }
            return;
        };
        self.history.update(cx, |history, cx| {
            history.restore_view(sha, state.query, window, cx)
        });
    }
    /// True when the center shows the history list, which may take the focus.
    fn history_visible(&self, cx: &App) -> bool {
        let chrome = self.chrome.read(cx);
        self.repository.is_some()
            && !self.settings
            && !self.palette_open
            && !chrome.repository_browser_open()
            && !chrome.choosing_repository()
            && !self.inspector.read(cx).central_active()
    }
    /// Give the keyboard to the history list when shown, else to the workspace.
    fn restore_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.history_visible(cx) {
            self.history
                .update(cx, |history, cx| history.focus_list(window, cx));
        } else {
            self.focus.focus(window, cx);
        }
    }
    fn pick_repository(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.confirm_leave_clone(window, cx, |view, window, cx| {
            view.pick_repository(window, cx)
        }) {
            return;
        }
        if !self
            .chrome
            .update(cx, |chrome, cx| chrome.begin_repository_picker(cx))
        {
            return;
        }
        let previous_focus = if self.settings || self.chrome.read(cx).repository_browser_open() {
            Some(self.focus.clone())
        } else {
            window.focused(cx)
        };
        self.leave_settings(window, cx);
        self.chrome
            .update(cx, |chrome, cx| chrome.close_repository_browser(cx));
        let picker = rfd::AsyncFileDialog::new()
            .set_parent(window)
            .set_title("Ouvrir un dépôt Git");
        cx.notify();
        self.picker_task = Some(cx.spawn_in(window, async move |this, cx| {
            let folder = picker.pick_folder().await;
            let _ = cx.update(|window, cx| {
                let _ = this.update(cx, |view, cx| {
                    view.picker_task = None;
                    view.chrome
                        .update(cx, |chrome, cx| chrome.finish_repository_picker(cx));
                    if let Some(folder) = folder {
                        view.focus.focus(window, cx);
                        view.open(
                            folder.path().to_string_lossy().into_owned(),
                            false,
                            None,
                            window,
                            cx,
                        );
                    } else {
                        previous_focus
                            .as_ref()
                            .unwrap_or(&view.focus)
                            .focus(window, cx);
                    }
                    cx.notify();
                });
            });
        }));
    }
    fn refresh_recent(&mut self, cx: &mut Context<Self>) {
        let Some(request) = self
            .chrome
            .update(cx, |chrome, _| chrome.begin_recent_metadata())
        else {
            return;
        };
        let paths = request.paths().to_vec();
        self.recent_task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { mangrove_core::repository_names(&paths) })
                .await;
            if let Ok(names) = result {
                let _ = this.update(cx, |view, cx| {
                    view.chrome.update(cx, |chrome, cx| {
                        chrome.apply_recent_metadata(request, &names, cx);
                    });
                });
            }
        }));
    }
    fn toggle_solo(&mut self, name: Option<String>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(repo) = self.repository.clone() else {
            return;
        };
        let mut solo = repo.solo_refs.clone();
        if let Some(name) = name {
            if let Some(index) = solo.iter().position(|entry| entry == &name) {
                solo.remove(index);
            } else {
                solo.push(name);
            }
        } else {
            solo.clear();
        }
        self.open(repo.path.clone(), true, Some(solo), window, cx);
    }
    fn navigate_reference(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(repo) = self.repository.as_ref() else {
            return;
        };
        if let Some(reference) = repo
            .references
            .iter()
            .find(|reference| reference.name == name)
        {
            let sha = reference.sha.clone();
            self.history
                .update(cx, |view, cx| view.jump_to(sha, window, cx));
        }
    }
    fn sync_selection(&mut self, cx: &mut Context<Self>) {
        let inspector = self.inspector.read(cx);
        let sha = inspector
            .selection_sha()
            .filter(|sha| !sha.is_empty())
            .map(str::to_owned);
        let stash = inspector.selected_stash().unwrap_or_default().to_owned();
        let working = inspector.is_working_tree();
        self.history
            .update(cx, |view, cx| view.set_working_tree_selection(working, cx));
        self.sidebar
            .update(cx, |view, cx| view.set_selection(sha, stash, working, cx));
    }
    fn history_event(&mut self, event: &HistoryEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            HistoryEvent::Selected { sha, keyboard } => {
                self.inspector.update(cx, |view, cx| {
                    if *keyboard {
                        view.select_commit_keyboard(sha.clone(), cx);
                    } else {
                        view.select_commit(sha.clone(), cx);
                    }
                });
                self.sync_selection(cx);
            }
            HistoryEvent::ToggleSolo(name) => self.toggle_solo(Some(name.clone()), window, cx),
            HistoryEvent::ExitSoloAndNavigate(sha) => {
                self.pending_navigation = Some(sha.clone());
                self.toggle_solo(None, window, cx);
            }
            HistoryEvent::ClearSolo => self.toggle_solo(None, window, cx),
            HistoryEvent::SelectWorkingTree => self.select_working(cx),
            HistoryEvent::RefreshRepository => self.refresh(window, cx),
            HistoryEvent::Changed => {
                let outside = self.history.read(cx).selection_outside_results();
                self.inspector
                    .update(cx, |view, cx| view.set_outside_results(outside, cx));
                self.sync_chrome(cx);
            }
        }
    }
    fn select_working(&mut self, cx: &mut Context<Self>) {
        if !self.history.read(cx).has_working_tree_changes() {
            return;
        }
        self.history.update(cx, |view, cx| view.clear_selection(cx));
        self.inspector
            .update(cx, |view, cx| view.select_working_tree(cx));
        self.sync_selection(cx);
    }
    fn sidebar_event(&mut self, event: &SidebarEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.repository.is_none() && !matches!(event, SidebarEvent::OpenWorktree(_)) {
            return;
        }
        match event {
            SidebarEvent::NavigateReference(name) => self.navigate_reference(name, window, cx),
            SidebarEvent::ToggleSolo(name) => self.toggle_solo(Some(name.clone()), window, cx),
            SidebarEvent::OpenWorktree(path) => self.open(path.clone(), false, None, window, cx),
            SidebarEvent::SelectStash(sha) => {
                self.history.update(cx, |view, cx| view.clear_selection(cx));
                self.inspector
                    .update(cx, |view, cx| view.select_stash(sha.clone(), cx));
                self.sync_selection(cx);
            }
            SidebarEvent::StashUnavailable { repository_id, sha } => {
                if self
                    .repository
                    .as_ref()
                    .is_some_and(|repo| repo.id == *repository_id)
                    && self.inspector.read(cx).selected_stash() == Some(sha.as_str())
                    && self
                        .inspector
                        .update(cx, |view, cx| view.clear_stash_selection(cx))
                {
                    self.history
                        .update(cx, |view, cx| view.restore_commit_selection(window, cx));
                    self.sync_selection(cx);
                }
            }
        }
    }
    fn inspector_event(
        &mut self,
        event: &InspectorEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            InspectorEvent::ModeChanged(active) => {
                if !active
                    && !self.palette_open
                    && !self.settings
                    && self.repository.is_some()
                    && !self.chrome.read(cx).choosing_repository()
                {
                    self.history
                        .update(cx, |history, cx| history.focus_list(window, cx));
                } else if *active
                    && (self.focus.is_focused(window)
                        || self.history.focus_handle(cx).is_focused(window))
                {
                    // A restored central view takes the keys from the hidden list.
                    self.inspector
                        .update(cx, |inspector, cx| inspector.focus_workspace(window, cx));
                }
                cx.notify();
            }
            InspectorEvent::NavigateCommit(sha) => self
                .history
                .update(cx, |view, cx| view.jump_to(sha.clone(), window, cx)),
            InspectorEvent::WorkingTreeSummary {
                available,
                staged,
                unstaged,
            } => {
                let selected = self.inspector.read(cx).is_working_tree();
                let has_changes = *available && (*staged > 0 || *unstaged > 0);
                if selected && !has_changes {
                    self.inspector
                        .update(cx, |view, cx| view.clear_working_tree_selection(cx));
                }
                self.history.update(cx, |view, cx| {
                    view.set_working_tree_summary(
                        *available,
                        *staged,
                        *unstaged,
                        selected && has_changes,
                        cx,
                    );
                    if selected && !has_changes {
                        view.restore_commit_selection(window, cx);
                    }
                });
                if selected && !has_changes {
                    self.sync_selection(cx);
                }
            }
            InspectorEvent::DiffFontChanged(size) => self.chrome.update(cx, |chrome, cx| {
                chrome.preferences.set_diff_font_size(*size);
                chrome.prefs_changed(window, cx);
            }),
            InspectorEvent::WorkingTreeCounts(counts) => self.history.update(cx, |view, cx| {
                view.set_working_tree_counts(counts.clone(), cx)
            }),
        }
    }
    fn chrome_event(&mut self, event: &ChromeEvent, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(
            event,
            ChromeEvent::NewTab
                | ChromeEvent::CloseRepositoryBrowser
                | ChromeEvent::ManageIntegrations
                | ChromeEvent::ShowSettings
        ) {
            let retry = event.clone();
            if self.confirm_leave_clone(window, cx, move |view, window, cx| {
                view.chrome_event(&retry, window, cx)
            }) {
                return;
            }
        }
        if matches!(
            event,
            ChromeEvent::OpenRepository(_) | ChromeEvent::NewTab | ChromeEvent::ProfileChanged
        ) {
            self.chrome
                .update(cx, |chrome, cx| chrome.close_repository_browser(cx));
        }
        match event {
            ChromeEvent::OpenRepository(path) => self.open(path.clone(), false, None, window, cx),
            ChromeEvent::PickRepository => self.pick_repository(window, cx),
            ChromeEvent::ActivateTab(id) => self.activate_tab(*id, window, cx),
            ChromeEvent::CloseTab(id) => self.close_tab(*id, window, cx),
            ChromeEvent::NewTab => self.new_tab(window, cx),
            ChromeEvent::BrowseRepositories => {
                self.leave_settings(window, cx);
                self.chrome
                    .update(cx, |chrome, cx| chrome.open_repository_browser(window, cx));
                cx.notify();
            }
            ChromeEvent::CloneRemote(url) => {
                self.leave_settings(window, cx);
                self.chrome
                    .update(cx, |chrome, cx| chrome.clone_remote(url, window, cx));
                cx.notify();
            }
            ChromeEvent::CloseRepositoryBrowser => {
                self.chrome
                    .update(cx, |chrome, cx| chrome.close_repository_browser(cx));
                self.restore_focus(window, cx);
                cx.notify();
            }
            ChromeEvent::ManageIntegrations => self.enter_settings(
                |settings, window, cx| settings.show_integrations(window, cx),
                window,
                cx,
            ),
            ChromeEvent::ShowSettings => {
                if self.settings || !self.chrome.read(cx).can_show_settings(cx) {
                    return;
                }
                let preferences = self.chrome.read(cx).preferences.clone();
                // The settings give the keyboard to their navigation.
                self.enter_settings(
                    move |settings, window, cx| {
                        settings.set_numbers(&preferences, window, cx);
                        settings.show(window, cx);
                    },
                    window,
                    cx,
                );
            }
            ChromeEvent::BackToWorkspace => {
                let to_clone = self.settings && self.settings_from_clone;
                self.leave_settings(window, cx);
                if to_clone {
                    self.chrome
                        .update(cx, |chrome, cx| chrome.open_repository_browser(window, cx));
                } else {
                    self.restore_focus(window, cx);
                }
                cx.notify();
            }
            ChromeEvent::Fetch => self.fetch(true, window, cx),
            ChromeEvent::PreferencesChanged => self.apply_preferences(window, cx),
            ChromeEvent::ProfileChanged => {
                self.recent_task = None;
                self.tabs = Tabs::default();
                self.tabs.create();
                self.tab_states.clear();
                self.restoring = None;
                self.clear_workspace(cx);
                self.refresh_recent(cx);
                self.apply_preferences(window, cx);
            }
            ChromeEvent::PaneResized(pane, width) => {
                if *pane == Pane::Left {
                    self.sidebar
                        .update(cx, |view, cx| view.set_width(*width, cx));
                }
                cx.notify();
            }
            ChromeEvent::DismissError => {
                // Only the banner shown is dismissed; a fetch error behind an
                // open or history error appears next, as it was never seen.
                let shown_fetch = self.error.is_none() && self.history.read(cx).error().is_none();
                self.error = None;
                if shown_fetch
                    && let (Some(notice), Some(repository)) =
                        (self.fetch_error.take(), self.repository.as_ref())
                {
                    self.fetch_feedback.dismiss(&repository.path, notice);
                }
                self.history
                    .update(cx, |history, cx| history.clear_error(cx));
                self.sync_chrome(cx);
            }
        }
    }
    fn schedule_fetch(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.fetch_timer = None;
        #[cfg(feature = "app-benchmark")]
        if self.perf.is_some() {
            return;
        }
        let interval = self.chrome.read(cx).preferences.auto_fetch_interval;
        if self.fetching
            || self.opening
            || interval == 0
            || self
                .repository
                .as_ref()
                .is_none_or(|repo| repo.remotes.is_empty())
        {
            return;
        }
        let generation = self.generation;
        let delay = self.fetch_feedback.delay(interval);
        self.fetch_timer = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(delay).await;
            let _ = cx.update(|window, cx| {
                let _ = this.update(cx, |view, cx| {
                    if view.generation == generation {
                        view.fetch(false, window, cx);
                    }
                });
            });
        }));
    }
    fn fetch(&mut self, manual: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(repo) = self.repository.clone() else {
            return;
        };
        if self.fetching || self.opening || repo.remotes.is_empty() {
            return;
        }
        self.fetching = true;
        self.fetch_timer = None;
        self.sync_chrome(cx);
        let engine = self.engine.clone();
        self.fetch_task = Some(cx.spawn_in(window, async move |this, cx| {
            let session = repo.id.clone();
            let result = cx
                .background_executor()
                .spawn(async move { engine.fetch_repository(&session) })
                .await;
            let _ = cx.update(|window, cx| {
                let _ = this.update(cx, |view, cx| {
                    view.fetching = false;
                    // A refresh may have replaced the generation meanwhile;
                    // the result still belongs to the same repository session.
                    if view
                        .repository
                        .as_ref()
                        .is_some_and(|current| current.id == repo.id)
                    {
                        let error = result.map_or_else(Some, |result| result.error);
                        view.fetch_error = view.fetch_feedback.finish(&repo.path, error, manual);
                        view.refresh(window, cx);
                    }
                    view.sync_chrome(cx);
                    view.schedule_fetch(window, cx);
                });
            });
        }));
    }
}

#[cfg(feature = "app-benchmark")]
impl Mangrove {
    fn perf_snapshot(&self, cx: &App) -> PerfSnapshot {
        let history = self.history.read(cx);
        let inspector = self.inspector.read(cx);
        let (diff_ready, syntax_ready, diff_rows) = inspector.benchmark_readiness(cx);
        let file = inspector.selected_file_path();
        let syntax_target = file.and_then(|path| self.perf.as_ref()?.syntax_target(path));
        let (working_ready, working_files, working_displayed, file_scroll_offset) =
            inspector.benchmark_working_files();
        PerfSnapshot {
            history_ready: !inspector.central_active() && history.visible_loaded_rows() >= 10,
            index_complete: history.index_complete(),
            details_ready: inspector.details_ready()
                && inspector
                    .details()
                    .is_some_and(|details| details.sha == history.selected_sha()),
            diff_ready,
            syntax_ready,
            coloring: inspector.is_coloring(),
            selected_index: history.selected_index(),
            syntax_target,
            heavy_diff: file.is_some_and(|path| {
                self.perf
                    .as_ref()
                    .is_some_and(|perf| path.ends_with(perf.heavy_file()))
            }),
            file_count: inspector.details().map_or(0, |details| details.files.len()),
            history_total: history.total(),
            diff_rows,
            working_ready,
            working_files,
            working_displayed,
            file_scroll_offset,
        }
    }
    fn schedule_perf(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(perf) = self.perf.as_ref() else {
            return;
        };
        let wait = perf.wake_after();
        if wait.is_zero() {
            cx.on_next_frame(window, |view, window, cx| view.perf_tick(window, cx));
        } else {
            self.perf_task = Some(cx.spawn_in(window, async move |this, cx| {
                cx.background_executor().timer(wait).await;
                let _ = cx.update(|window, cx| {
                    let _ = this.update(cx, |view, cx| view.perf_tick(window, cx));
                });
            }));
        }
    }
    fn perf_tick(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let snapshot = self.perf_snapshot(cx);
        let Some(perf) = self.perf.as_mut() else {
            return;
        };
        let viewport = window.viewport_size();
        perf.set_viewport(
            f32::from(viewport.width) as usize,
            f32::from(viewport.height) as usize,
        );
        perf.set_accessibility_active(window.is_a11y_active());
        perf.set_scroll_window_state(window.is_window_active(), window.is_visible());
        let action = perf.tick(snapshot);
        match action {
            Some(PerfAction::SelectCommit(index)) => self
                .history
                .update(cx, |view, cx| view.select_index(index, window, cx)),
            Some(PerfAction::SelectCommitKeyboard(index)) => self
                .history
                .update(cx, |view, cx| view.select_index_keyboard(index, window, cx)),
            Some(PerfAction::OpenSyntaxFile(slot)) => {
                let path = self.perf.as_ref().unwrap().syntax_file(slot).to_owned();
                self.perf_open_file(&path, cx);
            }
            Some(PerfAction::OpenHeavyDiff) => {
                let path = self.perf.as_ref().unwrap().heavy_file().to_owned();
                self.perf_open_file(&path, cx);
            }
            Some(PerfAction::CloseDiff) => self
                .inspector
                .update(cx, |view, cx| view.close_workspace(cx)),
            Some(PerfAction::ScrollHistory(offset)) => self
                .history
                .update(cx, |view, cx| view.scroll_to_offset(offset, cx)),
            Some(PerfAction::ScrollDiff(offset)) => self
                .inspector
                .update(cx, |view, cx| view.benchmark_scroll_to(offset, cx)),
            Some(PerfAction::SelectWorking) => self.select_working(cx),
            Some(PerfAction::OpenWorkingFile) => {
                let path = self.perf.as_ref().unwrap().heavy_file().to_owned();
                let found = self
                    .inspector
                    .update(cx, |view, cx| view.benchmark_open_working_file(&path, cx));
                if !found && let Some(perf) = self.perf.as_mut() {
                    perf.fail("Benchmark working file is missing".into());
                }
            }
            Some(PerfAction::ScrollFiles(offset)) => self
                .inspector
                .update(cx, |view, cx| view.benchmark_scroll_files_to(offset, cx)),
            Some(PerfAction::Finish) => {
                self.perf = None;
                cx.quit();
                return;
            }
            None => {}
        }
        // Visual stages use actual rendering opportunities; idle/index stay quiet.
        if self
            .perf
            .as_ref()
            .is_some_and(|perf| perf.wake_after().is_zero())
        {
            cx.notify();
        }
        self.schedule_perf(window, cx);
    }
    fn perf_open_file(&mut self, suffix: &str, cx: &mut Context<Self>) {
        let index = self.inspector.read(cx).details().and_then(|details| {
            details
                .files
                .iter()
                .find(|file| file.path.ends_with(suffix))
                .map(|file| file.index)
        });
        if let Some(index) = index {
            self.inspector
                .update(cx, |view, cx| view.select_file(index, cx));
        } else if let Some(perf) = self.perf.as_mut() {
            perf.fail("Benchmark file is missing".into());
        }
    }
}

impl Render for Mangrove {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let prefs = self.chrome.read(cx).preferences.clone();
        let p = prefs.palette();
        let root = div()
            .id("mangrove-app")
            .key_context(ROOT_CONTEXT)
            .track_focus(&self.focus)
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .overflow_hidden()
            .bg(rgb(p.bg))
            .text_color(rgb(p.text))
            .font_family(SANS)
            .text_size(px(16. + prefs.font_size_offset))
            .font_weight(FontWeight::NORMAL)
            .line_height(relative(1.5))
            .capture_action(cx.listener(|view, _: &ShowCommandPalette, window, cx| {
                view.open_palette(window, cx);
                cx.stop_propagation();
            }))
            .capture_action(cx.listener(Self::guard_palette::<OpenRepository>))
            .capture_action(cx.listener(Self::guard_palette::<NewTab>))
            .capture_action(cx.listener(Self::guard_palette::<CloseTab>))
            .capture_action(cx.listener(Self::guard_palette::<NextTab>))
            .capture_action(cx.listener(Self::guard_palette::<PreviousTab>))
            .capture_action(cx.listener(Self::guard_palette::<SelectTab>))
            .capture_action(cx.listener(Self::guard_palette::<ShowSettings>))
            .capture_action(cx.listener(Self::guard_palette::<Search>))
            .capture_action(cx.listener(Self::guard_palette::<ZoomIn>))
            .capture_action(cx.listener(Self::guard_palette::<ZoomOut>))
            .capture_action(cx.listener(Self::guard_palette::<ZoomReset>))
            .capture_action(cx.listener(Self::guard_palette::<Quit>))
            .on_action(
                cx.listener(|view, _: &OpenRepository, window, cx| {
                    view.pick_repository(window, cx)
                }),
            )
            .on_action(cx.listener(|view, _: &NewTab, window, cx| view.new_tab(window, cx)))
            .on_action(cx.listener(|view, _: &CloseTab, window, cx| {
                // The settings cover the tabs; closing closes them, not a hidden tab.
                if view.settings {
                    view.chrome_event(&ChromeEvent::BackToWorkspace, window, cx);
                } else if let Some(id) = view.tabs.active {
                    view.close_tab(id, window, cx);
                }
            }))
            .on_action(cx.listener(|view, _: &NextTab, window, cx| view.cycle_tab(1, window, cx)))
            .on_action(
                cx.listener(|view, _: &PreviousTab, window, cx| view.cycle_tab(-1, window, cx)),
            )
            .on_action(cx.listener(|view, action: &SelectTab, window, cx| {
                if let Some(id) = view.tabs.by_shortcut(action.position) {
                    view.activate_tab(id, window, cx);
                }
            }))
            .on_action(cx.listener(|view, _: &Quit, window, cx| {
                if !view.confirm_leave_clone(window, cx, |_, _, cx| cx.quit()) {
                    cx.quit();
                }
            }))
            .on_action(|_: &MinimizeWindow, window, _| window.minimize_window())
            .on_action(|_: &ZoomWindow, window, _| window.zoom_window())
            .on_action(cx.listener(|view, _: &ShowSettings, window, cx| {
                view.chrome_event(&ChromeEvent::ShowSettings, window, cx)
            }))
            .on_action(cx.listener(|view, _: &Search, window, cx| {
                if view.repository.is_none()
                    || view.confirm_leave_clone(window, cx, |_, window, cx| {
                        window.dispatch_action(Box::new(Search), cx)
                    })
                {
                    return;
                }
                view.leave_settings(window, cx);
                view.chrome
                    .update(cx, |chrome, cx| chrome.close_repository_browser(cx));
                view.inspector
                    .update(cx, |inspector, cx| inspector.close_workspace(cx));
                // ModeChanged restores the list focus through a queued
                // event; focus the search after that event and the view switch.
                cx.on_next_frame(window, |view, window, cx| {
                    if view.history_visible(cx) {
                        view.history
                            .update(cx, |history, cx| history.focus_search(window, cx));
                    }
                });
                cx.notify();
            }))
            .on_action(cx.listener(|view, _: &ZoomIn, _, cx| {
                view.chrome
                    .update(cx, |chrome, cx| chrome.zoom(Some(1), cx))
            }))
            .on_action(cx.listener(|view, _: &ZoomOut, _, cx| {
                view.chrome
                    .update(cx, |chrome, cx| chrome.zoom(Some(-1), cx))
            }))
            .on_action(cx.listener(|view, _: &ZoomReset, _, cx| {
                view.chrome.update(cx, |chrome, cx| chrome.zoom(None, cx))
            }))
            .on_action(cx.listener(|view, _: &Escape, window, cx| view.escape(window, cx)))
            .on_action(cx.listener(|view, _: &InputEscape, window, cx| view.escape(window, cx)));
        let root = root.when(self.palette_open, |root| {
            root.child(deferred(self.command_palette.clone()).with_priority(100))
        });
        if self.settings {
            return root.child(self.chrome.read(cx).settings.clone());
        }
        if self.chrome.read(cx).repository_browser_open() {
            let tabs = self
                .chrome
                .update(cx, |chrome, cx| chrome.render_tabs(window, cx));
            let browser = self.chrome.update(cx, |chrome, cx| {
                chrome.render_repository_browser(window, cx)
            });
            return root.child(tabs).child(browser);
        }
        let width = f32::from(window.viewport_size().width);
        let layout = prefs.pane_layout(width);
        self.sidebar
            .update(cx, |view, cx| view.set_width(layout.left, cx));
        let error_toast = self.chrome.update(cx, |chrome, cx| {
            chrome.render_error(layout.right, window, cx)
        });
        let tabs = self
            .chrome
            .update(cx, |chrome, cx| chrome.render_tabs(window, cx));
        let topbar = self
            .chrome
            .update(cx, |chrome, cx| chrome.render_topbar(window, cx));
        let sidebar_ready = self.sidebar.read(cx).repository.is_some();
        // The home screen has no references to show; it uses the full width.
        let left_pane = (sidebar_ready || self.repository.is_some() || self.opening).then(|| {
            let sidebar = if sidebar_ready {
                render_cached_view(self.sidebar.clone(), window)
            } else {
                self.chrome
                    .read(cx)
                    .render_empty_sidebar()
                    .into_any_element()
            };
            let footer = self.chrome.read(cx).render_sidebar_footer();
            let divider = self.chrome.update(cx, |chrome, cx| {
                chrome.render_divider(Pane::Left, width, window, cx)
            });
            [
                div()
                    .w(px(layout.left))
                    .flex_shrink_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    .bg(rgb(p.panel))
                    .child(div().flex_1().min_h_0().child(sidebar))
                    .child(footer)
                    .into_any_element(),
                divider,
            ]
        });
        let center = if self.repository.is_some() && self.inspector.read(cx).central_active() {
            self.inspector
                .update(cx, |view, cx| view.render_workspace(window, cx))
        } else if self.repository.is_some() {
            render_cached_view(self.history.clone(), window)
        } else if self.opening {
            div().flex_1().into_any_element()
        } else {
            self.chrome
                .update(cx, |chrome, cx| chrome.render_welcome(window, cx))
        };
        let right_divider = self.chrome.update(cx, |chrome, cx| {
            chrome.render_divider(Pane::Right, width, window, cx)
        });
        root.child(tabs).child(
            div()
                .flex()
                .flex_1()
                .min_h_0()
                .overflow_hidden()
                .children(left_pane.into_iter().flatten())
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .min_h_0()
                        .child(topbar)
                        .child(
                            div()
                                .relative()
                                .flex()
                                .flex_1()
                                .min_h_0()
                                .overflow_hidden()
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .min_h_0()
                                        .h_full()
                                        .flex()
                                        .flex_col()
                                        .child(center),
                                )
                                .when(self.repository.is_some(), |view| {
                                    view.child(right_divider).child(
                                        div().w(px(layout.right)).h_full().flex_shrink_0().child(
                                            render_cached_view(self.inspector.clone(), window),
                                        ),
                                    )
                                })
                                .children(error_toast),
                        ),
                ),
        )
    }
}
