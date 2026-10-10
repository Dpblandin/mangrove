//! The catalogue is derived from live view state. Dispatch uses the same
//! workspace operations as menus and buttons, after checking the catalogue again.
use super::*;
use crate::{
    command_palette::{Command, CommandEntry},
    file_browser::FileView,
    format::{control_shortcut_label, display_path, short_sha, shortcut_label},
    settings::SettingsSection,
    tabs::HOME,
    theme::ThemeId,
};

impl Mangrove {
    pub(super) fn guard_palette<A: Action>(
        &mut self,
        _: &A,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.palette_open {
            cx.stop_propagation();
        }
    }
    fn command_entries(&self, cx: &App) -> Vec<CommandEntry> {
        let chrome = self.chrome.read(cx);
        let prefs = &chrome.preferences;
        let blocked = (!chrome.can_show_settings(cx))
            .then_some("Un sélecteur ou un redimensionnement est actif");
        let mut entries = vec![
            CommandEntry::new(
                Command::OpenRepository,
                "Ouvrir un dépôt…",
                "Dépôts",
                "open repository local dossier",
            )
            .shortcut(shortcut_label("O"))
            .unavailable(blocked.or(self.opening.then_some("Ouverture en cours"))),
            CommandEntry::new(
                Command::NewTab,
                "Nouvel onglet",
                "Onglets",
                "new tab accueil home récents favoris",
            )
            .detail("Ouvre l’accueil dans un nouvel onglet")
            .shortcut(shortcut_label("T"))
            .unavailable(blocked),
            CommandEntry::new(
                Command::BrowseRepositories,
                "Cloner ou consulter un dépôt distant…",
                "Dépôts",
                "clone github gitlab bitbucket remote",
            )
            .unavailable(blocked),
            CommandEntry::new(
                Command::Settings(None),
                "Ouvrir les paramètres",
                "Paramètres",
                "settings preferences configuration",
            )
            .shortcut(shortcut_label(","))
            .unavailable(blocked),
        ];
        for (section, label, keywords) in [
            (
                SettingsSection::General,
                "Paramètres : Général",
                "settings apparence autofetch images",
            ),
            (
                SettingsSection::Profiles,
                "Paramètres : Profils",
                "settings personnel professionnel comptes",
            ),
            (
                SettingsSection::Ssh,
                "Paramètres : SSH",
                "settings clés agent authentification",
            ),
            (
                SettingsSection::Integrations,
                "Paramètres : Intégrations",
                "settings github gitlab bitbucket connexion oauth token",
            ),
        ] {
            entries.push(
                CommandEntry::new(
                    Command::Settings(Some(section)),
                    label,
                    "Paramètres",
                    keywords,
                )
                .unavailable(blocked),
            );
        }
        for theme in ThemeId::ALL {
            entries.push(
                CommandEntry::new(
                    Command::Theme(theme),
                    format!("Thème : {}", theme.label()),
                    "Apparence",
                    "theme apparence couleur settings",
                )
                .active(prefs.theme == theme),
            );
        }
        entries.extend([
            CommandEntry::new(
                Command::Zoom(Some(1)),
                "Agrandir le texte",
                "Affichage",
                "zoom taille police",
            )
            .shortcut(shortcut_label("+")),
            CommandEntry::new(
                Command::Zoom(Some(-1)),
                "Réduire le texte",
                "Affichage",
                "zoom taille police",
            )
            .shortcut(shortcut_label("−")),
            CommandEntry::new(
                Command::Zoom(None),
                "Rétablir la taille du texte",
                "Affichage",
                "zoom reset taille police défaut",
            )
            .shortcut(shortcut_label("0")),
            CommandEntry::new(
                Command::DiffZoom(1),
                "Agrandir le texte des diffs",
                "Affichage",
                "zoom police fichier code",
            ),
            CommandEntry::new(
                Command::DiffZoom(-1),
                "Réduire le texte des diffs",
                "Affichage",
                "zoom police fichier code",
            ),
            CommandEntry::new(
                Command::ResetPane(Pane::Left),
                "Réinitialiser la largeur du panneau des branches",
                "Affichage",
                "sidebar panneau gauche reset",
            ),
            CommandEntry::new(
                Command::ResetPane(Pane::Right),
                "Réinitialiser la largeur du panneau des détails",
                "Affichage",
                "inspector panneau droit reset",
            ),
            CommandEntry::new(
                Command::ToggleImagePreviews,
                if prefs.show_image_previews {
                    "Désactiver les aperçus des images"
                } else {
                    "Activer les aperçus des images"
                },
                "Paramètres",
                "image aperçu preview settings",
            ),
            CommandEntry::new(
                Command::ToggleAutoFetch,
                if prefs.auto_fetch_interval == 0 {
                    "Activer l’auto-fetch"
                } else {
                    "Désactiver l’auto-fetch"
                },
                "Paramètres",
                "fetch automatique intervalle settings",
            )
            .detail(if prefs.auto_fetch_interval == 0 {
                "Toutes les minutes ; intervalle réglable dans Général".into()
            } else {
                format!("Intervalle actuel : {} min", prefs.auto_fetch_interval)
            }),
            CommandEntry::new(
                Command::Quit,
                "Quitter Mangrove",
                "Application",
                "quit exit fermer",
            )
            .shortcut(shortcut_label("Q")),
        ]);
        if let Some(id) = self.tabs.active {
            entries.push(
                CommandEntry::new(
                    Command::CloseTab(id),
                    "Fermer l’onglet actif",
                    "Onglets",
                    "close tab",
                )
                .shortcut(shortcut_label("W"))
                .unavailable(blocked),
            );
        }
        if self.tabs.entries.len() > 1 {
            for (step, label, shift) in
                [(1, "Onglet suivant", false), (-1, "Onglet précédent", true)]
            {
                entries.push(
                    CommandEntry::new(
                        Command::CycleTab(step),
                        label,
                        "Onglets",
                        "next previous tab changer",
                    )
                    .shortcut(control_shortcut_label("Tab", shift))
                    .unavailable(blocked),
                );
            }
        }
        for (index, tab) in self.tabs.entries.iter().enumerate() {
            let mut entry = CommandEntry::new(
                Command::ActivateTab(tab.id),
                format!("Onglet : {}", tab.name),
                "Onglets",
                "switch tab changer",
            )
            .detail(tab.path.as_deref().map_or_else(
                || format!("{HOME} : dépôts récents et favoris"),
                display_path,
            ))
            .active(self.tabs.active == Some(tab.id))
            .unavailable(blocked);
            if let Some(digit) = self.tabs.shortcut_digit(index) {
                entry = entry.shortcut(shortcut_label(&digit.to_string()));
            }
            entries.push(entry);
        }
        for recent in &prefs.recent {
            entries.push(
                CommandEntry::new(
                    Command::OpenRecent(recent.path.clone()),
                    format!("Ouvrir un récent : {}", recent.name),
                    "Dépôts",
                    "recent open repository",
                )
                .detail(display_path(&recent.path))
                .unavailable(blocked),
            );
            entries.push(
                CommandEntry::new(
                    Command::RemoveRecent(recent.path.clone()),
                    format!("Retirer des récents : {}", recent.name),
                    "Dépôts",
                    "recent supprimer liste",
                )
                .detail(display_path(&recent.path)),
            );
        }
        let inspector = self.inspector.read(cx);
        let [selection, file] = inspector.command_revision();
        let scope = [self.generation, selection, file];
        let unavailable = if self.repository.is_none() {
            Some("Ouvrez un dépôt")
        } else if self.opening {
            Some("Actualisation du dépôt en cours")
        } else {
            blocked
        };
        let mut repository_commands = vec![
            CommandEntry::new(
                Command::History,
                "Aller à l’historique du dépôt",
                "Git",
                "history graphe commits retour",
            ),
            CommandEntry::new(
                Command::Search,
                "Rechercher des commits",
                "Git",
                "search auteur message sha",
            )
            .shortcut(shortcut_label("F")),
            CommandEntry::new(
                Command::Refresh,
                "Actualiser le dépôt",
                "Git",
                "refresh reload changements",
            ),
            CommandEntry::new(
                Command::References,
                "Filtrer les branches et les tags",
                "Git",
                "references remote sidebar rechercher",
            ),
            CommandEntry::new(
                Command::Worktrees,
                "Consulter les worktrees",
                "Git",
                "arbre travail branches",
            ),
            CommandEntry::new(
                Command::Stashes,
                "Consulter les stashes",
                "Git",
                "stash modifications sauvegardées",
            ),
        ];
        entries.push(
            CommandEntry::new(
                Command::WorkingTree,
                "Consulter les modifications locales",
                "Git",
                "working tree staged unstaged fichiers",
            )
            .scoped(scope)
            .unavailable(
                unavailable.or((!self.history.read(cx).has_working_tree_changes())
                    .then_some("Aucune modification locale")),
            ),
        );
        let has_remotes = self
            .repository
            .as_ref()
            .is_some_and(|repo| !repo.remotes.is_empty());
        entries.push(
            CommandEntry::new(
                Command::Fetch,
                "Récupérer les références distantes",
                "Git",
                "fetch remote actualiser",
            )
            .scoped(scope)
            .unavailable(unavailable.or(if self.fetching {
                Some("Fetch en cours")
            } else if !has_remotes {
                Some("Aucun remote configuré")
            } else {
                None
            })),
        );
        if let Some(repo) = &self.repository {
            if !repo.solo_refs.is_empty() {
                repository_commands.push(CommandEntry::new(
                    Command::ClearSolo,
                    "Quitter le mode Solo",
                    "Git",
                    "branches graphe toutes",
                ));
            }
            if let Some(sha) = inspector.selection_sha() {
                repository_commands.push(
                    CommandEntry::new(
                        Command::CopySha(sha.into()),
                        "Copier le SHA du commit",
                        "Git",
                        "copy hash presse papiers",
                    )
                    .detail(short_sha(sha)),
                );
                for reference in repo.references.iter().filter(|reference| {
                    reference.sha == sha && matches!(reference.kind.as_str(), "local" | "remote")
                }) {
                    let active = repo.solo_refs.contains(&reference.name);
                    repository_commands.push(
                        CommandEntry::new(
                            Command::ToggleSolo(reference.name.clone()),
                            format!(
                                "{} du mode Solo : {}",
                                if active { "Retirer" } else { "Ajouter" },
                                reference.label
                            ),
                            "Git",
                            "branch branche solo isoler",
                        )
                        .active(active),
                    );
                }
                for parent in inspector.command_parents() {
                    repository_commands.push(
                        CommandEntry::new(
                            Command::NavigateCommit(parent.clone()),
                            format!("Aller au commit parent : {}", short_sha(&parent)),
                            "Git",
                            "parent navigation",
                        )
                        .detail(parent),
                    );
                }
            }
            let sidebar = self.sidebar.read(cx);
            if !sidebar.context.worktrees.loading
                && let Some(list) = &sidebar.context.worktrees.data
            {
                for item in &list.worktrees {
                    repository_commands.push(
                        CommandEntry::new(
                            Command::OpenWorktree(item.path.clone()),
                            format!("Ouvrir le worktree : {}", item.head_name),
                            "Worktrees",
                            "branch branche arbre",
                        )
                        .detail(item.path.clone())
                        .active(item.path == repo.path)
                        .unavailable((!item.available).then_some("Ce worktree est indisponible")),
                    );
                }
            }
            if !sidebar.context.stashes.loading
                && let Some(list) = &sidebar.context.stashes.data
            {
                for stash in &list.stashes {
                    repository_commands.push(
                        CommandEntry::new(
                            Command::InspectStash(stash.sha.clone()),
                            format!("Consulter stash@{{{}}} : {}", stash.index, stash.message),
                            "Stashes",
                            "stash sauvegarde",
                        )
                        .detail(short_sha(&stash.sha))
                        .active(inspector.selected_stash() == Some(stash.sha.as_str())),
                    );
                }
            }
            if let Some((path, commit)) = inspector.command_file() {
                for (mode, label, keywords) in [
                    (FileView::Diff, "Voir le diff du fichier", "patch diff"),
                    (
                        FileView::File,
                        "Voir le contenu du fichier",
                        "content contenu",
                    ),
                    (
                        FileView::History,
                        "Voir l’historique du fichier",
                        "history log historique",
                    ),
                    (
                        FileView::Blame,
                        "Voir le blame du fichier",
                        "blame auteurs lignes",
                    ),
                ] {
                    if commit || mode == FileView::Diff {
                        repository_commands.push(
                            CommandEntry::new(Command::FileView(mode), label, "Fichiers", keywords)
                                .detail(path),
                        );
                    }
                }
                repository_commands.push(
                    CommandEntry::new(
                        Command::Compact(!inspector.compact()),
                        if inspector.compact() {
                            "Afficher le diff dans la zone centrale"
                        } else {
                            "Afficher le diff dans le panneau des détails"
                        },
                        "Affichage",
                        "diff compact inspecteur",
                    )
                    .detail(path),
                );
            }
            if inspector.central_active() {
                repository_commands.push(
                    CommandEntry::new(
                        Command::CloseFile,
                        "Fermer la vue du fichier",
                        "Fichiers",
                        "diff retour historique",
                    )
                    .shortcut("Échap"),
                );
            }
        }
        entries.extend(repository_commands.into_iter().map(|entry| {
            let mut entry = entry.scoped(scope);
            if entry.disabled.is_none() {
                entry.disabled = unavailable.map(str::to_owned);
            }
            entry
        }));
        if self.error.is_some()
            || self.fetch_error.is_some()
            || self.history.read(cx).error().is_some()
        {
            entries.push(CommandEntry::new(
                Command::DismissError,
                "Fermer le message d’erreur",
                "Application",
                "dismiss error notification",
            ));
        }
        entries
    }

    pub(super) fn open_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.chrome.read(cx).can_show_settings(cx) {
            return;
        }
        if !self.palette_open {
            self.palette_previous_focus = window.focused(cx);
        }
        self.palette_open = true;
        let entries = self.command_entries(cx);
        self.command_palette
            .update(cx, |palette, cx| palette.open(entries, window, cx));
        self.sync_palette(cx);
        cx.notify();
    }
    pub(super) fn close_palette(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.palette_open {
            return;
        }
        self.palette_open = false;
        // The workspace root carries no keyboard behavior of its own.
        match self
            .palette_previous_focus
            .take()
            .filter(|previous| *previous != self.focus)
        {
            Some(previous) => previous.focus(window, cx),
            None => self.restore_focus(window, cx),
        }
        cx.notify();
    }
    pub(super) fn sync_palette(&mut self, cx: &mut Context<Self>) {
        if !self.palette_open {
            return;
        }
        let entries = self.command_entries(cx);
        let prefs = self.chrome.read(cx).preferences.clone();
        self.command_palette.update(cx, |palette, cx| {
            palette.sync(entries, prefs.palette(), prefs.font_size_offset, cx)
        });
    }
    fn show_workspace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.leave_settings(window, cx);
        self.chrome
            .update(cx, |chrome, cx| chrome.close_repository_browser(cx));
        cx.notify();
    }
    pub(super) fn execute_command(
        &mut self,
        entry: &CommandEntry,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.palette_open || !entry.is_current(&self.command_entries(cx)) {
            self.sync_palette(cx);
            return;
        }
        self.close_palette(window, cx);
        match &entry.command {
            Command::OpenRepository => self.pick_repository(window, cx),
            Command::NewTab => self.new_tab(window, cx),
            Command::CloseTab(id) => self.close_tab(*id, window, cx),
            Command::ActivateTab(id) => self.activate_tab(*id, window, cx),
            Command::CycleTab(step) => self.cycle_tab(*step, window, cx),
            Command::OpenRecent(path) => self.open(path.clone(), false, None, window, cx),
            Command::RemoveRecent(path) => self
                .chrome
                .update(cx, |chrome, cx| chrome.remove_recent(path, cx)),
            Command::BrowseRepositories => {
                self.chrome_event(&ChromeEvent::BrowseRepositories, window, cx)
            }
            Command::Settings(section) => {
                self.chrome_event(&ChromeEvent::ShowSettings, window, cx);
                if let Some(section) = section {
                    self.chrome
                        .read(cx)
                        .settings
                        .clone()
                        .update(cx, |view, cx| view.select_section(*section, window, cx));
                }
            }
            Command::Theme(theme) => self
                .chrome
                .update(cx, |chrome, cx| chrome.select_theme(*theme, window, cx)),
            Command::Zoom(delta) => self.chrome.update(cx, |chrome, cx| chrome.zoom(*delta, cx)),
            Command::DiffZoom(delta) => self.chrome.update(cx, |chrome, cx| {
                chrome
                    .preferences
                    .set_diff_font_size(chrome.preferences.diff_font_size + *delta as f32);
                chrome.prefs_changed(window, cx);
            }),
            Command::ResetPane(pane) => self.chrome.update(cx, |chrome, cx| {
                chrome.preferences.reset_pane(*pane);
                chrome.prefs_changed(window, cx);
            }),
            Command::ToggleImagePreviews => self.chrome.update(cx, |chrome, cx| {
                chrome.preferences.show_image_previews = !chrome.preferences.show_image_previews;
                chrome.settings.update(cx, |settings, cx| {
                    settings.set_image_previews(chrome.preferences.show_image_previews, cx)
                });
                chrome.prefs_changed(window, cx);
            }),
            Command::ToggleAutoFetch => self.chrome.update(cx, |chrome, cx| {
                chrome.preferences.auto_fetch_interval =
                    if chrome.preferences.auto_fetch_interval == 0 {
                        1
                    } else {
                        0
                    };
                chrome.settings.update(cx, |settings, cx| {
                    settings.set_interval(chrome.preferences.auto_fetch_interval, window, cx)
                });
                chrome.prefs_changed(window, cx);
            }),
            Command::DismissError => self.chrome_event(&ChromeEvent::DismissError, window, cx),
            Command::Quit => cx.quit(),
            Command::History | Command::CloseFile => {
                self.show_workspace(window, cx);
                self.inspector
                    .update(cx, |view, cx| view.close_workspace(cx));
                self.history
                    .update(cx, |view, cx| view.focus_list(window, cx));
            }
            Command::Search => {
                self.focus.focus(window, cx);
                window.dispatch_action(Box::new(Search), cx);
            }
            Command::WorkingTree => {
                self.show_workspace(window, cx);
                self.inspector
                    .update(cx, |view, cx| view.close_workspace(cx));
                self.select_working(cx);
                self.history
                    .update(cx, |view, cx| view.focus_list(window, cx));
            }
            Command::Refresh => self.refresh(window, cx),
            Command::Fetch => self.fetch(true, window, cx),
            Command::References => {
                self.show_workspace(window, cx);
                self.sidebar
                    .update(cx, |view, cx| view.focus_references(window, cx));
            }
            Command::Worktrees | Command::Stashes => {
                self.show_workspace(window, cx);
                self.sidebar.update(cx, |view, cx| {
                    view.show_context(entry.command == Command::Worktrees, window, cx)
                });
            }
            Command::OpenWorktree(path) => {
                self.sidebar_event(&SidebarEvent::OpenWorktree(path.clone()), window, cx)
            }
            Command::InspectStash(sha) => {
                self.show_workspace(window, cx);
                self.sidebar_event(&SidebarEvent::SelectStash(sha.clone()), window, cx);
                self.focus.focus(window, cx);
            }
            Command::ToggleSolo(name) => {
                self.show_workspace(window, cx);
                self.toggle_solo(Some(name.clone()), window, cx);
            }
            Command::ClearSolo => {
                self.show_workspace(window, cx);
                self.toggle_solo(None, window, cx);
            }
            Command::CopySha(sha) => cx.write_to_clipboard(ClipboardItem::new_string(sha.clone())),
            Command::NavigateCommit(sha) => {
                self.show_workspace(window, cx);
                self.history
                    .update(cx, |view, cx| view.jump_to(sha.clone(), window, cx));
            }
            Command::FileView(mode) => {
                self.show_workspace(window, cx);
                self.inspector
                    .update(cx, |view, cx| view.show_file_view(*mode, window, cx));
            }
            Command::Compact(compact) => {
                self.show_workspace(window, cx);
                self.inspector
                    .update(cx, |view, cx| view.set_compact(*compact, cx));
                self.focus.focus(window, cx);
            }
        }
        cx.notify();
    }
}
