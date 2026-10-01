//! GitLab public-client OAuth with PKCE. Run blocking entry points on a background
//! executor. Authorization URLs, codes and credential bundles have no Debug or
//! Serialize implementation; only a public client ID belongs in profile metadata.
use std::{
    collections::HashMap,
    net::{Ipv4Addr, TcpListener},
    sync::{Arc, LazyLock, Mutex, MutexGuard, TryLockError, Weak},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
    Engine,
};
use secrecy::zeroize::{Zeroize, Zeroizing};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};
use url::Url;

use crate::{
    Account, Authentication, CancellationToken, CredentialVault, ExposeSecret, HttpTransport,
    Provider, ProviderClient, RemoteError, Result, SafeApiUrl, SecretToken, Transport,
};

pub const REDIRECT_URI: &str = "http://127.0.0.1:47321/oauth/callback";
const CALLBACK_PORT: u16 = 47321;
const CALLBACK_PATH: &str = "/oauth/callback";
pub(crate) const AUTHORIZATION_TIMEOUT: Duration = Duration::from_secs(300);
const MAX_CALLBACK_BYTES: usize = 8192;
pub(crate) const MAX_TOKEN_BYTES: usize = 16 * 1024;
const VAULT_PREFIX: &str = "mangrove-oauth-v1:";
type RefreshLocks = HashMap<(String, String), Weak<Mutex<()>>>;
static REFRESH_LOCKS: LazyLock<Mutex<RefreshLocks>> = LazyLock::new(|| Mutex::new(HashMap::new()));

pub(crate) fn validate_client_id(client_id: &str) -> Result<()> {
    if client_id.is_empty()
        || client_id.len() > 256
        || !client_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    {
        return Err(RemoteError::InvalidConfiguration);
    }
    Ok(())
}

/// Link to public application registration, preserving a self-hosted subpath.
pub fn application_settings_url(server_url: &str) -> Result<String> {
    Ok(SafeApiUrl::new(server_url, "")?
        .url("-/user_settings/applications")?
        .into())
}

pub struct GitLabOAuth {
    listener: TcpListener,
    authorization_url: Zeroizing<String>,
    token_endpoint: Url,
    client_id: String,
    state: SecretToken,
    verifier: SecretToken,
    deadline: Instant,
}

impl GitLabOAuth {
    /// Reserve the callback port before opening the browser. An occupied port is
    /// an error: never authorize through a listener belonging to another process.
    pub fn begin(server_url: &str, client_id: &str) -> Result<Self> {
        validate_client_id(client_id)?;
        let base = SafeApiUrl::new(server_url, "oauth")?;
        let state = random_secret()?;
        let verifier = random_secret()?;
        let mut authorize = base.url("authorize")?;
        authorize
            .query_pairs_mut()
            .append_pair("client_id", client_id)
            .append_pair("redirect_uri", REDIRECT_URI)
            .append_pair("response_type", "code")
            .append_pair("scope", "read_api")
            .append_pair("state", state.expose_secret())
            .append_pair("code_challenge", &challenge(verifier.expose_secret()))
            .append_pair("code_challenge_method", "S256");
        let listener = callback_listener()?;
        Ok(Self {
            listener,
            authorization_url: Zeroizing::new(authorize.into()),
            token_endpoint: base.url("token")?,
            client_id: client_id.into(),
            state,
            verifier,
            deadline: Instant::now() + AUTHORIZATION_TIMEOUT,
        })
    }

    pub fn authorization_url(&self) -> &str {
        &self.authorization_url
    }

    pub fn finish(self, cancel: &CancellationToken) -> Result<OAuthCredentials> {
        self.finish_with(&HttpTokenTransport, cancel)
    }

    fn finish_with(
        self,
        transport: &dyn TokenTransport,
        cancel: &CancellationToken,
    ) -> Result<OAuthCredentials> {
        let code = authorization_code(self.listener, &self.state, self.deadline, cancel)?;
        cancel.check()?;
        let request = TokenRequest {
            endpoint: &self.token_endpoint,
            basic_auth: None,
            fields: vec![
                ("client_id", SecretToken::from(self.client_id.as_str())),
                ("grant_type", SecretToken::from("authorization_code")),
                ("redirect_uri", SecretToken::from(REDIRECT_URI)),
                ("code", code),
                ("code_verifier", self.verifier),
            ],
        };
        let response = transport.post(&request, cancel)?;
        parse_credentials(&response, self.token_endpoint, self.client_id)
    }
}

pub(crate) fn random_secret() -> Result<SecretToken> {
    let mut bytes = Zeroizing::new([0u8; 32]);
    getrandom::fill(bytes.as_mut()).map_err(|_| RemoteError::Network)?;
    Ok(SecretToken::from(URL_SAFE_NO_PAD.encode(bytes.as_ref())))
}

pub(crate) fn callback_listener() -> Result<TcpListener> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, CALLBACK_PORT)).map_err(|error| {
        match error.kind() {
            std::io::ErrorKind::AddrInUse => RemoteError::OAuthPortInUse,
            _ => RemoteError::Network,
        }
    })?;
    listener
        .set_nonblocking(true)
        .map_err(|_| RemoteError::Network)?;
    Ok(listener)
}

pub(crate) fn authorization_code(
    listener: TcpListener,
    state: &SecretToken,
    deadline: Instant,
    cancel: &CancellationToken,
) -> Result<SecretToken> {
    cancel.check()?;
    let remaining = deadline.saturating_duration_since(Instant::now());
    runtime()?.block_on(async {
        let listener = tokio::net::TcpListener::from_std(listener).map_err(|_| RemoteError::Network)?;
        tokio::select! {
            biased;
            _ = cancel.cancelled() => Err(RemoteError::Cancelled),
            result = tokio::time::timeout(remaining, receive_callback(listener, state.expose_secret())) => {
                result.map_err(|_| RemoteError::Timeout)?
            }
        }
    })
}

fn challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

async fn receive_callback(listener: tokio::net::TcpListener, state: &str) -> Result<SecretToken> {
    loop {
        let (mut stream, peer) = listener.accept().await.map_err(|_| RemoteError::Network)?;
        if !peer.ip().is_loopback() {
            continue;
        }
        let result =
            tokio::time::timeout(Duration::from_secs(2), read_callback(&mut stream, state)).await;
        let outcome = result.ok().flatten();
        let (status, message) = match &outcome {
            Some(Ok(_)) => (
                "200 OK",
                "Autorisation reçue. Vous pouvez revenir dans Mangrove.",
            ),
            Some(Err(_)) => (
                "200 OK",
                "Autorisation refusée. Vous pouvez revenir dans Mangrove.",
            ),
            None => ("400 Bad Request", "Requête de connexion non valide."),
        };
        // Fixed response: never echo callback data or load third-party assets.
        let response = format!("HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nContent-Security-Policy: default-src 'none'\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{message}", message.len());
        let _ = tokio::time::timeout(
            Duration::from_secs(1),
            stream.write_all(response.as_bytes()),
        )
        .await;
        if let Some(outcome) = outcome {
            return outcome;
        }
    }
}

async fn read_callback(stream: &mut TcpStream, state: &str) -> Option<Result<SecretToken>> {
    let mut bytes = Zeroizing::new(Vec::new());
    let mut chunk = Zeroizing::new([0u8; 1024]);
    loop {
        let count = stream.read(chunk.as_mut()).await.ok()?;
        if count == 0 || bytes.len() + count > MAX_CALLBACK_BYTES {
            return None;
        }
        bytes.extend_from_slice(&chunk[..count]);
        if bytes.windows(4).any(|part| part == b"\r\n\r\n") {
            break;
        }
    }
    parse_callback(std::str::from_utf8(&bytes).ok()?, state)
}

fn parse_callback(request: &str, state: &str) -> Option<Result<SecretToken>> {
    let (head, body) = request.split_once("\r\n\r\n")?;
    if !body.is_empty() {
        return None;
    }
    let mut lines = head.split("\r\n");
    let mut request_line = lines.next()?.split(' ');
    if request_line.next()? != "GET" {
        return None;
    }
    let target = request_line.next()?;
    if !matches!(request_line.next()?, "HTTP/1.1" | "HTTP/1.0")
        || request_line.next().is_some()
        || !target.starts_with('/')
        || target.starts_with("//")
        || target.contains(['#', '\\'])
    {
        return None;
    }
    let mut host_seen = false;
    for line in lines {
        let (name, value) = line.split_once(':')?;
        if name.eq_ignore_ascii_case("host") {
            if host_seen || value.trim() != "127.0.0.1:47321" {
                return None;
            }
            host_seen = true;
        } else if name.eq_ignore_ascii_case("transfer-encoding")
            || (name.eq_ignore_ascii_case("content-length") && value.trim() != "0")
        {
            return None;
        }
    }
    if !host_seen {
        return None;
    }
    let (path, query) = target.split_once('?')?;
    if path != CALLBACK_PATH {
        return None;
    }
    let mut callback_state = None;
    let mut code = None;
    let mut error = None;
    let mut fields = std::collections::HashSet::new();
    for (key, value) in url::form_urlencoded::parse(query.as_bytes()) {
        if !fields.insert(key.to_string()) {
            return None;
        }
        match key.as_ref() {
            "state" => callback_state = Some(Zeroizing::new(value.into_owned())),
            "code" => code = Some(SecretToken::from(value.into_owned())),
            "error" => error = Some(Zeroizing::new(value.into_owned())),
            // GitLab can include error_description/error_uri; neither is shown.
            "error_description" | "error_uri" => {}
            _ => return None,
        }
    }
    if callback_state.as_deref().map(|value| value.as_str()) != Some(state) {
        return None;
    }
    match (code, error) {
        (Some(code), None) if valid_secret(code.expose_secret()) => Some(Ok(code)),
        (None, Some(_)) => Some(Err(RemoteError::CredentialsExpired)),
        _ => None,
    }
}

/// Access and refresh tokens are bundled into one OS-vault entry. Profile JSON
/// contains only Authentication::GitLabOAuth and its public application ID.
pub struct OAuthCredentials {
    access: SecretToken,
    refresh: SecretToken,
    expires_at: u64,
    token_endpoint: Url,
    client_id: String,
}

impl OAuthCredentials {
    fn access_token(&self) -> SecretToken {
        self.access.clone()
    }

    /// Build a GET-only provider client that also shields the refresh token from
    /// any untrusted API text or pagination echo, without exposing it to the UI.
    pub fn provider_client<T: Transport>(
        &self,
        transport: T,
        account: Account,
    ) -> Result<ProviderClient<T>> {
        account.validate()?;
        if account.authentication
            != (Authentication::GitLabOAuth {
                client_id: self.client_id.clone(),
            })
            || SafeApiUrl::new(&account.server_url, "oauth")?.url("token")? != self.token_endpoint
        {
            return Err(RemoteError::InvalidConfiguration);
        }
        Ok(
            ProviderClient::new(transport, account, self.access_token())?
                .protect_secret(&self.refresh),
        )
    }

    pub fn store(
        &self,
        vault: &dyn CredentialVault,
        profile_id: &str,
        account_id: &str,
    ) -> Result<()> {
        vault.store(profile_id, account_id, &self.encode()?)
    }

    fn encode(&self) -> Result<SecretToken> {
        #[derive(Serialize)]
        struct Stored<'a> {
            access_token: &'a str,
            refresh_token: &'a str,
            expires_at: u64,
            token_endpoint: &'a str,
            client_id: &'a str,
        }
        let packet = Stored {
            access_token: self.access.expose_secret(),
            refresh_token: self.refresh.expose_secret(),
            expires_at: self.expires_at,
            token_endpoint: self.token_endpoint.as_str(),
            client_id: &self.client_id,
        };
        let json = Zeroizing::new(
            serde_json::to_string(&packet).map_err(|_| RemoteError::InvalidResponse)?,
        );
        if VAULT_PREFIX.len() + json.len() > MAX_TOKEN_BYTES {
            return Err(RemoteError::ResponseTooLarge);
        }
        Ok(SecretToken::from(format!(
            "{VAULT_PREFIX}{}",
            json.as_str()
        )))
    }
}

#[derive(Deserialize)]
struct StoredCredentials {
    access_token: String,
    refresh_token: String,
    expires_at: u64,
    token_endpoint: String,
    client_id: String,
}
impl Drop for StoredCredentials {
    fn drop(&mut self) {
        self.access_token.zeroize();
        self.refresh_token.zeroize();
    }
}

fn decode_credentials(
    packet: &SecretToken,
    endpoint: &Url,
    client_id: &str,
) -> Result<OAuthCredentials> {
    if packet.expose_secret().len() > MAX_TOKEN_BYTES {
        return Err(RemoteError::ResponseTooLarge);
    }
    let json = packet
        .expose_secret()
        .strip_prefix(VAULT_PREFIX)
        .ok_or(RemoteError::CredentialsExpired)?;
    let stored: StoredCredentials =
        serde_json::from_str(json).map_err(|_| RemoteError::CredentialsExpired)?;
    if stored.token_endpoint != endpoint.as_str()
        || stored.client_id != client_id
        || !valid_secret(&stored.access_token)
        || !valid_secret(&stored.refresh_token)
    {
        return Err(RemoteError::InvalidConfiguration);
    }
    Ok(OAuthCredentials {
        access: SecretToken::from(stored.access_token.as_str()),
        refresh: SecretToken::from(stored.refresh_token.as_str()),
        expires_at: stored.expires_at,
        token_endpoint: endpoint.clone(),
        client_id: client_id.into(),
    })
}

/// Read/refresh credentials and build a protected native HTTP client in one
/// background operation. Neither the refresh token nor its bundle leaves here.
/// Concurrent refreshes are serialized; logout wins the atomic vault comparison.
pub fn account_client(
    vault: &dyn CredentialVault,
    profile_id: &str,
    account: &Account,
    cancel: &CancellationToken,
) -> Result<ProviderClient<HttpTransport>> {
    let credentials =
        read_credentials_with(vault, profile_id, account, cancel, &HttpTokenTransport)?;
    credentials.provider_client(HttpTransport::new()?, account.clone())
}

enum AccountCredentials {
    PersonalToken(SecretToken),
    OAuth(OAuthCredentials),
    GitHub(crate::github_oauth::GitHubCredentials),
    Bitbucket(crate::bitbucket_oauth::BitbucketCredentials),
}
impl AccountCredentials {
    #[cfg(test)]
    fn access_token(&self) -> SecretToken {
        match self {
            Self::PersonalToken(token) => token.clone(),
            Self::OAuth(credentials) => credentials.access_token(),
            Self::GitHub(credentials) => credentials.access_token(),
            Self::Bitbucket(credentials) => credentials.access_token(),
        }
    }
    fn provider_client<T: Transport>(
        &self,
        transport: T,
        account: Account,
    ) -> Result<ProviderClient<T>> {
        match self {
            Self::PersonalToken(token) => ProviderClient::new(transport, account, token.clone()),
            Self::OAuth(credentials) => credentials.provider_client(transport, account),
            Self::GitHub(credentials) => credentials.provider_client(transport, account),
            Self::Bitbucket(credentials) => credentials.provider_client(transport, account),
        }
    }
}

#[cfg(test)]
fn read_account_token_with(
    vault: &dyn CredentialVault,
    profile_id: &str,
    account: &Account,
    cancel: &CancellationToken,
    transport: &dyn TokenTransport,
) -> Result<SecretToken> {
    Ok(read_credentials_with(vault, profile_id, account, cancel, transport)?.access_token())
}

fn read_credentials_with(
    vault: &dyn CredentialVault,
    profile_id: &str,
    account: &Account,
    cancel: &CancellationToken,
    transport: &dyn TokenTransport,
) -> Result<AccountCredentials> {
    account.validate()?;
    crate::model::validate_id(profile_id)?;
    cancel.check()?;
    if account.authentication == Authentication::PersonalToken {
        let token = vault
            .read(profile_id, &account.id)?
            .ok_or(RemoteError::CredentialsExpired)?;
        if token.expose_secret().starts_with(VAULT_PREFIX)
            || crate::github_oauth::is_credential_packet(&token)
            || crate::bitbucket_oauth::is_credential_packet(&token)
        {
            return Err(RemoteError::InvalidConfiguration);
        }
        cancel.check()?;
        return Ok(AccountCredentials::PersonalToken(token));
    }
    if matches!(account.authentication, Authentication::GitHubOAuth { .. }) {
        return crate::github_oauth::read_credentials_with(
            vault, profile_id, account, cancel, transport,
        )
        .map(AccountCredentials::GitHub);
    }
    if matches!(
        account.authentication,
        Authentication::BitbucketOAuth { .. }
    ) {
        return crate::bitbucket_oauth::read_credentials_with(
            vault, profile_id, account, cancel, transport,
        )
        .map(AccountCredentials::Bitbucket);
    }
    if account.provider != Provider::GitLab {
        return Err(RemoteError::InvalidConfiguration);
    }
    let Authentication::GitLabOAuth { client_id } = &account.authentication else {
        unreachable!()
    };
    let endpoint = SafeApiUrl::new(&account.server_url, "oauth")?.url("token")?;
    let account_lock = refresh_lock(profile_id, &account.id)?;
    let _guard = lock_refresh(&account_lock, cancel)?;
    let previous = vault
        .read(profile_id, &account.id)?
        .ok_or(RemoteError::CredentialsExpired)?;
    let credentials = decode_credentials(&previous, &endpoint, client_id)?;
    if credentials.expires_at > now()?.saturating_add(30) {
        cancel.check()?;
        return Ok(AccountCredentials::OAuth(credentials));
    }
    let request = TokenRequest {
        endpoint: &endpoint,
        basic_auth: None,
        fields: vec![
            ("client_id", SecretToken::from(client_id.as_str())),
            ("grant_type", SecretToken::from("refresh_token")),
            ("redirect_uri", SecretToken::from(REDIRECT_URI)),
            ("refresh_token", credentials.refresh.clone()),
        ],
    };
    let response = transport.post(&request, cancel)?;
    let rotated = parse_credentials(&response, endpoint.clone(), client_id.clone())?;
    // Once a rotated response exists, preserve it even if the view was cancelled.
    // CAS prevents this write from resurrecting a deleted or replaced account.
    if !vault.replace_if_matches(profile_id, &account.id, &previous, &rotated.encode()?)? {
        return Err(RemoteError::CredentialsExpired);
    }
    cancel.check()?;
    Ok(AccountCredentials::OAuth(rotated))
}

pub(crate) fn lock_refresh<'a>(
    lock: &'a Mutex<()>,
    cancel: &CancellationToken,
) -> Result<MutexGuard<'a, ()>> {
    loop {
        cancel.check()?;
        match lock.try_lock() {
            Ok(guard) => return Ok(guard),
            Err(TryLockError::Poisoned(_)) => return Err(RemoteError::VaultUnavailable),
            Err(TryLockError::WouldBlock) => std::thread::sleep(Duration::from_millis(10)),
        }
    }
}

pub(crate) fn refresh_lock(profile_id: &str, account_id: &str) -> Result<Arc<Mutex<()>>> {
    let mut locks = REFRESH_LOCKS
        .lock()
        .map_err(|_| RemoteError::VaultUnavailable)?;
    locks.retain(|_, lock| lock.strong_count() > 0);
    let key = (profile_id.into(), account_id.into());
    if let Some(lock) = locks.get(&key).and_then(Weak::upgrade) {
        return Ok(lock);
    }
    let lock = Arc::new(Mutex::new(()));
    locks.insert(key, Arc::downgrade(&lock));
    Ok(lock)
}

pub(crate) struct TokenRequest<'a> {
    pub(crate) endpoint: &'a Url,
    pub(crate) basic_auth: Option<(&'a str, &'a SecretToken)>,
    pub(crate) fields: Vec<(&'static str, SecretToken)>,
}
pub(crate) trait TokenTransport: Send + Sync {
    fn post(
        &self,
        request: &TokenRequest<'_>,
        cancel: &CancellationToken,
    ) -> Result<Zeroizing<Vec<u8>>>;
    fn post_until(
        &self,
        request: &TokenRequest<'_>,
        cancel: &CancellationToken,
        _deadline: Instant,
    ) -> Result<Zeroizing<Vec<u8>>> {
        self.post(request, cancel)
    }
}
pub(crate) struct HttpTokenTransport;
impl TokenTransport for HttpTokenTransport {
    fn post(
        &self,
        request: &TokenRequest<'_>,
        cancel: &CancellationToken,
    ) -> Result<Zeroizing<Vec<u8>>> {
        cancel.check()?;
        runtime()?.block_on(async {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => Err(RemoteError::Cancelled),
                result = post_token(request, false) => result,
            }
        })
    }
    fn post_until(
        &self,
        request: &TokenRequest<'_>,
        cancel: &CancellationToken,
        deadline: Instant,
    ) -> Result<Zeroizing<Vec<u8>>> {
        cancel.check()?;
        runtime()?.block_on(async {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => Err(RemoteError::Cancelled),
                result = tokio::time::timeout(
                    deadline.saturating_duration_since(Instant::now()),
                    post_token(request, true),
                ) => result.map_err(|_| RemoteError::Timeout)?,
            }
        })
    }
}

async fn post_token(request: &TokenRequest<'_>, oauth_errors: bool) -> Result<Zeroizing<Vec<u8>>> {
    // SafeApiUrl is constructed above, and the OAuth-only POST boundary never
    // accepts redirects or resource writes. Confidential-consumer authentication
    // is allowed only at Bitbucket Cloud's exact token endpoint.
    if request.basic_auth.is_some()
        && request.endpoint.as_str() != crate::bitbucket_oauth::TOKEN_ENDPOINT
    {
        return Err(RemoteError::InvalidConfiguration);
    }
    let client = reqwest::Client::builder()
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .referer(false)
        .timeout(Duration::from_secs(20))
        .connect_timeout(Duration::from_secs(8))
        .user_agent("Mangrove/0.1 OAuth")
        .build()
        .map_err(|_| RemoteError::Network)?;
    let form = Zeroizing::new(
        url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(
                request
                    .fields
                    .iter()
                    .map(|(key, value)| (*key, value.expose_secret())),
            )
            .finish(),
    );
    let mut builder = client
        .post(request.endpoint.clone())
        .header("Content-Type", "application/x-www-form-urlencoded")
        .header("Accept", "application/json")
        .body(form.as_bytes().to_vec());
    if let Some((client_id, secret)) = request.basic_auth {
        let credentials = Zeroizing::new(format!("{client_id}:{}", secret.expose_secret()));
        let encoded = Zeroizing::new(STANDARD.encode(credentials.as_bytes()));
        let value = Zeroizing::new(format!("Basic {}", encoded.as_str()));
        let mut header = reqwest::header::HeaderValue::from_str(&value)
            .map_err(|_| RemoteError::InvalidConfiguration)?;
        header.set_sensitive(true);
        builder = builder.header(reqwest::header::AUTHORIZATION, header);
    }
    let mut response = builder.send().await.map_err(token_network_error)?;
    let status = response.status().as_u16();
    match status {
        200 => {}
        400 | 401 | 403 if oauth_errors => {}
        400 | 401 | 403 => return Err(RemoteError::CredentialsExpired),
        429 => {
            return Err(RemoteError::RateLimited {
                retry_after_seconds: None,
            })
        }
        status => return Err(RemoteError::Http { status }),
    }
    if response
        .content_length()
        .is_some_and(|size| size > MAX_TOKEN_BYTES as u64)
    {
        return Err(RemoteError::ResponseTooLarge);
    }
    let mut bytes = Zeroizing::new(Vec::new());
    while let Some(chunk) = response.chunk().await.map_err(token_network_error)? {
        if bytes.len() + chunk.len() > MAX_TOKEN_BYTES {
            return Err(RemoteError::ResponseTooLarge);
        }
        bytes.extend_from_slice(&chunk);
    }
    if status != 200 {
        #[derive(Deserialize)]
        struct OAuthError<'a> {
            #[serde(borrow)]
            error: &'a str,
        }
        if serde_json::from_slice::<OAuthError<'_>>(&bytes)
            .map_or(true, |response| response.error.is_empty())
        {
            return Err(RemoteError::CredentialsExpired);
        }
    }
    Ok(bytes)
}

fn token_network_error(error: reqwest::Error) -> RemoteError {
    if error.is_timeout() {
        RemoteError::Timeout
    } else {
        RemoteError::Network
    }
}

pub(crate) fn runtime() -> Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|_| RemoteError::Network)
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: String,
    token_type: String,
    expires_in: u64,
    created_at: u64,
    scope: Option<String>,
}
impl Drop for TokenResponse {
    fn drop(&mut self) {
        self.access_token.zeroize();
        self.refresh_token.zeroize();
    }
}
fn parse_credentials(
    bytes: &[u8],
    token_endpoint: Url,
    client_id: String,
) -> Result<OAuthCredentials> {
    if bytes.len() > MAX_TOKEN_BYTES {
        return Err(RemoteError::ResponseTooLarge);
    }
    let response: TokenResponse =
        serde_json::from_slice(bytes).map_err(|_| RemoteError::InvalidResponse)?;
    // RFC 6749 permits omitting scope when it equals the requested read_api.
    // If present, reject any elevation or unsupported subset instead of granting
    // write permissions through an incorrectly registered application.
    if !response.token_type.eq_ignore_ascii_case("bearer")
        || response
            .scope
            .as_ref()
            .is_some_and(|scope| scope.split_whitespace().collect::<Vec<_>>() != ["read_api"])
        || !valid_secret(&response.access_token)
        || !valid_secret(&response.refresh_token)
        || response.expires_in == 0
        || response.expires_in > 365 * 24 * 3600
        || response.created_at > now()?.saturating_add(300)
    {
        return Err(RemoteError::InvalidResponse);
    }
    let expires_at = response
        .created_at
        .checked_add(response.expires_in)
        .ok_or(RemoteError::InvalidResponse)?;
    if expires_at <= now()? {
        return Err(RemoteError::CredentialsExpired);
    }
    Ok(OAuthCredentials {
        access: SecretToken::from(response.access_token.as_str()),
        refresh: SecretToken::from(response.refresh_token.as_str()),
        expires_at,
        token_endpoint,
        client_id,
    })
}
pub(crate) fn valid_secret(value: &str) -> bool {
    !value.is_empty() && value.len() <= 4096 && !value.chars().any(char::is_control)
}
pub(crate) fn now() -> Result<u64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| RemoteError::InvalidResponse)?
        .as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{Read, Write},
        sync::atomic::{AtomicUsize, Ordering},
    };

    #[derive(Default)]
    struct MemoryVault {
        value: Mutex<Option<SecretToken>>,
        fail_replace: bool,
    }
    impl CredentialVault for MemoryVault {
        fn store(&self, _: &str, _: &str, token: &SecretToken) -> Result<()> {
            *self.value.lock().unwrap() = Some(token.clone());
            Ok(())
        }
        fn read(&self, _: &str, _: &str) -> Result<Option<SecretToken>> {
            Ok(self.value.lock().unwrap().clone())
        }
        fn delete(&self, _: &str, _: &str) -> Result<()> {
            *self.value.lock().unwrap() = None;
            Ok(())
        }
        fn replace_if_matches(
            &self,
            _: &str,
            _: &str,
            previous: &SecretToken,
            next: &SecretToken,
        ) -> Result<bool> {
            let mut value = self.value.lock().unwrap();
            if self.fail_replace {
                return Err(RemoteError::VaultUnavailable);
            }
            if value
                .as_ref()
                .is_some_and(|value| value.expose_secret() == previous.expose_secret())
            {
                *value = Some(next.clone());
                Ok(true)
            } else {
                Ok(false)
            }
        }
    }
    struct MockTokens<F>(F);
    impl<F> TokenTransport for MockTokens<F>
    where
        F: Fn(&TokenRequest<'_>, &CancellationToken) -> Result<Zeroizing<Vec<u8>>> + Send + Sync,
    {
        fn post(
            &self,
            request: &TokenRequest<'_>,
            cancel: &CancellationToken,
        ) -> Result<Zeroizing<Vec<u8>>> {
            (self.0)(request, cancel)
        }
    }
    fn token_response(access: &str, refresh: &str) -> Zeroizing<Vec<u8>> {
        Zeroizing::new(serde_json::to_vec(&serde_json::json!({
            "access_token": access, "refresh_token": refresh, "token_type": "Bearer", "expires_in":7200, "created_at": now().unwrap()
        })).unwrap())
    }
    fn account() -> Account {
        Account {
            id: "oauth-account".into(),
            label: "GitLab".into(),
            provider: Provider::GitLab,
            server_url: "https://gitlab.com".into(),
            username: None,
            authentication: Authentication::GitLabOAuth {
                client_id: "public-app-id".into(),
            },
        }
    }
    fn credentials(expires_at: u64) -> OAuthCredentials {
        OAuthCredentials {
            access: SecretToken::from("original-access"),
            refresh: SecretToken::from("original-refresh"),
            expires_at,
            token_endpoint: Url::parse("https://gitlab.com/oauth/token").unwrap(),
            client_id: "public-app-id".into(),
        }
    }
    fn callback(target: &str, host: &str) -> String {
        format!("GET {target} HTTP/1.1\r\nHost: {host}\r\n\r\n")
    }
    fn field<'a>(request: &'a TokenRequest<'_>, name: &str) -> &'a str {
        request
            .fields
            .iter()
            .find(|(key, _)| *key == name)
            .unwrap()
            .1
            .expose_secret()
    }

    #[test]
    fn pkce_matches_rfc7636_vector_and_authorization_uses_read_only_public_client() {
        // Published RFC 7636 Appendix B vector, independent of our generator.
        assert_eq!(
            challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
        let flow = GitLabOAuth::begin("https://gitlab.example/enterprise", "public-id").unwrap();
        assert_eq!(
            flow.listener.local_addr().unwrap().ip(),
            std::net::IpAddr::V4(Ipv4Addr::LOCALHOST)
        );
        assert_eq!(flow.listener.local_addr().unwrap().port(), CALLBACK_PORT);
        assert!(matches!(
            GitLabOAuth::begin("https://gitlab.example/enterprise", "public-id"),
            Err(RemoteError::OAuthPortInUse)
        ));
        let url = Url::parse(flow.authorization_url()).unwrap();
        assert_eq!(url.path(), "/enterprise/oauth/authorize");
        let params: HashMap<_, _> = url
            .query_pairs()
            .map(|(key, value)| (key.into_owned(), value.into_owned()))
            .collect();
        assert_eq!(params["client_id"], "public-id");
        assert_eq!(params["scope"], "read_api");
        assert_eq!(params["redirect_uri"], REDIRECT_URI);
        assert_eq!(params["code_challenge_method"], "S256");
        assert_eq!(
            params["code_challenge"],
            challenge(flow.verifier.expose_secret())
        );
        assert!(flow.verifier.expose_secret().len() >= 43);
        assert_ne!(params["state"], flow.verifier.expose_secret());
        assert!(!params.contains_key("client_secret"));
        assert_eq!(
            application_settings_url("https://gitlab.example/enterprise").unwrap(),
            "https://gitlab.example/enterprise/-/user_settings/applications"
        );
        for server in [
            "http://gitlab.com",
            "https://secret@gitlab.com",
            "https://gitlab.com/?token=secret",
            "https://gitlab.com/a%2f..%2f",
        ] {
            assert!(application_settings_url(server).is_err());
        }
    }

    #[test]
    fn callback_requires_matching_state_exact_host_path_and_unique_parameters() {
        let valid = callback(
            "/oauth/callback?code=authorization-code&state=expected-state",
            "127.0.0.1:47321",
        );
        assert_eq!(
            parse_callback(&valid, "expected-state")
                .unwrap()
                .unwrap()
                .expose_secret(),
            "authorization-code"
        );
        for request in [
            callback(
                "/oauth/callback?code=authorization-code&state=wrong-state",
                "127.0.0.1:47321",
            ),
            callback(
                "/oauth/callback?code=authorization-code&state=expected-state",
                "evil.example",
            ),
            callback(
                "/favicon.ico?code=authorization-code&state=expected-state",
                "127.0.0.1:47321",
            ),
            callback(
                "/oauth/callback?code=a&code=b&state=expected-state",
                "127.0.0.1:47321",
            ),
            callback(
                "/oauth/callback?code=a&state=expected-state&%73tate=expected-state",
                "127.0.0.1:47321",
            ),
            callback(
                "/oauth/callback?code=a&error=access_denied&state=expected-state",
                "127.0.0.1:47321",
            ),
            callback(
                "https://evil.example/oauth/callback?code=a&state=expected-state",
                "127.0.0.1:47321",
            ),
            valid.replace("\r\n\r\n", "\r\nHost: 127.0.0.1:47321\r\n\r\n"),
            valid.replace("GET ", "POST "),
            valid.replace("\r\n\r\n", "\r\nTransfer-Encoding: chunked\r\n\r\n"),
        ] {
            assert!(
                parse_callback(&request, "expected-state").is_none(),
                "invalid callback authorized"
            );
        }
        let rejected = callback("/oauth/callback?error=access_denied&state=expected-state&error_description=untrusted-secret", "127.0.0.1:47321");
        assert!(matches!(
            parse_callback(&rejected, "expected-state"),
            Some(Err(RemoteError::CredentialsExpired))
        ));
    }

    fn loopback_flow() -> (GitLabOAuth, std::net::SocketAddr) {
        // Exercise real socket I/O on an ephemeral loopback port so the test does
        // not steal the application callback port; its HTTP Host stays the exact
        // registered redirect authority.
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        (
            GitLabOAuth {
                listener,
                authorization_url: Zeroizing::new(String::new()),
                token_endpoint: Url::parse("https://gitlab.com/oauth/token").unwrap(),
                client_id: "public-app-id".into(),
                state: SecretToken::from("expected-state"),
                verifier: SecretToken::from("a".repeat(43)),
                deadline: Instant::now() + AUTHORIZATION_TIMEOUT,
            },
            address,
        )
    }
    fn send_callback(address: std::net::SocketAddr, request: &str) -> String {
        let mut stream = std::net::TcpStream::connect(address).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        stream.write_all(request.as_bytes()).unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        response
    }

    fn assert_connection_closed(mut stream: std::net::TcpStream) {
        match stream.read(&mut [0]) {
            Ok(0) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::ConnectionAborted
                ) => {}
            result => panic!("pending callback socket did not close: {result:?}"),
        }
    }

    #[test]
    fn real_loopback_ignores_invalid_requests_then_exchanges_code_without_echoing_credentials() {
        let (flow, address) = loopback_flow();
        let thread = std::thread::spawn(move || {
            flow.finish_with(
                &MockTokens(|request: &TokenRequest<'_>, _: &CancellationToken| {
                    assert_eq!(request.endpoint.as_str(), "https://gitlab.com/oauth/token");
                    assert_eq!(field(request, "code"), "authorization-code");
                    assert_eq!(field(request, "grant_type"), "authorization_code");
                    assert_eq!(field(request, "redirect_uri"), REDIRECT_URI);
                    assert_eq!(field(request, "code_verifier"), "a".repeat(43));
                    assert!(!request
                        .fields
                        .iter()
                        .any(|(key, _)| *key == "client_secret"));
                    Ok(token_response("new-access", "new-refresh"))
                }),
                &CancellationToken::new(),
            )
        });
        for target in [
            "/favicon.ico",
            "/oauth/callback?code=attacker-code&state=wrong-state",
        ] {
            let response = send_callback(address, &callback(target, "127.0.0.1:47321"));
            assert!(response.starts_with("HTTP/1.1 400"));
            assert!(!response.contains("attacker-code"));
        }
        let response = send_callback(
            address,
            &callback(
                "/oauth/callback?code=authorization-code&state=expected-state",
                "127.0.0.1:47321",
            ),
        );
        assert!(response.starts_with("HTTP/1.1 200"));
        assert!(response.contains("Cache-Control: no-store"));
        assert!(!response.contains("authorization-code"));
        let credentials = thread.join().unwrap().unwrap();
        assert_eq!(credentials.access_token().expose_secret(), "new-access");
    }

    #[test]
    fn cancelling_callback_wait_physically_closes_the_pending_socket_without_token_exchange() {
        let (flow, address) = loopback_flow();
        let cancellation = CancellationToken::new();
        let worker_cancel = cancellation.clone();
        let (result_tx, result_rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let result = flow.finish_with(
                &MockTokens(|_: &TokenRequest<'_>, _: &CancellationToken| {
                    panic!("cancelled authorization exchanged code")
                }),
                &worker_cancel,
            );
            let _ = result_tx.send(result);
        });
        // A response proves the worker passed its initial cancellation check
        // and is awaiting callbacks inside the cancellable future.
        let response = send_callback(address, &callback("/favicon.ico", "127.0.0.1:47321"));
        assert!(response.starts_with("HTTP/1.1 400"));
        let pending = std::net::TcpStream::connect(address).unwrap();
        pending
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        cancellation.cancel();
        let result = result_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("authorization did not stop after cancellation");
        assert!(matches!(result, Err(RemoteError::Cancelled)));
        worker.join().unwrap();
        assert_connection_closed(pending);
    }

    #[test]
    fn expired_authorization_wait_times_out_without_exchange() {
        let (mut flow, address) = loopback_flow();
        // Observe our established connection: a freed ephemeral port can be
        // reused by another socket test before a new connection probes it.
        let pending = std::net::TcpStream::connect(address).unwrap();
        pending
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        flow.deadline = Instant::now();
        assert!(matches!(
            flow.finish_with(
                &MockTokens(|_: &TokenRequest<'_>, _: &CancellationToken| panic!(
                    "timed out authorization exchanged code"
                )),
                &CancellationToken::new()
            ),
            Err(RemoteError::Timeout)
        ));
        assert_connection_closed(pending);
    }

    #[test]
    fn token_response_rejects_elevated_scopes_invalid_type_and_oversized_secrets() {
        let endpoint = Url::parse("https://gitlab.com/oauth/token").unwrap();
        for (key, value) in [
            ("scope", serde_json::json!("api")),
            ("scope", serde_json::json!("read_api write_repository")),
            ("scope", serde_json::json!("")),
            ("token_type", serde_json::json!("basic")),
            ("refresh_token", serde_json::json!("")),
            ("access_token", serde_json::json!("secret\r\n")),
            ("access_token", serde_json::json!("a".repeat(4097))),
            ("refresh_token", serde_json::json!("r".repeat(4097))),
            ("expires_in", serde_json::json!(0)),
            ("created_at", serde_json::json!(now().unwrap() + 1000)),
        ] {
            let mut json: serde_json::Value =
                serde_json::from_slice(&token_response("access", "refresh")).unwrap();
            json[key] = value;
            assert!(parse_credentials(
                &serde_json::to_vec(&json).unwrap(),
                endpoint.clone(),
                "public-id".into()
            )
            .is_err());
        }
        let mut json: serde_json::Value =
            serde_json::from_slice(&token_response("access", "refresh")).unwrap();
        json["scope"] = serde_json::json!("read_api");
        assert!(parse_credentials(
            &serde_json::to_vec(&json).unwrap(),
            endpoint.clone(),
            "public-id".into()
        )
        .is_ok());
        assert!(matches!(
            parse_credentials(
                &vec![b' '; MAX_TOKEN_BYTES + 1],
                endpoint,
                "public-id".into()
            ),
            Err(RemoteError::ResponseTooLarge)
        ));
    }

    #[test]
    fn vault_bundle_keeps_refresh_secret_out_of_metadata_and_binds_client_and_server() {
        let vault = MemoryVault::default();
        credentials(now().unwrap() + 7200)
            .store(&vault, "personal", "oauth-account")
            .unwrap();
        let cancellation = CancellationToken::new();
        let never_network = MockTokens(|_: &TokenRequest<'_>, _: &CancellationToken| {
            panic!("fresh credential refreshed")
        });
        assert_eq!(
            read_account_token_with(
                &vault,
                "personal",
                &account(),
                &cancellation,
                &never_network
            )
            .unwrap()
            .expose_secret(),
            "original-access"
        );
        let metadata = serde_json::to_string(&account()).unwrap();
        assert!(!metadata.contains("original-access") && !metadata.contains("original-refresh"));
        let mut changed = account();
        changed.server_url = "https://attacker.example".into();
        assert!(matches!(
            read_account_token_with(&vault, "personal", &changed, &cancellation, &never_network),
            Err(RemoteError::InvalidConfiguration)
        ));
        changed = account();
        changed.authentication = Authentication::GitLabOAuth {
            client_id: "another-id".into(),
        };
        assert!(matches!(
            read_account_token_with(&vault, "personal", &changed, &cancellation, &never_network),
            Err(RemoteError::InvalidConfiguration)
        ));
        changed = account();
        changed.authentication = Authentication::PersonalToken;
        assert!(matches!(
            read_account_token_with(&vault, "personal", &changed, &cancellation, &never_network),
            Err(RemoteError::InvalidConfiguration)
        ));
    }

    #[test]
    fn refresh_rotates_both_tokens_once_and_preserves_received_rotation_after_cancel() {
        let vault = MemoryVault::default();
        credentials(1)
            .store(&vault, "personal", "oauth-account")
            .unwrap();
        let cancellation = CancellationToken::new();
        let tokens = MockTokens(|request: &TokenRequest<'_>, cancel: &CancellationToken| {
            assert_eq!(field(request, "refresh_token"), "original-refresh");
            assert_eq!(field(request, "grant_type"), "refresh_token");
            assert_eq!(field(request, "redirect_uri"), REDIRECT_URI);
            assert_eq!(field(request, "client_id"), "public-app-id");
            assert!(!request
                .fields
                .iter()
                .any(|(key, _)| *key == "client_secret"));
            cancel.cancel();
            Ok(token_response("rotated-access", "rotated-refresh"))
        });
        assert!(matches!(
            read_account_token_with(&vault, "personal", &account(), &cancellation, &tokens),
            Err(RemoteError::Cancelled)
        ));
        let packet = vault.read("personal", "oauth-account").unwrap().unwrap();
        assert!(packet.expose_secret().contains("rotated-refresh"));
        assert!(!packet.expose_secret().contains("original-refresh"));
        assert_eq!(
            read_account_token_with(
                &vault,
                "personal",
                &account(),
                &CancellationToken::new(),
                &MockTokens(|_: &TokenRequest<'_>, _: &CancellationToken| panic!(
                    "fresh rotated token refreshed"
                ))
            )
            .unwrap()
            .expose_secret(),
            "rotated-access"
        );
    }

    #[test]
    fn logout_during_refresh_wins_and_failed_store_never_returns_unsaved_rotated_token() {
        let vault = MemoryVault::default();
        credentials(1)
            .store(&vault, "personal", "oauth-account")
            .unwrap();
        let tokens = MockTokens(|_: &TokenRequest<'_>, _: &CancellationToken| {
            vault.delete("personal", "oauth-account").unwrap();
            Ok(token_response("rotated-access", "rotated-refresh"))
        });
        assert!(matches!(
            read_account_token_with(
                &vault,
                "personal",
                &account(),
                &CancellationToken::new(),
                &tokens
            ),
            Err(RemoteError::CredentialsExpired)
        ));
        assert!(vault.read("personal", "oauth-account").unwrap().is_none());
        let failed = MemoryVault {
            fail_replace: true,
            ..Default::default()
        };
        credentials(1)
            .store(&failed, "personal", "oauth-account")
            .unwrap();
        assert!(matches!(
            read_account_token_with(
                &failed,
                "personal",
                &account(),
                &CancellationToken::new(),
                &MockTokens(
                    |_: &TokenRequest<'_>, _: &CancellationToken| Ok(token_response(
                        "rotated-access",
                        "rotated-refresh"
                    ))
                )
            ),
            Err(RemoteError::VaultUnavailable)
        ));
    }

    #[test]
    fn simultaneous_reads_serialize_one_account_refresh_and_share_saved_rotation() {
        let vault = MemoryVault::default();
        credentials(1)
            .store(&vault, "personal", "oauth-account")
            .unwrap();
        let calls = AtomicUsize::new(0);
        let tokens = MockTokens(|_: &TokenRequest<'_>, _: &CancellationToken| {
            calls.fetch_add(1, Ordering::SeqCst);
            std::thread::sleep(Duration::from_millis(30));
            Ok(token_response("rotated-access", "rotated-refresh"))
        });
        std::thread::scope(|scope| {
            let workers: Vec<_> = (0..2)
                .map(|_| {
                    scope.spawn(|| {
                        read_account_token_with(
                            &vault,
                            "personal",
                            &account(),
                            &CancellationToken::new(),
                            &tokens,
                        )
                    })
                })
                .collect();
            for worker in workers {
                assert_eq!(
                    worker.join().unwrap().unwrap().expose_secret(),
                    "rotated-access"
                );
            }
        });
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn legacy_account_json_defaults_to_personal_token_and_oauth_config_rejects_other_providers() {
        let legacy = r#"{"id":"account","label":"GitLab","provider":"GitLab","server_url":"https://gitlab.com","username":null}"#;
        let decoded: Account = serde_json::from_str(legacy).unwrap();
        assert_eq!(decoded.authentication, Authentication::PersonalToken);
        assert!(decoded.validate().is_ok());
        let mut oauth = account();
        oauth.provider = Provider::GitHub;
        assert_eq!(oauth.validate(), Err(RemoteError::InvalidConfiguration));
        oauth = account();
        oauth.authentication = Authentication::GitLabOAuth {
            client_id: "secret\nvalue".into(),
        };
        assert_eq!(oauth.validate(), Err(RemoteError::InvalidConfiguration));
    }

    struct EchoRepositories;
    impl Transport for EchoRepositories {
        fn get(
            &self,
            request: &crate::HttpRequest,
            cancel: &CancellationToken,
        ) -> Result<crate::HttpResponse> {
            cancel.check()?;
            assert!(request
                .headers
                .iter()
                .any(|(name, value)| name.eq_ignore_ascii_case("authorization")
                    && value.expose_secret() == "Bearer original-access"));
            assert!(!request
                .headers
                .iter()
                .any(|(name, _)| name.eq_ignore_ascii_case("private-token")));
            Ok(crate::HttpResponse { status:200, headers: Default::default(), body:serde_json::to_vec(&serde_json::json!([{
                "id":42,"name":"Project original-refresh", "path_with_namespace":"group/repo", "web_url":"https://gitlab.com/project/%6friginal-refresh"
            }])).unwrap() })
        }
    }

    #[test]
    fn oauth_provider_shields_refresh_echoes_while_old_access_only_constructor_exposes_them() {
        let credentials = credentials(now().unwrap() + 7200);
        // Behavioral control: the former access-only construction cannot know
        // the refresh secret and leaks this response. The opaque helper must.
        let old = ProviderClient::new(EchoRepositories, account(), credentials.access_token())
            .unwrap()
            .repositories(None, &CancellationToken::new())
            .unwrap();
        assert!(old.items[0].name.contains("original-refresh"));
        let protected = credentials
            .provider_client(EchoRepositories, account())
            .unwrap()
            .repositories(None, &CancellationToken::new())
            .unwrap();
        assert_eq!(protected.items[0].name, "Project [redacted]");
        assert!(protected.items[0].web_url.is_none());
        assert!(!format!("{protected:?}").contains("original-refresh"));
    }

    #[test]
    fn token_request_deadline_reports_a_network_timeout() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let endpoint = Url::parse(&format!(
            "https://{}/oauth/token",
            listener.local_addr().unwrap()
        ))
        .unwrap();
        assert!(matches!(
            HttpTokenTransport.post_until(
                &TokenRequest {
                    endpoint: &endpoint,
                    basic_auth: None,
                    fields: vec![("client_id", SecretToken::from("public-id"))],
                },
                &CancellationToken::new(),
                Instant::now() + Duration::from_millis(50),
            ),
            Err(RemoteError::Timeout)
        ));
    }

    #[test]
    fn cancelling_token_https_physically_drops_a_pending_socket() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let cancel = CancellationToken::new();
        let (accepted_tx, accepted_rx) = std::sync::mpsc::channel();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            accepted_tx.send(()).unwrap();
            let mut bytes = [0u8; 1024];
            loop {
                match stream.read(&mut bytes) {
                    Ok(0) => break,
                    Ok(_) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::ConnectionReset => break,
                    Err(error) => panic!("pending HTTPS socket did not close: {error}"),
                }
            }
        });
        let worker_cancel = cancel.clone();
        let worker = std::thread::spawn(move || {
            let endpoint = Url::parse(&format!("https://{address}/oauth/token")).unwrap();
            HttpTokenTransport.post(
                &TokenRequest {
                    endpoint: &endpoint,
                    basic_auth: None,
                    fields: vec![("grant_type", SecretToken::from("authorization_code"))],
                },
                &worker_cancel,
            )
        });
        accepted_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        cancel.cancel();
        assert!(matches!(
            worker.join().unwrap(),
            Err(RemoteError::Cancelled)
        ));
        server.join().unwrap();
    }
}
