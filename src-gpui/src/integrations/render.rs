//! Native settings and remote-repository views. Credentials and background
//! work remain owned by the parent view.
use super::*;
use crate::settings::ui::{
    self, CONTENT_MAX_WIDTH, PAGE_GAP, PAGE_PADDING, action, card, card_title, danger_action,
    field, field_label, input_control, link_action, note, page_header, primary_action,
};
use crate::theme::{RADIUS_LG, RADIUS_MD, RADIUS_SM, mix};
use gpui_kit::base::{RadioGroup, SelectableText};
use gpui_kit::component::spinner::Spinner;

impl IntegrationsView {
    /// The message, confirmation or error reported at `spot`, with a way to
    /// stop the operation it describes.
    fn feedback_at(&self, spot: &Spot, cx: &mut Context<Self>) -> Option<Div> {
        let feedback = self.feedback.as_ref().filter(|feedback| {
            &feedback.spot == spot
                // The repository select shows its own loading state.
                && !(feedback.tone == Tone::Progress
                    && self.pending == Some(PendingOperation::RepositoryRead))
        })?;
        let cancellable = feedback.tone == Tone::Progress
            && self.pending.is_some_and(|operation| {
                !matches!(
                    operation,
                    PendingOperation::Disconnect | PendingOperation::RepositoryRead
                )
            });
        Some(
            div()
                .flex()
                .items_center()
                .gap(px(12.))
                .child(
                    ui::feedback(
                        feedback.tone,
                        feedback.text.clone(),
                        self.palette,
                        self.offset,
                    )
                    .flex_1(),
                )
                .when(cancellable, |row| {
                    row.child(
                        action("remote-cancel", "Annuler", self.palette, self.offset).on_click(
                            cx.listener(|this, _, window, cx| this.cancel_pending(window, cx)),
                        ),
                    )
                }),
        )
    }

    fn clone_content(&self, repository: Option<&Repository>, cx: &mut Context<Self>) -> Div {
        let p = self.palette;
        let offset = self.offset;
        let remote = self.clone_remote(cx);
        let ssh = remote
            .as_ref()
            .is_some_and(|url| url.starts_with("ssh://") || !url.starts_with("https://"));
        let parent = expand_home(&self.clone_parent.read(cx).value());
        let name = self.clone_name.read(cx).value().trim().to_string();
        let destination = (parent.is_absolute() && valid_clone_directory_name(&name))
            .then(|| display_path(&parent.join(&name).to_string_lossy()));
        let mut form = card(p)
            .gap(px(18.))
            .child(card_title(
                if self.direct_clone {
                    "Depuis une URL"
                } else {
                    "Depuis un compte connecté"
                },
                offset,
            ))
            .child(clone_row(
                "Dossier parent",
                p,
                offset,
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .child(
                        input_control(
                            &self.clone_parent,
                            "clone-parent",
                            "Dossier parent",
                            p,
                            offset,
                        )
                        .flex_1()
                        .min_w_0(),
                    )
                    .child(
                        action("clone-browse", "Parcourir…", p, offset)
                            .disabled(self.pending.is_some() || self.picking_path)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.pick_path(PathPickerTarget::CloneParent, window, cx)
                            })),
                    ),
            ));
        if self.direct_clone {
            form = form.child(clone_row(
                "URL du dépôt",
                p,
                offset,
                input_control(
                    &self.clone_url,
                    "clone-url",
                    "URL du dépôt (HTTPS ou SSH)",
                    p,
                    offset,
                ),
            ));
        } else {
            let loading = self.pending == Some(PendingOperation::RepositoryRead);
            let load_state = self.repository_load_state();
            let load_trigger =
                RepositoryLoadTrigger::new(cx.entity().downgrade(), self.repository_select_scope);
            form = form.child(clone_row(
                "Dépôt à cloner",
                p,
                offset,
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(
                        div()
                            .relative()
                            .w_full()
                            .h(px(36.))
                            .child(
                                Select::new(&self.repository_select)
                                    .id("repository-select")
                                    .w_full()
                                    .h(px(36.))
                                    .px(px(12.))
                                    .py(px(6.))
                                    .border_1()
                                    .border_color(rgb(ui::field_border(p)))
                                    .rounded(RADIUS_MD)
                                    .bg(rgb(p.bg))
                                    .text_color(rgb(p.text))
                                    .text_size(px(12. + offset))
                                    .menu_max_h(px(300.))
                                    .cleanable(true)
                                    .when(loading, |select| select.pr(px(36.)))
                                    .placeholder(if loading && self.repositories.is_empty() {
                                        "Chargement des dépôts…"
                                    } else {
                                        "Choisir un dépôt…"
                                    })
                                    .search_placeholder("Rechercher un dépôt…")
                                    .accessibility_label(if loading {
                                        "Dépôt à cloner, chargement des dépôts"
                                    } else {
                                        "Dépôt à cloner"
                                    })
                                    .disabled(
                                        !self.can_select_repository()
                                            || (self.repositories.is_empty()
                                                && self.repositories_next.is_none()),
                                    )
                                    .empty(move |_, _| RepositoryEmpty {
                                        trigger: load_trigger.clone(),
                                        state: load_state,
                                        palette: p,
                                        offset,
                                    }),
                            )
                            .when(loading, |view| {
                                view.child(
                                    div()
                                        .absolute()
                                        .right(px(12.))
                                        .top(px(10.))
                                        .size(px(16.))
                                        .child(Spinner::new().small().color(rgb(p.accent).into())),
                                )
                            }),
                    )
                    .child(self.repository_list_controls(cx)),
            ));
        }
        form = form.child(clone_row(
            "Nouveau dossier",
            p,
            offset,
            div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(input_control(
                    &self.clone_name,
                    "clone-directory-name",
                    "Nouveau dossier",
                    p,
                    offset,
                ))
                .when_some(
                    destination.as_ref().filter(|_| remote.is_some()),
                    |view, destination| {
                        view.child(
                            note(format!("Destination : {destination}"), p)
                                .text_size(px(11. + offset)),
                        )
                    },
                ),
        ));
        if let Some(repository) = repository {
            let mut protocols = div().flex().items_center().gap(px(8.));
            for (id, label, ssh, available) in [
                (
                    "clone-protocol-ssh",
                    "SSH",
                    true,
                    repository.ssh_clone_url.is_some() && self.profile.ssh != SshChoice::Disabled,
                ),
                (
                    "clone-protocol-https",
                    "HTTPS",
                    false,
                    repository.clone_url.is_some(),
                ),
            ] {
                protocols = protocols.child(
                    action(id, label, p, offset)
                        .disabled(!available || self.pending.is_some())
                        .when(self.clone_ssh == ssh, |button| {
                            button
                                .bg(rgb(p.accent_soft))
                                .border_color(rgb(p.accent_border))
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.clone_ssh = ssh;
                            cx.notify();
                        })),
                );
            }
            form = form
                .child(clone_row("Protocole", p, offset, protocols))
                .when_some(remote.as_ref(), |view, remote| {
                    view.child(clone_row(
                        "",
                        p,
                        offset,
                        note(remote, p).text_size(px(11. + offset)),
                    ))
                });
        }
        if remote.is_some() {
            form = form.child(clone_row("", p, offset, note(if ssh { "Utilise l’agent ou la clé SSH de votre profil." } else { "Utilise les identifiants HTTPS de Git sur cet ordinateur. Le token de l’intégration reste réservé à l’API." }, p)));
        }
        form.child(clone_row(
            "",
            p,
            offset,
            div().flex().child(
                primary_action("clone-repository", "Cloner le dépôt", p, offset)
                    .disabled(
                        remote.is_none()
                            || destination.is_none()
                            || self.pending.is_some()
                            || self.picking_path,
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.clone_selected(cx))),
            ),
        ))
    }

    fn repository_list_controls(&self, cx: &mut Context<Self>) -> Div {
        let p = self.palette;
        let offset = self.offset;
        let control = if self.pending == Some(PendingOperation::RepositoryRead) {
            link_action("repositories-cancel", "Annuler", p, offset).on_click(cx.listener(
                |this, _, _, cx| {
                    if this.pending != Some(PendingOperation::RepositoryRead) {
                        return;
                    }
                    this.cancel();
                    // Keep the visible end-of-list sentinel from restarting the
                    // same page until the user explicitly retries.
                    this.repository_load_error = Some(mangrove_remote::RemoteError::Cancelled);
                    this.feedback = None;
                    cx.notify();
                },
            ))
        } else {
            link_action("repositories-reload", "Actualiser", p, offset)
                .disabled(self.pending.is_some() || self.picking_path)
                .on_click(
                    cx.listener(|this, _, _, cx| this.read(ReadRequest::Repositories(None), cx)),
                )
        };
        let description = count_label(self.repositories.len(), "dépôt chargé", "dépôts chargés");
        div()
            .flex()
            .items_center()
            .gap(px(8.))
            .child(
                note(description, p)
                    .flex_1()
                    .truncate()
                    .text_size(px(11. + offset)),
            )
            .child(control)
    }

    fn profiles_content(&self, cx: &mut Context<Self>) -> Div {
        let p = self.palette;
        let offset = self.offset;
        let mut profiles = div().flex().flex_col().gap(px(10.));
        for profile in &self.profiles {
            let id = profile.id.clone();
            let active = profile.id == self.profile.id;
            let row = div()
                .id(SharedString::from(format!("profile-row-{id}")))
                .w_full()
                .min_w_0()
                .flex()
                .items_center()
                .gap(px(12.))
                .p(px(14.))
                .rounded(RADIUS_LG)
                .border_1()
                .border_color(rgb(if active { p.accent_border } else { p.border }))
                .bg(rgb(if active {
                    ui::selected_fill(p)
                } else {
                    p.panel
                }));
            if self.renaming_profile.as_ref() == Some(&profile.id) {
                let empty = self.profile_rename.read(cx).value().trim().is_empty();
                profiles = profiles.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(8.))
                        .child(
                            row.child(profile_avatar(profile, p, 36.))
                                .child(
                                    input_control(
                                        &self.profile_rename,
                                        "profile-rename",
                                        "Nom du profil",
                                        p,
                                        offset,
                                    )
                                    .flex_1(),
                                )
                                .child(
                                    primary_action("profile-rename-save", "Enregistrer", p, offset)
                                        .disabled(empty)
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.save_profile_name(window, cx)
                                        })),
                                )
                                .child(
                                    link_action("profile-rename-cancel", "Annuler", p, offset)
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.cancel_profile_rename(window, cx)
                                        })),
                                ),
                        )
                        .children(self.feedback_at(&Spot::ProfileRename, cx)),
                );
                continue;
            }
            let activate_id = id.clone();
            let rename_id = id.clone();
            let delete_id = id.clone();
            let deleting = self.deleting_profile.as_ref() == Some(&profile.id);
            profiles = profiles.child(
                row.child(profile_avatar(profile, p, 36.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .truncate()
                                    .child(profile.name.clone()),
                            )
                            .child(
                                note(
                                    if deleting {
                                        "Suppression…".to_string()
                                    } else {
                                        count_label(
                                            profile.accounts.len(),
                                            "compte connecté",
                                            "comptes connectés",
                                        )
                                    },
                                    p,
                                )
                                .text_size(px(11. + offset)),
                            ),
                    )
                    .when(active, |row| row.child(active_badge(p, offset)))
                    .when(!active, |row| {
                        row.child(
                            action(
                                format!("profile-activate-{activate_id}"),
                                "Activer",
                                p,
                                offset,
                            )
                            .accessibility_label(format!("Activer le profil {}", profile.name))
                            .disabled(deleting)
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.activate_profile(activate_id.clone(), cx)
                                },
                            )),
                        )
                    })
                    .child(
                        link_action(format!("profile-rename-{rename_id}"), "Renommer", p, offset)
                            .accessibility_label(format!("Renommer le profil {}", profile.name))
                            .disabled(deleting)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.start_profile_rename(rename_id.clone(), window, cx)
                            })),
                    )
                    .child(
                        link_action(
                            format!("profile-delete-{delete_id}"),
                            "Supprimer",
                            p,
                            offset,
                        )
                        .text_color(rgb(p.danger))
                        .accessibility_label(format!("Supprimer le profil {}", profile.name))
                        .disabled(!self.can_delete_profile(&profile.id))
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.confirm_profile_deletion(&delete_id, window, cx)
                            },
                        )),
                    ),
            );
        }
        let profiles = profiles
            .children(self.feedback_at(&Spot::ProfileList, cx))
            .child(
                note(
                    "Activer un profil ferme les onglets de dépôts ouverts. Le profil actif ne peut pas être supprimé ; supprimer un profil retire ses comptes du coffre du système.",
                    p,
                )
                .text_size(px(11. + offset)),
            );
        let creating = self.profile_name.read(cx).value().trim().is_empty();
        let new_profile = card(p)
            .child(card_title("Nouveau profil", offset))
            .child(
                div()
                    .flex()
                    .items_end()
                    .gap(px(12.))
                    .child(field(
                        &self.profile_name,
                        "profile-name",
                        "Nom du profil",
                        p,
                        offset,
                    ))
                    .child(
                        primary_action("profile-create", "Créer", p, offset)
                            .disabled(creating)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.create_profile(window, cx)),
                            ),
                    ),
            )
            .children(self.feedback_at(&Spot::NewProfile, cx));
        let mut favorites = card(p)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(card_title("Favoris", offset))
                    .child(note(
                        format!(
                            "Dépôts du profil « {} » proposés sur l’accueil. Un favori local s’ouvre directement.",
                            self.profile.name
                        ),
                        p,
                    )),
            );
        for (index, favorite) in self.profile.favorites.iter().take(MAX_ROWS).enumerate() {
            let value = favorite.clone();
            favorites = favorites.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .child(icon(IconName::FolderGit2, 16., p.muted))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .whitespace_normal()
                            .child(display_path(&value)),
                    )
                    .when(PathBuf::from(favorite).is_absolute(), |view| {
                        let path = favorite.clone();
                        view.child(
                            action(format!("favorite-open-{index}"), "Ouvrir", p, offset)
                                .accessibility_label(format!("Ouvrir {}", display_path(&path)))
                                .on_click(cx.listener(move |_, _, _, cx| {
                                    cx.emit(IntegrationsEvent::OpenLocalFavorite(path.clone()))
                                })),
                        )
                    })
                    .child(
                        link_action(format!("favorite-remove-{index}"), "Retirer", p, offset)
                            .accessibility_label(format!(
                                "Retirer {} des favoris",
                                display_path(&value)
                            ))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.profile.favorites.retain(|favorite| favorite != &value);
                                this.feedback = None;
                                cx.emit(IntegrationsEvent::FavoritesChanged(
                                    this.profile.favorites.clone(),
                                ));
                                cx.notify();
                            })),
                    ),
            );
        }
        let adding = self.favorite.read(cx).value().trim().is_empty();
        let favorites = favorites
            .child(
                div()
                    .flex()
                    .items_end()
                    .gap(px(12.))
                    .child(field(
                        &self.favorite,
                        "profile-favorite",
                        "Ajouter un dépôt local ou distant",
                        p,
                        offset,
                    ))
                    .child(
                        action("favorite-browse", "Parcourir…", p, offset)
                            .disabled(self.pending.is_some() || self.picking_path)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.pick_path(PathPickerTarget::Favorite, window, cx)
                            })),
                    )
                    .child(
                        action("favorite-add", "Ajouter", p, offset)
                            .disabled(adding)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.add_favorite(window, cx)),
                            ),
                    ),
            )
            .children(self.feedback_at(&Spot::Favorite, cx));
        div()
            .flex()
            .flex_col()
            .gap(px(PAGE_GAP))
            .child(page_header(
                "Profils",
                "Chaque profil garde ses comptes, ses favoris, sa clé SSH, son apparence et ses dépôts récents.",
                p,
                offset,
            ))
            .child(profiles)
            .child(new_profile)
            .child(favorites)
    }

    fn ssh_content(&self, cx: &mut Context<Self>) -> Div {
        let p = self.palette;
        let offset = self.offset;
        let key_selected =
            self.choosing_key || matches!(self.profile.ssh, SshChoice::KeyFile { .. });
        let options = [
            (
                "ssh-agent",
                "Agent SSH",
                !key_selected && self.profile.ssh == SshChoice::Agent,
            ),
            ("ssh-key", "Clé privée", key_selected),
            (
                "ssh-disabled",
                "Désactivé",
                !key_selected && self.profile.ssh == SshChoice::Disabled,
            ),
        ];
        let mut group = RadioGroup::new("ssh-mode")
            .axis(Axis::Horizontal)
            .flex()
            .gap(px(8.))
            .on_key_down(cx.listener(Self::ssh_mode_key));
        for (index, (id, label, checked)) in options.into_iter().enumerate() {
            group = group.child(
                ui::choice(id, checked, p)
                    .track_focus(&self.ssh_focus[index])
                    .tab_stop(checked)
                    .set_position(index + 1, options.len())
                    .accessibility_label(label)
                    .gap(px(8.))
                    .min_h(px(ui::CONTROL_HEIGHT))
                    .px(px(14.))
                    .text_size(px(12. + offset))
                    .text_color(rgb(if checked { p.text } else { p.muted }))
                    .when(checked, |radio| radio.font_weight(FontWeight::MEDIUM))
                    .child(radio_mark(checked, p))
                    .child(label)
                    .on_change({
                        let view = cx.entity().downgrade();
                        move |_, _, window, cx| {
                            let _ = view.update(cx, |this, cx| {
                                this.choose_ssh_mode(index, true, window, cx)
                            });
                        }
                    }),
            );
        }
        let explanation = if key_selected {
            "Utilise un fichier de clé privée existant. Une clé protégée par une passphrase doit être chargée dans l’agent SSH."
        } else if self.profile.ssh == SshChoice::Agent {
            "Utilise l’agent SSH déjà démarré sur cet ordinateur et les identités qu’il a chargées."
        } else {
            "Les consultations et les clones SSH sont désactivés pour ce profil."
        };
        let mut authentication = card(p)
            .child(card_title("Authentification", offset))
            .child(group)
            .child(note(explanation, p))
            .children(self.feedback_at(&Spot::Ssh, cx));
        if key_selected {
            let missing = match &self.profile.ssh {
                SshChoice::KeyFile { path } => !path.is_file(),
                _ => false,
            };
            let empty = self.key_path.read(cx).value().trim().is_empty();
            authentication = authentication.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .pt(px(16.))
                    .border_t_1()
                    .border_color(rgb(p.border))
                    .child(
                        div()
                            .flex()
                            .items_end()
                            .gap(px(12.))
                            .child(field(
                                &self.key_path,
                                "ssh-key-path",
                                "Fichier de la clé privée",
                                p,
                                offset,
                            ))
                            .child(
                                action("ssh-key-browse", "Parcourir…", p, offset)
                                    .disabled(self.pending.is_some() || self.picking_path)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.pick_path(PathPickerTarget::SshKey, window, cx)
                                    })),
                            )
                            .child(
                                primary_action("ssh-key-save", "Utiliser cette clé", p, offset)
                                    .disabled(empty)
                                    .on_click(cx.listener(|this, _, _, cx| this.save_ssh_key(cx))),
                            ),
                    )
                    .children(self.feedback_at(&Spot::SshKey, cx).or_else(|| {
                        missing.then(|| {
                            ui::feedback(
                                Tone::Error,
                                "Le fichier de cette clé est introuvable. Choisissez-en un autre ou passez à l’agent SSH.",
                                p,
                                offset,
                            )
                        })
                    })),
            );
        }
        let reading = self.pending == Some(PendingOperation::Read);
        let remote_empty = self.ssh_remote.read(cx).value().trim().is_empty();
        let mut check = card(p)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(card_title("Vérifier un accès SSH", offset))
                    .child(note(
                        "Liste les branches et les tags du serveur sans télécharger d’objets Git.",
                        p,
                    )),
            )
            .child(
                div()
                    .flex()
                    .items_end()
                    .gap(px(12.))
                    .child(field(
                        &self.ssh_remote,
                        "ssh-remote",
                        "URL SSH du dépôt",
                        p,
                        offset,
                    ))
                    .child(
                        primary_action("ssh-read-references", "Vérifier", p, offset)
                            .accessibility_label("Vérifier l’accès et lister les références")
                            .disabled(
                                remote_empty
                                    || self.profile.ssh == SshChoice::Disabled
                                    || (self.pending.is_some() && !reading),
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.read_ssh(cx))),
                    ),
            )
            .child(
                note(
                    if self.profile.ssh == SshChoice::Disabled && self.choosing_key {
                        "Retenez d’abord la clé avec « Utiliser cette clé » pour vérifier un accès."
                    } else if self.profile.ssh == SshChoice::Disabled {
                        "Choisissez l’agent SSH ou une clé privée pour vérifier un accès."
                    } else {
                        "Utilisez le nom réel du serveur, déjà présent dans known_hosts. Les alias et options de ~/.ssh/config ne sont pas utilisés."
                    },
                    p,
                )
                .text_size(px(11. + offset)),
            )
            .children(self.feedback_at(&Spot::SshRemote, cx));
        if !self.ssh_references.is_empty() {
            check = check.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .p(px(12.))
                    .rounded(RADIUS_MD)
                    .border_1()
                    .border_color(rgb(p.border))
                    .bg(rgb(p.bg))
                    .font_family(crate::theme::MONO)
                    .text_size(px(11. + offset))
                    .children(
                        self.ssh_references
                            .iter()
                            .map(|reference| note(reference, p).text_color(rgb(p.text))),
                    ),
            );
        }
        div()
            .flex()
            .flex_col()
            .gap(px(PAGE_GAP))
            .child(page_header(
                "SSH",
                format!(
                    "Accès SSH du profil « {} », utilisé pour vérifier un accès et cloner en SSH.",
                    self.profile.name
                ),
                p,
                offset,
            ))
            .child(authentication)
            .child(check)
    }

    fn provider_grid(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let p = self.palette;
        let offset = self.offset;
        let mut grid = RadioGroup::new("provider-editions")
            .axis(Axis::Horizontal)
            .grid()
            .grid_cols(3)
            .gap(px(8.))
            .on_key_down(cx.listener(Self::provider_key));
        for (index, edition) in ProviderEdition::ALL.into_iter().enumerate() {
            let count = self
                .profile
                .accounts
                .iter()
                .filter(|account| edition.matches(account))
                .count();
            let selected = self.edition == edition;
            let status = if count == 0 {
                "Non connecté".to_string()
            } else {
                count_label(count, "compte connecté", "comptes connectés")
            };
            grid = grid.child(
                ui::choice(SharedString::from(format!("provider-{index}")), selected, p)
                    .track_focus(&self.provider_focus[index])
                    .tab_stop(selected)
                    .set_position(index + 1, ProviderEdition::ALL.len())
                    .disabled(self.pending == Some(PendingOperation::Disconnect))
                    .accessibility_label(format!("{}, {status}", edition.label()))
                    .min_w_0()
                    .gap(px(10.))
                    .px(px(12.))
                    .py(px(10.))
                    .child(provider_mark(edition, p, 28.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(
                                div()
                                    .text_size(px(12. + offset))
                                    .font_weight(FontWeight::MEDIUM)
                                    .whitespace_normal()
                                    .child(edition.label()),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(4.))
                                    .text_size(px(11. + offset))
                                    .text_color(rgb(if count > 0 { p.success } else { p.muted }))
                                    .when(count > 0, |line| {
                                        line.child(icon(IconName::CircleCheck, 12., p.success))
                                    })
                                    .child(div().min_w_0().truncate().child(status)),
                            ),
                    )
                    .on_change({
                        let view = cx.entity().downgrade();
                        move |_, _, window, cx| {
                            let _ = view
                                .update(cx, |this, cx| this.select_provider(edition, window, cx));
                        }
                    }),
            );
        }
        grid
    }

    fn integrations_content(&self, cx: &mut Context<Self>) -> Div {
        let p = self.palette;
        let offset = self.offset;
        let accounts: Vec<_> = self
            .profile
            .accounts
            .iter()
            .filter(|account| self.edition.matches(account))
            .collect();
        let mut content = div()
            .flex()
            .flex_col()
            .gap(px(PAGE_GAP))
            .child(page_header(
                "Intégrations",
                format!(
                    "Comptes du profil « {} ». Ils permettent de parcourir et de cloner vos dépôts depuis Nouvel onglet → Cloner.",
                    self.profile.name
                ),
                p,
                offset,
            ))
            .child(self.provider_grid(cx))
            .children(self.feedback_at(&Spot::Accounts, cx));
        for account in &accounts {
            let remove_id = account.id.clone();
            let refresh_id = account.id.clone();
            let cached = self.identities.get(&account.id);
            let display_name = cached
                .map(|identity| identity.display_name.as_str())
                .unwrap_or(&account.label);
            let identity = cached
                .map(|identity| identity.username.as_str())
                .or_else(|| {
                    account.username.as_deref().filter(|_| {
                        account.provider != Provider::BitbucketCloud
                            || matches!(
                                account.authentication,
                                Authentication::BitbucketOAuth { .. }
                            )
                    })
                });
            let method = match account.authentication {
                Authentication::GitLabOAuth { .. }
                | Authentication::GitHubOAuth { .. }
                | Authentication::BitbucketOAuth { .. } => "Connexion par navigateur",
                Authentication::PersonalToken => "Token personnel",
            };
            content = content.child(
                card(p)
                    .child(
                        div()
                            .flex()
                            .items_start()
                            .gap(px(16.))
                            .child(avatar(identity.unwrap_or(display_name), p, 48.))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .gap(px(4.))
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(6.))
                                            .text_size(px(11. + offset))
                                            .text_color(rgb(p.success))
                                            .child(icon(IconName::CircleCheck, 13., p.success))
                                            .child("Connecté"),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(15. + offset))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .whitespace_normal()
                                            .child(display_name.to_string()),
                                    )
                                    .when_some(identity, |view, name| {
                                        view.child(note(format!("@{name}"), p))
                                    })
                                    .child(
                                        note(&account.server_url, p).text_size(px(11. + offset)),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .flex_wrap()
                            .gap(px(12.))
                            .pt(px(14.))
                            .border_t_1()
                            .border_color(rgb(p.border))
                            .child(
                                div()
                                    .flex_1()
                                    .flex()
                                    .items_center()
                                    .gap(px(7.))
                                    .text_color(rgb(p.muted))
                                    .text_size(px(11. + offset))
                                    .child(icon(IconName::ShieldCheck, 14., p.muted))
                                    .child(method),
                            )
                            .child(
                                link_action(
                                    format!("account-refresh-{refresh_id}"),
                                    "Vérifier le compte",
                                    p,
                                    offset,
                                )
                                .disabled(self.pending.is_some())
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        this.refresh_identity(refresh_id.clone(), cx)
                                    },
                                )),
                            )
                            .child(
                                danger_action(
                                    format!("disconnect-{remove_id}"),
                                    "Déconnecter…",
                                    p,
                                    offset,
                                )
                                .accessibility_label(format!(
                                    "Déconnecter le compte {display_name}"
                                ))
                                .disabled(self.pending.is_some())
                                .on_click(cx.listener(
                                    move |this, _, window, cx| {
                                        this.confirm_disconnect(remove_id.clone(), window, cx)
                                    },
                                )),
                            ),
                    )
                    .children(self.feedback_at(&Spot::Account(account.id.clone()), cx)),
            );
        }
        if !accounts.is_empty() && !self.adding_account {
            return content.child(
                div().flex().child(
                    link_action(
                        "account-add-another",
                        "Connecter un autre compte",
                        p,
                        offset,
                    )
                    .disabled(self.pending.is_some())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.adding_account = true;
                        this.feedback = None;
                        cx.notify();
                    })),
                ),
            );
        }
        content.child(self.connection_form(!accounts.is_empty(), cx))
    }

    fn connection_form(&self, has_accounts: bool, cx: &mut Context<Self>) -> Div {
        let p = self.palette;
        let offset = self.offset;
        let provider = self.edition.provider();
        let browser = supports_browser_connection(provider) && self.browser_connection;
        let mut form = card(p)
            .gap(px(18.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(card_title(
                        &if has_accounts {
                            format!("Ajouter un compte {}", self.edition.label())
                        } else {
                            format!("Connecter un compte {}", self.edition.label())
                        },
                        offset,
                    ))
                    .when(has_accounts, |view| {
                        view.child(
                            link_action("account-add-cancel", "Annuler", p, offset).on_click(
                                cx.listener(|this, _, window, cx| {
                                    this.cancel();
                                    this.adding_account = false;
                                    this.clear_connection_secrets(window, cx);
                                    this.feedback = None;
                                    cx.notify();
                                }),
                            ),
                        )
                    }),
            )
            .child(field(
                &self.label,
                "account-label",
                "Nom du compte (facultatif)",
                p,
                offset,
            ));
        if self.needs_server() {
            form = form.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(field(
                        &self.server,
                        "account-server",
                        if self.edition == ProviderEdition::BitbucketCloud {
                            "URL du workspace"
                        } else {
                            "URL du serveur"
                        },
                        p,
                        offset,
                    ))
                    .children(self.feedback_at(&Spot::Server, cx)),
            );
        }
        if browser {
            return form.child(self.browser_connection_form(cx));
        }
        if provider == Provider::BitbucketCloud {
            form = form.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(field(
                        &self.username,
                        "account-username",
                        "Adresse e-mail Atlassian",
                        p,
                        offset,
                    ))
                    .child(
                        note("Nécessaire avec un API token Atlassian.", p)
                            .text_size(px(11. + offset)),
                    ),
            );
        }
        let permissions = match provider {
            Provider::GitLab => "Droit nécessaire : read_api.",
            Provider::GitHub => {
                "Droits nécessaires : lecture des dépôts, des pull requests et des checks."
            }
            _ => "Droits nécessaires : lecture des dépôts, des pull requests et des statuts CI.",
        };
        form.child(
            div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .child(field(
                    &self.token,
                    "account-token",
                    "Token personnel",
                    p,
                    offset,
                ))
                .child(note(permissions, p).text_size(px(11. + offset))),
        )
        .child(
            div()
                .flex()
                .items_center()
                .flex_wrap()
                .gap(px(12.))
                .child(
                    primary_action("account-connect", "Connecter le compte", p, offset)
                        .disabled(!self.can_connect(cx))
                        .on_click(cx.listener(|this, _, window, cx| this.connect(window, cx))),
                )
                .when(supports_browser_connection(provider), |view| {
                    view.child(
                        link_action(
                            "oauth-browser-mode",
                            "Se connecter avec le navigateur",
                            p,
                            offset,
                        )
                        .disabled(self.pending.is_some())
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.set_browser_connection(true, window, cx)
                        })),
                    )
                }),
        )
        .children(self.feedback_at(&Spot::Connection, cx))
        .child(vault_note(p, offset))
    }

    fn browser_connection_form(&self, cx: &mut Context<Self>) -> Div {
        let p = self.palette;
        let offset = self.offset;
        let github = self.edition.provider() == Provider::GitHub;
        let bitbucket = self.edition.provider() == Provider::BitbucketCloud;
        let provider = self.edition.brand();
        let id_label = if github {
            "Client ID de l’application"
        } else if bitbucket {
            "Clé du consumer (Key)"
        } else {
            "Application ID"
        };
        let mut content = div().flex().flex_col().gap(px(16.)).child(note(
            if github {
                "Mangrove ouvre GitHub dans votre navigateur ; saisissez-y le code temporaire affiché ici pour autoriser la connexion."
            } else {
                "Mangrove ouvre votre navigateur pour que vous l’autorisiez. La connexion est conservée dans le coffre du système."
            },
            p,
        ));
        if github {
            content = content.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .child(div().flex().child(
                        ui::checkbox(
                            "github-private-repositories",
                            "Inclure les dépôts privés",
                            self.github_private_repos,
                            p,
                            offset,
                        )
                        .disabled(self.pending.is_some())
                        .on_change({
                            let view = cx.entity().downgrade();
                            move |_, _, _, cx| {
                                let _ = view.update(cx, |this, cx| {
                                    this.github_private_repos = !this.github_private_repos;
                                    cx.notify();
                                });
                            }
                        }),
                    ))
                    .child(
                        note(
                            if self.github_private_repos {
                                "GitHub demande alors un droit qui inclut l’écriture. Mangrove s’en sert uniquement pour consulter."
                            } else {
                                "Seuls les dépôts publics seront accessibles."
                            },
                            p,
                        )
                        .text_size(px(11. + offset)),
                    ),
            );
        }
        let mut credentials = div().flex().flex_col().gap(px(12.)).child(field(
            &self.client_id,
            "oauth-client-id",
            id_label,
            p,
            offset,
        ));
        if bitbucket {
            credentials = credentials.child(field(
                &self.consumer_secret,
                "oauth-consumer-secret",
                "Secret du consumer",
                p,
                offset,
            ));
        }
        if self.app_configured {
            // A known application only needs its credentials checked.
            content = content.child(credentials).child(
                div().flex().child(
                    Button::new("oauth-setup-toggle")
                        .gap(px(6.))
                        .px(px(4.))
                        .py(px(6.))
                        .border_1()
                        .border_color(transparent_black())
                        .rounded(RADIUS_MD)
                        .text_size(px(12. + offset))
                        .text_color(rgb(p.muted))
                        .cursor_pointer()
                        .accessibility_label(if self.setup_open {
                            "Masquer la création de l’application"
                        } else {
                            "Afficher la création de l’application"
                        })
                        .hover(move |style| style.text_color(rgb(p.text)))
                        .focus_visible(move |style| ui::focus_ring(style, p.accent))
                        .child(icon(
                            if self.setup_open {
                                IconName::ChevronDown
                            } else {
                                IconName::ChevronRight
                            },
                            14.,
                            p.muted,
                        ))
                        .child(format!("Créer une autre application {provider}"))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.setup_open = !this.setup_open;
                            cx.notify();
                        })),
                ),
            );
            if self.setup_open {
                content = content.child(self.application_setup(cx));
            }
        } else {
            // Without an application, the connection walks through its creation.
            content = content.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(16.))
                    .p(px(16.))
                    .rounded(RADIUS_LG)
                    .border_1()
                    .border_color(rgb(p.border))
                    .bg(rgb(p.bg))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.))
                            .child(field_label("Configuration requise", p, offset))
                            .child(note(
                                format!(
                                    "La connexion par navigateur passe par une application Mangrove enregistrée dans votre compte {provider}."
                                ),
                                p,
                            )),
                    )
                    .child(setup_step(
                        1,
                        &format!("Créez l’application dans {provider}"),
                        self.application_setup(cx),
                        p,
                        offset,
                    ))
                    .child(setup_step(
                        2,
                        if bitbucket {
                            "Collez sa clé et son secret"
                        } else {
                            "Collez son identifiant"
                        },
                        credentials,
                        p,
                        offset,
                    )),
            );
        }
        if let Some(authorization) = &self.github_authorization {
            content = content.child(self.github_code(authorization, cx));
        }
        content
            .child(
                div()
                    .flex()
                    .items_center()
                    .flex_wrap()
                    .gap(px(12.))
                    .child(
                        primary_action(
                            "oauth-browser-connect",
                            &format!("Se connecter avec {provider}"),
                            p,
                            offset,
                        )
                        .disabled(!self.can_connect(cx))
                        .on_click(cx.listener(|this, _, window, cx| this.connect(window, cx))),
                    )
                    .child(
                        link_action(
                            "oauth-personal-token",
                            "Utiliser un token personnel",
                            p,
                            offset,
                        )
                        .disabled(self.pending.is_some())
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.set_browser_connection(false, window, cx)
                        })),
                    ),
            )
            .children(self.feedback_at(&Spot::Connection, cx))
            .child(vault_note(p, offset))
    }

    /// How to register Mangrove with the provider, with the values to copy.
    fn application_setup(&self, cx: &mut Context<Self>) -> Div {
        let p = self.palette;
        let github = self.edition.provider() == Provider::GitHub;
        let bitbucket = self.edition.provider() == Provider::BitbucketCloud;
        let provider = self.edition.brand();
        let mut setup = div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(note(
                if github {
                    "Créez une OAuth App nommée Mangrove, cochez « Enable Device Flow », puis copiez son Client ID. Aucun secret n’est nécessaire."
                } else if bitbucket {
                    "Dans les paramètres du workspace, créez un consumer OAuth Mangrove avec les droits Account, Repositories et Pull requests en lecture, puis copiez sa Key et son Secret."
                } else {
                    "Créez une application Mangrove non confidentielle avec le droit read_api, puis copiez son Application ID. Aucun secret n’est nécessaire."
                },
                p,
            ))
            .child(
                div().flex().child(
                    action(
                        "oauth-create-application",
                        &format!("Ouvrir {provider}"),
                        p,
                        self.offset,
                    )
                    .accessibility_label(format!(
                        "Créer l’application dans {provider}, dans le navigateur"
                    ))
                    .disabled(
                        self.pending.is_some()
                            || (self.needs_server()
                                && self.server.read(cx).value().trim().is_empty()),
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.open_application_settings(cx))),
                ),
            );
        if bitbucket {
            setup = setup.child(self.copyable_oauth_url(
                "oauth-bitbucket-callback-url",
                "Callback URL",
                oauth::REDIRECT_URI,
                cx,
            ));
        } else if github {
            setup = setup
                .child(self.copyable_oauth_url(
                    "oauth-homepage-url",
                    "Homepage URL",
                    "https://github.com/Dpblandin/mangrove",
                    cx,
                ))
                .child(self.copyable_oauth_url(
                    "oauth-callback-url",
                    "Authorization callback URL",
                    oauth::REDIRECT_URI,
                    cx,
                ))
                .child(
                    note(
                        "GitHub exige cette URL de callback, mais la connexion par code ne l’utilise pas.",
                        p,
                    )
                    .text_size(px(11. + self.offset)),
                );
        } else {
            setup = setup.child(self.copyable_oauth_url(
                "oauth-redirect-url",
                "Redirect URI",
                oauth::REDIRECT_URI,
                cx,
            ));
        }
        setup
    }

    fn github_code(&self, authorization: &GitHubAuthorization, cx: &mut Context<Self>) -> Div {
        let p = self.palette;
        let offset = self.offset;
        let generation = self.generation;
        div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .p(px(16.))
            .rounded(RADIUS_LG)
            .border_1()
            .border_color(rgb(p.accent_border))
            .bg(rgb(p.accent_soft))
            .child(note("Code à saisir dans GitHub", p))
            .child(
                div()
                    .text_size(px(24. + offset))
                    .font_weight(FontWeight::SEMIBOLD)
                    .font_family(crate::theme::MONO)
                    .child(authorization.user_code.expose_secret().to_string()),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap(px(10.))
                    .child(
                        action(
                            "github-device-copy",
                            if authorization.copied {
                                "Code copié"
                            } else {
                                "Copier le code"
                            },
                            p,
                            offset,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if this.generation != generation {
                                return;
                            }
                            let Some(authorization) = &mut this.github_authorization else {
                                return;
                            };
                            cx.write_to_clipboard(ClipboardItem::new_string(
                                authorization.user_code.expose_secret().to_string(),
                            ));
                            authorization.copied = true;
                            cx.notify();
                        })),
                    )
                    .child(
                        action("github-device-open", "Rouvrir GitHub", p, offset).on_click(
                            cx.listener(move |this, _, _, cx| {
                                if this.generation != generation {
                                    return;
                                }
                                if let Some(authorization) = &this.github_authorization {
                                    cx.open_url(&authorization.verification_url);
                                }
                            }),
                        ),
                    ),
            )
            .child(note(
                "En attente de votre autorisation. Ce code expire après quelques minutes.",
                p,
            ))
    }

    /// A value to paste into the provider's form, named by that form's field.
    fn copyable_oauth_url(
        &self,
        id: &'static str,
        field: &'static str,
        value: &'static str,
        cx: &mut Context<Self>,
    ) -> Div {
        let p = self.palette;
        let label = format!("Champ « {field} »");
        div()
            .flex()
            .items_center()
            .gap(px(12.))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .text_size(px(11. + self.offset))
                    .child(note(label, p))
                    .child(
                        div()
                            .min_w_0()
                            .whitespace_normal()
                            .line_height(relative(1.6))
                            .text_color(rgb(p.text))
                            .font_family(crate::theme::MONO)
                            .cursor(CursorStyle::IBeam)
                            .child(
                                SelectableText::new(id, value)
                                    .selection_color(rgb(p.selection).into()),
                            ),
                    ),
            )
            .child(
                action(
                    format!("{id}-copy"),
                    if self.copied == Some(id) {
                        "Copié"
                    } else {
                        "Copier"
                    },
                    p,
                    self.offset,
                )
                .accessibility_label(format!("Copier la valeur du champ « {field} »"))
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(value.to_string()));
                    this.copied = Some(id);
                    this.copied_task = Some(cx.spawn(async move |this, cx| {
                        cx.background_executor()
                            .timer(std::time::Duration::from_secs(2))
                            .await;
                        let _ = this.update(cx, |this, cx| {
                            this.copied = None;
                            cx.notify();
                        });
                    }));
                    cx.notify();
                })),
            )
    }

    fn accounts_rail(&self, cx: &mut Context<Self>) -> Div {
        let p = self.palette;
        let mut rail = div().w_full().p(px(12.)).child(
            div()
                .px(px(8.))
                .py(px(12.))
                .text_size(px(11. + self.offset))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(p.muted))
                .child("COMPTES CONNECTÉS"),
        );
        rail = rail.child(
            Button::new("browse-direct-url")
                .w_full()
                .min_w_0()
                .px(px(10.))
                .py(px(12.))
                .gap(px(10.))
                .rounded(RADIUS_MD)
                .justify_start()
                .bg(rgb(if self.direct_clone {
                    p.selection
                } else {
                    p.panel
                }))
                .text_color(rgb(p.text))
                .cursor_pointer()
                .accessibility_label("Cloner avec une URL")
                .disabled(self.direct_clone || self.cloning())
                .hover(move |style| style.bg(rgb(p.hover)))
                .focus_visible(move |style| style.bg(rgb(p.selection)))
                .child(icon(IconName::Link, 18., p.muted))
                .child("URL d’un dépôt")
                .on_click(cx.listener(|this, _, window, cx| this.select_direct_clone(window, cx))),
        );
        for account in &self.profile.accounts {
            let id = account.id.clone();
            let edition = ProviderEdition::ALL
                .into_iter()
                .find(|edition| edition.matches(account))
                .unwrap_or(ProviderEdition::GitLab);
            rail = rail.child(
                Button::new(SharedString::from(format!("browse-account-{id}")))
                    .w_full()
                    .min_w_0()
                    .px(px(10.))
                    .py(px(12.))
                    .gap(px(10.))
                    .rounded(RADIUS_MD)
                    .justify_start()
                    .bg(rgb(if self.selected_account.as_ref() == Some(&id) {
                        p.selection
                    } else {
                        p.panel
                    }))
                    .cursor_pointer()
                    .accessibility_label(format!("Parcourir les dépôts de {}", account.label))
                    .disabled(self.selected_account.as_ref() == Some(&id) || self.cloning())
                    .hover(move |style| style.bg(rgb(p.hover)))
                    .focus_visible(move |style| style.bg(rgb(p.selection)))
                    .child(provider_mark(edition, p, 28.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_left()
                            .child(
                                div()
                                    .font_weight(FontWeight::MEDIUM)
                                    .whitespace_normal()
                                    .child(account.label.clone()),
                            )
                            .child(note(edition.label(), p).text_size(px(11. + self.offset))),
                    )
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.select_account(id.clone(), cx)),
                    ),
            );
        }
        let rail = rail.child(
            div().mt(px(18.)).flex().child(
                link_action(
                    "browse-manage-connections",
                    "Gérer les connexions",
                    p,
                    self.offset,
                )
                .on_click(
                    cx.listener(|_, _, _, cx| cx.emit(IntegrationsEvent::OpenIntegrationSettings)),
                ),
            ),
        );
        div()
            .relative()
            .w(px(228.))
            .flex_none()
            .h_full()
            .bg(rgb(p.panel))
            .border_r_1()
            .border_color(rgb(p.border))
            .child(
                div()
                    .id("accounts-rail-scroll")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.rail_scroll)
                    .child(rail),
            )
            .child(scrollbar_theme(
                Scrollbar::vertical(&self.rail_scroll).id("accounts-rail-scrollbar"),
            ))
    }

    fn repositories_content(&self, cx: &mut Context<Self>) -> Div {
        let p = self.palette;
        let offset = self.offset;
        let mut content = div()
            .flex()
            .flex_col()
            .gap(px(20.))
            .child(self.clone_content(self.selected_repository.as_ref(), cx));
        if self.direct_clone || self.profile.accounts.is_empty() {
            return content.when(self.profile.accounts.is_empty(), |view| {
                view.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(12.))
                        .child(note(
                            "Vous pouvez aussi parcourir les dépôts d’un compte connecté.",
                            p,
                        ))
                        .child(
                            link_action("browse-connect-account", "Connecter un compte", p, offset)
                                .on_click(cx.listener(|_, _, _, cx| {
                                    cx.emit(IntegrationsEvent::OpenIntegrationSettings)
                                })),
                        ),
                )
            });
        }
        if let Some(repository) = &self.selected_repository {
            let favorite = repository
                .web_url
                .clone()
                .unwrap_or_else(|| repository.full_name.clone());
            content = content.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(12.))
                    .child(title(&format!("PR/MR · {}", repository.full_name)))
                    .child(
                        link_action(
                            "selected-repository-favorite",
                            if self.profile.favorites.contains(&favorite) {
                                "Retirer des favoris"
                            } else {
                                "Ajouter aux favoris"
                            },
                            p,
                            offset,
                        )
                        .disabled(self.pending.is_some() || self.picking_path)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if let Some(index) = this
                                .profile
                                .favorites
                                .iter()
                                .position(|value| value == &favorite)
                            {
                                this.profile.favorites.remove(index);
                            } else if this.profile.favorites.len() < MAX_ROWS {
                                this.profile.favorites.push(favorite.clone());
                            }
                            cx.emit(IntegrationsEvent::FavoritesChanged(
                                this.profile.favorites.clone(),
                            ));
                            cx.notify();
                        })),
                    ),
            );
            let selected = repository.clone();
            content = content.child(
                div().flex().child(
                    action(
                        "changes-load",
                        if self.changes_loaded {
                            "Actualiser les PR/MR"
                        } else {
                            "Consulter les PR/MR"
                        },
                        p,
                        offset,
                    )
                    .disabled(self.pending.is_some())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.read(ReadRequest::Changes(selected.clone(), None), cx)
                    })),
                ),
            );
            if self.changes_loaded && self.changes.is_empty() && self.pending.is_none() {
                content = content.child(note("Aucune PR/MR ouverte pour ce dépôt.", p));
            }
            for (index, change) in self.changes.iter().enumerate() {
                let change = change.clone();
                content = content.child(
                    action(
                        format!("change-{index}"),
                        &format!("#{} · {} · {}", change.id, change.title, change.state),
                        p,
                        offset,
                    )
                    .disabled(self.pending == Some(PendingOperation::Clone))
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.select_change(change.clone(), cx)),
                    ),
                );
            }
            if let Some(cursor) = self.changes_next.clone() {
                let repository = repository.clone();
                content = content.child(
                    action("changes-next", "Page suivante des PR/MR", p, offset)
                        .disabled(self.pending.is_some())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.read(
                                ReadRequest::Changes(repository.clone(), Some(cursor.clone())),
                                cx,
                            )
                        })),
                );
            }
        }
        if let (Some(repository), Some(change)) = (&self.selected_repository, &self.selected_change)
        {
            content = content
                .child(title(&change.title))
                .child(note(&change.description, p))
                .child(title("Discussions"));
            for discussion in &self.discussions {
                for comment in discussion.comments.iter().take(MAX_ROWS) {
                    content = content.child(
                        div()
                            .border_l_2()
                            .border_color(rgb(p.border))
                            .pl(px(12.))
                            .child(
                                div()
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(comment.author.clone()),
                            )
                            .child(note(&comment.body, p)),
                    );
                }
            }
            if self.discussions.is_empty() && self.pending.is_none() {
                content = content.child(note("Aucune discussion chargée.", p));
            }
            if let Some(cursor) = self.discussions_next.clone() {
                let repository = repository.clone();
                let change = change.clone();
                content = content.child(
                    action(
                        "discussions-next",
                        "Page suivante des discussions",
                        p,
                        offset,
                    )
                    .disabled(self.pending.is_some())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.read(
                            ReadRequest::Discussions(
                                repository.clone(),
                                change.clone(),
                                Some(cursor.clone()),
                            ),
                            cx,
                        )
                    })),
                );
            }
            content = content.child(title("Résultats CI"));
            if let Some(sha) = change.head_sha.clone() {
                let repository = repository.clone();
                content = content.child(
                    action("statuses-load", "Consulter les statuts CI", p, offset)
                        .disabled(self.pending.is_some())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.read(
                                ReadRequest::Statuses(repository.clone(), sha.clone(), None),
                                cx,
                            )
                        })),
                );
            } else {
                content = content.child(note("Révision source non fournie par cet hébergeur.", p));
            }
            for status in &self.statuses {
                content = content.child(
                    div()
                        .child(format!("{} · {}", status.name, status.state))
                        .child(note(&status.description, p)),
                );
            }
            if let (Some(cursor), Some(sha)) = (self.statuses_next.clone(), change.head_sha.clone())
            {
                let repository = repository.clone();
                content = content.child(
                    action("statuses-next", "Page suivante des statuts CI", p, offset)
                        .disabled(self.pending.is_some())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.read(
                                ReadRequest::Statuses(
                                    repository.clone(),
                                    sha.clone(),
                                    Some(cursor.clone()),
                                ),
                                cx,
                            )
                        })),
                );
            }
        }
        content
    }
}

impl Render for IntegrationsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_repository_select(window, cx);
        if self.profile_select_dirty {
            self.sync_profile_select(window, cx);
        }
        if std::mem::take(&mut self.key_draft_reset)
            && let SshChoice::KeyFile { path } = &self.profile.ssh
        {
            // An unapplied key path does not outlive the visit that typed it.
            let path = display_path(&path.to_string_lossy());
            self.key_path
                .update(cx, |input, cx| input.set_value(path, window, cx));
        }
        let p = self.palette;
        let content = match self.section {
            IntegrationsSection::Profiles => self.profiles_content(cx),
            IntegrationsSection::Ssh => self.ssh_content(cx),
            IntegrationsSection::Integrations => self.integrations_content(cx),
            IntegrationsSection::Repositories => self.repositories_content(cx),
        };
        let page_feedback = self.feedback_at(&Spot::Page, cx);
        div()
            .id("integrations-view")
            .track_focus(&self.focus_handle)
            // The first Escape leaves the field; the next one reaches the app.
            .on_action(cx.listener(|this, _: &InputEscape, window, cx| {
                if this
                    .profile_rename
                    .read(cx)
                    .focus_handle(cx)
                    .is_focused(window)
                {
                    this.cancel_profile_rename(window, cx);
                } else if this.input_focused(window, cx) {
                    this.focus_handle.focus(window, cx);
                } else {
                    cx.propagate();
                }
            }))
            .size_full()
            .min_w_0()
            .min_h_0()
            .flex()
            .bg(rgb(p.bg))
            .text_color(rgb(p.text))
            .text_size(px(12. + self.offset))
            .when(self.section == IntegrationsSection::Repositories, |view| {
                view.child(self.accounts_rail(cx))
            })
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_w_0()
                    .min_h_0()
                    .h_full()
                    .child(
                        div()
                            .id("integrations-scroll")
                            .size_full()
                            .overflow_y_scroll()
                            .track_scroll(&self.scroll)
                            .p(px(PAGE_PADDING))
                            .child(
                                div()
                                    .w_full()
                                    .max_w(px(CONTENT_MAX_WIDTH))
                                    .mx_auto()
                                    .when_some(page_feedback, |view, feedback| {
                                        view.child(
                                            div()
                                                .mb(px(20.))
                                                .p(px(14.))
                                                .border_1()
                                                .border_color(rgb(p.border))
                                                .rounded(RADIUS_MD)
                                                .bg(rgb(p.panel))
                                                .child(feedback),
                                        )
                                    })
                                    .child(content),
                            ),
                    )
                    .child(scrollbar_theme(
                        Scrollbar::vertical(&self.scroll).id("integrations-scrollbar"),
                    )),
            )
    }
}

fn title(label: &str) -> Div {
    card_title(label, 0.)
}

/// The active profile's status, distinct from the row's text actions.
fn active_badge(p: Palette, offset: f32) -> Div {
    div()
        .flex_none()
        .flex()
        .items_center()
        .gap(px(6.))
        .px(px(10.))
        .py(px(4.))
        .rounded_full()
        .bg(rgb(mix(p.success, p.panel, 0.16)))
        .text_size(px(11. + offset))
        .font_weight(FontWeight::MEDIUM)
        .text_color(rgb(p.success))
        .child(icon(IconName::CircleCheck, 12., p.success))
        .child("Profil actif")
}

/// The dot of a radio option drawn as a segmented button.
fn radio_mark(checked: bool, p: Palette) -> Div {
    div()
        .size(px(14.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .border_1()
        .border_color(rgb(if checked { p.accent } else { p.muted }))
        .when(checked, |mark| {
            mark.child(div().size(px(6.)).rounded_full().bg(rgb(p.accent)))
        })
}

fn setup_step(number: usize, label: &str, body: impl IntoElement, p: Palette, offset: f32) -> Div {
    div()
        .flex()
        .items_start()
        .gap(px(12.))
        .child(
            div()
                .size(px(22.))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded_full()
                .border_1()
                .border_color(rgb(p.accent_border))
                .text_size(px(11. + offset))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(rgb(p.accent))
                .child(number.to_string()),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(10.))
                .pt(px(2.))
                .child(field_label(label, p, offset))
                .child(body),
        )
}

fn vault_note(p: Palette, offset: f32) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(7.))
        .text_size(px(11. + offset))
        .text_color(rgb(p.muted))
        .child(icon(IconName::LockKeyhole, 13., p.muted))
        .child("Les identifiants sont conservés dans le coffre du système.")
}

fn avatar(label: &str, p: Palette, size: f32) -> Div {
    avatar_shape(size)
        .bg(rgb(p.accent_soft))
        .text_color(rgb(p.accent))
        .child(initials(label))
}

fn profile_avatar(profile: &Profile, p: Palette, size: f32) -> Div {
    let (color, background) = profile_colors(profile, p);
    avatar_shape(size)
        .bg(rgb(background))
        .text_color(rgb(color))
        .child(profile_monogram(&profile.name))
}

fn avatar_shape(size: f32) -> Div {
    div()
        .size(px(size))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .font_weight(FontWeight::SEMIBOLD)
        .text_size(px(size * 0.34))
}

fn provider_mark(edition: ProviderEdition, p: Palette, size: f32) -> Div {
    div()
        .size(px(size))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(RADIUS_SM)
        .border_1()
        .border_color(rgb(p.border))
        .child(match edition.provider() {
            Provider::GitHub => icon(IconName::Github, size * 0.65, p.muted).into_any_element(),
            Provider::GitLab => {
                static LOGO: std::sync::OnceLock<Arc<Image>> = std::sync::OnceLock::new();
                let logo = LOGO.get_or_init(|| {
                    Arc::new(Image::from_bytes(
                        ImageFormat::Svg,
                        include_bytes!("../../assets/brands/gitlab.svg").to_vec(),
                    ))
                });
                img(logo.clone()).size(px(size * 0.7)).into_any_element()
            }
            Provider::BitbucketCloud | Provider::BitbucketServer => {
                static LOGO: std::sync::OnceLock<Arc<Image>> = std::sync::OnceLock::new();
                let logo = LOGO.get_or_init(|| {
                    Arc::new(Image::from_bytes(
                        ImageFormat::Svg,
                        include_bytes!("../../assets/brands/bitbucket.svg").to_vec(),
                    ))
                });
                img(logo.clone()).size(px(size * 0.9)).into_any_element()
            }
        })
}

fn clone_row(label: &str, p: Palette, offset: f32, control: impl IntoElement) -> Div {
    div()
        .flex()
        .items_start()
        .gap(px(18.))
        .child(
            div()
                .w(px(132.))
                .flex_none()
                .pt(px(9.))
                .text_size(px(12. + offset))
                .text_color(rgb(p.muted))
                .whitespace_normal()
                .child(label.to_string()),
        )
        .child(div().flex_1().min_w_0().child(control))
}
