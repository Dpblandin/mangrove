use crate::{
    format::{count_label, display_path, initials},
    theme::{Palette, icon, scrollbar_theme},
};
use gpui_kit::assets::IconName;
use gpui_kit::{
    base::Button,
    component::{
        Disableable, Sizable,
        input::{Input, InputEvent, InputState},
        scroll::Scrollbar,
        searchable_list::{SearchableGroup, SearchableListDelegate, SearchableListItem},
        select::{Select, SelectEvent, SelectState},
    },
    prelude::*,
    *,
};
use mangrove_remote::{
    Account, Authentication, CancellationToken, CredentialVault, ExposeSecret, HttpTransport,
    OsCredentialVault, Profile, ProfileSession, Provider, SecretToken, SessionTicket,
    bitbucket_oauth::{self, BitbucketCredentials, BitbucketOAuth},
    github_oauth::{self, GitHubCredentials, GitHubDeviceFlow},
    oauth::{self, GitLabOAuth, OAuthCredentials},
    providers::{
        AccountIdentity, ChangeRequest, CiStatus, Discussion, Page, PageCursor, ProviderClient,
        Repository,
    },
    ssh::SshChoice,
};
use std::{
    cell::RefCell,
    collections::HashMap,
    path::PathBuf,
    rc::Rc,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

mod render;
mod repository_select;

#[cfg(test)]
use repository_select::repository_option_groups;
use repository_select::{
    RepositoryEmpty, RepositoryLoadState, RepositoryLoadTrigger, RepositoryMenu, RepositoryMenuData,
};

const MAX_ROWS: usize = 250;

/// Events only contain profile preferences and account metadata. Credentials
/// are consumed by the backend vault and never emitted to another view.
#[derive(Clone, Debug)]
pub enum IntegrationsEvent {
    ProfileSelected(String),
    ProfileCreated(String),
    ProfileRenamed {
        profile_id: String,
        name: String,
    },
    /// Emitted once the profile's credentials have left the vault.
    ProfileDeleted(String),
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
    OpenIntegrationSettings,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum IntegrationsSection {
    Profiles,
    Ssh,
    Integrations,
    Repositories,
}

/// Cloud and enterprise editions have different setup fields, while sharing
/// their provider's transport and credential handling.
#[derive(Clone, Copy, PartialEq, Eq)]
enum ProviderEdition {
    GitHub,
    GitHubEnterprise,
    GitLab,
    GitLabSelfManaged,
    BitbucketCloud,
    BitbucketDataCenter,
}

impl ProviderEdition {
    const ALL: [Self; 6] = [
        Self::GitHub,
        Self::GitHubEnterprise,
        Self::GitLab,
        Self::GitLabSelfManaged,
        Self::BitbucketCloud,
        Self::BitbucketDataCenter,
    ];

    fn provider(self) -> Provider {
        match self {
            Self::GitHub | Self::GitHubEnterprise => Provider::GitHub,
            Self::GitLab | Self::GitLabSelfManaged => Provider::GitLab,
            Self::BitbucketCloud => Provider::BitbucketCloud,
            Self::BitbucketDataCenter => Provider::BitbucketServer,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::GitHub => "GitHub.com",
            Self::GitHubEnterprise => "GitHub Enterprise",
            Self::GitLab => "GitLab.com",
            Self::GitLabSelfManaged => "GitLab auto-hébergé",
            Self::BitbucketCloud => "Bitbucket Cloud",
            Self::BitbucketDataCenter => "Bitbucket Data Center",
        }
    }

    fn server(self) -> &'static str {
        match self {
            Self::GitHub => "https://github.com",
            Self::GitLab => "https://gitlab.com",
            Self::BitbucketCloud => "https://bitbucket.org/",
            _ => "",
        }
    }

    fn custom_server(self) -> bool {
        matches!(
            self,
            Self::GitHubEnterprise | Self::GitLabSelfManaged | Self::BitbucketDataCenter
        )
    }

    fn matches(self, account: &Account) -> bool {
        if self.provider() != account.provider {
            return false;
        }
        let cloud = match account.provider {
            Provider::GitHub => matches!(
                account.server_url.trim_end_matches('/'),
                "https://github.com" | "https://api.github.com"
            ),
            Provider::GitLab => account.server_url.trim_end_matches('/') == "https://gitlab.com",
            _ => true,
        };
        match self {
            Self::GitHub | Self::GitLab => cloud,
            Self::GitHubEnterprise | Self::GitLabSelfManaged => !cloud,
            _ => true,
        }
    }
}

type RepositorySelect = SelectState<RepositoryMenu>;

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
    Identity,
    RepositoryRead,
    Read,
    Clone,
}

#[derive(Clone, Copy)]
enum PathPickerTarget {
    CloneParent,
    SshKey,
}

enum ConnectionRequest {
    PersonalToken(SecretToken),
    GitLabBrowser {
        client_id: String,
    },
    GitHubBrowser {
        client_id: String,
        private_repos: bool,
    },
    BitbucketBrowser {
        client_id: String,
        consumer_secret: SecretToken,
    },
}

enum ConnectionCredentials {
    PersonalToken(SecretToken),
    GitLabBrowser(GitLabOAuth),
    GitHubBrowser(GitHubDeviceFlow),
    BitbucketBrowser(BitbucketOAuth),
}

enum ConnectedCredentials {
    PersonalToken(SecretToken),
    GitLabOAuth(OAuthCredentials),
    GitHubOAuth(GitHubCredentials),
    BitbucketOAuth(BitbucketCredentials),
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
            Self::GitHubOAuth(credentials) => credentials.provider_client(transport, account),
            Self::BitbucketOAuth(credentials) => credentials.provider_client(transport, account),
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
            Self::GitHubOAuth(credentials) => credentials.store(vault, profile_id, account_id),
            Self::BitbucketOAuth(credentials) => credentials.store(vault, profile_id, account_id),
        }
    }
}

enum ReadResponse {
    Repositories(Page<Repository>),
    Changes(Page<ChangeRequest>),
    Discussions(Page<Discussion>),
    Statuses(Page<CiStatus>),
}

struct GitHubAuthorization {
    user_code: SecretToken,
    verification_url: String,
    copied: bool,
}

/// Bounded remote collections are retained only for the active profile/account.
/// Every network operation runs on the background executor and carries both
/// a backend cancellation ticket and a view generation.
pub struct IntegrationsView {
    profile: Profile,
    profiles: Vec<Profile>,
    section: IntegrationsSection,
    palette: Palette,
    offset: f32,
    scroll: ScrollHandle,
    rail_scroll: ScrollHandle,
    focus_handle: FocusHandle,
    identities: HashMap<String, AccountIdentity>,
    profile_name: Entity<InputState>,
    profile_rename: Entity<InputState>,
    renaming_profile: Option<String>,
    deleting_profile: Option<String>,
    label: Entity<InputState>,
    server: Entity<InputState>,
    username: Entity<InputState>,
    token: Entity<InputState>,
    client_id: Entity<InputState>,
    consumer_secret: Entity<InputState>,
    browser_connection: bool,
    oauth_config_open: bool,
    github_private_repos: bool,
    github_authorization: Option<GitHubAuthorization>,
    key_path: Entity<InputState>,
    favorite: Entity<InputState>,
    ssh_remote: Entity<InputState>,
    ssh_references: Vec<String>,
    edition: ProviderEdition,
    adding_account: bool,
    repository_select: Entity<RepositorySelect>,
    repository_select_subscription: Option<Subscription>,
    repository_select_scope: u64,
    repository_select_dirty: bool,
    repository_select_reset: bool,
    repository_menu_data: Rc<RefCell<RepositoryMenuData>>,
    clone_url: Entity<InputState>,
    direct_clone: bool,
    clone_parent: Entity<InputState>,
    clone_name: Entity<InputState>,
    generated_clone_name: String,
    clone_ssh: bool,
    picking_path: bool,
    picker_task: Option<Task<()>>,
    clone_prompt: bool,
    repository_load_error: Option<mangrove_remote::RemoteError>,
    _subscriptions: Vec<Subscription>,
    session: Arc<ProfileSession>,
    vault: Arc<OsCredentialVault>,
    generation: u64,
    task: Option<Task<()>>,
    selected_account: Option<String>,
    selected_repository: Option<Repository>,
    selected_change: Option<ChangeRequest>,
    repositories: Vec<Repository>,
    changes: Vec<ChangeRequest>,
    changes_loaded: bool,
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
        let key_path = text_input("~/.ssh/id_ed25519", window, cx);
        if let SshChoice::KeyFile { path } = &profile.ssh {
            key_path.update(cx, |input, cx| {
                input.set_value(display_path(&path.to_string_lossy()), window, cx)
            });
        }
        let client_id = configured_browser_client_id(&profile, ProviderEdition::GitLab);
        let repository_menu_data = Rc::new(RefCell::new(RepositoryMenuData::default()));
        let options = RepositoryMenu::new(
            repository_menu_data.clone(),
            RepositoryLoadTrigger::new(cx.entity().downgrade(), 0),
            RepositoryLoadState::Complete,
            palette,
        );
        let repository_select =
            cx.new(|cx| SelectState::new(options, None, window, cx).searchable(true));
        let clone_url = text_input(
            "https://serveur/groupe/depot.git ou git@serveur:groupe/depot.git",
            window,
            cx,
        );
        let clone_name = text_input("Nom du dossier à créer", window, cx);
        let clone_parent = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Choisir un dossier")
                .default_value(default_clone_parent())
        });
        let profile_rename = text_input("Nom du profil", window, cx);
        let mut subscriptions = vec![];
        subscriptions.push(cx.subscribe_in(
            &profile_rename,
            window,
            |this, _, event, window, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    this.save_profile_name(window, cx);
                }
            },
        ));
        subscriptions.push(
            cx.subscribe_in(&clone_url, window, |this, _, event, window, cx| {
                if matches!(event, InputEvent::Change) {
                    let current = this.clone_name.read(cx).value();
                    if current.is_empty() || current.as_ref() == this.generated_clone_name {
                        let generated =
                            clone_directory_from_remote(this.clone_url.read(cx).value().trim());
                        this.generated_clone_name = generated.clone();
                        this.clone_name
                            .update(cx, |input, cx| input.set_value(generated, window, cx));
                    }
                    cx.notify();
                }
            }),
        );
        for input in [&clone_name, &clone_parent] {
            subscriptions.push(cx.subscribe(input, |_, _, event, cx| {
                if matches!(event, InputEvent::Change) {
                    cx.notify();
                }
            }));
        }
        Self {
            profiles: vec![profile.clone()],
            session: Arc::new(ProfileSession::new(&profile.id)),
            profile,
            section: IntegrationsSection::Integrations,
            palette,
            offset,
            scroll: ScrollHandle::default(),
            rail_scroll: ScrollHandle::default(),
            focus_handle: cx.focus_handle(),
            identities: HashMap::new(),
            profile_name: text_input("Nom du nouveau profil", window, cx),
            profile_rename,
            renaming_profile: None,
            deleting_profile: None,
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
                    .placeholder("ID public de l’application Mangrove")
                    .default_value(client_id)
            }),
            consumer_secret: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("Secret du consumer Bitbucket")
                    .masked(true)
            }),
            browser_connection: true,
            oauth_config_open: oauth_config_required(ProviderEdition::GitLab),
            github_private_repos: false,
            github_authorization: None,
            key_path,
            favorite: text_input("Chemin local ou URL d’un dépôt", window, cx),
            ssh_remote: text_input("ssh://git@serveur/groupe/depot.git", window, cx),
            ssh_references: Vec::new(),
            edition: ProviderEdition::GitLab,
            adding_account: false,
            repository_select,
            repository_select_subscription: None,
            repository_select_scope: 0,
            repository_select_dirty: true,
            repository_select_reset: true,
            repository_menu_data,
            clone_url,
            direct_clone: false,
            clone_parent,
            clone_name,
            generated_clone_name: "depot".into(),
            clone_ssh: false,
            picking_path: false,
            picker_task: None,
            clone_prompt: false,
            repository_load_error: None,
            _subscriptions: subscriptions,
            vault: Arc::new(OsCredentialVault::new()),
            generation: 0,
            task: None,
            selected_account: None,
            selected_repository: None,
            selected_change: None,
            repositories: Vec::new(),
            changes: Vec::new(),
            changes_loaded: false,
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
        self.scroll.set_offset(point(px(0.), px(0.)));
        if self.section != section {
            self.cancel();
            self.clear_reads();
            self.message = None;
            self.error = None;
            self.section = section;
        }
        cx.notify();
    }

    pub fn open_repositories(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.cloning() {
            return;
        }
        self.set_section(IntegrationsSection::Repositories, cx);
        let account_id = self
            .selected_account
            .clone()
            .filter(|id| self.profile.accounts.iter().any(|a| &a.id == id))
            .or_else(|| {
                self.profile
                    .accounts
                    .first()
                    .map(|account| account.id.clone())
            });
        if let Some(account_id) = account_id {
            self.select_account(account_id, cx);
        } else {
            self.select_direct_clone(window, cx);
        }
    }

    pub fn close_repositories(&mut self, cx: &mut Context<Self>) {
        self.set_section(IntegrationsSection::Integrations, cx);
    }

    pub fn leave_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.cancel();
        self.clear_connection_secrets(window, cx);
        self.message = None;
        cx.notify();
    }

    pub fn choosing_path(&self) -> bool {
        self.picking_path
    }

    pub fn cloning(&self) -> bool {
        self.pending == Some(PendingOperation::Clone)
    }

    /// Asks before cancelling a running clone. Returns false when no clone
    /// runs; otherwise `then` runs once the user confirms the cancellation.
    pub fn confirm_cancel_clone(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        then: impl FnOnce(&mut Window, &mut App) + 'static,
    ) -> bool {
        if !self.cloning() {
            return false;
        }
        if self.clone_prompt {
            return true;
        }
        self.clone_prompt = true;
        let answer = window.prompt(
            PromptLevel::Warning,
            "Annuler le clonage en cours ?",
            Some("Un dossier incomplet peut subsister à l’emplacement choisi."),
            &[
                PromptButton::ok("Annuler le clonage"),
                PromptButton::cancel("Continuer le clonage"),
            ],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let confirmed = answer.await == Ok(0);
            let _ = cx.update(|window, cx| {
                let current = this.update(cx, |view, cx| {
                    view.clone_prompt = false;
                    // The clone may have finished while the prompt was open.
                    if confirmed && view.cloning() {
                        view.cancel();
                        cx.notify();
                    }
                });
                if confirmed && current.is_ok() {
                    then(window, cx);
                }
            });
        })
        .detach();
        true
    }

    pub fn set_theme(&mut self, palette: Palette, offset: f32, cx: &mut Context<Self>) {
        self.palette = palette;
        self.offset = offset;
        self.repository_select_dirty = true;
        cx.notify();
    }

    pub fn set_profile(&mut self, profile: Profile, window: &mut Window, cx: &mut Context<Self>) {
        let switched = self.profile.id != profile.id;
        let accounts_changed = self.profile.accounts != profile.accounts;
        let ssh_changed = self.profile.ssh != profile.ssh;
        if switched || accounts_changed || ssh_changed {
            self.identities.clear();
            self.session.switch(&profile.id);
            self.cancel();
            self.clear_reads();
            self.selected_account = None;
            self.clear_connection_secrets(window, cx);
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
                &self.clone_name,
                &self.clone_url,
            ] {
                input.update(cx, |state, cx| state.set_value("", window, cx));
            }
            self.edition = ProviderEdition::GitLab;
            self.adding_account = false;
            self.renaming_profile = None;
            self.direct_clone = false;
            self.clone_ssh = false;
            self.clone_parent.update(cx, |input, cx| {
                input.set_value(default_clone_parent(), window, cx)
            });
            self.browser_connection = true;
            self.github_private_repos = false;
            let client_id = configured_browser_client_id(&profile, ProviderEdition::GitLab);
            self.oauth_config_open = oauth_config_required(ProviderEdition::GitLab);
            self.client_id
                .update(cx, |input, cx| input.set_value(client_id, window, cx));
            self.server.update(cx, |input, cx| {
                input.set_value("https://gitlab.com", window, cx)
            });
        }
        self.key_path.update(cx, |input, cx| {
            let value = match &profile.ssh {
                SshChoice::KeyFile { path } => display_path(&path.to_string_lossy()),
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
        self.picker_task = None;
        self.picking_path = false;
        self.github_authorization = None;
        self.repository_select_dirty = true;
    }

    fn clear_reads(&mut self) {
        self.invalidate_repository_select();
        self.ssh_references.clear();
        self.repositories.clear();
        self.repositories_next = None;
        self.repository_load_error = None;
        self.repository_menu_data = Rc::new(RefCell::new(RepositoryMenuData::default()));
        self.clear_changes();
    }

    fn clear_changes(&mut self) {
        self.selected_repository = None;
        self.changes.clear();
        self.changes_loaded = false;
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

    fn select_provider(
        &mut self,
        edition: ProviderEdition,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cancel();
        self.clear_reads();
        self.scroll.set_offset(point(px(0.), px(0.)));
        self.edition = edition;
        self.adding_account = false;
        self.browser_connection = true;
        self.github_private_repos = false;
        self.server.update(cx, |input, cx| {
            input.set_value(edition.server(), window, cx)
        });
        self.clear_connection_secrets(window, cx);
        for input in [&self.label, &self.username] {
            input.update(cx, |input, cx| input.set_value("", window, cx));
        }
        let client_id = configured_browser_client_id(&self.profile, edition);
        self.oauth_config_open = oauth_config_required(edition);
        self.client_id
            .update(cx, |input, cx| input.set_value(client_id, window, cx));
        self.message = None;
        self.error = None;
        cx.notify();
    }

    fn clear_connection_secrets(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for input in [&self.token, &self.consumer_secret] {
            input.update(cx, |input, cx| input.set_value("", window, cx));
        }
    }

    fn set_browser_connection(
        &mut self,
        browser: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.browser_connection = browser;
        self.clear_connection_secrets(window, cx);
        self.error = None;
        cx.notify();
    }

    fn open_application_settings(&mut self, cx: &mut Context<Self>) {
        let server_url = self.server.read(cx).value().trim().to_string();
        let result = match self.edition.provider() {
            Provider::GitHub => github_oauth::application_settings_url(&server_url),
            Provider::GitLab => oauth::application_settings_url(&server_url),
            Provider::BitbucketCloud => bitbucket_oauth::application_settings_url(&server_url),
            _ => return,
        };
        match result {
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
            self.clear_connection_secrets(window, cx);
            self.error = Some(
                "Le profil contient déjà 32 comptes. Déconnectez un compte avant d’en ajouter un."
                    .into(),
            );
            cx.notify();
            return;
        }
        let provider = self.edition.provider();
        let browser = supports_browser_connection(provider) && self.browser_connection;
        let (request, authentication) = if browser {
            let client_id = self.client_id.read(cx).value().trim().to_string();
            if client_id.is_empty() {
                self.oauth_config_open = true;
                self.error = Some(format!(
                    "Créez une application {} puis saisissez son ID public dans la configuration avancée.",
                    provider_label(provider)
                ));
                cx.notify();
                return;
            }
            if provider == Provider::GitHub {
                (
                    ConnectionRequest::GitHubBrowser {
                        client_id: client_id.clone(),
                        private_repos: self.github_private_repos,
                    },
                    Authentication::GitHubOAuth { client_id },
                )
            } else if provider == Provider::BitbucketCloud {
                let secret = self.consumer_secret.read(cx).value();
                let consumer_secret = SecretToken::from(secret.to_string());
                self.clear_connection_secrets(window, cx);
                if secret.is_empty() {
                    self.oauth_config_open = true;
                    self.error = Some(
                        "Saisissez le Secret du consumer OAuth Bitbucket dans la configuration avancée."
                            .into(),
                    );
                    cx.notify();
                    return;
                }
                (
                    ConnectionRequest::BitbucketBrowser {
                        client_id: client_id.clone(),
                        consumer_secret,
                    },
                    Authentication::BitbucketOAuth { client_id },
                )
            } else {
                (
                    ConnectionRequest::GitLabBrowser {
                        client_id: client_id.clone(),
                    },
                    Authentication::GitLabOAuth { client_id },
                )
            }
        } else {
            let draft = self.token.read(cx).value();
            let token: SecretToken = draft.to_string().into();
            // set_value also clears input undo history; a submitted token cannot
            // reappear through undo or be returned in an event payload.
            self.clear_connection_secrets(window, cx);
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
        let default_label = label.is_empty();
        let server_url = self.server.read(cx).value().trim().to_string();
        let username = self.username.read(cx).value().trim().to_string();
        let account = Account {
            id: fresh_account_id(),
            label: if label.is_empty() {
                provider_label(provider).to_string()
            } else {
                label
            },
            provider,
            server_url,
            username: (provider == Provider::BitbucketCloud && !browser && !username.is_empty())
                .then_some(username),
            authentication,
        };
        self.cancel();
        self.pending = Some(PendingOperation::Connect);
        self.error = None;
        self.message = Some(if browser {
            format!("Préparation de la connexion {}…", provider_label(provider))
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
            let preparation_cancel = worker_ticket.cancellation.clone();
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
                        ConnectionRequest::BitbucketBrowser { client_id, consumer_secret } => {
                            BitbucketOAuth::begin(&preparation_account.server_url, &client_id, consumer_secret)
                                .map(ConnectionCredentials::BitbucketBrowser)
                        }
                        ConnectionRequest::GitHubBrowser { client_id, private_repos } => {
                            GitHubDeviceFlow::begin(
                                &preparation_account.server_url,
                                &client_id,
                                private_repos,
                                &preparation_cancel,
                            ).map(ConnectionCredentials::GitHubBrowser)
                        }
                    }
                })
                .await;
            let current = this
                .update(cx, |view, cx| {
                    if !view.accepts(&ticket, generation) {
                        return false;
                    }
                    let callback_authorization = match &credentials {
                        Ok(ConnectionCredentials::GitLabBrowser(flow)) => {
                            Some(flow.authorization_url())
                        }
                        Ok(ConnectionCredentials::BitbucketBrowser(flow)) => {
                            Some(flow.authorization_url())
                        }
                        _ => None,
                    };
                    if let Some(url) = callback_authorization {
                        // The listener is bound before opening the browser. A
                        // superseded profile cannot open a stale login page.
                        cx.open_url(url);
                        view.message = Some(
                            "Autorisez Mangrove dans votre navigateur, puis revenez ici.".into(),
                        );
                        cx.notify();
                    }
                    if let Ok(ConnectionCredentials::GitHubBrowser(flow)) = &credentials {
                        view.github_authorization = Some(GitHubAuthorization {
                            user_code: flow.user_code().into(),
                            verification_url: flow.verification_url().to_string(),
                            copied: false,
                        });
                        cx.open_url(flow.verification_url());
                        view.message = Some("Saisissez le code affiché ci-dessous dans GitHub, puis autorisez Mangrove.".into());
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
                        ConnectionCredentials::BitbucketBrowser(flow) => {
                            ConnectedCredentials::BitbucketOAuth(flow.finish(&worker_ticket.cancellation)?)
                        }
                        ConnectionCredentials::GitHubBrowser(flow) => {
                            ConnectedCredentials::GitHubOAuth(flow.finish(&worker_ticket.cancellation)?)
                        }
                    };
                    let client = credentials.provider_client(worker_account.clone())?;
                    let identity = client.account_identity(&worker_ticket.cancellation)?;
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
                    Ok(identity)
                })
                .await;
            let accepted = this
                .update(cx, |view, cx| {
                    if !view.accepts(&ticket, generation) {
                        return false;
                    }
                    view.pending = None;
                    view.github_authorization = None;
                    match result {
                        Ok(identity) => {
                            let mut account = account.clone();
                            if let Some(identity) = identity {
                                account.username = Some(identity.username);
                                if default_label {
                                    account.label = identity.display_name;
                                }
                            }
                            view.selected_account = Some(account.id.clone());
                            view.profile.accounts.push(account.clone());
                            view.clear_reads();
                            view.adding_account = false;
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
        self.scroll.set_offset(point(px(0.), px(0.)));
        self.direct_clone = false;
        self.cancel();
        self.clear_reads();
        self.selected_account = Some(id);
        if self.section == IntegrationsSection::Repositories {
            self.read(ReadRequest::Repositories(None), cx);
        }
    }

    fn refresh_identity(&mut self, id: String, cx: &mut Context<Self>) {
        if self.pending.is_some() {
            return;
        }
        let Some(account) = self
            .profile
            .accounts
            .iter()
            .find(|account| account.id == id)
            .cloned()
        else {
            return;
        };
        self.cancel();
        let generation = self.generation;
        let ticket = self.session.ticket(&account.id);
        let worker_ticket = ticket.clone();
        let vault = self.vault.clone();
        self.pending = Some(PendingOperation::Identity);
        self.message = Some("Vérification du compte…".into());
        self.error = None;
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
                    client.account_identity(&worker_ticket.cancellation)
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                if !view.accepts(&ticket, generation) {
                    return;
                }
                view.pending = None;
                match result {
                    Ok(identity) => {
                        if let Some(identity) = identity {
                            view.identities.insert(ticket.account_id.clone(), identity);
                        }
                        view.message = Some("Compte vérifié.".into());
                    }
                    Err(error) => {
                        view.message = None;
                        view.error = Some(error.to_string());
                    }
                }
                cx.notify();
            });
        }));
        cx.notify();
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
        if self.section != IntegrationsSection::Repositories {
            return;
        }
        let Some(account) = self
            .profile
            .accounts
            .iter()
            .find(|a| Some(&a.id) == self.selected_account.as_ref())
            .cloned()
        else {
            return;
        };
        let repository_read = matches!(request, ReadRequest::Repositories(_));
        let append = matches!(&request, ReadRequest::Repositories(Some(_)));
        self.cancel();
        if repository_read && !append {
            self.clear_reads();
        }
        if repository_read {
            self.repository_load_error = None;
        }
        self.pending = Some(if repository_read {
            PendingOperation::RepositoryRead
        } else {
            PendingOperation::Read
        });
        self.error = None;
        self.message = (!append).then(|| "Lecture distante en cours…".into());
        let ticket = self.session.ticket(&account.id);
        let worker_ticket = ticket.clone();
        let generation = self.generation;
        let vault = self.vault.clone();
        if !append {
            self.scroll.set_offset(point(px(0.), px(0.)));
        }
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
                        let result = view
                            .repository_menu_data
                            .borrow_mut()
                            .append(&mut view.repositories, page.items);
                        match result {
                            Ok(()) => view.repositories_next = page.next,
                            Err(error) => {
                                view.error = Some(error.to_string());
                                view.repository_load_error = Some(error);
                            }
                        }
                    }
                    Ok(ReadResponse::Changes(page)) => {
                        view.changes = bounded(page.items);
                        view.changes_loaded = true;
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
                        if repository_read {
                            view.repository_load_error = Some(error);
                        }
                    }
                }
                view.repository_select_dirty = true;
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn invalidate_repository_select(&mut self) {
        self.repository_select_scope = self.repository_select_scope.wrapping_add(1);
        self.repository_select_dirty = true;
        self.repository_select_reset = true;
    }

    fn can_select_repository(&self) -> bool {
        !self.picking_path
            && matches!(
                self.pending,
                None | Some(PendingOperation::Read | PendingOperation::RepositoryRead)
            )
    }

    fn repository_load_state(&self) -> RepositoryLoadState {
        if self.pending == Some(PendingOperation::RepositoryRead) {
            return RepositoryLoadState::Loading;
        }
        if self.repositories_next.is_none()
            || self.pending.is_some()
            || self.repository_load_error == Some(mangrove_remote::RemoteError::ResponseTooLarge)
        {
            return RepositoryLoadState::Complete;
        }
        if self.repository_load_error.is_some() {
            RepositoryLoadState::Retry
        } else {
            RepositoryLoadState::Ready
        }
    }

    fn load_more_repositories(&mut self, scope: u64, retry: bool, cx: &mut Context<Self>) {
        if scope != self.repository_select_scope
            || self.section != IntegrationsSection::Repositories
            || self.direct_clone
            || self.picking_path
            || self.pending.is_some()
            || self.repository_load_error == Some(mangrove_remote::RemoteError::ResponseTooLarge)
            || (!retry && self.repository_load_error.is_some())
        {
            return;
        }
        if let Some(cursor) = self.repositories_next.clone() {
            self.read(ReadRequest::Repositories(Some(cursor)), cx);
        }
    }

    fn sync_repository_select(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.repository_select_dirty {
            return;
        }
        self.repository_select_dirty = false;
        let options = RepositoryMenu::new(
            self.repository_menu_data.clone(),
            RepositoryLoadTrigger::new(cx.entity().downgrade(), self.repository_select_scope),
            self.repository_load_state(),
            self.palette,
        );
        if !self.repository_select_reset {
            // Pages only append sections: existing full/filtered IndexPaths stay
            // valid, including kit's deferred confirmations and scroll position.
            self.repository_select.update(cx, |select, cx| {
                select.set_items(options, window, cx);
                cx.notify();
            });
            return;
        }
        self.repository_select_reset = false;
        // Source changes replace the entity so old confirmation callbacks cannot
        // select a repository from another profile/account.
        self.repository_select_subscription = None;
        if !self.direct_clone && self.selected_repository.is_none() {
            self.generated_clone_name.clear();
            self.clone_name
                .update(cx, |input, cx| input.set_value("", window, cx));
        }
        let selected_index = self
            .selected_repository
            .as_ref()
            .and_then(|repository| options.position(&repository.id));
        let select =
            cx.new(|cx| SelectState::new(options, selected_index, window, cx).searchable(true));
        let scope = self.repository_select_scope;
        self.repository_select_subscription =
            Some(
                cx.subscribe_in(&select, window, move |this, source, event, window, cx| {
                    if scope != this.repository_select_scope
                        || source.entity_id() != this.repository_select.entity_id()
                        || this.section != IntegrationsSection::Repositories
                        || this.direct_clone
                    {
                        return;
                    }
                    if !this.can_select_repository() {
                        this.invalidate_repository_select();
                        cx.notify();
                        return;
                    }
                    match event {
                        SelectEvent::Confirm(Some(id)) => {
                            if this
                                .selected_repository
                                .as_ref()
                                .is_some_and(|repository| &repository.id == id)
                            {
                                return;
                            }
                            if let Some(repository) = this
                                .repositories
                                .iter()
                                .find(|repository| &repository.id == id)
                                .cloned()
                            {
                                this.select_repository(repository, window, cx);
                            }
                        }
                        SelectEvent::Confirm(None) => {
                            this.cancel();
                            this.clear_changes();
                            this.generated_clone_name.clear();
                            this.clone_name
                                .update(cx, |input, cx| input.set_value("", window, cx));
                            this.message = None;
                            this.error = None;
                            cx.notify();
                        }
                    }
                }),
            );
        self.repository_select = select;
    }

    fn select_repository(
        &mut self,
        repository: Repository,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.cancel();
        self.message = None;
        self.error = None;
        self.clone_ssh =
            repository.ssh_clone_url.is_some() && self.profile.ssh != SshChoice::Disabled;
        self.generated_clone_name = clone_directory_name(&repository.name);
        self.clone_name.update(cx, |input, cx| {
            input.set_value(self.generated_clone_name.clone(), window, cx)
        });
        self.clear_changes();
        self.selected_repository = Some(repository);
        self.scroll.set_offset(point(px(0.), px(0.)));
        cx.notify();
    }

    fn select_change(&mut self, change: ChangeRequest, cx: &mut Context<Self>) {
        let Some(repository) = self.selected_repository.clone() else {
            return;
        };
        self.clear_details();
        self.selected_change = Some(change.clone());
        self.read(ReadRequest::Discussions(repository, change, None), cx);
    }

    fn pick_path(&mut self, target: PathPickerTarget, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending.is_some() || self.picking_path {
            return;
        }
        let picker = rfd::AsyncFileDialog::new()
            .set_parent(window)
            .set_title(match target {
                PathPickerTarget::CloneParent => "Choisir le dossier parent du clone",
                PathPickerTarget::SshKey => "Choisir une clé privée SSH existante",
            });
        let generation = self.generation;
        let profile_id = self.profile.id.clone();
        self.picking_path = true;
        self.picker_task = Some(cx.spawn_in(window, async move |this, cx| {
            let selection = match target {
                PathPickerTarget::CloneParent => picker.pick_folder().await,
                PathPickerTarget::SshKey => picker.pick_file().await,
            };
            let _ = cx.update(|window, cx| {
                let _ = this.update(cx, |view, cx| {
                    if view.generation != generation || view.profile.id != profile_id {
                        return;
                    }
                    view.picking_path = false;
                    view.picker_task = None;
                    if let Some(selection) = selection {
                        let input = match target {
                            PathPickerTarget::CloneParent => &view.clone_parent,
                            PathPickerTarget::SshKey => &view.key_path,
                        };
                        input.update(cx, |input, cx| {
                            input.set_value(
                                display_path(&selection.path().to_string_lossy()),
                                window,
                                cx,
                            )
                        });
                    }
                    cx.notify();
                });
            });
        }));
        cx.notify();
    }

    fn clone_selected(&mut self, cx: &mut Context<Self>) {
        self.scroll.set_offset(point(px(0.), px(0.)));
        if self.pending.is_some() || self.picking_path {
            return;
        }
        let remote = self.clone_remote(cx);
        let Some(remote) = remote else {
            return;
        };
        let parent = expand_home(&self.clone_parent.read(cx).value());
        let name = self.clone_name.read(cx).value().trim().to_string();
        if !parent.is_absolute() || !valid_clone_directory_name(&name) {
            self.error =
                Some("Choisissez un dossier parent absolu et un nom de dossier simple.".into());
            cx.notify();
            return;
        }
        let destination = parent.join(name);
        let ssh = self.profile.ssh.clone();
        let account_id = self
            .selected_account
            .clone()
            .unwrap_or_else(|| "direct-clone".into());
        self.cancel();
        let generation = self.generation;
        let ticket = self.session.ticket(&account_id);
        let worker_ticket = ticket.clone();
        self.pending = Some(PendingOperation::Clone);
        self.message = Some(format!(
            "Clonage vers {}…",
            display_path(&destination.to_string_lossy())
        ));
        self.error = None;
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    mangrove_remote::clone_repository(
                        &remote,
                        &destination,
                        &ssh,
                        &worker_ticket.cancellation,
                    )
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                if !view.accepts(&ticket, generation) {
                    return;
                }
                view.pending = None;
                view.repository_select_dirty = true;
                match result {
                    Ok(path) => {
                        view.message = None;
                        cx.emit(IntegrationsEvent::OpenLocalFavorite(
                            path.to_string_lossy().into_owned(),
                        ));
                    }
                    Err(error) => {
                        view.message = None;
                        view.error = Some(error.to_string());
                    }
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn select_direct_clone(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.scroll.set_offset(point(px(0.), px(0.)));
        self.cancel();
        self.clear_reads();
        self.direct_clone = true;
        self.selected_account = None;
        self.generated_clone_name =
            clone_directory_from_remote(self.clone_url.read(cx).value().trim());
        self.clone_name.update(cx, |input, cx| {
            input.set_value(self.generated_clone_name.clone(), window, cx)
        });
        self.message = None;
        self.error = None;
        cx.notify();
    }

    fn clone_remote(&self, cx: &App) -> Option<String> {
        if self.direct_clone {
            let url = self.clone_url.read(cx).value().trim().to_string();
            (!url.is_empty()).then_some(url)
        } else {
            self.selected_repository.as_ref().and_then(|repository| {
                if self.clone_ssh {
                    repository.ssh_clone_url.clone()
                } else {
                    repository.clone_url.clone()
                }
            })
        }
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

    fn start_profile_rename(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(name) = self
            .profiles
            .iter()
            .find(|profile| profile.id == id)
            .map(|profile| profile.name.clone())
        else {
            return;
        };
        self.profile_rename.update(cx, |input, cx| {
            input.set_value(name, window, cx);
            input.focus(window, cx);
        });
        self.renaming_profile = Some(id);
        self.error = None;
        cx.notify();
    }

    fn cancel_profile_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.renaming_profile = None;
        self.focus_handle.focus(window, cx);
        cx.notify();
    }

    fn save_profile_name(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.renaming_profile.clone() else {
            return;
        };
        let name = self.profile_rename.read(cx).value().trim().to_string();
        if name.is_empty() {
            self.error = Some("Saisissez un nom de profil.".into());
            cx.notify();
            return;
        }
        let changed = self
            .profiles
            .iter()
            .any(|profile| profile.id == id && profile.name != name);
        self.error = None;
        self.cancel_profile_rename(window, cx);
        if changed {
            cx.emit(IntegrationsEvent::ProfileRenamed {
                profile_id: id,
                name,
            });
        }
    }

    fn can_delete_profile(&self, id: &str) -> bool {
        id != self.profile.id && self.profiles.len() > 1 && self.deleting_profile.is_none()
    }

    fn confirm_profile_deletion(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(profile) = self
            .profiles
            .iter()
            .find(|profile| profile.id == id)
            .cloned()
            .filter(|profile| self.can_delete_profile(&profile.id))
        else {
            return;
        };
        let answer = window.prompt(
            PromptLevel::Warning,
            &format!("Supprimer le profil « {} » ?", profile.name),
            Some(&profile_deletion_detail(&profile)),
            &[
                PromptButton::ok("Supprimer le profil"),
                PromptButton::cancel("Annuler"),
            ],
            cx,
        );
        cx.spawn(async move |this, cx| {
            if answer.await == Ok(0) {
                let _ = this.update(cx, |view, cx| view.delete_profile(profile, cx));
            }
        })
        .detach();
    }

    /// Credentials leave the vault before the metadata, so a vault failure
    /// keeps the profile and the deletion can be retried.
    fn delete_profile(&mut self, profile: Profile, cx: &mut Context<Self>) {
        if !self.can_delete_profile(&profile.id) {
            return;
        }
        self.deleting_profile = Some(profile.id.clone());
        self.error = None;
        let vault = self.vault.clone();
        cx.spawn(async move |this, cx| {
            let (profile, result) = cx
                .background_executor()
                .spawn(async move {
                    let result =
                        mangrove_remote::delete_profile_credentials(vault.as_ref(), &profile);
                    (profile, result)
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.deleting_profile = None;
                match result {
                    Ok(()) => cx.emit(IntegrationsEvent::ProfileDeleted(profile.id)),
                    Err(error) => {
                        view.error = Some(format!("Le profil n’a pas été supprimé : {error}"))
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
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

fn configured_browser_client_id(profile: &Profile, edition: ProviderEdition) -> String {
    let variable = match edition {
        ProviderEdition::GitLab => "MANGROVE_GITLAB_CLIENT_ID",
        ProviderEdition::GitHub => "MANGROVE_GITHUB_CLIENT_ID",
        ProviderEdition::BitbucketCloud => "MANGROVE_BITBUCKET_CLIENT_ID",
        _ => return String::new(),
    };
    profile
        .accounts
        .iter()
        .filter(|account| edition.matches(account))
        .find_map(|account| match (&account.authentication, edition) {
            (Authentication::GitLabOAuth { client_id }, ProviderEdition::GitLab)
            | (Authentication::GitHubOAuth { client_id }, ProviderEdition::GitHub)
            | (Authentication::BitbucketOAuth { client_id }, ProviderEdition::BitbucketCloud) => {
                Some(client_id.clone())
            }
            _ => None,
        })
        .unwrap_or_else(|| {
            std::env::var(variable)
                .unwrap_or_default()
                .trim()
                .to_string()
        })
}

fn supports_browser_connection(provider: Provider) -> bool {
    matches!(
        provider,
        Provider::GitLab | Provider::GitHub | Provider::BitbucketCloud
    )
}

fn provider_label(provider: Provider) -> &'static str {
    match provider {
        Provider::GitLab => "GitLab",
        Provider::GitHub => "GitHub",
        Provider::BitbucketCloud => "Bitbucket Cloud",
        Provider::BitbucketServer => "Bitbucket Server",
    }
}

fn default_clone_parent() -> String {
    dirs::home_dir()
        .map(|home| display_path(&home.to_string_lossy()))
        .unwrap_or_default()
}

/// Reads a path typed with the `~` shorthand shown by `display_path`.
fn expand_home(value: &str) -> PathBuf {
    let value = value.trim();
    let rest = match value.strip_prefix('~') {
        Some("") => "",
        Some(rest) => match rest.strip_prefix(['/', std::path::MAIN_SEPARATOR]) {
            Some(rest) => rest,
            None => return PathBuf::from(value),
        },
        None => return PathBuf::from(value),
    };
    match dirs::home_dir() {
        Some(home) if rest.is_empty() => home,
        Some(home) => home.join(rest),
        None => PathBuf::from(value),
    }
}

/// Bitbucket always needs its consumer secret. GitHub and GitLab keep their
/// application settings folded until a connection attempt requires them.
fn oauth_config_required(edition: ProviderEdition) -> bool {
    edition == ProviderEdition::BitbucketCloud
}

/// Two letters keep single-word names apart, such as Personnel and Professionnel.
fn profile_monogram(name: &str) -> String {
    if name.split_whitespace().nth(1).is_some() {
        return initials(name);
    }
    let mut letters = name.chars().filter(|c| c.is_alphanumeric());
    let Some(first) = letters.next() else {
        return String::new();
    };
    first
        .to_uppercase()
        .chain(letters.next().into_iter().flat_map(char::to_lowercase))
        .collect()
}

/// A stable hue per profile id, independent of the profile's position.
fn profile_hue(id: &str, hues: usize) -> usize {
    // FNV-1a stays identical across launches and Rust releases.
    let hash = id.bytes().fold(0x811c_9dc5_u32, |hash, byte| {
        (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193)
    });
    hash as usize % hues
}

fn profile_deletion_detail(profile: &Profile) -> String {
    let accounts = match profile.accounts.len() {
        0 => String::new(),
        1 => {
            "Son compte connecté sera déconnecté et ses identifiants retirés du coffre du système. "
                .into()
        }
        count => format!(
            "Ses {count} comptes connectés seront déconnectés et leurs identifiants retirés du coffre du système. "
        ),
    };
    format!(
        "{accounts}Ses favoris, préférences et dépôts récents seront supprimés ; les dépôts sur le disque ne sont pas modifiés. Cette action est irréversible."
    )
}

fn valid_clone_directory_name(name: &str) -> bool {
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ((stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.len() == 4
            && stem.as_bytes()[3].is_ascii_digit()
            && stem.as_bytes()[3] != b'0');
    !name.is_empty()
        && name != "."
        && name != ".."
        && name.len() <= 255
        && !reserved
        && !name.ends_with(['.', ' '])
        && !name.contains(['/', '\\', '<', '>', ':', '"', '|', '?', '*'])
        && !name.chars().any(char::is_control)
}

fn clone_directory_from_remote(remote: &str) -> String {
    clone_directory_name(
        remote
            .trim_end_matches('/')
            .rsplit(['/', ':'])
            .next()
            .unwrap_or_default(),
    )
}

fn clone_directory_name(name: &str) -> String {
    let candidate = name.strip_suffix(".git").unwrap_or(name);
    if valid_clone_directory_name(candidate) {
        candidate.to_string()
    } else {
        "depot".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        IntegrationsEvent, ProviderEdition, accepts_response, bounded,
        configured_browser_client_id, disconnect_completion,
    };
    use mangrove_remote::{
        Account, Authentication, Profile, ProfileSession, Provider, RemoteError,
    };

    #[test]
    fn repository_options_keep_full_names_and_stable_ids_in_namespace_groups() {
        use gpui_kit::component::searchable_list::SearchableListItem;
        let repository = |id: &str, full_name: &str| mangrove_remote::providers::Repository {
            id: id.into(),
            name: "api".into(),
            full_name: full_name.into(),
            web_url: None,
            clone_url: None,
            ssh_clone_url: None,
        };
        let groups = super::repository_option_groups(&[
            repository("2", "other/api"),
            repository("1", "Team/Services/api"),
            repository("3", "Team/Services/worker"),
        ]);
        let team = groups
            .iter()
            .find(|group| group.title.as_ref() == "Team/Services")
            .unwrap();
        assert_eq!(team.items.len(), 2);
        assert_eq!(team.items[0].value(), "1");
        assert_eq!(team.items[0].title().as_ref(), "Team/Services/api");
        assert!(team.items[0].matches("TEAM/SERVICES/API"));
        assert!(!team.items[0].matches("other/api"));
        let other = groups
            .iter()
            .find(|group| group.title.as_ref() == "other")
            .unwrap();
        assert_eq!(other.items[0].value(), "2");
    }

    #[test]
    fn clone_destination_names_cannot_escape_the_selected_parent() {
        for invalid in [
            "",
            ".",
            "..",
            "../outside",
            "folder/other",
            "folder\\other",
            "/tmp/other",
            "C:\\other",
            "line\nfeed",
            "CON",
            "nul.txt",
            "COM1",
            "trailing.",
            "trailing ",
        ] {
            assert!(!super::valid_clone_directory_name(invalid), "{invalid:?}");
        }
        for valid in [
            "Hello-World",
            "mon dépôt",
            "équipe",
            "repo.git",
            ".hidden-repo",
            "company-service-9",
        ] {
            assert!(super::valid_clone_directory_name(valid), "{valid:?}");
        }
        assert_eq!(
            super::clone_directory_name("Hello-World.git"),
            "Hello-World"
        );
        assert_eq!(super::clone_directory_name("../../outside"), "depot");
        assert_eq!(super::clone_directory_name("repo.git.git"), "repo.git");
        assert_eq!(
            super::clone_directory_from_remote("https://github.com/octocat/Hello-World.git"),
            "Hello-World"
        );
        assert_eq!(
            super::clone_directory_from_remote("git@example.com:group/repository.git"),
            "repository"
        );
    }

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
        assert_eq!(
            configured_browser_client_id(&personal, ProviderEdition::GitLab),
            "personal-id"
        );
        assert_eq!(
            configured_browser_client_id(&professional, ProviderEdition::GitLab),
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
    fn browser_application_id_prefill_keeps_provider_and_edition_isolation() {
        let mut profile = Profile::new("personal", "Personnel");
        for (id, provider, server, authentication) in [
            (
                "gitlab",
                Provider::GitLab,
                "https://gitlab.com",
                Authentication::GitLabOAuth {
                    client_id: "gitlab-id".into(),
                },
            ),
            (
                "enterprise",
                Provider::GitHub,
                "https://github.example.com",
                Authentication::GitHubOAuth {
                    client_id: "enterprise-id".into(),
                },
            ),
            (
                "github",
                Provider::GitHub,
                "https://github.com/",
                Authentication::GitHubOAuth {
                    client_id: "github-id".into(),
                },
            ),
            (
                "bitbucket",
                Provider::BitbucketCloud,
                "https://bitbucket.org/test-workspace",
                Authentication::BitbucketOAuth {
                    client_id: "bitbucket-key".into(),
                },
            ),
        ] {
            profile.accounts.push(Account {
                id: id.into(),
                label: "Compte".into(),
                provider,
                server_url: server.into(),
                username: None,
                authentication,
            });
        }
        assert_eq!(
            configured_browser_client_id(&profile, ProviderEdition::GitHub),
            "github-id"
        );
        assert_eq!(
            configured_browser_client_id(&profile, ProviderEdition::GitLab),
            "gitlab-id"
        );
        assert_eq!(
            configured_browser_client_id(&profile, ProviderEdition::BitbucketCloud),
            "bitbucket-key"
        );
        assert!(
            configured_browser_client_id(&profile, ProviderEdition::BitbucketDataCenter).is_empty()
        );
        assert!(
            configured_browser_client_id(&profile, ProviderEdition::GitHubEnterprise).is_empty()
        );
    }

    #[test]
    fn default_profiles_get_distinct_monograms_and_hues() {
        let profiles = mangrove_remote::Profiles::default();
        let [personal, professional] = &profiles.profiles[..] else {
            panic!("two default profiles");
        };
        assert_eq!(super::profile_monogram(&personal.name), "Pe");
        assert_eq!(super::profile_monogram(&professional.name), "Pr");
        assert_ne!(
            super::profile_hue(&personal.id, 8),
            super::profile_hue(&professional.id, 8)
        );
        assert_eq!(super::profile_monogram("Client Acme"), "CA");
        assert_eq!(super::profile_monogram("é"), "É");
        assert_eq!(super::profile_monogram("  "), "");
    }

    #[test]
    fn profile_deletion_detail_names_the_credentials_it_removes() {
        let mut profile = Profile::new("work", "Travail");
        assert!(super::profile_deletion_detail(&profile).starts_with("Ses favoris"));
        for id in ["one", "two"] {
            profile.accounts.push(Account {
                id: id.into(),
                label: "Compte".into(),
                provider: Provider::GitLab,
                server_url: "https://gitlab.com".into(),
                username: None,
                authentication: Authentication::PersonalToken,
            });
            let detail = super::profile_deletion_detail(&profile);
            assert!(detail.contains("coffre du système"), "{detail}");
        }
        assert!(super::profile_deletion_detail(&profile).starts_with("Ses 2 comptes connectés"));
    }

    #[test]
    fn typed_paths_accept_the_displayed_home_shorthand() {
        let home = dirs::home_dir().unwrap();
        assert_eq!(super::expand_home(" ~ "), home);
        assert_eq!(super::expand_home("~/code"), home.join("code"));
        assert_eq!(
            super::expand_home(&crate::format::display_path(
                &home.join("a b").to_string_lossy()
            )),
            home.join("a b")
        );
        assert_eq!(
            super::expand_home("~other"),
            std::path::PathBuf::from("~other")
        );
        assert_eq!(
            super::expand_home("/tmp/repo"),
            std::path::PathBuf::from("/tmp/repo")
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
