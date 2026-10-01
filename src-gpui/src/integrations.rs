use crate::theme::{Palette, scrollbar_theme};
use gpui_kit::{
    base::Button,
    component::{
        Sizable,
        input::{Input, InputState},
        scroll::Scrollbar,
    },
    prelude::*,
    *,
};
use mangrove_remote::{
    Account, Authentication, CancellationToken, CredentialVault, HttpTransport, OsCredentialVault,
    Profile, ProfileSession, Provider, SecretToken, SessionTicket,
    oauth::{self, GitLabOAuth, OAuthCredentials},
    providers::{
        ChangeRequest, CiStatus, Discussion, Page, PageCursor, ProviderClient, Repository,
    },
    ssh::SshChoice,
};
use std::{
    path::PathBuf,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

const MAX_ROWS: usize = 250;

/// Events only contain profile preferences and account metadata. Credentials
/// are consumed by the backend vault and never emitted to another view.
#[derive(Clone, Debug)]
pub enum IntegrationsEvent {
    ProfileSelected(String),
    ProfileCreated(String),
    AccountAdded {
        profile_id: String,
        account: Account,
    },
    AccountRemoved {
        profile_id: String,
        account_id: String,
    },
    FavoritesChanged(Vec<String>),
    SshChanged(SshChoice),
    OpenLocalFavorite(String),
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum IntegrationsSection {
    Profiles,
    Integrations,
}

#[derive(Clone)]
enum ReadRequest {
    Repositories(Option<PageCursor>),
    Changes(Repository, Option<PageCursor>),
    Discussions(Repository, ChangeRequest, Option<PageCursor>),
    Statuses(Repository, String, Option<PageCursor>),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum PendingOperation {
    Connect,
    Disconnect,
    Read,
}

enum ConnectionRequest {
    PersonalToken(SecretToken),
    GitLabBrowser { client_id: String },
}

enum ConnectionCredentials {
    PersonalToken(SecretToken),
    GitLabBrowser(GitLabOAuth),
}

enum ConnectedCredentials {
    PersonalToken(SecretToken),
    GitLabOAuth(OAuthCredentials),
}

impl ConnectedCredentials {
    fn provider_client(
        &self,
        account: Account,
    ) -> mangrove_remote::Result<ProviderClient<HttpTransport>> {
        let transport = HttpTransport::new()?;
        match self {
            Self::PersonalToken(token) => ProviderClient::new(transport, account, token.clone()),
            Self::GitLabOAuth(credentials) => credentials.provider_client(transport, account),
        }
    }

    fn store(
        &self,
        vault: &dyn CredentialVault,
        profile_id: &str,
        account_id: &str,
    ) -> mangrove_remote::Result<()> {
        match self {
            Self::PersonalToken(token) => vault.store(profile_id, account_id, token),
            Self::GitLabOAuth(credentials) => credentials.store(vault, profile_id, account_id),
        }
    }
}

enum ReadResponse {
    Repositories(Page<Repository>),
    Changes(Page<ChangeRequest>),
    Discussions(Page<Discussion>),
    Statuses(Page<CiStatus>),
}

/// Small, bounded pages are retained only for the active profile/account.
/// Every network operation runs on the background executor and carries both
/// a backend cancellation ticket and a view generation.
pub struct IntegrationsView {
    profile: Profile,
    profiles: Vec<Profile>,
    section: IntegrationsSection,
    palette: Palette,
    offset: f32,
    scroll: ScrollHandle,
    profile_name: Entity<InputState>,
    label: Entity<InputState>,
    server: Entity<InputState>,
    username: Entity<InputState>,
    token: Entity<InputState>,
    client_id: Entity<InputState>,
    gitlab_browser: bool,
    gitlab_config_open: bool,
    key_path: Entity<InputState>,
    favorite: Entity<InputState>,
    ssh_remote: Entity<InputState>,
    ssh_references: Vec<String>,
    provider: Provider,
    session: Arc<ProfileSession>,
    vault: Arc<OsCredentialVault>,
    generation: u64,
    task: Option<Task<()>>,
    selected_account: Option<String>,
    selected_repository: Option<Repository>,
    selected_change: Option<ChangeRequest>,
    repositories: Vec<Repository>,
    changes: Vec<ChangeRequest>,
    discussions: Vec<Discussion>,
    statuses: Vec<CiStatus>,
    repositories_next: Option<PageCursor>,
    changes_next: Option<PageCursor>,
    discussions_next: Option<PageCursor>,
    statuses_next: Option<PageCursor>,
    pending: Option<PendingOperation>,
    message: Option<String>,
    error: Option<String>,
}

impl EventEmitter<IntegrationsEvent> for IntegrationsView {}

impl Drop for IntegrationsView {
    fn drop(&mut self) {
        self.session.invalidate();
    }
}

impl IntegrationsView {
    pub fn new(
        profile: Profile,
        palette: Palette,
        offset: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let text_input =
            |placeholder: &'static str, window: &mut Window, cx: &mut Context<Self>| {
                cx.new(|cx| InputState::new(window, cx).placeholder(placeholder))
            };
        let key_path = text_input("/Users/…/.ssh/id_ed25519", window, cx);
        if let SshChoice::KeyFile { path } = &profile.ssh {
            key_path.update(cx, |input, cx| {
                input.set_value(path.to_string_lossy().to_string(), window, cx)
            });
        }
        let client_id = configured_gitlab_client_id(&profile);
        let gitlab_config_open = client_id.is_empty();
        Self {
            profiles: vec![profile.clone()],
            session: Arc::new(ProfileSession::new(&profile.id)),
            profile,
            section: IntegrationsSection::Integrations,
            palette,
            offset,
            scroll: ScrollHandle::default(),
            profile_name: text_input("Nom du nouveau profil", window, cx),
            label: text_input("Nom du compte", window, cx),
            server: cx.new(|cx| InputState::new(window, cx).default_value("https://gitlab.com")),
            username: text_input("Adresse e-mail Atlassian (Bitbucket Cloud)", window, cx),
            token: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("Token personnel")
                    .masked(true)
            }),
            client_id: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("Application ID public de Mangrove")
                    .default_value(client_id)
            }),
            gitlab_browser: true,
            gitlab_config_open,
            key_path,
            favorite: text_input("Chemin local ou URL d’un dépôt", window, cx),
            ssh_remote: text_input("ssh://git@serveur/groupe/depot.git", window, cx),
            ssh_references: Vec::new(),
            provider: Provider::GitLab,
            vault: Arc::new(OsCredentialVault::new()),
            generation: 0,
            task: None,
            selected_account: None,
            selected_repository: None,
            selected_change: None,
            repositories: Vec::new(),
            changes: Vec::new(),
            discussions: Vec::new(),
            statuses: Vec::new(),
            repositories_next: None,
            changes_next: None,
            discussions_next: None,
            statuses_next: None,
            pending: None,
            message: None,
            error: None,
        }
    }

    pub fn set_profiles(&mut self, profiles: Vec<Profile>, _: &mut Window, cx: &mut Context<Self>) {
        self.profiles = profiles;
        cx.notify();
    }

    pub fn set_section(&mut self, section: IntegrationsSection, cx: &mut Context<Self>) {
        self.section = section;
        cx.notify();
    }

    pub fn set_theme(&mut self, palette: Palette, offset: f32, cx: &mut Context<Self>) {
        self.palette = palette;
        self.offset = offset;
        cx.notify();
    }

    pub fn set_profile(&mut self, profile: Profile, window: &mut Window, cx: &mut Context<Self>) {
        let switched = self.profile.id != profile.id;
        let accounts_changed = self.profile.accounts != profile.accounts;
        let ssh_changed = self.profile.ssh != profile.ssh;
        if switched || accounts_changed || ssh_changed {
            self.session.switch(&profile.id);
            self.cancel();
            self.clear_reads();
            self.selected_account = None;
            self.token
                .update(cx, |input, cx| input.set_value("", window, cx));
            self.message = None;
            self.error = None;
        }
        if switched {
            for input in [
                &self.label,
                &self.username,
                &self.favorite,
                &self.ssh_remote,
                &self.profile_name,
            ] {
                input.update(cx, |state, cx| state.set_value("", window, cx));
            }
            self.provider = Provider::GitLab;
            self.gitlab_browser = true;
            let client_id = configured_gitlab_client_id(&profile);
            self.gitlab_config_open = client_id.is_empty();
            self.client_id
                .update(cx, |input, cx| input.set_value(client_id, window, cx));
            self.server.update(cx, |input, cx| {
                input.set_value("https://gitlab.com", window, cx)
            });
        }
        self.key_path.update(cx, |input, cx| {
            let value = match &profile.ssh {
                SshChoice::KeyFile { path } => path.to_string_lossy().to_string(),
                _ => String::new(),
            };
            input.set_value(value, window, cx);
        });
        self.profile = profile;
        cx.notify();
    }

    /// Called if persisting the newly added account metadata fails. No orphan
    /// credential is left behind after a rejected profile update.
    pub fn rollback_new_account(
        &mut self,
        profile_id: &str,
        account_id: &str,
        cx: &mut Context<Self>,
    ) {
        let profile_id = profile_id.to_string();
        let account_id = account_id.to_string();
        let vault = self.vault.clone();
        cx.background_executor()
            .spawn(async move {
                let _ = vault.delete(&profile_id, &account_id);
            })
            .detach();
    }

    pub fn set_error(&mut self, message: String, cx: &mut Context<Self>) {
        self.error = Some(message);
        cx.notify();
    }

    fn cancel(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.session.invalidate();
        self.task = None;
        self.pending = None;
    }

    fn clear_reads(&mut self) {
        self.ssh_references.clear();
        self.repositories.clear();
        self.repositories_next = None;
        self.clear_changes();
    }

    fn clear_changes(&mut self) {
        self.selected_repository = None;
        self.changes.clear();
        self.changes_next = None;
        self.clear_details();
    }

    fn clear_details(&mut self) {
        self.selected_change = None;
        self.discussions.clear();
        self.statuses.clear();
        self.discussions_next = None;
        self.statuses_next = None;
    }

    fn select_provider(&mut self, provider: Provider, window: &mut Window, cx: &mut Context<Self>) {
        self.provider = provider;
        self.gitlab_browser = true;
        let url = match self.provider {
            Provider::GitLab => "https://gitlab.com",
            Provider::GitHub => "https://github.com",
            Provider::BitbucketCloud => "https://bitbucket.org/<workspace>",
            Provider::BitbucketServer => "https://bitbucket.example.com",
        };
        self.server
            .update(cx, |input, cx| input.set_value(url, window, cx));
        self.token
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.error = None;
        cx.notify();
    }

    fn set_gitlab_browser(&mut self, browser: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.gitlab_browser = browser;
        self.token
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.error = None;
        cx.notify();
    }

    fn open_gitlab_application_settings(&mut self, cx: &mut Context<Self>) {
        let server_url = self.server.read(cx).value().trim().to_string();
        match oauth::application_settings_url(&server_url) {
            Ok(url) => cx.open_url(&url),
            Err(error) => {
                self.error = Some(error.to_string());
                cx.notify();
            }
        }
    }

    fn connect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending.is_some() {
            return;
        }
        if self.profile.accounts.len() >= 32 {
            self.token
                .update(cx, |input, cx| input.set_value("", window, cx));
            self.error = Some(
                "Le profil contient déjà 32 comptes. Déconnectez un compte avant d’en ajouter un."
                    .into(),
            );
            cx.notify();
            return;
        }
        let browser = self.provider == Provider::GitLab && self.gitlab_browser;
        let (request, authentication) = if browser {
            let client_id = self.client_id.read(cx).value().trim().to_string();
            if client_id.is_empty() {
                self.gitlab_config_open = true;
                self.error = Some(
                    "Créez une application GitLab puis saisissez son Application ID public.".into(),
                );
                cx.notify();
                return;
            }
            (
                ConnectionRequest::GitLabBrowser {
                    client_id: client_id.clone(),
                },
                Authentication::GitLabOAuth { client_id },
            )
        } else {
            let draft = self.token.read(cx).value();
            let token: SecretToken = draft.to_string().into();
            // set_value also clears input undo history; a submitted token cannot
            // reappear through undo or be returned in an event payload.
            self.token
                .update(cx, |input, cx| input.set_value("", window, cx));
            if draft.is_empty() {
                self.error = Some("Saisissez un token personnel.".into());
                cx.notify();
                return;
            }
            (
                ConnectionRequest::PersonalToken(token),
                Authentication::PersonalToken,
            )
        };
        let label = self.label.read(cx).value().trim().to_string();
        let server_url = self.server.read(cx).value().trim().to_string();
        let username = self.username.read(cx).value().trim().to_string();
        let account = Account {
            id: fresh_account_id(),
            label: if label.is_empty() {
                provider_label(self.provider).to_string()
            } else {
                label
            },
            provider: self.provider,
            server_url,
            username: (self.provider == Provider::BitbucketCloud && !username.is_empty())
                .then_some(username),
            authentication,
        };
        self.cancel();
        self.pending = Some(PendingOperation::Connect);
        self.error = None;
        self.message = Some(if browser {
            "Ouverture de GitLab dans votre navigateur…".into()
        } else {
            "Vérification du compte…".into()
        });
        let ticket = self.session.ticket(&account.id);
        let session = self.session.clone();
        let vault = self.vault.clone();
        let generation = self.generation;
        let worker_account = account.clone();
        let worker_ticket = ticket.clone();
        // Detached ownership allows the background worker to remove a newly
        // written credential if cancellation races with the secure vault.
        cx.spawn(async move |this, cx| {
            let preparation_account = worker_account.clone();
            let credentials = cx
                .background_executor()
                .spawn(async move {
                    preparation_account.validate()?;
                    match request {
                        ConnectionRequest::PersonalToken(token) => {
                            Ok(ConnectionCredentials::PersonalToken(token))
                        }
                        ConnectionRequest::GitLabBrowser { client_id } => {
                            GitLabOAuth::begin(&preparation_account.server_url, &client_id)
                                .map(ConnectionCredentials::GitLabBrowser)
                        }
                    }
                })
                .await;
            let current = this
                .update(cx, |view, cx| {
                    if !view.accepts(&ticket, generation) {
                        return false;
                    }
                    if let Ok(ConnectionCredentials::GitLabBrowser(flow)) = &credentials {
                        // The listener is bound before opening the browser. A
                        // superseded profile cannot open a stale login page.
                        cx.open_url(flow.authorization_url());
                        view.message = Some(
                            "Autorisez Mangrove dans votre navigateur, puis revenez ici.".into(),
                        );
                        cx.notify();
                    }
                    true
                })
                .unwrap_or(false);
            if !current {
                return;
            }
            let result = cx
                .background_executor()
                .spawn(async move {
                    let credentials = match credentials? {
                        ConnectionCredentials::PersonalToken(token) => {
                            ConnectedCredentials::PersonalToken(token)
                        }
                        ConnectionCredentials::GitLabBrowser(flow) => {
                            ConnectedCredentials::GitLabOAuth(
                                flow.finish(&worker_ticket.cancellation)?,
                            )
                        }
                    };
                    let client = credentials.provider_client(worker_account.clone())?;
                    let page = client.repositories(None, &worker_ticket.cancellation)?;
                    if !session.is_current(&worker_ticket) {
                        return Err(mangrove_remote::RemoteError::Cancelled);
                    }
                    credentials.store(
                        vault.as_ref(),
                        &worker_ticket.profile_id,
                        &worker_account.id,
                    )?;
                    if !session.is_current(&worker_ticket) {
                        vault.delete(&worker_ticket.profile_id, &worker_account.id)?;
                        return Err(mangrove_remote::RemoteError::Cancelled);
                    }
                    Ok(page)
                })
                .await;
            let accepted = this
                .update(cx, |view, cx| {
                    if !view.accepts(&ticket, generation) {
                        return false;
                    }
                    view.pending = None;
                    match result {
                        Ok(page) => {
                            view.selected_account = Some(account.id.clone());
                            view.profile.accounts.push(account.clone());
                            view.repositories = bounded(page.items);
                            view.repositories_next = page.next;
                            view.clear_changes();
                            view.message = Some(
                                "Compte connecté. Les lectures distantes sont disponibles.".into(),
                            );
                            cx.emit(IntegrationsEvent::AccountAdded {
                                profile_id: ticket.profile_id.clone(),
                                account: account.clone(),
                            });
                        }
                        Err(error) => {
                            view.message = None;
                            view.error = Some(error.to_string());
                        }
                    }
                    cx.notify();
                    true
                })
                .unwrap_or(false);
            if !accepted {
                // The entity can disappear after the vault write completes.
                let cleanup = OsCredentialVault::new();
                cx.background_executor()
                    .spawn(async move {
                        let _ = cleanup.delete(&ticket.profile_id, &account.id);
                    })
                    .detach();
            }
        })
        .detach();
        cx.notify();
    }

    fn accepts(&self, ticket: &SessionTicket, generation: u64) -> bool {
        accepts_response(
            generation,
            self.generation,
            &ticket.profile_id,
            &self.profile.id,
        ) && self.session.is_current(ticket)
    }

    fn select_account(&mut self, id: String, cx: &mut Context<Self>) {
        self.cancel();
        self.clear_reads();
        self.selected_account = Some(id);
        self.read(ReadRequest::Repositories(None), cx);
    }

    fn disconnect(&mut self, id: String, cx: &mut Context<Self>) {
        if self.pending.is_some() {
            return;
        }
        self.cancel();
        self.clear_reads();
        self.selected_account = None;
        self.pending = Some(PendingOperation::Disconnect);
        self.error = None;
        self.message = Some("Suppression du token du coffre…".into());
        let ticket = self.session.ticket(&id);
        let generation = self.generation;
        let vault = self.vault.clone();
        let worker_ticket = ticket.clone();
        cx.spawn(async move |this, cx| {
            let result =
                cx.background_executor()
                    .spawn(async move {
                        vault.delete(&worker_ticket.profile_id, &worker_ticket.account_id)
                    })
                    .await;
            let _ = this.update(cx, |view, cx| {
                let completion =
                    disconnect_completion(&ticket, view.accepts(&ticket, generation), result);
                if let Some(result) = completion.current_result {
                    view.pending = None;
                    match result {
                        Ok(()) => {
                            view.message =
                                Some("Compte déconnecté et token supprimé du coffre.".into());
                        }
                        Err(error) => {
                            view.message = None;
                            view.error = Some(error.to_string());
                        }
                    }
                    cx.notify();
                }
                if let Some(event) = completion.metadata_event {
                    cx.emit(event);
                }
            });
        })
        .detach();
        cx.notify();
    }

    fn read(&mut self, request: ReadRequest, cx: &mut Context<Self>) {
        let Some(account) = self
            .profile
            .accounts
            .iter()
            .find(|a| Some(&a.id) == self.selected_account.as_ref())
            .cloned()
        else {
            return;
        };
        self.cancel();
        self.pending = Some(PendingOperation::Read);
        self.error = None;
        self.message = Some("Lecture distante en cours…".into());
        let ticket = self.session.ticket(&account.id);
        let worker_ticket = ticket.clone();
        let generation = self.generation;
        let vault = self.vault.clone();
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    let client = oauth::account_client(
                        vault.as_ref(),
                        &worker_ticket.profile_id,
                        &account,
                        &worker_ticket.cancellation,
                    )?;
                    perform_read(&client, request, &worker_ticket.cancellation)
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                if !view.accepts(&ticket, generation) {
                    return;
                }
                view.pending = None;
                view.message = None;
                match result {
                    Ok(ReadResponse::Repositories(page)) => {
                        view.repositories = bounded(page.items);
                        view.repositories_next = page.next;
                        view.clear_changes();
                    }
                    Ok(ReadResponse::Changes(page)) => {
                        view.changes = bounded(page.items);
                        view.changes_next = page.next;
                        view.clear_details();
                    }
                    Ok(ReadResponse::Discussions(page)) => {
                        view.discussions = bounded(page.items);
                        view.discussions_next = page.next;
                    }
                    Ok(ReadResponse::Statuses(page)) => {
                        view.statuses = bounded(page.items);
                        view.statuses_next = page.next;
                    }
                    Err(error) => {
                        view.error = Some(error.to_string());
                    }
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn select_repository(&mut self, repository: Repository, cx: &mut Context<Self>) {
        self.clear_changes();
        self.selected_repository = Some(repository.clone());
        self.read(ReadRequest::Changes(repository, None), cx);
    }

    fn select_change(&mut self, change: ChangeRequest, cx: &mut Context<Self>) {
        let Some(repository) = self.selected_repository.clone() else {
            return;
        };
        self.clear_details();
        self.selected_change = Some(change.clone());
        self.read(ReadRequest::Discussions(repository, change, None), cx);
    }

    fn add_favorite(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let value = self.favorite.read(cx).value().trim().to_string();
        if !value.is_empty()
            && !self.profile.favorites.contains(&value)
            && self.profile.favorites.len() < MAX_ROWS
        {
            self.profile.favorites.push(value);
            self.favorite
                .update(cx, |input, cx| input.set_value("", window, cx));
            cx.emit(IntegrationsEvent::FavoritesChanged(
                self.profile.favorites.clone(),
            ));
            cx.notify();
        }
    }

    fn ssh_choice(&mut self, choice: SshChoice, cx: &mut Context<Self>) {
        self.cancel();
        self.ssh_references.clear();
        self.message = None;
        self.profile.ssh = choice.clone();
        cx.emit(IntegrationsEvent::SshChanged(choice));
        cx.notify();
    }

    fn read_ssh(&mut self, cx: &mut Context<Self>) {
        let remote = self.ssh_remote.read(cx).value().trim().to_string();
        if remote.is_empty() {
            return;
        }
        let choice = self.profile.ssh.clone();
        self.cancel();
        self.pending = Some(PendingOperation::Read);
        self.error = None;
        self.ssh_references.clear();
        self.message = Some("Lecture des références SSH…".into());
        let ticket = self.session.ticket("ssh-references");
        let worker_ticket = ticket.clone();
        let generation = self.generation;
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    mangrove_remote::ssh::read_references(
                        &remote,
                        &choice,
                        &worker_ticket.cancellation,
                    )
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                if !view.accepts(&ticket, generation) {
                    return;
                }
                view.pending = None;
                view.message = None;
                match result {
                    Ok(references) => {
                        view.ssh_references = references
                            .into_iter()
                            .take(MAX_ROWS)
                            .map(|reference| {
                                format!("{} · {}", reference.name, reference.object_id)
                            })
                            .collect()
                    }
                    Err(error) => view.error = Some(error.to_string()),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn profiles_content(&self, cx: &mut Context<Self>) -> Div {
        let p = self.palette;
        let offset = self.offset;
        let mut content = div().flex().flex_col().gap(px(16.))
            .child(title("Profils"))
            .child(note("Les comptes, favoris et le choix SSH sont propres à chaque profil. Le changement de profil annule ses lectures distantes.", p));
        for profile in &self.profiles {
            let id = profile.id.clone();
            content = content.child(
                action(format!("profile-{id}"), &profile.name, p, offset)
                    .when(profile.id == self.profile.id, |button| {
                        button.bg(rgb(p.selection))
                    })
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(IntegrationsEvent::ProfileSelected(id.clone()))
                    })),
            );
        }
        content = content.child(
            div()
                .flex()
                .gap(px(8.))
                .child(field(
                    &self.profile_name,
                    "profile-name",
                    "Nom du profil",
                    p,
                    offset,
                ))
                .child(
                    action("profile-create", "Créer un profil", p, offset).on_click(cx.listener(
                        |this, _, window, cx| {
                            let name = this.profile_name.read(cx).value().trim().to_string();
                            if !name.is_empty() {
                                this.profile_name
                                    .update(cx, |input, cx| input.set_value("", window, cx));
                                cx.emit(IntegrationsEvent::ProfileCreated(name));
                            }
                        },
                    )),
                ),
        );
        content = content.child(title("Favoris du profil"));
        for (index, favorite) in self.profile.favorites.iter().take(MAX_ROWS).enumerate() {
            let value = favorite.clone();
            content =
                content.child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(12.))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .whitespace_normal()
                                .child(value.clone()),
                        )
                        .when(PathBuf::from(favorite).is_absolute(), |view| {
                            let path = favorite.clone();
                            view.child(
                                action(format!("favorite-open-{index}"), "Ouvrir", p, offset)
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        cx.emit(IntegrationsEvent::OpenLocalFavorite(path.clone()))
                                    })),
                            )
                        })
                        .child(
                            action(format!("favorite-remove-{index}"), "Retirer", p, offset)
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
        content = content.child(
            div()
                .flex()
                .gap(px(8.))
                .child(field(
                    &self.favorite,
                    "profile-favorite",
                    "Chemin local ou URL d’un dépôt",
                    p,
                    offset,
                ))
                .child(
                    action("favorite-add", "Ajouter aux favoris", p, offset)
                        .on_click(cx.listener(|this, _, window, cx| this.add_favorite(window, cx))),
                ),
        );
        content = content.child(title("SSH"))
            .child(note("Réutilise votre agent ou une clé existante. Lecture des références uniquement : aucun clone ni téléchargement d’objet.", p));
        for (id, label, choice) in [
            ("ssh-agent", "Agent SSH", SshChoice::Agent),
            ("ssh-disabled", "Désactivé", SshChoice::Disabled),
        ] {
            let selected = self.profile.ssh == choice;
            content = content.child(
                action(id, label, p, offset)
                    .when(selected, |button| button.bg(rgb(p.selection)))
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.ssh_choice(choice.clone(), cx)),
                    ),
            );
        }
        content = content.child(
            div()
                .flex()
                .gap(px(8.))
                .child(field(
                    &self.key_path,
                    "ssh-key-path",
                    "Chemin absolu de la clé SSH existante",
                    p,
                    offset,
                ))
                .child(
                    action("ssh-key-save", "Utiliser cette clé", p, offset).on_click(cx.listener(
                        |this, _, _, cx| {
                            let value = this.key_path.read(cx).value().trim().to_string();
                            let path = PathBuf::from(&value);
                            if path.is_absolute() && !value.chars().any(char::is_control) {
                                this.error = None;
                                this.ssh_choice(SshChoice::KeyFile { path }, cx);
                            } else {
                                this.error = Some("Indiquez un chemin absolu de clé SSH.".into());
                                cx.notify();
                            }
                        },
                    )),
                ),
        );
        content = content.child(field(&self.ssh_remote, "ssh-remote", "URL SSH du dépôt à consulter", p, offset))
            .child(note("Indiquez le vrai nom du serveur ; les alias et hooks de votre configuration SSH ne sont pas utilisés. Le serveur doit déjà être reconnu dans known_hosts.", p))
            .child(action("ssh-read-references", "Consulter les références SSH", p, offset).disabled(self.pending.is_some())
                .on_click(cx.listener(|this, _, _, cx| this.read_ssh(cx))));
        for reference in &self.ssh_references {
            content = content.child(note(reference, p));
        }
        content
    }

    fn integrations_content(&self, cx: &mut Context<Self>) -> Div {
        let p = self.palette;
        let offset = self.offset;
        let mut content = div().flex().flex_col().gap(px(16.))
            .child(title("Intégrations"))
            .child(note(&format!("Profil actif : {}. Consultation des dépôts, PR/MR, discussions et CI en lecture seule.", self.profile.name), p));
        for account in &self.profile.accounts {
            let id = account.id.clone();
            let remove_id = id.clone();
            content = content
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(
                            action(
                                format!("account-{id}"),
                                &format!(
                                    "{} · {}",
                                    account.label,
                                    provider_label(account.provider)
                                ),
                                p,
                                offset,
                            )
                            .flex_1()
                            .min_w_0()
                            .when(self.selected_account.as_ref() == Some(&id), |button| {
                                button.bg(rgb(p.selection))
                            })
                            .on_click(cx.listener(
                                move |this, _, _, cx| this.select_account(id.clone(), cx),
                            )),
                        )
                        .child(
                            action(format!("disconnect-{remove_id}"), "Déconnecter", p, offset)
                                .disabled(self.pending.is_some())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.disconnect(remove_id.clone(), cx)
                                })),
                        ),
                )
                .child(note(&account.server_url, p));
        }
        content = content.child(title("Connecter un compte"));
        let mut providers = div().flex().flex_wrap().gap(px(8.));
        for provider in [
            Provider::GitLab,
            Provider::GitHub,
            Provider::BitbucketCloud,
            Provider::BitbucketServer,
        ] {
            providers = providers.child(
                action(
                    format!("provider-{}", provider_label(provider)),
                    provider_label(provider),
                    p,
                    offset,
                )
                .disabled(self.pending.is_some())
                .when(self.provider == provider, |button| {
                    button.bg(rgb(p.selection))
                })
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.select_provider(provider, window, cx)
                })),
            );
        }
        content = content.child(providers)
            .child(field(&self.label, "account-label", "Nom du compte (facultatif)", p, offset))
            .child(field(&self.server, "account-server", "URL du serveur", p, offset))
            .when(self.provider == Provider::BitbucketCloud, |view| view.child(note("Bitbucket Cloud : indiquez https://bitbucket.org/votre-workspace. Pour un API token, saisissez aussi votre adresse e-mail ; les access tokens utilisent Bearer sans nom d’utilisateur.", p)))
            .when(self.provider == Provider::BitbucketCloud, |view| view.child(field(&self.username, "account-username", "Adresse e-mail Atlassian", p, offset)));
        if self.provider == Provider::GitLab && self.gitlab_browser {
            content = content
                .child(action("gitlab-browser-connect", "Se connecter avec GitLab", p, offset)
                    .disabled(self.pending.is_some())
                    .on_click(cx.listener(|this, _, window, cx| this.connect(window, cx))))
                .child(note("L’autorisation s’effectue dans votre navigateur. Mangrove conserve la connexion dans le coffre sécurisé du système.", p))
                .child(action("gitlab-application-config", if self.gitlab_config_open { "Masquer la configuration" } else { "Configurer l’application GitLab" }, p, offset)
                    .disabled(self.pending.is_some())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.gitlab_config_open = !this.gitlab_config_open;
                        cx.notify();
                    })))
                .when(self.gitlab_config_open, |view| {
                    view.child(field(&self.client_id, "gitlab-client-id", "Application ID public", p, offset))
                        .child(note("Cet ID est fourni lorsque vous créez une application Mangrove dans GitLab. Il peut être partagé ; aucun secret d’application n’est nécessaire.", p))
                        .child(action("gitlab-create-application", "Créer une application GitLab", p, offset)
                            .disabled(self.pending.is_some())
                            .on_click(cx.listener(|this, _, _, cx| this.open_gitlab_application_settings(cx))))
                        .child(note(&format!("Dans GitLab : nom Mangrove, option Confidential décochée, autorisation read_api et URL de redirection {}.", oauth::REDIRECT_URI), p))
                })
                .child(action("gitlab-personal-token", "Utiliser un token personnel", p, offset)
                    .disabled(self.pending.is_some())
                    .on_click(cx.listener(|this, _, window, cx| this.set_gitlab_browser(false, window, cx))));
        } else {
            content = content
                .child(field(&self.token, "account-token", "Token personnel", p, offset))
                .child(note("Le token est effacé du champ après validation et conservé dans le coffre sécurisé du système. GitLab : read_api. GitHub : dépôts, PR et checks en lecture. Bitbucket : dépôts, PR et statuts CI en lecture.", p))
                .child(action("account-connect", "Connecter avec le token", p, offset)
                    .disabled(self.pending.is_some())
                    .on_click(cx.listener(|this, _, window, cx| this.connect(window, cx))))
                .when(self.provider == Provider::GitLab, |view| {
                    view.child(action("gitlab-browser-mode", "Se connecter par navigateur", p, offset)
                        .disabled(self.pending.is_some())
                        .on_click(cx.listener(|this, _, window, cx| this.set_gitlab_browser(true, window, cx))))
                });
        }
        if self.selected_account.is_some() {
            content = content.child(
                div()
                    .flex()
                    .gap(px(8.))
                    .child(title("Dépôts accessibles"))
                    .child(
                        action("repositories-reload", "Actualiser", p, offset)
                            .disabled(self.pending.is_some())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.read(ReadRequest::Repositories(None), cx)
                            })),
                    ),
            );
            if self.repositories.is_empty() && self.pending.is_none() {
                content = content.child(note("Aucun dépôt chargé.", p));
            }
            for (index, repository) in self.repositories.iter().enumerate() {
                let repository = repository.clone();
                let favorite = repository
                    .web_url
                    .clone()
                    .unwrap_or_else(|| repository.full_name.clone());
                content = content.child(
                    div()
                        .flex()
                        .gap(px(8.))
                        .child(
                            action(
                                format!("repository-{index}"),
                                &repository.full_name,
                                p,
                                offset,
                            )
                            .flex_1()
                            .min_w_0()
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.select_repository(repository.clone(), cx)
                                },
                            )),
                        )
                        .child(
                            action(
                                format!("remote-favorite-{index}"),
                                if self.profile.favorites.contains(&favorite) {
                                    "Favori"
                                } else {
                                    "Ajouter aux favoris"
                                },
                                p,
                                offset,
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
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
                                },
                            )),
                        ),
                );
            }
            if let Some(cursor) = self.repositories_next.clone() {
                content = content.child(
                    action("repositories-next", "Page suivante des dépôts", p, offset)
                        .disabled(self.pending.is_some())
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.read(ReadRequest::Repositories(Some(cursor.clone())), cx)
                        })),
                );
            }
        }
        if let Some(repository) = &self.selected_repository {
            content = content.child(title(&format!("PR/MR · {}", repository.full_name)));
            if self.changes.is_empty() && self.pending.is_none() {
                content = content.child(note("Aucune PR/MR chargée.", p));
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
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let p = self.palette;
        let content = match self.section {
            IntegrationsSection::Profiles => self.profiles_content(cx),
            IntegrationsSection::Integrations => self.integrations_content(cx),
        };
        div()
            .id("integrations-view")
            .relative()
            .size_full()
            .min_w_0()
            .min_h_0()
            .bg(rgb(p.bg))
            .text_color(rgb(p.text))
            .text_size(px(12. + self.offset))
            .child(
                div()
                    .id("integrations-scroll")
                    .size_full()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .px(px(40.))
                    .py(px(36.))
                    .child(
                        div()
                            .w_full()
                            .max_w(px(900.))
                            .child(content)
                            .when_some(self.message.clone(), |view, message| {
                                view.child(note(&message, p).mt(px(16.)))
                            })
                            .when_some(self.error.clone(), |view, error| {
                                view.child(note(&error, p).mt(px(16.)).text_color(rgb(p.danger)))
                            })
                            .when(
                                self.pending.is_some_and(|operation| {
                                    operation != PendingOperation::Disconnect
                                }),
                                |view| {
                                    view.child(
                                        action("remote-cancel", "Annuler", p, self.offset)
                                            .mt(px(12.))
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.cancel();
                                                this.message = None;
                                                cx.notify();
                                            })),
                                    )
                                },
                            ),
                    ),
            )
            .child(scrollbar_theme(
                Scrollbar::vertical(&self.scroll).id("integrations-scrollbar"),
            ))
    }
}

struct DisconnectCompletion {
    metadata_event: Option<IntegrationsEvent>,
    current_result: Option<mangrove_remote::Result<()>>,
}

fn disconnect_completion(
    ticket: &SessionTicket,
    current: bool,
    result: mangrove_remote::Result<()>,
) -> DisconnectCompletion {
    // Vault deletion cannot be interrupted. Persist its result for the
    // original profile, while a stale callback cannot alter the current view.
    let metadata_event = result.is_ok().then(|| IntegrationsEvent::AccountRemoved {
        profile_id: ticket.profile_id.clone(),
        account_id: ticket.account_id.clone(),
    });
    DisconnectCompletion {
        metadata_event,
        current_result: current.then_some(result),
    }
}

fn perform_read<T: mangrove_remote::Transport>(
    client: &ProviderClient<T>,
    request: ReadRequest,
    cancel: &CancellationToken,
) -> Result<ReadResponse, mangrove_remote::RemoteError> {
    match request {
        ReadRequest::Repositories(cursor) => client
            .repositories(cursor.as_ref(), cancel)
            .map(ReadResponse::Repositories),
        ReadRequest::Changes(repository, cursor) => client
            .pull_requests(&repository, cursor.as_ref(), cancel)
            .map(ReadResponse::Changes),
        ReadRequest::Discussions(repository, change, cursor) => client
            .discussions(&repository, &change, cursor.as_ref(), cancel)
            .map(ReadResponse::Discussions),
        ReadRequest::Statuses(repository, sha, cursor) => client
            .statuses(&repository, &sha, cursor.as_ref(), cancel)
            .map(ReadResponse::Statuses),
    }
}

fn bounded<T>(mut rows: Vec<T>) -> Vec<T> {
    rows.truncate(MAX_ROWS);
    rows
}

fn accepts_response(
    request_generation: u64,
    current_generation: u64,
    request_profile: &str,
    current_profile: &str,
) -> bool {
    request_generation == current_generation && request_profile == current_profile
}

fn fresh_account_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    format!(
        "account-{}-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    )
}

fn configured_gitlab_client_id(profile: &Profile) -> String {
    profile
        .accounts
        .iter()
        .filter(|account| {
            account.provider == Provider::GitLab
                && account.server_url.trim_end_matches('/') == "https://gitlab.com"
        })
        .find_map(|account| match &account.authentication {
            Authentication::GitLabOAuth { client_id } => Some(client_id.clone()),
            Authentication::PersonalToken => None,
        })
        .unwrap_or_else(|| {
            std::env::var("MANGROVE_GITLAB_CLIENT_ID")
                .unwrap_or_default()
                .trim()
                .to_string()
        })
}

fn provider_label(provider: Provider) -> &'static str {
    match provider {
        Provider::GitLab => "GitLab",
        Provider::GitHub => "GitHub",
        Provider::BitbucketCloud => "Bitbucket Cloud",
        Provider::BitbucketServer => "Bitbucket Server",
    }
}

fn title(label: &str) -> Div {
    div()
        .text_size(px(20.))
        .font_weight(FontWeight::SEMIBOLD)
        .child(label.to_string())
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
    Button::new(id.into())
        .px(px(12.))
        .py(px(8.))
        .border_1()
        .border_color(rgb(p.border))
        .rounded(px(5.))
        .text_size(px(12. + offset))
        .text_color(rgb(p.text))
        .cursor_pointer()
        .accessibility_label(label.to_string())
        .hover(move |style| style.bg(rgb(p.hover)))
        .focus_visible(move |style| style.border_color(rgb(p.accent)))
        .styles(|styles| styles.disabled(|style| style.opacity(0.5).cursor_default()))
        .child(div().min_w_0().whitespace_normal().child(label.to_string()))
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
        .child(
            Input::new(state)
                .id(id)
                .aria_label(label)
                .appearance(false)
                .with_size(px(36.))
                .w_full()
                .px(px(12.))
                .py_0()
                .border_1()
                .border_color(rgb(p.border))
                .rounded(px(4.))
                .bg(rgb(p.bg))
                .text_color(rgb(p.text))
                .text_size(px(12. + offset)),
        )
}

#[cfg(test)]
mod tests {
    use super::{
        IntegrationsEvent, accepts_response, bounded, configured_gitlab_client_id,
        disconnect_completion,
    };
    use mangrove_remote::{
        Account, Authentication, Profile, ProfileSession, Provider, RemoteError,
    };

    #[test]
    fn gitlab_application_id_prefill_uses_the_active_profile_and_instance() {
        let account = |id: &str, server_url: &str, client_id: &str| Account {
            id: id.into(),
            label: "GitLab".into(),
            provider: Provider::GitLab,
            server_url: server_url.into(),
            username: None,
            authentication: Authentication::GitLabOAuth {
                client_id: client_id.into(),
            },
        };
        let mut personal = Profile::new("personal", "Personnel");
        personal.accounts.push(account(
            "enterprise",
            "https://gitlab.example.com",
            "enterprise-id",
        ));
        personal.accounts.push(account(
            "personal-account",
            "https://gitlab.com/",
            "personal-id",
        ));
        let mut professional = Profile::new("professional", "Professionnel");
        professional.accounts.push(account(
            "professional-account",
            "https://gitlab.com",
            "professional-id",
        ));
        assert_eq!(configured_gitlab_client_id(&personal), "personal-id");
        assert_eq!(
            configured_gitlab_client_id(&professional),
            "professional-id"
        );
    }

    #[test]
    fn completed_vault_deletion_keeps_its_profile_after_switch_without_touching_new_view() {
        let session = ProfileSession::new("personal");
        let deletion = session.ticket("personal-account");
        // A vault operation already started before the user switches profiles.
        session.switch("professional");
        let completion = disconnect_completion(&deletion, session.is_current(&deletion), Ok(()));
        assert!(matches!(
            completion.metadata_event,
            Some(IntegrationsEvent::AccountRemoved { profile_id, account_id })
                if profile_id == "personal" && account_id == "personal-account"
        ));
        assert!(completion.current_result.is_none());

        let failed = disconnect_completion(
            &deletion,
            session.is_current(&deletion),
            Err(RemoteError::VaultUnavailable),
        );
        assert!(failed.metadata_event.is_none());
        assert!(failed.current_result.is_none());

        let current = session.ticket("professional-account");
        let failed = disconnect_completion(
            &current,
            session.is_current(&current),
            Err(RemoteError::VaultUnavailable),
        );
        assert!(failed.metadata_event.is_none());
        assert_eq!(
            failed.current_result,
            Some(Err(RemoteError::VaultUnavailable))
        );
    }

    #[test]
    fn stale_profile_and_account_reads_cannot_replace_current_data() {
        assert!(accepts_response(2, 2, "personal", "personal"));
        assert!(!accepts_response(1, 2, "personal", "personal"));
        assert!(!accepts_response(2, 2, "personal", "professional"));
        // Switching away and back still changes the generation.
        assert!(!accepts_response(2, 4, "personal", "personal"));
    }

    #[test]
    fn untrusted_pages_cannot_grow_retained_lists_without_bound() {
        assert_eq!(bounded((0..1_000).collect()).len(), super::MAX_ROWS);
        assert_eq!(bounded(vec![1, 2]), vec![1, 2]);
    }
}
