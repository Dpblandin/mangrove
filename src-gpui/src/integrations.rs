use crate::{
    format::{count_label, display_path, initials},
    recent_repositories::is_remote_favorite,
    settings::ui::{Tone, field_border},
    theme::{Palette, RADIUS_MD, icon, scrollbar_theme},
};
use gpui_kit::assets::IconName;
use gpui_kit::{
    base::Button,
    component::{
        IndexPath, Sizable,
        input::{Escape as InputEscape, InputEvent, InputState},
        scroll::Scrollbar,
        searchable_list::{
            SearchableGroup, SearchableListDelegate, SearchableListItem, SearchableVec,
        },
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

/// Where a progress note, a confirmation or an error appears: next to the
/// field or action it concerns rather than above the page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Spot {
    /// Problems without a field of their own, such as an unreadable profile
    /// file, and the clone screen's operations.
    Page,
    NewProfile,
    ProfileRename,
    ProfileList,
    Favorite,
    Ssh,
    SshKey,
    SshRemote,
    /// The server or workspace URL of the connection form.
    Server,
    Connection,
    /// Above the accounts of the selected edition.
    Accounts,
    Account(String),
}

impl IntegrationsEvent {
    /// Where a failure to save this change is reported.
    pub(crate) fn spot(&self) -> Spot {
        match self {
            Self::ProfileCreated(_) => Spot::NewProfile,
            Self::AccountAdded { .. } => Spot::Connection,
            Self::AccountRemoved { account_id, .. } => Spot::Account(account_id.clone()),
            Self::FavoritesChanged(_) => Spot::Favorite,
            Self::SshChanged(_) => Spot::Ssh,
            Self::ProfileSelected(_)
            | Self::ProfileRenamed { .. }
            | Self::ProfileDeleted(_)
            | Self::OpenLocalFavorite(_)
            | Self::OpenIntegrationSettings => Spot::ProfileList,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
struct Feedback {
    spot: Spot,
    tone: Tone,
    text: String,
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
            _ => "",
        }
    }

    /// An example of the server or workspace URL the edition expects.
    fn server_example(self) -> &'static str {
        match self {
            Self::GitHubEnterprise => "https://github.exemple.com",
            Self::GitLabSelfManaged => "https://gitlab.exemple.com",
            Self::BitbucketCloud => "https://bitbucket.org/votre-workspace",
            Self::BitbucketDataCenter => "https://bitbucket.exemple.com",
            _ => "",
        }
    }

    /// The name the browser connection is offered with.
    fn brand(self) -> &'static str {
        match self {
            Self::GitHub => "GitHub",
            Self::GitHubEnterprise => "GitHub Enterprise",
            Self::GitLab | Self::GitLabSelfManaged => "GitLab",
            Self::BitbucketCloud => "Bitbucket Cloud",
            Self::BitbucketDataCenter => "Bitbucket Data Center",
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
type ProfileSelect = SelectState<SearchableVec<ProfileOption>>;

/// A profile in the settings header's switcher, with its avatar colors.
#[derive(Clone)]
struct ProfileOption {
    id: String,
    name: String,
    monogram: String,
    color: u32,
    background: u32,
}

impl ProfileOption {
    fn new(profile: &Profile, p: Palette) -> Self {
        let (color, background) = profile_colors(profile, p);
        Self {
            id: profile.id.clone(),
            name: profile.name.clone(),
            monogram: profile_monogram(&profile.name),
            color,
            background,
        }
    }

    fn row(&self) -> Div {
        div()
            .flex()
            .items_center()
            .gap(px(8.))
            .min_w_0()
            .child(
                div()
                    .size(px(20.))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .bg(rgb(self.background))
                    .text_color(rgb(self.color))
                    .text_size(px(9.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(self.monogram.clone()),
            )
            .child(div().min_w_0().truncate().child(self.name.clone()))
    }
}

impl SearchableListItem for ProfileOption {
    type Value = String;

    fn title(&self) -> SharedString {
        self.name.clone().into()
    }

    fn display_title(&self) -> Option<AnyElement> {
        Some(self.row().into_any_element())
    }

    fn render(&self, _: &mut Window, _: &mut App) -> impl IntoElement {
        self.row()
    }

    fn value(&self) -> &String {
        &self.id
    }
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
    Identity,
    RepositoryRead,
    Read,
    Clone,
}

#[derive(Clone, Copy)]
enum PathPickerTarget {
    CloneParent,
    SshKey,
    Favorite,
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
    /// The edition's OAuth application ID was already known when the form
    /// opened; otherwise the form walks through creating the application.
    app_configured: bool,
    /// Creation instructions unfolded for an already configured application.
    setup_open: bool,
    github_private_repos: bool,
    github_authorization: Option<GitHubAuthorization>,
    key_path: Entity<InputState>,
    /// The key option is shown before a key has been retained.
    choosing_key: bool,
    /// The key field returns to the retained key once the section is left.
    key_draft_reset: bool,
    /// An « Activer » button that disappears hands the keyboard to the page.
    refocus_on_switch: bool,
    /// Radio groups take a single tab stop; arrows move the choice.
    ssh_focus: [FocusHandle; 3],
    provider_focus: [FocusHandle; 6],
    /// The copied value's button reads « Copié » for a moment.
    copied: Option<&'static str>,
    copied_task: Option<Task<()>>,
    favorite: Entity<InputState>,
    ssh_remote: Entity<InputState>,
    ssh_references: Vec<String>,
    edition: ProviderEdition,
    adding_account: bool,
    profile_select: Entity<ProfileSelect>,
    /// The switcher's avatars wait for the next render to take a new theme.
    profile_select_dirty: bool,
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
    feedback: Option<Feedback>,
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
        let edition = default_edition(&profile);
        let client_id = configured_browser_client_id(&profile, edition);
        let app_configured = !client_id.is_empty();
        let repository_menu_data = Rc::new(RefCell::new(RepositoryMenuData::default()));
        let options = RepositoryMenu::new(
            repository_menu_data.clone(),
            RepositoryLoadTrigger::new(cx.entity().downgrade(), 0),
            RepositoryLoadState::Complete,
            palette,
            offset,
        );
        let repository_select =
            cx.new(|cx| SelectState::new(options, None, window, cx).searchable(true));
        let profile_select = cx.new(|cx| {
            SelectState::new(
                SearchableVec::new(vec![ProfileOption::new(&profile, palette)]),
                Some(IndexPath::new(0)),
                window,
                cx,
            )
        });
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
        let profile_name = text_input("Par exemple : Client Acme", window, cx);
        let label = text_input("Nom affiché dans Mangrove", window, cx);
        let server = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder(edition.server_example())
                .default_value(edition.server())
        });
        let username = text_input("nom@exemple.com", window, cx);
        let token = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Collez le token")
                .masked(true)
        });
        let client_id = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Collez l’identifiant de l’application")
                .default_value(client_id)
        });
        let consumer_secret = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("Collez le secret du consumer")
                .masked(true)
        });
        let favorite = text_input(
            "~/projets/depot ou https://serveur/groupe/depot",
            window,
            cx,
        );
        let ssh_remote = text_input("git@serveur:groupe/depot.git", window, cx);
        let mut subscriptions = vec![];
        // Typing retires the error a field caused; Enter submits its form.
        for (input, spot) in [
            (&profile_name, Spot::NewProfile),
            (&profile_rename, Spot::ProfileRename),
            (&favorite, Spot::Favorite),
            (&key_path, Spot::SshKey),
            (&ssh_remote, Spot::SshRemote),
            (&label, Spot::Connection),
            (&server, Spot::Server),
            (&username, Spot::Connection),
            (&token, Spot::Connection),
            (&client_id, Spot::Connection),
            (&consumer_secret, Spot::Connection),
        ] {
            subscriptions.push(cx.subscribe_in(
                input,
                window,
                move |this, _, event, window, cx| match event {
                    InputEvent::Change => {
                        this.settle_feedback_at(&spot);
                        cx.notify();
                    }
                    InputEvent::PressEnter { .. } => this.submit(&spot, window, cx),
                    InputEvent::Focus | InputEvent::Blur => {}
                },
            ));
        }
        subscriptions.push(cx.subscribe_in(
            &profile_select,
            window,
            |this, _, event: &SelectEvent<SearchableVec<ProfileOption>>, window, cx| {
                if let SelectEvent::Confirm(Some(id)) = event
                    && *id != this.profile.id
                {
                    // The switcher shows the active profile until the switch,
                    // which may still be refused or cancelled, completes.
                    this.sync_profile_select(window, cx);
                    this.select_profile(id.clone(), cx);
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
            profile_name,
            profile_rename,
            renaming_profile: None,
            deleting_profile: None,
            label,
            server,
            username,
            token,
            client_id,
            consumer_secret,
            browser_connection: true,
            app_configured,
            setup_open: false,
            github_private_repos: false,
            github_authorization: None,
            key_path,
            choosing_key: false,
            key_draft_reset: false,
            refocus_on_switch: false,
            ssh_focus: std::array::from_fn(|_| cx.focus_handle()),
            provider_focus: std::array::from_fn(|_| cx.focus_handle()),
            copied: None,
            copied_task: None,
            favorite,
            ssh_remote,
            ssh_references: Vec::new(),
            edition,
            adding_account: false,
            profile_select,
            profile_select_dirty: false,
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
            feedback: None,
        }
    }

    pub fn set_profiles(
        &mut self,
        profiles: Vec<Profile>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.profiles = profiles;
        self.sync_profile_select(window, cx);
        cx.notify();
    }

    pub fn profile_name(&self) -> &str {
        &self.profile.name
    }

    /// The settings header's profile switcher.
    pub fn profile_switcher(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        if self.profile_select_dirty {
            self.sync_profile_select(window, cx);
        }
        let p = self.palette;
        Select::new(&self.profile_select)
            .id("profile-switcher")
            .w(px(220.))
            .h(px(32.))
            .px(px(10.))
            .border_1()
            .border_color(rgb(field_border(p)))
            .rounded(RADIUS_MD)
            .bg(rgb(p.bg))
            .text_color(rgb(p.text))
            .text_size(px(12. + self.offset))
            .menu_max_h(px(320.))
            .accessibility_label(format!("Profil actif : {}", self.profile.name))
    }

    fn sync_profile_select(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.profile_select_dirty = false;
        let p = self.palette;
        let mut options: Vec<_> = self
            .profiles
            .iter()
            .map(|profile| ProfileOption::new(profile, p))
            .collect();
        if !options.iter().any(|option| option.id == self.profile.id) {
            options.insert(0, ProfileOption::new(&self.profile, p));
        }
        let active = self.profile.id.clone();
        self.profile_select.update(cx, |select, cx| {
            select.set_items(SearchableVec::new(options), window, cx);
            select.set_selected_value(&active, window, cx);
            cx.notify();
        });
    }

    pub fn set_section(&mut self, section: IntegrationsSection, cx: &mut Context<Self>) {
        self.scroll.set_offset(point(px(0.), px(0.)));
        if self.section != section {
            self.cancel();
            self.clear_reads();
            self.feedback = None;
            self.choosing_key = false;
            self.key_draft_reset = true;
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
        // Reopened settings start without the previous visit's messages, and
        // show the SSH choice that is actually saved.
        self.feedback = None;
        self.choosing_key = false;
        self.key_draft_reset = true;
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
        self.profile_select_dirty = true;
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
        }
        if switched {
            // Only a switch drops the messages: a confirmation of this
            // profile's own change, such as a new account, stays visible.
            self.feedback = None;
            if std::mem::take(&mut self.refocus_on_switch) {
                self.focus_handle.focus(window, cx);
            }
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
            self.edition = default_edition(&profile);
            self.adding_account = false;
            self.choosing_key = false;
            self.renaming_profile = None;
            self.direct_clone = false;
            self.clone_ssh = false;
            self.clone_parent.update(cx, |input, cx| {
                input.set_value(default_clone_parent(), window, cx)
            });
            self.browser_connection = true;
            self.github_private_repos = false;
            let client_id = configured_browser_client_id(&profile, self.edition);
            self.app_configured = !client_id.is_empty();
            self.setup_open = false;
            self.client_id
                .update(cx, |input, cx| input.set_value(client_id, window, cx));
            let edition = self.edition;
            self.server.update(cx, |input, cx| {
                input.set_value(edition.server(), window, cx);
                input.set_placeholder(edition.server_example(), window, cx);
            });
        }
        // Choosing the agent keeps a typed key path for a later return.
        if let SshChoice::KeyFile { path } = &profile.ssh {
            let path = display_path(&path.to_string_lossy());
            self.key_path
                .update(cx, |input, cx| input.set_value(path, window, cx));
        } else if switched {
            self.key_path
                .update(cx, |input, cx| input.set_value("", window, cx));
        }
        self.profile = profile;
        self.sync_profile_select(window, cx);
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

    pub fn set_error(&mut self, spot: Spot, message: String, cx: &mut Context<Self>) {
        self.report(spot, Tone::Error, message);
        cx.notify();
    }

    fn report(&mut self, spot: Spot, tone: Tone, text: impl Into<String>) {
        self.feedback = Some(Feedback {
            spot,
            tone,
            text: text.into(),
        });
    }

    /// Editing a field retires its error or confirmation; a running operation
    /// keeps its progress note.
    fn settle_feedback_at(&mut self, spot: &Spot) {
        if self
            .feedback
            .as_ref()
            .is_some_and(|feedback| &feedback.spot == spot && feedback.tone != Tone::Progress)
        {
            self.feedback = None;
        }
    }

    fn submit(&mut self, spot: &Spot, window: &mut Window, cx: &mut Context<Self>) {
        match spot {
            Spot::NewProfile => self.create_profile(window, cx),
            Spot::ProfileRename => self.save_profile_name(window, cx),
            Spot::Favorite => self.add_favorite(window, cx),
            Spot::SshKey => self.save_ssh_key(cx),
            Spot::SshRemote => self.read_ssh(cx),
            Spot::Connection | Spot::Server if self.can_connect(cx) => self.connect(window, cx),
            _ => {}
        }
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
            input.set_value(edition.server(), window, cx);
            input.set_placeholder(edition.server_example(), window, cx);
        });
        // The account name may stay; credentials and addresses belong to an edition.
        self.clear_connection_secrets(window, cx);
        self.username
            .update(cx, |input, cx| input.set_value("", window, cx));
        let client_id = configured_browser_client_id(&self.profile, edition);
        self.app_configured = !client_id.is_empty();
        self.setup_open = false;
        self.client_id
            .update(cx, |input, cx| input.set_value(client_id, window, cx));
        self.feedback = None;
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
        self.settle_feedback_at(&Spot::Connection);
        cx.notify();
    }

    /// The edition needs its own server or workspace URL.
    fn needs_server(&self) -> bool {
        self.edition.custom_server() || self.edition == ProviderEdition::BitbucketCloud
    }

    /// Explains why the server or workspace URL cannot be used.
    fn server_refusal(&self, cx: &App) -> Option<String> {
        if !self.needs_server() {
            return None;
        }
        let server = self.server.read(cx).value().trim().to_string();
        let example = self.edition.server_example();
        if self.edition == ProviderEdition::BitbucketCloud {
            bitbucket_oauth::application_settings_url(&server)
                .is_err()
                .then(|| format!("Indiquez l’adresse HTTPS du workspace, par exemple {example}."))
        } else {
            mangrove_remote::SafeApiUrl::new(&server, "")
                .is_err()
                .then(|| format!("Indiquez l’adresse HTTPS du serveur, par exemple {example}."))
        }
    }

    fn open_application_settings(&mut self, cx: &mut Context<Self>) {
        if let Some(refusal) = self.server_refusal(cx) {
            self.report(Spot::Server, Tone::Error, refusal);
            cx.notify();
            return;
        }
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
                self.report(Spot::Connection, Tone::Error, error.to_string());
                cx.notify();
            }
        }
    }

    fn connect(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending.is_some() {
            return;
        }
        if let Some(refusal) = self.server_refusal(cx) {
            self.report(Spot::Server, Tone::Error, refusal);
            cx.notify();
            return;
        }
        if self.profile.accounts.len() >= 32 {
            self.clear_connection_secrets(window, cx);
            self.report(
                Spot::Connection,
                Tone::Error,
                "Le profil contient déjà 32 comptes. Déconnectez un compte avant d’en ajouter un.",
            );
            cx.notify();
            return;
        }
        let provider = self.edition.provider();
        let browser = supports_browser_connection(provider) && self.browser_connection;
        let (request, authentication) = if browser {
            let client_id = self.client_id.read(cx).value().trim().to_string();
            if client_id.is_empty() {
                self.report(
                    Spot::Connection,
                    Tone::Error,
                    format!(
                        "Créez l’application Mangrove dans {} puis collez son identifiant.",
                        provider_label(provider)
                    ),
                );
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
                    self.report(
                        Spot::Connection,
                        Tone::Error,
                        "Collez le secret du consumer OAuth Bitbucket.",
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
                self.report(Spot::Connection, Tone::Error, "Collez un token personnel.");
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
        self.report(
            Spot::Connection,
            Tone::Progress,
            if browser {
                format!("Préparation de la connexion {}…", provider_label(provider))
            } else {
                "Vérification du compte…".into()
            },
        );
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
                        view.report(
                            Spot::Connection,
                            Tone::Progress,
                            "Autorisez Mangrove dans votre navigateur, puis revenez ici.",
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
                        view.report(
                            Spot::Connection,
                            Tone::Progress,
                            "Saisissez le code affiché ci-dessous dans GitHub, puis autorisez Mangrove.",
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
                            view.report(
                                Spot::Account(account.id.clone()),
                                Tone::Success,
                                "Compte connecté. Ses dépôts sont disponibles dans Nouvel onglet → Cloner.",
                            );
                            cx.emit(IntegrationsEvent::AccountAdded {
                                profile_id: ticket.profile_id.clone(),
                                account: account.clone(),
                            });
                        }
                        Err(error) => {
                            view.report(Spot::Connection, Tone::Error, error.to_string());
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
        self.report(
            Spot::Account(account.id.clone()),
            Tone::Progress,
            "Vérification du compte…",
        );
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
                        view.report(
                            Spot::Account(ticket.account_id.clone()),
                            Tone::Success,
                            "Compte vérifié.",
                        );
                    }
                    Err(error) => {
                        view.report(
                            Spot::Account(ticket.account_id.clone()),
                            Tone::Error,
                            error.to_string(),
                        );
                    }
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    /// Removing the token cannot be undone from Mangrove, so ask first.
    fn confirm_disconnect(&mut self, id: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(account) = self
            .profile
            .accounts
            .iter()
            .find(|account| account.id == id)
        else {
            return;
        };
        let name = self
            .identities
            .get(&id)
            .map(|identity| identity.display_name.clone())
            .unwrap_or_else(|| account.label.clone());
        let answer = window.prompt(
            PromptLevel::Warning,
            &format!("Déconnecter le compte « {name} » ?"),
            Some(&format!(
                "Son token sera retiré du coffre du système et il faudra reconnecter le compte pour consulter ses dépôts. Le token n’est pas révoqué sur {}.",
                provider_label(account.provider)
            )),
            // Return must not confirm a removal: Cancel comes first and keeps
            // Escape, so the destructive button needs an explicit choice.
            &[
                PromptButton::cancel("Annuler"),
                PromptButton::ok("Déconnecter"),
            ],
            cx,
        );
        cx.spawn(async move |this, cx| {
            if answer.await == Ok(1) {
                let _ = this.update(cx, |view, cx| view.disconnect(id, cx));
            }
        })
        .detach();
    }

    /// The connection form's required fields hold a value.
    fn can_connect(&self, cx: &App) -> bool {
        let filled = |input: &Entity<InputState>| !input.read(cx).value().trim().is_empty();
        let provider = self.edition.provider();
        let server = !self.needs_server() || filled(&self.server);
        let credentials = if supports_browser_connection(provider) && self.browser_connection {
            filled(&self.client_id)
                && (provider != Provider::BitbucketCloud || filled(&self.consumer_secret))
        } else {
            filled(&self.token)
        };
        self.pending.is_none() && server && credentials
    }

    /// Stops the running operation from its progress note.
    fn cancel_pending(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let cloning = self.cloning();
        self.cancel();
        self.clear_connection_secrets(window, cx);
        self.feedback = None;
        if cloning {
            self.report(
                Spot::Page,
                Tone::Error,
                "Clonage annulé. Un dossier incomplet peut subsister ; choisissez un nouveau nom avant de réessayer.",
            );
        }
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
        self.report(
            Spot::Account(id.clone()),
            Tone::Progress,
            "Suppression du token du coffre…",
        );
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
                        Ok(()) => view.report(
                            Spot::Accounts,
                            Tone::Success,
                            "Compte déconnecté et token supprimé du coffre.",
                        ),
                        Err(error) => view.report(
                            Spot::Account(ticket.account_id.clone()),
                            Tone::Error,
                            error.to_string(),
                        ),
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
        self.feedback = None;
        if !append {
            self.report(Spot::Page, Tone::Progress, "Lecture distante en cours…");
        }
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
                view.feedback = None;
                match result {
                    Ok(ReadResponse::Repositories(page)) => {
                        let result = view
                            .repository_menu_data
                            .borrow_mut()
                            .append(&mut view.repositories, page.items);
                        match result {
                            Ok(()) => view.repositories_next = page.next,
                            Err(error) => {
                                view.report(Spot::Page, Tone::Error, error.to_string());
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
                        view.report(Spot::Page, Tone::Error, error.to_string());
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
            self.offset,
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
                            this.feedback = None;
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
        self.feedback = None;
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
                PathPickerTarget::Favorite => "Choisir un dépôt local",
            });
        let generation = self.generation;
        let profile_id = self.profile.id.clone();
        self.picking_path = true;
        self.picker_task = Some(cx.spawn_in(window, async move |this, cx| {
            let selection = match target {
                PathPickerTarget::CloneParent | PathPickerTarget::Favorite => {
                    picker.pick_folder().await
                }
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
                        let (input, spot) = match target {
                            PathPickerTarget::CloneParent => (&view.clone_parent, Spot::Page),
                            PathPickerTarget::SshKey => (&view.key_path, Spot::SshKey),
                            PathPickerTarget::Favorite => (&view.favorite, Spot::Favorite),
                        };
                        let input = input.clone();
                        view.settle_feedback_at(&spot);
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
            self.report(
                Spot::Page,
                Tone::Error,
                "Choisissez un dossier parent absolu et un nom de dossier simple.",
            );
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
        self.report(
            Spot::Page,
            Tone::Progress,
            format!(
                "Clonage vers {}…",
                display_path(&destination.to_string_lossy())
            ),
        );
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
                        view.feedback = None;
                        cx.emit(IntegrationsEvent::OpenLocalFavorite(
                            path.to_string_lossy().into_owned(),
                        ));
                    }
                    Err(error) => view.report(Spot::Page, Tone::Error, error.to_string()),
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    /// Starts a direct clone of `url`, unless a clone is already running.
    pub fn prefill_clone_url(&mut self, url: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.cloning() {
            return;
        }
        self.clone_url
            .update(cx, |input, cx| input.set_value(url.to_owned(), window, cx));
        self.select_direct_clone(window, cx);
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
        self.feedback = None;
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
        let value = favorite_entry(&self.favorite.read(cx).value());
        let local = !value.is_empty() && !is_remote_favorite(&value);
        let refusal = if value.is_empty() {
            Some("Indiquez le chemin ou l’URL d’un dépôt.".to_string())
        } else if local && !PathBuf::from(&value).is_absolute() {
            Some(
                "Indiquez un chemin absolu, par exemple ~/projets/depot, ou l’URL du dépôt.".into(),
            )
        } else if local && !PathBuf::from(&value).is_dir() {
            Some("Aucun dossier à cet emplacement.".into())
        } else if self
            .profile
            .favorites
            .iter()
            .any(|favorite| same_favorite(favorite, &value))
        {
            Some("Ce dépôt est déjà dans les favoris.".into())
        } else if self.profile.favorites.len() >= MAX_ROWS {
            Some(format!(
                "Les favoris sont limités à {MAX_ROWS} dépôts. Retirez-en un avant d’en ajouter."
            ))
        } else {
            None
        };
        if let Some(refusal) = refusal {
            self.report(Spot::Favorite, Tone::Error, refusal);
            cx.notify();
            return;
        }
        self.feedback = None;
        self.profile.favorites.push(value);
        self.favorite
            .update(cx, |input, cx| input.set_value("", window, cx));
        cx.emit(IntegrationsEvent::FavoritesChanged(
            self.profile.favorites.clone(),
        ));
        cx.notify();
    }

    fn input_focused(&self, window: &Window, cx: &App) -> bool {
        [
            &self.profile_name,
            &self.label,
            &self.server,
            &self.username,
            &self.token,
            &self.client_id,
            &self.consumer_secret,
            &self.key_path,
            &self.favorite,
            &self.ssh_remote,
            &self.clone_url,
            &self.clone_parent,
            &self.clone_name,
        ]
        .into_iter()
        .any(|input| input.read(cx).focus_handle(cx).is_focused(window))
            // An open select hands its focus to the search field of its menu;
            // leaving that field closes the menu.
            || self
                .repository_select
                .read(cx)
                .focus_handle(cx)
                .is_focused(window)
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
        self.feedback = None;
        cx.notify();
    }

    fn cancel_profile_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.renaming_profile = None;
        self.focus_handle.focus(window, cx);
        cx.notify();
    }

    /// Two profiles cannot share a name: the switcher and the confirmations
    /// identify a profile by its name.
    fn profile_name_refusal(&self, name: &str, renamed: Option<&str>) -> Option<String> {
        profile_name_error(name).or_else(|| {
            self.profiles
                .iter()
                .filter(|profile| Some(profile.id.as_str()) != renamed)
                .any(|profile| profile.name.trim().to_lowercase() == name.to_lowercase())
                .then(|| "Un autre profil porte déjà ce nom.".into())
        })
    }

    /// A refused name stays in its field with the reason, like a rename.
    fn create_profile(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.profile_name.read(cx).value().trim().to_string();
        if let Some(error) = self.profile_name_refusal(&name, None) {
            self.report(Spot::NewProfile, Tone::Error, error);
            cx.notify();
            return;
        }
        self.profile_name
            .update(cx, |input, cx| input.set_value("", window, cx));
        self.report(
            Spot::NewProfile,
            Tone::Success,
            format!("Profil « {name} » créé. Activez-le pour l’utiliser."),
        );
        cx.emit(IntegrationsEvent::ProfileCreated(name));
        cx.notify();
    }

    fn save_profile_name(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(id) = self.renaming_profile.clone() else {
            return;
        };
        let name = self.profile_rename.read(cx).value().trim().to_string();
        if let Some(error) = self.profile_name_refusal(&name, Some(&id)) {
            self.report(Spot::ProfileRename, Tone::Error, error);
            cx.notify();
            return;
        }
        let changed = self
            .profiles
            .iter()
            .any(|profile| profile.id == id && profile.name != name);
        self.feedback = None;
        self.cancel_profile_rename(window, cx);
        if changed {
            cx.emit(IntegrationsEvent::ProfileRenamed {
                profile_id: id,
                name,
            });
        }
    }

    /// Activates from a profile row, whose button gives way to the badge.
    fn activate_profile(&mut self, id: String, cx: &mut Context<Self>) {
        self.refocus_on_switch = true;
        self.select_profile(id, cx);
    }

    /// A profile whose credentials are leaving the vault cannot become active.
    fn select_profile(&mut self, id: String, cx: &mut Context<Self>) {
        if self.deleting_profile.as_ref() != Some(&id) {
            cx.emit(IntegrationsEvent::ProfileSelected(id));
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
            // As for a disconnection, Return cannot delete.
            &[
                PromptButton::cancel("Annuler"),
                PromptButton::ok("Supprimer le profil"),
            ],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if answer.await == Ok(1) {
                let _ = this.update_in(cx, |view, window, cx| {
                    // The deleted row's buttons disappear; keep the keyboard in the page.
                    view.focus_handle.focus(window, cx);
                    view.delete_profile(profile, cx)
                });
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
        self.feedback = None;
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
                    Err(error) => view.report(
                        Spot::ProfileList,
                        Tone::Error,
                        format!("Le profil n’a pas été supprimé : {error}"),
                    ),
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
        self.choosing_key = false;
        self.feedback = None;
        self.profile.ssh = choice.clone();
        cx.emit(IntegrationsEvent::SshChanged(choice));
        cx.notify();
    }

    /// Shows the key field; the choice applies once a key is retained.
    fn choose_ssh_key(&mut self, focus_field: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !matches!(self.profile.ssh, SshChoice::KeyFile { .. }) {
            self.choosing_key = true;
            self.feedback = None;
            if focus_field {
                self.key_path
                    .update(cx, |input, cx| input.focus(window, cx));
            }
        }
        cx.notify();
    }

    fn choose_ssh_mode(
        &mut self,
        index: usize,
        focus_field: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match index {
            0 => self.ssh_choice(SshChoice::Agent, cx),
            1 => self.choose_ssh_key(focus_field, window, cx),
            _ => self.ssh_choice(SshChoice::Disabled, cx),
        }
    }

    /// Arrows choose the next SSH mode and keep the keyboard on the group.
    fn ssh_mode_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(target) = radio_key_target(&self.ssh_focus, 1, event, window) else {
            return;
        };
        self.ssh_focus[target].focus(window, cx);
        self.choose_ssh_mode(target, false, window, cx);
        cx.stop_propagation();
    }

    /// Arrows move through the 3 × 2 grid of editions and select them.
    fn provider_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending == Some(PendingOperation::Disconnect) {
            return;
        }
        let Some(target) = radio_key_target(&self.provider_focus, 3, event, window) else {
            return;
        };
        self.provider_focus[target].focus(window, cx);
        self.select_provider(ProviderEdition::ALL[target], window, cx);
        cx.stop_propagation();
    }

    fn save_ssh_key(&mut self, cx: &mut Context<Self>) {
        match ssh_key_path(&self.key_path.read(cx).value()) {
            Ok(path) => {
                self.ssh_choice(SshChoice::KeyFile { path }, cx);
                self.report(Spot::SshKey, Tone::Success, "Clé retenue pour ce profil.");
            }
            Err(error) => self.report(Spot::SshKey, Tone::Error, error),
        }
        cx.notify();
    }

    fn read_ssh(&mut self, cx: &mut Context<Self>) {
        let remote = self.ssh_remote.read(cx).value().trim().to_string();
        if remote.is_empty() {
            self.report(
                Spot::SshRemote,
                Tone::Error,
                "Indiquez l’URL SSH d’un dépôt.",
            );
            cx.notify();
            return;
        }
        if self.profile.ssh == SshChoice::Disabled {
            self.report(
                Spot::SshRemote,
                Tone::Error,
                "SSH est désactivé pour ce profil. Choisissez l’agent SSH ou une clé privée.",
            );
            cx.notify();
            return;
        }
        let choice = self.profile.ssh.clone();
        self.cancel();
        self.pending = Some(PendingOperation::Read);
        self.ssh_references.clear();
        self.report(
            Spot::SshRemote,
            Tone::Progress,
            "Lecture des références SSH…",
        );
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
                match result {
                    Ok(references) => {
                        let count = references.len();
                        view.ssh_references = references
                            .into_iter()
                            .take(MAX_ROWS)
                            .map(|reference| {
                                format!("{} · {}", reference.name, reference.object_id)
                            })
                            .collect();
                        view.report(
                            Spot::SshRemote,
                            Tone::Success,
                            match count {
                                0 => "Accès vérifié : le dépôt n’annonce aucune branche ni aucun tag.".to_string(),
                                count if count > MAX_ROWS => format!(
                                    "Accès vérifié : {count} références, dont les {MAX_ROWS} premières sont affichées."
                                ),
                                count => format!(
                                    "Accès vérifié : {}.",
                                    count_label(count, "référence", "références")
                                ),
                            },
                        );
                    }
                    Err(error) => view.report(Spot::SshRemote, Tone::Error, error.to_string()),
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

/// Local favorites are stored with `~` expanded so the home screen can open them.
fn favorite_entry(value: &str) -> String {
    let value = value.trim();
    if is_remote_favorite(value) {
        value.to_string()
    } else {
        let path = expand_home(value).to_string_lossy().into_owned();
        match path.trim_end_matches(['/', std::path::MAIN_SEPARATOR]) {
            "" => path,
            trimmed => trimmed.to_string(),
        }
    }
}

/// Two favorites lead to the same repository: local paths are compared once
/// links such as /tmp are resolved, URLs without a trailing slash.
fn same_favorite(left: &str, right: &str) -> bool {
    let key = |value: &str| {
        if is_remote_favorite(value) {
            value.trim_end_matches('/').to_string()
        } else {
            std::fs::canonicalize(value)
                .map(|path| path.to_string_lossy().into_owned())
                .unwrap_or_else(|_| value.to_string())
        }
    };
    left == right || key(left) == key(right)
}

/// Resolves a typed key path, refusing what cannot serve as a private key.
fn ssh_key_path(value: &str) -> Result<PathBuf, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err("Indiquez le chemin d’une clé privée.".into());
    }
    if value.chars().any(char::is_control) {
        return Err("Retirez les caractères de contrôle du chemin.".into());
    }
    let path = expand_home(value);
    if !path.is_absolute() {
        return Err("Indiquez un chemin absolu, par exemple ~/.ssh/id_ed25519.".into());
    }
    let metadata = std::fs::metadata(&path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            "Aucun fichier à cet emplacement.".to_string()
        } else {
            "Ce fichier ne peut pas être lu. Vérifiez ses permissions.".to_string()
        }
    })?;
    if metadata.is_dir() {
        return Err("Ce chemin désigne un dossier. Choisissez le fichier de la clé privée.".into());
    }
    if path.extension().is_some_and(|extension| extension == "pub") {
        return Err(
            "Ce fichier est une clé publique. Choisissez la clé privée, sans l’extension .pub."
                .into(),
        );
    }
    std::fs::File::open(&path)
        .map_err(|_| "Ce fichier ne peut pas être lu. Vérifiez ses permissions.".to_string())?;
    Ok(path)
}

/// The option a key leads to from the focused option of a radio group.
fn radio_key_target(
    options: &[FocusHandle],
    columns: usize,
    event: &KeyDownEvent,
    window: &Window,
) -> Option<usize> {
    let current = options.iter().position(|focus| focus.is_focused(window))?;
    radio_target(current, options.len(), columns, &event.keystroke)
}

/// The option an arrow leads to in a radio group laid out in rows of
/// `columns`: ← and → move by one, ↑ and ↓ by a row, all wrapping around.
fn radio_target(current: usize, len: usize, columns: usize, key: &Keystroke) -> Option<usize> {
    if key.modifiers.alt || key.modifiers.control || key.modifiers.platform {
        return None;
    }
    let row = if columns > 1 { columns } else { 1 };
    let step = match key.key.as_str() {
        "left" => len - 1,
        "right" => 1,
        "up" => len - row,
        "down" => row,
        "home" => return Some(0),
        "end" => return Some(len - 1),
        _ => return None,
    };
    Some((current + step) % len)
}

/// The first edition holding an account, otherwise the first of the list.
fn default_edition(profile: &Profile) -> ProviderEdition {
    ProviderEdition::ALL
        .into_iter()
        .find(|edition| {
            profile
                .accounts
                .iter()
                .any(|account| edition.matches(account))
        })
        .unwrap_or(ProviderEdition::ALL[0])
}

/// Two letters keep single-word names apart, such as Personnel and Professionnel.
fn profile_monogram(name: &str) -> String {
    if name.split_whitespace().nth(1).is_some() {
        return initials(name);
    }
    let mut letters = name.chars().filter(|c| c.is_alphanumeric());
    let Some(first) = letters.next() else {
        // A name without letters, such as an emoji, shows its first symbol.
        return name.trim().chars().take(1).collect();
    };
    first
        .to_uppercase()
        .chain(letters.next().into_iter().flat_map(char::to_lowercase))
        .collect()
}

/// A profile's monogram color and avatar fill.
fn profile_colors(profile: &Profile, p: Palette) -> (u32, u32) {
    let hue = p.graph[profile_hue(&profile.id, p.graph.len())];
    (hue, crate::theme::mix(hue, p.panel, 0.18))
}

/// A stable hue per profile id, independent of the profile's position.
fn profile_hue(id: &str, hues: usize) -> usize {
    // FNV-1a stays identical across launches and Rust releases.
    let hash = id.bytes().fold(0x811c_9dc5_u32, |hash, byte| {
        (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193)
    });
    hash as usize % hues
}

/// Explains why the profile store would refuse `name`.
pub(crate) fn profile_name_error(name: &str) -> Option<String> {
    Profile::validate_name(name).err()?;
    Some(if name.trim().is_empty() {
        "Saisissez un nom de profil.".into()
    } else if name.chars().count() > Profile::MAX_NAME_CHARS {
        format!(
            "Choisissez un nom de profil de {} caractères au plus.",
            Profile::MAX_NAME_CHARS
        )
    } else {
        "Retirez les tabulations et autres caractères de contrôle du nom du profil.".into()
    })
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
        InputEscape, IntegrationsEvent, IntegrationsSection, IntegrationsView, ProviderEdition,
        Spot, Tone, accepts_response, bounded, configured_browser_client_id, default_edition,
        disconnect_completion, ssh_key_path,
    };
    use crate::theme::{Palette, ThemeId};
    use gpui_kit::{
        AnyWindowHandle, AppContext, Context, Entity, Focusable, InteractiveElement, IntoElement,
        ParentElement, Render, Styled, TestAppContext, Window, WindowOptions, div,
        test::TestWindowExt,
    };
    use mangrove_remote::{
        Account, Authentication, Profile, ProfileSession, Provider, RemoteError,
        providers::Repository, ssh::SshChoice,
    };
    use std::{cell::RefCell, rc::Rc};

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
    fn local_favorites_expand_the_home_shorthand_and_urls_stay_verbatim() {
        let home = dirs::home_dir().unwrap();
        assert_eq!(
            super::favorite_entry(" ~/code/app "),
            home.join("code/app").to_string_lossy()
        );
        assert_eq!(super::favorite_entry("~"), home.to_string_lossy());
        for verbatim in [
            "/srv/repos/app",
            "~other/app",
            "https://example.com/~team/app.git",
            "ssh://git@example.com/~team/app.git",
            "deploy@example.com:~team/app.git",
        ] {
            assert_eq!(super::favorite_entry(verbatim), verbatim);
        }
    }

    #[gpui_kit::test]
    fn an_added_home_favorite_is_stored_as_an_absolute_path(cx: &mut TestAppContext) {
        let (window, view, events, _) = open_integrations(cx);
        let path = dirs::home_dir().unwrap().to_string_lossy().into_owned();
        let add = |cx: &mut TestAppContext, typed: &str| {
            cx.update_window(window, |_, window, cx| {
                view.update(cx, |view, cx| {
                    view.favorite
                        .update(cx, |input, cx| input.set_value(typed, window, cx));
                    view.add_favorite(window, cx);
                })
            })
            .unwrap();
            cx.run_until_parked();
            view.read_with(cx, |view, _| {
                error_at(view, Spot::Favorite).map(str::to_string)
            })
        };
        assert_eq!(add(cx, "~/"), None);
        assert_eq!(
            add(cx, &path).as_deref(),
            Some("Ce dépôt est déjà dans les favoris.")
        );
        assert!(add(cx, "code").unwrap().contains("chemin absolu"));
        assert_eq!(
            add(cx, &format!("~/mangrove-absent-{}", std::process::id())).as_deref(),
            Some("Aucun dossier à cet emplacement.")
        );
        assert!(matches!(
            &events.borrow()[..],
            [IntegrationsEvent::FavoritesChanged(favorites)] if favorites == &[path]
        ));
    }

    #[test]
    fn stale_profile_and_account_reads_cannot_replace_current_data() {
        assert!(accepts_response(2, 2, "personal", "personal"));
        assert!(!accepts_response(1, 2, "personal", "personal"));
        assert!(!accepts_response(2, 2, "personal", "professional"));
        // Switching away and back still changes the generation.
        assert!(!accepts_response(2, 4, "personal", "personal"));
    }

    type EventLog = Rc<RefCell<Vec<IntegrationsEvent>>>;

    fn open_integrations(
        cx: &mut TestAppContext,
    ) -> (
        AnyWindowHandle,
        Entity<IntegrationsView>,
        EventLog,
        Entity<EscapeProbe>,
    ) {
        cx.update(gpui_kit::init);
        let (window, probe) = cx
            .update(|cx| {
                gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                    let view = cx.new(|cx| {
                        let mut view = IntegrationsView::new(
                            Profile::new("personal", "Personnel"),
                            Palette::for_theme(ThemeId::Classic),
                            0.,
                            window,
                            cx,
                        );
                        view.set_profiles(
                            vec![
                                Profile::new("personal", "Personnel"),
                                Profile::new("work", "Travail"),
                            ],
                            window,
                            cx,
                        );
                        view.set_section(IntegrationsSection::Profiles, cx);
                        view
                    });
                    cx.new(|_| EscapeProbe { view, escapes: 0 })
                })
            })
            .unwrap();
        cx.update_window(window, |_, window, _| window.activate_window())
            .unwrap();
        let view = probe.read_with(cx, |probe, _| probe.view.clone());
        let events = Rc::new(RefCell::new(Vec::new()));
        let captured = events.clone();
        cx.update(|cx| {
            cx.subscribe(&view, move |_, event: &IntegrationsEvent, _| {
                captured.borrow_mut().push(event.clone());
            })
            .detach()
        });
        (window, view, events, probe)
    }

    /// Stands for the app root, which closes the clone screen when an input
    /// lets Escape through.
    struct EscapeProbe {
        view: Entity<IntegrationsView>,
        escapes: usize,
    }

    impl Render for EscapeProbe {
        fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .size_full()
                .on_action(cx.listener(|probe, _: &InputEscape, _, _| probe.escapes += 1))
                .child(self.view.clone())
        }
    }

    fn click(cx: &mut TestAppContext, window: AnyWindowHandle, id: &'static str) {
        cx.update_window(window, |_, window, cx| window.click(id, cx))
            .unwrap();
        cx.run_until_parked();
    }

    #[gpui_kit::test]
    fn escape_in_the_repository_search_leaves_the_field_not_the_clone_screen(
        cx: &mut TestAppContext,
    ) {
        let (window, view, _, probe) = open_integrations(cx);
        let repository = |id: &str, full_name: &str| Repository {
            id: id.into(),
            name: full_name.rsplit('/').next().unwrap().into(),
            full_name: full_name.into(),
            web_url: None,
            clone_url: None,
            ssh_clone_url: None,
        };
        view.update(cx, |view, cx| {
            view.set_section(IntegrationsSection::Repositories, cx);
            let mut repositories = Vec::new();
            view.repository_menu_data
                .borrow_mut()
                .append(
                    &mut repositories,
                    vec![repository("1", "team/api"), repository("2", "team/web")],
                )
                .unwrap();
            view.repositories = repositories;
            cx.notify();
        });
        click(cx, window, "repository-select");
        let state = |cx: &mut TestAppContext| {
            cx.update_window(window, |_, window, cx| {
                let view = view.read(cx);
                (
                    view.repository_select
                        .read(cx)
                        .focus_handle(cx)
                        .is_focused(window),
                    view.focus_handle.is_focused(window),
                    probe.read(cx).escapes,
                )
            })
            .unwrap()
        };
        assert_eq!(state(cx), (true, false, 0));

        cx.update_window(window, |_, window, cx| {
            window.input("api", cx);
            window.press("escape", cx);
        })
        .unwrap();
        cx.run_until_parked();
        assert_eq!(state(cx), (false, true, 0));
    }

    #[gpui_kit::test]
    fn a_profile_cannot_be_selected_while_its_deletion_runs(cx: &mut TestAppContext) {
        let (window, view, events, _) = open_integrations(cx);
        view.update(cx, |view, cx| {
            view.deleting_profile = Some("work".into());
            cx.notify();
        });
        click(cx, window, "profile-activate-work");
        view.update(cx, |view, cx| view.select_profile("work".into(), cx));
        cx.run_until_parked();
        assert!(events.borrow().is_empty());

        view.update(cx, |view, cx| {
            view.deleting_profile = None;
            cx.notify();
        });
        click(cx, window, "profile-activate-work");
        assert!(matches!(
            &events.borrow()[..],
            [IntegrationsEvent::ProfileSelected(id)] if id == "work"
        ));
    }

    #[gpui_kit::test]
    fn a_refused_profile_name_keeps_the_editor_open_with_its_reason(cx: &mut TestAppContext) {
        let (window, view, events, _) = open_integrations(cx);
        let rename = |cx: &mut TestAppContext, name: String| {
            cx.update_window(window, |_, window, cx| {
                view.update(cx, |view, cx| {
                    view.profile_rename
                        .update(cx, |input, cx| input.set_value(name, window, cx));
                    view.save_profile_name(window, cx);
                })
            })
            .unwrap();
            cx.run_until_parked();
        };
        cx.update_window(window, |_, window, cx| {
            view.update(cx, |view, cx| {
                view.start_profile_rename("work".into(), window, cx)
            })
        })
        .unwrap();
        rename(cx, "é".repeat(129));
        view.read_with(cx, |view, _| {
            assert_eq!(view.renaming_profile.as_deref(), Some("work"));
            assert_eq!(
                error_at(view, Spot::ProfileRename),
                Some("Choisissez un nom de profil de 128 caractères au plus.")
            );
        });
        rename(cx, " personnel ".into());
        view.read_with(cx, |view, _| {
            assert_eq!(
                error_at(view, Spot::ProfileRename),
                Some("Un autre profil porte déjà ce nom.")
            );
        });
        assert!(events.borrow().is_empty());

        rename(cx, "é".repeat(128));
        view.read_with(cx, |view, _| {
            assert!(view.renaming_profile.is_none());
            assert!(view.feedback.is_none());
        });
        assert!(matches!(
            &events.borrow()[..],
            [IntegrationsEvent::ProfileRenamed { profile_id, name }]
                if profile_id == "work" && name.chars().count() == 128
        ));
    }

    #[test]
    fn profile_name_errors_match_the_store_rules() {
        assert_eq!(super::profile_name_error("Client Acme"), None);
        assert_eq!(super::profile_name_error(&"é".repeat(128)), None);
        assert_eq!(
            super::profile_name_error("  ").as_deref(),
            Some("Saisissez un nom de profil.")
        );
        assert!(
            super::profile_name_error(&"x".repeat(129))
                .unwrap()
                .contains("128 caractères")
        );
        assert!(
            super::profile_name_error("tab\tname")
                .unwrap()
                .contains("caractères de contrôle")
        );
    }

    #[test]
    fn untrusted_pages_cannot_grow_retained_lists_without_bound() {
        assert_eq!(bounded((0..1_000).collect()).len(), super::MAX_ROWS);
        assert_eq!(bounded(vec![1, 2]), vec![1, 2]);
    }

    fn error_at(view: &IntegrationsView, spot: Spot) -> Option<&str> {
        view.feedback
            .as_ref()
            .filter(|feedback| feedback.spot == spot && feedback.tone == Tone::Error)
            .map(|feedback| feedback.text.as_str())
    }

    fn type_into(
        cx: &mut TestAppContext,
        window: AnyWindowHandle,
        view: &Entity<IntegrationsView>,
        input: impl Fn(&IntegrationsView) -> Entity<gpui_kit::component::input::InputState>,
        text: &str,
    ) {
        cx.update_window(window, |_, window, cx| {
            let input = input(view.read(cx));
            input.update(cx, |input, cx| input.focus(window, cx));
            window.press("secondary-a", cx);
            window.input(text, cx);
        })
        .unwrap();
        cx.run_until_parked();
    }

    #[gpui_kit::test]
    fn enter_creates_a_profile_and_an_error_waits_under_its_field_until_the_next_edit(
        cx: &mut TestAppContext,
    ) {
        let (window, view, events, _) = open_integrations(cx);
        type_into(
            cx,
            window,
            &view,
            |view| view.profile_name.clone(),
            "Travail",
        );
        cx.update_window(window, |_, window, cx| window.press("enter", cx))
            .unwrap();
        cx.run_until_parked();
        assert!(events.borrow().is_empty());
        view.read_with(cx, |view, _| {
            assert_eq!(
                error_at(view, Spot::NewProfile),
                Some("Un autre profil porte déjà ce nom.")
            )
        });

        cx.update_window(window, |_, window, cx| window.input(" 2", cx))
            .unwrap();
        cx.run_until_parked();
        view.read_with(cx, |view, _| assert!(view.feedback.is_none()));
        cx.update_window(window, |_, window, cx| window.press("enter", cx))
            .unwrap();
        cx.run_until_parked();
        assert!(matches!(
            &events.borrow()[..],
            [IntegrationsEvent::ProfileCreated(name)] if name == "Travail 2"
        ));
        view.read_with(cx, |view, cx| {
            assert!(view.profile_name.read(cx).value().is_empty())
        });
    }

    #[gpui_kit::test]
    fn a_duplicate_favorite_is_refused_with_a_reason(cx: &mut TestAppContext) {
        let (window, view, events, _) = open_integrations(cx);
        for _ in 0..2 {
            type_into(
                cx,
                window,
                &view,
                |view| view.favorite.clone(),
                "https://example.com/team/repo",
            );
            cx.update_window(window, |_, window, cx| window.press("enter", cx))
                .unwrap();
            cx.run_until_parked();
        }
        assert_eq!(events.borrow().len(), 1);
        view.read_with(cx, |view, _| {
            assert_eq!(
                error_at(view, Spot::Favorite),
                Some("Ce dépôt est déjà dans les favoris.")
            )
        });
    }

    #[gpui_kit::test]
    fn disconnecting_waits_for_a_confirmation(cx: &mut TestAppContext) {
        let (window, view, _, _) = open_integrations(cx);
        cx.update_window(window, |_, window, cx| {
            view.update(cx, |view, cx| {
                let mut profile = Profile::new("personal", "Personnel");
                profile.accounts.push(Account {
                    id: "github".into(),
                    label: "Octocat".into(),
                    provider: Provider::GitHub,
                    server_url: "https://github.com".into(),
                    username: None,
                    authentication: Authentication::PersonalToken,
                });
                view.set_profile(profile, window, cx);
                view.set_section(IntegrationsSection::Integrations, cx);
            })
        })
        .unwrap();
        cx.run_until_parked();
        click(cx, window, "disconnect-github");
        assert!(cx.has_pending_prompt());
        cx.simulate_prompt_answer("Annuler");
        cx.run_until_parked();
        view.read_with(cx, |view, _| {
            assert!(view.pending.is_none());
            assert_eq!(view.profile.accounts.len(), 1);
        });
    }

    #[test]
    fn missing_unreadable_and_public_keys_are_refused() {
        let directory =
            std::env::temp_dir().join(format!("mangrove-ssh-keys-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let private = directory.join("id_test");
        let public = directory.join("id_test.pub");
        std::fs::write(&private, "key").unwrap();
        std::fs::write(&public, "key").unwrap();
        let path = |path: &std::path::Path| path.to_string_lossy().into_owned();

        assert_eq!(ssh_key_path(&path(&private)), Ok(private.clone()));
        assert_eq!(
            ssh_key_path(&path(&directory.join("absent"))),
            Err("Aucun fichier à cet emplacement.".into())
        );
        assert!(
            ssh_key_path(&path(&directory))
                .unwrap_err()
                .contains("dossier")
        );
        assert!(ssh_key_path(&path(&public)).unwrap_err().contains(".pub"));
        assert!(
            ssh_key_path("id_test")
                .unwrap_err()
                .contains("chemin absolu")
        );
        assert!(ssh_key_path("  ").is_err());
        std::fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn the_first_connected_edition_is_preselected_otherwise_the_first_listed() {
        let mut profile = Profile::new("personal", "Personnel");
        assert!(default_edition(&profile) == ProviderEdition::GitHub);
        profile.accounts.push(Account {
            id: "gitlab".into(),
            label: "GitLab".into(),
            provider: Provider::GitLab,
            server_url: "https://gitlab.com".into(),
            username: None,
            authentication: Authentication::PersonalToken,
        });
        assert!(default_edition(&profile) == ProviderEdition::GitLab);
    }

    #[gpui_kit::test]
    fn connecting_needs_the_required_fields_of_the_chosen_method(cx: &mut TestAppContext) {
        let (window, view, _, _) = open_integrations(cx);
        let ready = |cx: &mut TestAppContext| view.read_with(cx, |view, cx| view.can_connect(cx));
        cx.update_window(window, |_, window, cx| {
            view.update(cx, |view, cx| {
                view.set_section(IntegrationsSection::Integrations, cx);
                view.select_provider(ProviderEdition::GitLab, window, cx);
                view.client_id
                    .update(cx, |input, cx| input.set_value("", window, cx));
            })
        })
        .unwrap();
        assert!(!ready(cx));
        type_into(
            cx,
            window,
            &view,
            |view| view.client_id.clone(),
            "application-id",
        );
        assert!(ready(cx));

        cx.update_window(window, |_, window, cx| {
            view.update(cx, |view, cx| {
                view.set_browser_connection(false, window, cx)
            })
        })
        .unwrap();
        assert!(!ready(cx));
        type_into(cx, window, &view, |view| view.token.clone(), "glpat-token");
        assert!(ready(cx));
    }

    #[gpui_kit::test]
    fn a_refused_key_keeps_the_current_ssh_choice(cx: &mut TestAppContext) {
        let (window, view, events, _) = open_integrations(cx);
        let directory =
            std::env::temp_dir().join(format!("mangrove-ssh-choice-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let key = directory.join("id_choice");
        std::fs::write(&key, "key").unwrap();
        cx.update_window(window, |_, window, cx| {
            view.update(cx, |view, cx| {
                view.set_section(IntegrationsSection::Ssh, cx);
                view.choose_ssh_key(true, window, cx);
            })
        })
        .unwrap();
        cx.run_until_parked();
        type_into(
            cx,
            window,
            &view,
            |view| view.key_path.clone(),
            &directory.join("absent").to_string_lossy(),
        );
        cx.update_window(window, |_, window, cx| window.press("enter", cx))
            .unwrap();
        cx.run_until_parked();
        assert!(events.borrow().is_empty());
        view.read_with(cx, |view, _| {
            assert_eq!(view.profile.ssh, SshChoice::Agent);
            assert_eq!(
                error_at(view, Spot::SshKey),
                Some("Aucun fichier à cet emplacement.")
            );
        });

        type_into(
            cx,
            window,
            &view,
            |view| view.key_path.clone(),
            &key.to_string_lossy(),
        );
        view.read_with(cx, |view, _| assert!(view.feedback.is_none()));
        cx.update_window(window, |_, window, cx| window.press("enter", cx))
            .unwrap();
        cx.run_until_parked();
        assert!(matches!(
            &events.borrow()[..],
            [IntegrationsEvent::SshChanged(SshChoice::KeyFile { path })] if path == &key
        ));
        std::fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn arrows_move_through_radio_groups_and_wrap() {
        let key = |name: &str| gpui_kit::Keystroke::parse(name).unwrap();
        // A row of three options.
        assert_eq!(super::radio_target(0, 3, 1, &key("right")), Some(1));
        assert_eq!(super::radio_target(0, 3, 1, &key("left")), Some(2));
        assert_eq!(super::radio_target(2, 3, 1, &key("down")), Some(0));
        // A 3 × 2 grid moves by rows with ↑ and ↓.
        assert_eq!(super::radio_target(1, 6, 3, &key("down")), Some(4));
        assert_eq!(super::radio_target(4, 6, 3, &key("up")), Some(1));
        assert_eq!(super::radio_target(4, 6, 3, &key("end")), Some(5));
        assert_eq!(super::radio_target(4, 6, 3, &key("cmd-right")), None);
        assert_eq!(super::radio_target(4, 6, 3, &key("a")), None);
    }

    #[test]
    fn a_favorite_matches_its_variants() {
        let home = dirs::home_dir().unwrap().to_string_lossy().into_owned();
        assert_eq!(super::favorite_entry(&format!("{home}/")), home);
        assert!(super::same_favorite(&home, &super::favorite_entry("~/")));
        assert!(super::same_favorite(
            "https://example.com/team/repo",
            "https://example.com/team/repo/"
        ));
        assert!(!super::same_favorite(
            "https://example.com/team/repo",
            "https://example.com/team/other"
        ));
    }

    #[gpui_kit::test]
    fn a_server_url_is_checked_before_connecting(cx: &mut TestAppContext) {
        let (window, view, _, _) = open_integrations(cx);
        cx.update_window(window, |_, window, cx| {
            view.update(cx, |view, cx| {
                view.set_section(IntegrationsSection::Integrations, cx);
                view.select_provider(ProviderEdition::GitLabSelfManaged, window, cx);
                view.set_browser_connection(false, window, cx);
            })
        })
        .unwrap();
        type_into(
            cx,
            window,
            &view,
            |view| view.server.clone(),
            "gitlab.exemple.com",
        );
        type_into(cx, window, &view, |view| view.token.clone(), "glpat-token");
        cx.update_window(window, |_, window, cx| window.press("enter", cx))
            .unwrap();
        cx.run_until_parked();
        view.read_with(cx, |view, cx| {
            assert!(error_at(view, Spot::Server).unwrap().contains("HTTPS"));
            assert!(view.pending.is_none());
            // The refused attempt keeps the token for a corrected address.
            assert_eq!(view.token.read(cx).value().as_ref(), "glpat-token");
        });
        type_into(
            cx,
            window,
            &view,
            |view| view.server.clone(),
            "https://gitlab.exemple.com",
        );
        view.read_with(cx, |view, cx| {
            assert!(view.feedback.is_none());
            assert!(view.server_refusal(cx).is_none());
        });
    }
}
