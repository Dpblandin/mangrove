//! Native settings and remote-repository views. Credentials and background
//! work remain owned by the parent view.
use super::*;
use crate::theme::{RADIUS_LG, RADIUS_MD, RADIUS_SM, mix};
use gpui_kit::base::SelectableText;
use gpui_kit::component::spinner::Spinner;

impl IntegrationsView {
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
            .child(subtitle(
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
                    "URL du dépôt · HTTPS ou SSH",
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
                                    .border_color(rgb(p.border))
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
                                        "Dépôt à cloner · chargement des dépôts"
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
                            note(&format!("Destination : {destination}"), p)
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
                    this.message = None;
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
                note(&description, p)
                    .flex_1()
                    .truncate()
                    .text_size(px(11. + offset)),
            )
            .child(control)
    }

    fn profiles_content(&self, cx: &mut Context<Self>) -> Div {
        let p = self.palette;
        let offset = self.offset;
        let mut content = div().flex().flex_col().gap(px(24.)).child(section_header(
            "Profils",
            "Un espace personnel pour vos comptes, préférences et dépôts récents.",
            p,
            offset,
        ));
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
                .gap(px(8.))
                .pr(px(12.))
                .rounded(RADIUS_LG)
                .border_1()
                .border_color(rgb(if active { p.accent_border } else { p.border }))
                .bg(rgb(if active { p.accent_soft } else { p.panel }));
            if self.renaming_profile.as_ref() == Some(&profile.id) {
                profiles = profiles.child(
                    row.p(px(12.))
                        .gap(px(12.))
                        .child(profile_avatar(profile, p, 36.))
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
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.save_profile_name(window, cx)
                                })),
                        )
                        .child(
                            link_action("profile-rename-cancel", "Annuler", p, offset).on_click(
                                cx.listener(|this, _, window, cx| {
                                    this.cancel_profile_rename(window, cx)
                                }),
                            ),
                        ),
                );
                continue;
            }
            let rename_id = id.clone();
            let delete_id = id.clone();
            let deleting = self.deleting_profile.as_ref() == Some(&profile.id);
            profiles = profiles.child(
                row.hover(move |style| style.bg(rgb(p.hover)))
                    .child(
                        Button::new(SharedString::from(format!("profile-{id}")))
                            .flex_1()
                            .min_w_0()
                            .p(px(16.))
                            .rounded(RADIUS_LG)
                            .border_1()
                            .border_color(transparent_black())
                            .justify_start()
                            .gap(px(14.))
                            .cursor_pointer()
                            .accessibility_label(format!("Sélectionner le profil {}", profile.name))
                            .focus_visible(move |style| style.border_color(rgb(p.accent)))
                            .disabled(deleting)
                            .styles(|styles| {
                                styles.disabled(|style| style.opacity(0.5).cursor_default())
                            })
                            .child(profile_avatar(profile, p, 36.))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_left()
                                    .child(
                                        div()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child(profile.name.clone()),
                                    )
                                    .child(
                                        note(
                                            &count_label(
                                                profile.accounts.len(),
                                                "compte connecté",
                                                "comptes connectés",
                                            ),
                                            p,
                                        )
                                        .text_size(px(11. + offset)),
                                    ),
                            )
                            .when(active, |view| {
                                view.child(badge("Actif", p.accent, p.accent_soft, offset))
                            })
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.select_profile(id.clone(), cx)
                            })),
                    )
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
                            if deleting {
                                "Suppression…"
                            } else {
                                "Supprimer"
                            },
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
        profiles = profiles.child(
            note(
                "Le profil actif ne peut pas être supprimé. Supprimer un profil retire ses comptes du coffre du système.",
                p,
            )
            .text_size(px(11. + offset)),
        );
        content = content.child(profiles).child(
            card(p).child(subtitle("Nouveau profil", offset)).child(
                div()
                    .flex()
                    .items_end()
                    .gap(px(12.))
                    .mt(px(14.))
                    .child(field(
                        &self.profile_name,
                        "profile-name",
                        "Nom du profil",
                        p,
                        offset,
                    ))
                    .child(
                        primary_action("profile-create", "Créer", p, offset).on_click(
                            cx.listener(|this, _, window, cx| this.create_profile(window, cx)),
                        ),
                    ),
            ),
        );
        let mut favorites = card(p)
            .gap(px(12.))
            .child(subtitle("Favoris", offset))
            .when(self.profile.favorites.is_empty(), |view| {
                view.child(note(
                    "Gardez vos dépôts les plus utilisés à portée de main.",
                    p,
                ))
            });
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
                            action(format!("favorite-open-{index}"), "Ouvrir", p, offset).on_click(
                                cx.listener(move |_, _, _, cx| {
                                    cx.emit(IntegrationsEvent::OpenLocalFavorite(path.clone()))
                                }),
                            ),
                        )
                    })
                    .child(
                        link_action(format!("favorite-remove-{index}"), "Retirer", p, offset)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.profile.favorites.retain(|favorite| favorite != &value);
                                cx.emit(IntegrationsEvent::FavoritesChanged(
                                    this.profile.favorites.clone(),
                                ));
                                cx.notify();
                            })),
                    ),
            );
        }
        content.child(
            favorites.child(
                div()
                    .flex()
                    .items_end()
                    .gap(px(12.))
                    .mt(px(4.))
                    .child(field(
                        &self.favorite,
                        "profile-favorite",
                        "Chemin local ou URL d’un dépôt",
                        p,
                        offset,
                    ))
                    .child(action("favorite-add", "Ajouter", p, offset).on_click(
                        cx.listener(|this, _, window, cx| this.add_favorite(window, cx)),
                    )),
            ),
        )
    }

    fn ssh_content(&self, cx: &mut Context<Self>) -> Div {
        let p = self.palette;
        let offset = self.offset;
        let mut choices = div().flex().gap(px(10.));
        for (id, label, choice) in [
            ("ssh-agent", "Agent SSH", SshChoice::Agent),
            ("ssh-disabled", "Désactivé", SshChoice::Disabled),
        ] {
            choices = choices.child(
                action(id, label, p, offset)
                    .when(self.profile.ssh == choice, |button| {
                        button
                            .bg(rgb(p.accent_soft))
                            .border_color(rgb(p.accent_border))
                    })
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.ssh_choice(choice.clone(), cx)),
                    ),
            );
        }
        let choice_label = match &self.profile.ssh {
            SshChoice::Agent => "Agent SSH du système",
            SshChoice::Disabled => "SSH désactivé",
            SshChoice::KeyFile { .. } => "Clé privée existante",
        };
        let mut content = div().flex().flex_col().gap(px(24.))
            .child(section_header("SSH", "Choisissez l’agent ou la clé à utiliser pour ce profil.", p, offset))
            .child(card(p).gap(px(16.)).child(div().flex().items_center().gap(px(12.))
                .child(icon(IconName::ShieldCheck, 22., p.accent))
                .child(div().child(subtitle(choice_label, offset)).child(note(&format!("Profil {}", self.profile.name), p))))
                .child(choices)
                .child(div().pt(px(16.)).border_t_1().border_color(rgb(p.border)).flex().items_end().gap(px(12.))
                    .child(field(&self.key_path, "ssh-key-path", "Chemin absolu d’une clé privée existante", p, offset))
                    .child(action("ssh-key-browse", "Parcourir…", p, offset).disabled(self.pending.is_some() || self.picking_path)
                        .on_click(cx.listener(|this, _, window, cx| this.pick_path(PathPickerTarget::SshKey, window, cx))))
                    .child(action("ssh-key-save", "Utiliser cette clé", p, offset).on_click(cx.listener(|this, _, _, cx| {
                        let value = this.key_path.read(cx).value().trim().to_string(); let path = expand_home(&value);
                        if path.is_absolute() && !value.chars().any(char::is_control) { this.error = None; this.ssh_choice(SshChoice::KeyFile { path }, cx); }
                        else { this.error = Some("Indiquez un chemin absolu de clé SSH.".into()); cx.notify(); }
                    })))))
            .child(card(p).gap(px(14.)).child(subtitle("Vérifier un accès SSH", offset))
                .child(note("Consulte les branches et tags du serveur sans télécharger les objets Git.", p))
                .child(field(&self.ssh_remote, "ssh-remote", "URL SSH du dépôt", p, offset))
                .child(note("Utilisez le nom réel du serveur, déjà reconnu dans known_hosts. Les alias et hooks de votre configuration SSH ne sont pas utilisés.", p))
                .child(div().flex().child(primary_action("ssh-read-references", "Consulter les références", p, offset).disabled(self.pending.is_some()).on_click(cx.listener(|this, _, _, cx| this.read_ssh(cx))))));
        for reference in &self.ssh_references {
            content = content.child(note(reference, p));
        }
        content
    }

    fn provider_rail(&self, cx: &mut Context<Self>) -> Div {
        let p = self.palette;
        let mut rail = div()
            .w(px(218.))
            .flex_none()
            .h_full()
            .bg(rgb(p.panel))
            .border_r_1()
            .border_color(rgb(p.border))
            .py(px(24.))
            .px(px(10.))
            .child(
                div()
                    .px(px(12.))
                    .mb(px(12.))
                    .text_size(px(11. + self.offset))
                    .text_color(rgb(p.muted))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("HÉBERGEMENT GIT"),
            );
        for (index, edition) in ProviderEdition::ALL.into_iter().enumerate() {
            let count = self
                .profile
                .accounts
                .iter()
                .filter(|account| edition.matches(account))
                .count();
            let selected = self.edition == edition;
            rail = rail.child(
                Button::new(SharedString::from(format!("provider-{index}")))
                    .w_full()
                    .min_w_0()
                    .min_h(px(48.))
                    .px(px(12.))
                    .py(px(10.))
                    .gap(px(10.))
                    .justify_start()
                    .rounded(RADIUS_MD)
                    .text_color(rgb(if selected { p.text } else { p.muted }))
                    .text_size(px(12. + self.offset))
                    .bg(rgb(if selected { p.selection } else { p.panel }))
                    .cursor_pointer()
                    .accessibility_label(edition.label())
                    .hover(move |style| style.bg(rgb(p.hover)))
                    .focus_visible(move |style| style.bg(rgb(p.selection)))
                    .disabled(self.pending == Some(PendingOperation::Disconnect))
                    .child(provider_mark(edition, p, 24.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .whitespace_normal()
                            .text_left()
                            .child(edition.label()),
                    )
                    .when(count > 0, |view| {
                        view.child(div().size(px(7.)).rounded_full().bg(rgb(p.success)))
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.select_provider(edition, window, cx)
                    })),
            );
        }
        rail.child(
            div()
                .mt(px(24.))
                .px(px(12.))
                .border_t_1()
                .border_color(rgb(p.border))
                .pt(px(16.))
                .child(note("Comptes du profil", p).text_size(px(11. + self.offset)))
                .child(
                    div()
                        .mt(px(4.))
                        .font_weight(FontWeight::MEDIUM)
                        .child(self.profile.name.clone()),
                ),
        )
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
        let mut content = div().flex().flex_col().gap(px(24.)).child(section_header(
            self.edition.label(),
            "Gérez vos connexions et retrouvez les dépôts depuis l’accueil.",
            p,
            offset,
        ));
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
                    .gap(px(18.))
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
                                    .gap(px(5.))
                                    .child(
                                        div()
                                            .flex()
                                            .items_center()
                                            .gap(px(7.))
                                            .text_color(rgb(p.success))
                                            .child(icon(IconName::CircleCheck, 14., p.success))
                                            .child("Compte connecté"),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(16. + offset))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child(display_name.to_string()),
                                    )
                                    .when_some(identity, |view, name| {
                                        view.child(note(&format!("@{name}"), p))
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
                            .justify_between()
                            .flex_wrap()
                            .gap(px(14.))
                            .pt(px(14.))
                            .border_t_1()
                            .border_color(rgb(p.border))
                            .child(
                                div()
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
                                    "Actualiser le compte",
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
                                action(format!("disconnect-{remove_id}"), "Déconnecter", p, offset)
                                    .disabled(self.pending.is_some())
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.disconnect(remove_id.clone(), cx)
                                    })),
                            ),
                    ),
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
                        this.message = None;
                        this.error = None;
                        cx.notify();
                    })),
                ),
            );
        }
        let mut form = card(p)
            .gap(px(18.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(subtitle(
                        if accounts.is_empty() {
                            "Connecter votre compte"
                        } else {
                            "Ajouter un compte"
                        },
                        offset,
                    ))
                    .when(!accounts.is_empty(), |view| {
                        view.child(
                            link_action("account-add-cancel", "Annuler", p, offset).on_click(
                                cx.listener(|this, _, window, cx| {
                                    this.cancel();
                                    this.adding_account = false;
                                    this.clear_connection_secrets(window, cx);
                                    this.message = None;
                                    this.error = None;
                                    cx.notify();
                                }),
                            ),
                        )
                    }),
            )
            .child(field(
                &self.label,
                "account-label",
                "Nom du compte · facultatif",
                p,
                offset,
            ));
        if self.edition.custom_server() {
            form = form.child(field(
                &self.server,
                "account-server",
                "URL du serveur",
                p,
                offset,
            ));
        } else if self.edition == ProviderEdition::BitbucketCloud {
            form = form.child(field(
                &self.server,
                "account-server",
                "Workspace · https://bitbucket.org/votre-workspace",
                p,
                offset,
            ));
        }
        if self.edition.provider() == Provider::BitbucketCloud && !self.browser_connection {
            form = form.child(field(
                &self.username,
                "account-username",
                "Adresse e-mail Atlassian · pour un API token",
                p,
                offset,
            ));
        }
        if supports_browser_connection(self.edition.provider()) && self.browser_connection {
            form = form.child(self.browser_connection_form(cx));
        } else {
            let permissions = match self.edition.provider() {
                Provider::GitLab => "GitLab : autorisation read_api.",
                Provider::GitHub => {
                    "GitHub : accès aux dépôts, pull requests et checks en lecture."
                }
                _ => "Bitbucket : accès aux dépôts, pull requests et statuts CI en lecture.",
            };
            form = form
                .child(field(
                    &self.token,
                    "account-token",
                    "Token personnel",
                    p,
                    offset,
                ))
                .child(note(permissions, p))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .flex_wrap()
                        .gap(px(12.))
                        .child(
                            primary_action("account-connect", "Connecter le compte", p, offset)
                                .disabled(self.pending.is_some())
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.connect(window, cx)),
                                ),
                        )
                        .when(
                            supports_browser_connection(self.edition.provider()),
                            |view| {
                                view.child(
                                    link_action(
                                        "oauth-browser-mode",
                                        "Connexion par navigateur",
                                        p,
                                        offset,
                                    )
                                    .disabled(self.pending.is_some())
                                    .on_click(cx.listener(
                                        |this, _, window, cx| {
                                            this.set_browser_connection(true, window, cx)
                                        },
                                    )),
                                )
                            },
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(7.))
                        .text_size(px(11. + offset))
                        .text_color(rgb(p.muted))
                        .child(icon(IconName::ShieldCheck, 14., p.muted))
                        .child("Identifiants enregistrés dans le coffre du système"),
                );
        }
        content.child(form)
    }

    fn browser_connection_form(&self, cx: &mut Context<Self>) -> Div {
        let p = self.palette;
        let offset = self.offset;
        let github = self.edition.provider() == Provider::GitHub;
        let bitbucket = self.edition.provider() == Provider::BitbucketCloud;
        let provider = provider_label(self.edition.provider());
        let mut content = div().flex().flex_col().gap(px(12.)).child(note(
            if github {
                "Mangrove ouvrira GitHub dans votre navigateur. Saisissez le code temporaire affiché ici pour autoriser la connexion."
            } else {
                "Autorisez Mangrove dans votre navigateur. La connexion est conservée dans le coffre sécurisé du système."
            },
            p,
        ));
        if github {
            content = content.child(
                gpui_kit::component::checkbox::Checkbox::new("github-private-repositories")
                    .label("Inclure les dépôts privés")
                    .checked(self.github_private_repos)
                    .disabled(self.pending.is_some())
                    .on_change(cx.listener(|this, checked, _, cx| {
                        this.github_private_repos = *checked;
                        cx.notify();
                    })),
            ).child(note(
                if self.github_private_repos {
                    "GitHub demande une autorisation qui inclut l’écriture pour accéder aux dépôts privés. Mangrove utilise ces accès pour la consultation."
                } else {
                    "L’accès aux dépôts privés n’est pas demandé."
                },
                p,
            ));
        }
        if let Some(authorization) = &self.github_authorization {
            let generation = self.generation;
            content = content.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .p(px(16.))
                    .rounded(RADIUS_LG)
                    .border_1()
                    .border_color(rgb(p.border))
                    .child(note("Code à saisir dans GitHub", p))
                    .child(
                        div()
                            .text_size(px(24. + offset))
                            .font_weight(FontWeight::SEMIBOLD)
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
                                        "Code copié !"
                                    } else {
                                        "Copier le code"
                                    },
                                    p,
                                    offset,
                                )
                                .on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        if this.generation != generation {
                                            return;
                                        }
                                        let Some(authorization) = &mut this.github_authorization
                                        else {
                                            return;
                                        };
                                        cx.write_to_clipboard(ClipboardItem::new_string(
                                            authorization.user_code.expose_secret().to_string(),
                                        ));
                                        authorization.copied = true;
                                        cx.notify();
                                    },
                                )),
                            )
                            .child(
                                action("github-device-open", "Ouvrir GitHub", p, offset).on_click(
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
                    )),
            );
        }
        content = content
            .child(
                div().flex().child(
                    primary_action(
                        "oauth-browser-connect",
                        &format!("Se connecter avec {provider}"),
                        p,
                        offset,
                    )
                    .disabled(self.pending.is_some())
                    .on_click(cx.listener(|this, _, window, cx| this.connect(window, cx))),
                ),
            )
            .child(
                div().flex().child(
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
            .child(
                div()
                    .flex()
                    .pt(px(12.))
                    .border_t_1()
                    .border_color(rgb(p.border))
                    .child(
                        Button::new("oauth-advanced-configuration")
                            .gap(px(6.))
                            .px(px(4.))
                            .py(px(6.))
                            .border_1()
                            .border_color(transparent_black())
                            .rounded(RADIUS_MD)
                            .text_size(px(12. + offset))
                            .text_color(rgb(p.muted))
                            .cursor_pointer()
                            .accessibility_label(if self.oauth_config_open {
                                "Masquer la configuration avancée"
                            } else {
                                "Afficher la configuration avancée"
                            })
                            .hover(move |style| style.text_color(rgb(p.text)))
                            .focus_visible(move |style| style.border_color(rgb(p.accent)))
                            .styles(|styles| {
                                styles.disabled(|style| style.opacity(0.5).cursor_default())
                            })
                            .disabled(self.pending.is_some())
                            .child(icon(
                                if self.oauth_config_open {
                                    IconName::ChevronDown
                                } else {
                                    IconName::ChevronRight
                                },
                                14.,
                                p.muted,
                            ))
                            .child("Configuration avancée")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.oauth_config_open = !this.oauth_config_open;
                                cx.notify();
                            })),
                    ),
            );
        if self.oauth_config_open {
            let mut configuration = div().flex().flex_col().gap(px(12.))
                .child(field(&self.client_id, "oauth-client-id", if github { "Client ID public" } else if bitbucket { "Consumer Key publique" } else { "Application ID public" }, p, offset))
                .child(note(
                    if github {
                        "Créez une OAuth App Mangrove dans GitHub, puis activez « Enable Device Flow » et recopiez son Client ID. Aucun secret d’application n’est nécessaire."
                    } else if bitbucket {
                        "Créez un consumer OAuth Mangrove dans votre workspace Bitbucket, avec Account · Read, Repositories · Read et Pull requests · Read. Recopiez sa Key et son Secret ci-dessous."
                    } else {
                        "Créez une application Mangrove non confidentielle dans GitLab, avec l’autorisation read_api. Aucun secret d’application n’est nécessaire."
                    },
                    p,
                ))
                .child(div().flex().child(action("oauth-create-application", &format!("Créer l’application dans {provider}"), p, offset)
                    .disabled(self.pending.is_some())
                    .on_click(cx.listener(|this, _, _, cx| this.open_application_settings(cx)))));
            if bitbucket {
                configuration = configuration
                    .child(field(&self.consumer_secret, "oauth-consumer-secret", "Consumer Secret · masqué", p, offset))
                    .child(note("Bitbucket exige le Secret pour connecter et renouveler le compte. Il est conservé uniquement dans le coffre du système ; aucun secret n’est inclus dans Mangrove.", p))
                    .child(self.copyable_oauth_url(
                        "oauth-bitbucket-callback-url",
                        "Callback URL",
                        oauth::REDIRECT_URI,
                        cx,
                    ));
            } else if github {
                configuration = configuration
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
                            "Cette URL de callback n’est pas utilisée par la connexion GitHub.",
                            p,
                        )
                        .text_size(px(11. + offset)),
                    );
            } else {
                configuration = configuration.child(self.copyable_oauth_url(
                    "oauth-redirect-url",
                    "URL de redirection",
                    oauth::REDIRECT_URI,
                    cx,
                ));
            }
            content = content.child(configuration);
        }
        content
    }

    fn copyable_oauth_url(
        &self,
        id: &'static str,
        label: &'static str,
        value: &'static str,
        cx: &mut Context<Self>,
    ) -> Div {
        let p = self.palette;
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
                            .cursor(CursorStyle::IBeam)
                            .child(
                                SelectableText::new(id, value)
                                    .selection_color(rgb(p.selection).into()),
                            ),
                    ),
            )
            .child(
                action(format!("{id}-copy"), "Copier", p, self.offset)
                    .accessibility_label(format!("Copier {label}"))
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(value.to_string()));
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
        let p = self.palette;
        let message = self
            .message
            .clone()
            .filter(|_| self.pending != Some(PendingOperation::RepositoryRead));
        let content = match self.section {
            IntegrationsSection::Profiles => self.profiles_content(cx),
            IntegrationsSection::Ssh => self.ssh_content(cx),
            IntegrationsSection::Integrations => self.integrations_content(cx),
            IntegrationsSection::Repositories => self.repositories_content(cx),
        };
        div()
            .id("integrations-view")
            .track_focus(&self.focus_handle)
            // The first Escape leaves the field; the next one reaches the app.
            .on_action(cx.listener(|this, _: &InputEscape, window, cx| {
                if this.profile_rename.read(cx).focus_handle(cx).is_focused(window) {
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
            .when(self.section == IntegrationsSection::Integrations, |view| {
                view.child(self.provider_rail(cx))
            })
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
                            .px(px(28.))
                            .py(px(28.))
                            .child(
                                div()
                                    .w_full()
                                    .max_w(px(760.))
                                    .when_some(message, |view, message| {
                                        view.child(
                                            div()
                                                .mb(px(20.))
                                                .flex()
                                                .items_center()
                                                .gap(px(8.))
                                                .text_color(rgb(p.muted))
                                                .when(self.pending.is_some(), |view| {
                                                    view.child(icon(
                                                        IconName::LoaderCircle,
                                                        16.,
                                                        p.accent,
                                                    ))
                                                })
                                                .child(note(&message, p)),
                                        )
                                    })
                                    .when_some(self.error.clone(), |view, error| {
                                        view.child(
                                            div()
                                                .mb(px(20.))
                                                .p(px(14.))
                                                .border_1()
                                                .border_color(rgb(p.danger).opacity(0.4))
                                                .rounded(RADIUS_MD)
                                                .bg(rgb(p.danger).opacity(0.07))
                                                .child(note(&error, p).text_color(rgb(p.danger))),
                                        )
                                    })
                                    .when(
                                        self.pending.is_some_and(|operation| {
                                            operation != PendingOperation::Disconnect
                                                && operation != PendingOperation::RepositoryRead
                                        }),
                                        |view| {
                                            view.child(
                                                div().flex().mb(px(20.)).child(
                                                    action(
                                                        "remote-cancel",
                                                        "Annuler",
                                                        p,
                                                        self.offset,
                                                    )
                                                    .on_click(cx.listener(|this, _, window, cx| {
                                                        let cloning = this.pending == Some(PendingOperation::Clone);
                                                        this.cancel();
                                                        this.clear_connection_secrets(window, cx);
                                                        this.message = cloning.then(|| "Clonage annulé. Un dossier incomplet peut subsister ; choisissez un nouveau nom avant de réessayer.".into());
                                                        cx.notify();
                                                    })),
                                                ),
                                            )
                                        },
                                    )
                                    .child(content),
                            ),
                    )
                    .child(scrollbar_theme(
                        Scrollbar::vertical(&self.scroll).id("integrations-scrollbar"),
                    )),
            )
    }
}

fn section_header(label: &str, description: &str, p: Palette, offset: f32) -> Div {
    div()
        .flex()
        .flex_col()
        .gap(px(8.))
        .child(
            div()
                .text_size(px(23. + offset))
                .font_weight(FontWeight::SEMIBOLD)
                .child(label.to_string()),
        )
        .child(note(description, p))
}

fn subtitle(label: &str, offset: f32) -> Div {
    div()
        .text_size(px(15. + offset))
        .font_weight(FontWeight::SEMIBOLD)
        .child(label.to_string())
}

fn title(label: &str) -> Div {
    subtitle(label, 0.)
}

fn card(p: Palette) -> Div {
    div()
        .flex()
        .flex_col()
        .min_w_0()
        .w_full()
        .p(px(22.))
        .border_1()
        .border_color(rgb(p.border))
        .rounded(RADIUS_LG)
        .bg(rgb(p.panel))
}

fn badge(label: &str, color: u32, background: u32, offset: f32) -> Div {
    div()
        .flex_none()
        .px(px(8.))
        .py(px(3.))
        .rounded(RADIUS_SM)
        .text_size(px(11. + offset))
        .text_color(rgb(color))
        .bg(rgb(background))
        .child(label.to_string())
}

fn avatar(label: &str, p: Palette, size: f32) -> Div {
    avatar_shape(size)
        .bg(rgb(p.accent_soft))
        .text_color(rgb(p.accent))
        .child(initials(label))
}

fn profile_avatar(profile: &Profile, p: Palette, size: f32) -> Div {
    let hue = p.graph[profile_hue(&profile.id, p.graph.len())];
    avatar_shape(size)
        .bg(rgb(mix(hue, p.panel, 0.18)))
        .text_color(rgb(hue))
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

fn primary_action(id: impl Into<SharedString>, label: &str, p: Palette, offset: f32) -> Button {
    action_base(id, label, p, offset)
        .bg(rgb(p.accent))
        .border_color(rgb(p.accent))
        .text_color(rgb(p.on_accent))
        .font_weight(FontWeight::SEMIBOLD)
        .hover(move |style| style.bg(rgb(p.accent).opacity(0.9)))
}

fn link_action(id: impl Into<SharedString>, label: &str, p: Palette, offset: f32) -> Button {
    action(id, label, p, offset)
        .border_color(transparent_black())
        .text_color(rgb(p.accent))
        .px(px(4.))
        .py(px(6.))
}

fn note(label: &str, p: Palette) -> Div {
    div()
        .min_w_0()
        .whitespace_normal()
        .line_height(relative(1.6))
        .text_color(rgb(p.muted))
        .child(label.to_string())
}

fn action(id: impl Into<SharedString>, label: &str, p: Palette, offset: f32) -> Button {
    action_base(id, label, p, offset).hover(move |style| style.bg(rgb(p.hover)))
}

fn action_base(id: impl Into<SharedString>, label: &str, p: Palette, offset: f32) -> Button {
    Button::new(id.into())
        .px(px(12.))
        .py(px(8.))
        .border_1()
        .border_color(rgb(p.border))
        .rounded(RADIUS_MD)
        .text_size(px(12. + offset))
        .text_color(rgb(p.text))
        .cursor_pointer()
        .accessibility_label(label.to_string())
        .focus_visible(move |style| style.border_color(rgb(p.accent)))
        .styles(|styles| styles.disabled(|style| style.opacity(0.5).cursor_default()))
        .child(div().min_w_0().whitespace_normal().child(label.to_string()))
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
                .pt(px(8.))
                .text_size(px(12. + offset))
                .text_color(rgb(p.muted))
                .whitespace_normal()
                .child(label.to_string()),
        )
        .child(div().flex_1().min_w_0().child(control))
}

fn input_control(
    state: &Entity<InputState>,
    id: &'static str,
    label: &'static str,
    p: Palette,
    offset: f32,
) -> Input {
    Input::new(state)
        .id(id)
        .aria_label(label)
        .appearance(false)
        .with_size(px(36.))
        .w_full()
        .min_w_0()
        .px(px(12.))
        .py_0()
        .border_1()
        .border_color(rgb(p.border))
        .rounded(RADIUS_MD)
        .bg(rgb(p.bg))
        .text_color(rgb(p.text))
        .text_size(px(12. + offset))
}

fn field(
    state: &Entity<InputState>,
    id: &'static str,
    label: &'static str,
    p: Palette,
    offset: f32,
) -> Div {
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(4.))
        .child(note(label, p))
        .child(input_control(state, id, label, p, offset))
}
