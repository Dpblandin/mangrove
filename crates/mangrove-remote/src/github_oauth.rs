//! GitHub public-client device authorization. Blocking entry points belong on a
//! background executor. Only the public application ID enters profile metadata;
//! device codes and credentials intentionally have no Debug/Serialize API.
use std::{
    sync::TryLockError,
    time::{Duration, Instant},
};

use secrecy::zeroize::{Zeroize, Zeroizing};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::{
    oauth::{self, HttpTokenTransport, TokenRequest, TokenTransport, MAX_TOKEN_BYTES},
    Account, Authentication, CancellationToken, CredentialVault, ExposeSecret, ProviderClient,
    RemoteError, Result, SafeApiUrl, SecretToken, Transport,
};

const VAULT_PREFIX: &str = "mangrove-github-device-v1:";
const DEVICE_LIFETIME: Duration = Duration::from_secs(900);
const DEVICE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";

/// Public registration page on the selected GitHub instance.
pub fn application_settings_url(server_url: &str) -> Result<String> {
    Ok(oauth_base(server_url)?
        .url("settings/applications/new")?
        .into())
}

pub struct GitHubDeviceFlow {
    device_code: SecretToken,
    user_code: Zeroizing<String>,
    verification_url: String,
    token_endpoint: Url,
    client_id: String,
    private_repos: bool,
    interval: Duration,
    deadline: Instant,
}

impl GitHubDeviceFlow {
    /// Request a short code before opening the browser. This does not need an
    /// application secret, callback server, or broad scope for public repositories.
    pub fn begin(
        server_url: &str,
        client_id: &str,
        private_repos: bool,
        cancel: &CancellationToken,
    ) -> Result<Self> {
        Self::begin_with(
            server_url,
            client_id,
            private_repos,
            cancel,
            &HttpTokenTransport,
            &SystemClock,
        )
    }

    pub fn verification_url(&self) -> &str {
        &self.verification_url
    }
    pub fn user_code(&self) -> &str {
        &self.user_code
    }

    pub fn finish(self, cancel: &CancellationToken) -> Result<GitHubCredentials> {
        self.finish_with(cancel, &HttpTokenTransport, &SystemClock)
    }

    fn begin_with(
        server_url: &str,
        client_id: &str,
        private_repos: bool,
        cancel: &CancellationToken,
        transport: &dyn TokenTransport,
        clock: &dyn PollClock,
    ) -> Result<Self> {
        oauth::validate_client_id(client_id)?;
        cancel.check()?;
        let base = oauth_base(server_url)?;
        let expected_verification = base.url("login/device")?;
        let endpoint = base.url("login/device/code")?;
        let mut fields = vec![("client_id", SecretToken::from(client_id))];
        if private_repos {
            fields.push(("scope", SecretToken::from("repo")));
        }
        let started = clock.now();
        let bytes = transport.post_until(
            &TokenRequest {
                endpoint: &endpoint,
                fields,
            },
            cancel,
            started
                .checked_add(Duration::from_secs(20))
                .ok_or(RemoteError::InvalidResponse)?,
        )?;
        cancel.check()?;
        check_size(&bytes)?;
        if let Some(error) = parse_error(&bytes)? {
            return Err(device_error(&error.error));
        }
        let response: DeviceResponse =
            serde_json::from_slice(&bytes).map_err(|_| RemoteError::InvalidResponse)?;
        let verification =
            Url::parse(&response.verification_uri).map_err(|_| RemoteError::InvalidResponse)?;
        // A response may not turn the browser into an arbitrary redirect or send
        // the code to a different origin, path, userinfo, fragment, or query.
        if verification != expected_verification
            || !oauth::valid_secret(&response.device_code)
            || !valid_user_code(&response.user_code)
            || response.expires_in == 0
            || response.expires_in > DEVICE_LIFETIME.as_secs()
            || response.interval == 0
            || response.interval > response.expires_in
        {
            return Err(RemoteError::InvalidResponse);
        }
        let deadline = started
            .checked_add(Duration::from_secs(response.expires_in))
            .ok_or(RemoteError::InvalidResponse)?;
        if clock.now() >= deadline {
            return Err(RemoteError::CredentialsExpired);
        }
        Ok(Self {
            device_code: SecretToken::from(response.device_code.as_str()),
            user_code: Zeroizing::new(response.user_code.clone()),
            verification_url: expected_verification.into(),
            token_endpoint: base.url("login/oauth/access_token")?,
            client_id: client_id.into(),
            private_repos,
            interval: Duration::from_secs(response.interval.max(5)),
            deadline,
        })
    }

    fn finish_with(
        mut self,
        cancel: &CancellationToken,
        transport: &dyn TokenTransport,
        clock: &dyn PollClock,
    ) -> Result<GitHubCredentials> {
        loop {
            cancel.check()?;
            let remaining = self.deadline.saturating_duration_since(clock.now());
            if remaining.is_zero() {
                return Err(RemoteError::CredentialsExpired);
            }
            clock.wait(self.interval.min(remaining), cancel)?;
            cancel.check()?;
            if clock.now() >= self.deadline {
                return Err(RemoteError::CredentialsExpired);
            }
            let response = transport.post_until(
                &TokenRequest {
                    endpoint: &self.token_endpoint,
                    fields: vec![
                        ("client_id", SecretToken::from(self.client_id.as_str())),
                        ("device_code", self.device_code.clone()),
                        ("grant_type", SecretToken::from(DEVICE_GRANT)),
                    ],
                },
                cancel,
                self.deadline,
            );
            cancel.check()?;
            if clock.now() >= self.deadline {
                return Err(RemoteError::CredentialsExpired);
            }
            let bytes = response?;
            check_size(&bytes)?;
            if let Some(error) = parse_error(&bytes)? {
                match error.error.as_str() {
                    "authorization_pending" => {}
                    "slow_down" => {
                        self.interval = self
                            .interval
                            .checked_add(Duration::from_secs(5))
                            .ok_or(RemoteError::InvalidResponse)?;
                        if let Some(interval) = error.interval {
                            if interval == 0 || interval > DEVICE_LIFETIME.as_secs() {
                                return Err(RemoteError::InvalidResponse);
                            }
                            self.interval = self.interval.max(Duration::from_secs(interval));
                        }
                    }
                    _ => return Err(device_error(&error.error)),
                }
                continue;
            }
            return parse_credentials(
                &bytes,
                self.token_endpoint,
                self.client_id,
                self.private_repos,
            );
        }
    }
}

trait PollClock {
    fn now(&self) -> Instant;
    fn wait(&self, duration: Duration, cancel: &CancellationToken) -> Result<()>;
}
struct SystemClock;
impl PollClock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
    fn wait(&self, duration: Duration, cancel: &CancellationToken) -> Result<()> {
        cancel.check()?;
        oauth::runtime()?.block_on(async {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => Err(RemoteError::Cancelled),
                _ = tokio::time::sleep(duration) => Ok(()),
            }
        })
    }
}

#[derive(Deserialize)]
struct DeviceResponse {
    device_code: String,
    user_code: String,
    verification_uri: String,
    expires_in: u64,
    interval: u64,
}
impl Drop for DeviceResponse {
    fn drop(&mut self) {
        self.device_code.zeroize();
        self.user_code.zeroize();
    }
}
struct ErrorResponse {
    error: Zeroizing<String>,
    interval: Option<u64>,
}
fn parse_error(bytes: &[u8]) -> Result<Option<ErrorResponse>> {
    #[derive(Deserialize)]
    struct Envelope {
        error: Option<String>,
        interval: Option<u64>,
        access_token: Option<serde::de::IgnoredAny>,
    }
    let response: Envelope =
        serde_json::from_slice(bytes).map_err(|_| RemoteError::InvalidResponse)?;
    let error = response.error.map(Zeroizing::new);
    if error.is_some() && response.access_token.is_some() {
        return Err(RemoteError::InvalidResponse);
    }
    Ok(error.map(|error| ErrorResponse {
        error,
        interval: response.interval,
    }))
}
fn device_error(error: &str) -> RemoteError {
    match error {
        "access_denied" => RemoteError::OAuthAccessDenied,
        "device_flow_disabled" => RemoteError::OAuthDeviceFlowDisabled,
        "incorrect_client_credentials" | "unsupported_grant_type" => {
            RemoteError::InvalidConfiguration
        }
        "expired_token" | "token_expired" | "incorrect_device_code" | "bad_refresh_token" => {
            RemoteError::CredentialsExpired
        }
        _ => RemoteError::InvalidResponse,
    }
}
fn valid_user_code(code: &str) -> bool {
    code.len() == 9
        && code.as_bytes()[4] == b'-'
        && code
            .bytes()
            .enumerate()
            .all(|(index, byte)| index == 4 || byte.is_ascii_alphanumeric())
}
fn check_size(bytes: &[u8]) -> Result<()> {
    if bytes.len() > MAX_TOKEN_BYTES {
        Err(RemoteError::ResponseTooLarge)
    } else {
        Ok(())
    }
}

/// Both tokens stay in one native-vault entry. Nonexpiring OAuth tokens are also
/// supported; a refresh token is never synthesized or required for those tokens.
pub struct GitHubCredentials {
    access: SecretToken,
    refresh: Option<SecretToken>,
    expires_at: Option<u64>,
    refresh_expires_at: Option<u64>,
    token_endpoint: Url,
    client_id: String,
    private_repos: bool,
}
impl GitHubCredentials {
    pub(crate) fn access_token(&self) -> SecretToken {
        self.access.clone()
    }
    pub fn provider_client<T: Transport>(
        &self,
        transport: T,
        account: Account,
    ) -> Result<ProviderClient<T>> {
        account.validate()?;
        if account.authentication
            != (Authentication::GitHubOAuth {
                client_id: self.client_id.clone(),
            })
            || token_endpoint(&account.server_url)? != self.token_endpoint
        {
            return Err(RemoteError::InvalidConfiguration);
        }
        let client = ProviderClient::new(transport, account, self.access_token())?;
        Ok(if let Some(refresh) = &self.refresh {
            client.protect_secret(refresh)
        } else {
            client
        })
    }
    pub fn store(
        &self,
        vault: &dyn CredentialVault,
        profile_id: &str,
        account_id: &str,
    ) -> Result<()> {
        crate::model::validate_id(profile_id)?;
        crate::model::validate_id(account_id)?;
        vault.store(
            profile_id,
            account_id,
            &self.encode(profile_id, account_id)?,
        )
    }
    fn encode(&self, profile_id: &str, account_id: &str) -> Result<SecretToken> {
        #[derive(Serialize)]
        struct Stored<'a> {
            access_token: &'a str,
            refresh_token: Option<&'a str>,
            expires_at: Option<u64>,
            refresh_expires_at: Option<u64>,
            token_endpoint: &'a str,
            client_id: &'a str,
            private_repos: bool,
            profile_id: &'a str,
            account_id: &'a str,
        }
        let stored = Stored {
            access_token: self.access.expose_secret(),
            refresh_token: self.refresh.as_ref().map(ExposeSecret::expose_secret),
            expires_at: self.expires_at,
            refresh_expires_at: self.refresh_expires_at,
            token_endpoint: self.token_endpoint.as_str(),
            client_id: &self.client_id,
            private_repos: self.private_repos,
            profile_id,
            account_id,
        };
        let json = Zeroizing::new(
            serde_json::to_string(&stored).map_err(|_| RemoteError::InvalidResponse)?,
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
fn token_endpoint(server_url: &str) -> Result<Url> {
    oauth_base(server_url)?.url("login/oauth/access_token")
}

fn oauth_base(server_url: &str) -> Result<SafeApiUrl> {
    let base = SafeApiUrl::new(server_url, "")?;
    let server = Url::parse(server_url).map_err(|_| RemoteError::InvalidConfiguration)?;
    if matches!(server.host_str(), Some("github.com" | "api.github.com")) {
        if !matches!(server.path(), "" | "/") || server.port().is_some() {
            return Err(RemoteError::InvalidConfiguration);
        }
        SafeApiUrl::new("https://github.com", "")
    } else {
        Ok(base)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredCredentials {
    access_token: String,
    refresh_token: Option<String>,
    expires_at: Option<u64>,
    refresh_expires_at: Option<u64>,
    token_endpoint: String,
    client_id: String,
    private_repos: bool,
    profile_id: String,
    account_id: String,
}
impl Drop for StoredCredentials {
    fn drop(&mut self) {
        self.access_token.zeroize();
        if let Some(refresh) = &mut self.refresh_token {
            refresh.zeroize();
        }
    }
}
pub(crate) fn is_credential_packet(token: &SecretToken) -> bool {
    token.expose_secret().starts_with(VAULT_PREFIX)
}
fn decode_credentials(
    packet: &SecretToken,
    account: &Account,
    profile_id: &str,
) -> Result<GitHubCredentials> {
    check_size(packet.expose_secret().as_bytes())?;
    let json = packet
        .expose_secret()
        .strip_prefix(VAULT_PREFIX)
        .ok_or(RemoteError::CredentialsExpired)?;
    let stored: StoredCredentials =
        serde_json::from_str(json).map_err(|_| RemoteError::CredentialsExpired)?;
    let Authentication::GitHubOAuth { client_id } = &account.authentication else {
        return Err(RemoteError::InvalidConfiguration);
    };
    let endpoint = token_endpoint(&account.server_url)?;
    if stored.token_endpoint != endpoint.as_str()
        || &stored.client_id != client_id
        || stored.profile_id != profile_id
        || stored.account_id != account.id
        || !oauth::valid_secret(&stored.access_token)
        || stored
            .refresh_token
            .as_ref()
            .is_some_and(|token| !oauth::valid_secret(token))
        || !valid_expiry_tuple(
            stored.expires_at,
            stored.refresh_token.as_deref(),
            stored.refresh_expires_at,
        )
    {
        return Err(RemoteError::InvalidConfiguration);
    }
    Ok(GitHubCredentials {
        access: SecretToken::from(stored.access_token.as_str()),
        refresh: stored.refresh_token.as_deref().map(SecretToken::from),
        expires_at: stored.expires_at,
        refresh_expires_at: stored.refresh_expires_at,
        token_endpoint: endpoint,
        client_id: client_id.clone(),
        private_repos: stored.private_repos,
    })
}

pub(crate) fn read_credentials_with(
    vault: &dyn CredentialVault,
    profile_id: &str,
    account: &Account,
    cancel: &CancellationToken,
    transport: &dyn TokenTransport,
) -> Result<GitHubCredentials> {
    account.validate()?;
    crate::model::validate_id(profile_id)?;
    cancel.check()?;
    let lock = oauth::refresh_lock(profile_id, &account.id)?;
    let _guard = loop {
        cancel.check()?;
        match lock.try_lock() {
            Ok(guard) => break guard,
            Err(TryLockError::Poisoned(_)) => return Err(RemoteError::VaultUnavailable),
            Err(TryLockError::WouldBlock) => std::thread::sleep(Duration::from_millis(10)),
        }
    };
    let previous = vault
        .read(profile_id, &account.id)?
        .ok_or(RemoteError::CredentialsExpired)?;
    let credentials = decode_credentials(&previous, account, profile_id)?;
    let now = oauth::now()?;
    if credentials
        .expires_at
        .is_none_or(|expiry| expiry > now.saturating_add(30))
    {
        cancel.check()?;
        return Ok(credentials);
    }
    if credentials
        .refresh_expires_at
        .is_none_or(|expiry| expiry <= now)
    {
        return Err(RemoteError::CredentialsExpired);
    }
    let refresh = credentials
        .refresh
        .as_ref()
        .ok_or(RemoteError::CredentialsExpired)?;
    let response = transport.post_until(
        &TokenRequest {
            endpoint: &credentials.token_endpoint,
            fields: vec![
                (
                    "client_id",
                    SecretToken::from(credentials.client_id.as_str()),
                ),
                ("grant_type", SecretToken::from("refresh_token")),
                ("refresh_token", refresh.clone()),
            ],
        },
        cancel,
        Instant::now()
            .checked_add(Duration::from_secs(20))
            .ok_or(RemoteError::InvalidResponse)?,
    )?;
    let rotated = parse_credentials(
        &response,
        credentials.token_endpoint,
        credentials.client_id,
        credentials.private_repos,
    )?;
    // Keep a received rotation even after cancellation. Logout/replacement wins
    // the same atomic comparison used for the existing GitLab credential packet.
    if !vault.replace_if_matches(
        profile_id,
        &account.id,
        &previous,
        &rotated.encode(profile_id, &account.id)?,
    )? {
        return Err(RemoteError::CredentialsExpired);
    }
    cancel.check()?;
    Ok(rotated)
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    token_type: String,
    expires_in: Option<u64>,
    refresh_token_expires_in: Option<u64>,
    scope: Option<String>,
}
impl Drop for TokenResponse {
    fn drop(&mut self) {
        self.access_token.zeroize();
        if let Some(refresh) = &mut self.refresh_token {
            refresh.zeroize();
        }
    }
}
fn valid_expiry_tuple(
    access: Option<u64>,
    refresh: Option<&str>,
    refresh_expiry: Option<u64>,
) -> bool {
    matches!(
        (access, refresh, refresh_expiry),
        (None, None, None) | (Some(_), Some(_), Some(_))
    )
}
fn parse_credentials(
    bytes: &[u8],
    endpoint: Url,
    client_id: String,
    private_repos: bool,
) -> Result<GitHubCredentials> {
    check_size(bytes)?;
    if let Some(error) = parse_error(bytes)? {
        return Err(device_error(&error.error));
    }
    let response: TokenResponse =
        serde_json::from_slice(bytes).map_err(|_| RemoteError::InvalidResponse)?;
    let scopes_valid = response.scope.as_ref().is_none_or(|scope| {
        let scopes = scope
            .split(|character: char| character == ',' || character.is_whitespace())
            .filter(|scope| !scope.is_empty())
            .collect::<Vec<_>>();
        if private_repos {
            scopes == ["repo"]
        } else {
            scopes.is_empty()
        }
    });
    if !response.token_type.eq_ignore_ascii_case("bearer")
        || !oauth::valid_secret(&response.access_token)
        || response
            .refresh_token
            .as_ref()
            .is_some_and(|token| !oauth::valid_secret(token))
        || !valid_expiry_tuple(
            response.expires_in,
            response.refresh_token.as_deref(),
            response.refresh_token_expires_in,
        )
        || !scopes_valid
    {
        return Err(RemoteError::InvalidResponse);
    }
    let now = oauth::now()?;
    let expiry = |duration: Option<u64>| -> Result<Option<u64>> {
        duration
            .map(|duration| {
                if duration == 0 || duration > 365 * 24 * 3600 {
                    return Err(RemoteError::InvalidResponse);
                }
                now.checked_add(duration)
                    .ok_or(RemoteError::InvalidResponse)
            })
            .transpose()
    };
    Ok(GitHubCredentials {
        access: SecretToken::from(response.access_token.as_str()),
        refresh: response.refresh_token.as_deref().map(SecretToken::from),
        expires_at: expiry(response.expires_in)?,
        refresh_expires_at: expiry(response.refresh_token_expires_in)?,
        token_endpoint: endpoint,
        client_id,
        private_repos,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{HttpRequest, HttpResponse, Provider};
    use std::{
        collections::BTreeMap,
        sync::{
            atomic::{AtomicUsize, Ordering},
            Mutex,
        },
    };

    struct Tokens<F>(F);
    impl<F> TokenTransport for Tokens<F>
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
    struct Clock {
        state: Mutex<(Instant, Vec<Duration>)>,
        cancel_wait: bool,
    }
    impl Clock {
        fn new() -> Self {
            Self {
                state: Mutex::new((Instant::now(), Vec::new())),
                cancel_wait: false,
            }
        }
        fn waits(&self) -> Vec<Duration> {
            self.state.lock().unwrap().1.clone()
        }
    }
    impl PollClock for Clock {
        fn now(&self) -> Instant {
            self.state.lock().unwrap().0
        }
        fn wait(&self, duration: Duration, cancel: &CancellationToken) -> Result<()> {
            cancel.check()?;
            let mut state = self.state.lock().unwrap();
            state.0 += duration;
            state.1.push(duration);
            if self.cancel_wait {
                cancel.cancel();
            }
            cancel.check()
        }
    }
    #[derive(Default)]
    struct Vault(Mutex<Option<SecretToken>>);
    impl CredentialVault for Vault {
        fn store(&self, _: &str, _: &str, token: &SecretToken) -> Result<()> {
            *self.0.lock().unwrap() = Some(token.clone());
            Ok(())
        }
        fn read(&self, _: &str, _: &str) -> Result<Option<SecretToken>> {
            Ok(self.0.lock().unwrap().clone())
        }
        fn delete(&self, _: &str, _: &str) -> Result<()> {
            *self.0.lock().unwrap() = None;
            Ok(())
        }
        fn replace_if_matches(
            &self,
            _: &str,
            _: &str,
            previous: &SecretToken,
            next: &SecretToken,
        ) -> Result<bool> {
            let mut value = self.0.lock().unwrap();
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
    fn bytes(json: serde_json::Value) -> Zeroizing<Vec<u8>> {
        Zeroizing::new(serde_json::to_vec(&json).unwrap())
    }
    fn device() -> Zeroizing<Vec<u8>> {
        bytes(serde_json::json!({
            "device_code": "secret-device-code", "user_code": "ABCD-1234",
            "verification_uri": "https://github.com/login/device", "expires_in":900, "interval":5
        }))
    }
    fn token(refresh: bool) -> Zeroizing<Vec<u8>> {
        if refresh {
            bytes(serde_json::json!({
                "access_token":"secret-access", "refresh_token":"secret-refresh", "token_type":"bearer", "scope":"repo",
                "expires_in":28800, "refresh_token_expires_in":15897600
            }))
        } else {
            bytes(
                serde_json::json!({"access_token":"secret-access", "token_type":"bearer", "scope":""}),
            )
        }
    }
    fn field<'a>(request: &'a TokenRequest<'_>, name: &str) -> Option<&'a str> {
        request
            .fields
            .iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| value.expose_secret())
    }
    fn start_flow(clock: &Clock, private: bool) -> GitHubDeviceFlow {
        GitHubDeviceFlow::begin_with(
            "https://github.com",
            "public-id",
            private,
            &CancellationToken::new(),
            &Tokens(|_: &TokenRequest<'_>, _: &CancellationToken| Ok(device())),
            clock,
        )
        .unwrap()
    }
    fn account() -> Account {
        Account {
            id: "github-account".into(),
            label: "GitHub".into(),
            provider: Provider::GitHub,
            server_url: "https://github.com".into(),
            username: None,
            authentication: Authentication::GitHubOAuth {
                client_id: "public-id".into(),
            },
        }
    }
    fn credentials(refresh: bool) -> GitHubCredentials {
        parse_credentials(
            &token(refresh),
            token_endpoint("https://github.com").unwrap(),
            "public-id".into(),
            refresh,
        )
        .unwrap()
    }

    #[test]
    fn browser_registration_and_device_request_are_instance_bound_and_scope_is_explicit() {
        let clock = Clock::new();
        for (server, browser_base) in [
            ("https://api.github.com", "https://github.com"),
            (
                "https://github.company.test/deployment",
                "https://github.company.test/deployment",
            ),
        ] {
            let flow = GitHubDeviceFlow::begin_with(
                server,
                "public-id",
                false,
                &CancellationToken::new(),
                &Tokens(|request: &TokenRequest<'_>, _: &CancellationToken| {
                    assert_eq!(
                        request.endpoint.as_str(),
                        format!("{browser_base}/login/device/code")
                    );
                    let mut response: serde_json::Value =
                        serde_json::from_slice(&device()).unwrap();
                    response["verification_uri"] =
                        serde_json::json!(format!("{browser_base}/login/device"));
                    response["interval"] = serde_json::json!(1);
                    Ok(bytes(response))
                }),
                &clock,
            )
            .unwrap();
            assert_eq!(
                flow.verification_url(),
                format!("{browser_base}/login/device")
            );
            assert_eq!(
                flow.token_endpoint.as_str(),
                format!("{browser_base}/login/oauth/access_token")
            );
            assert_eq!(flow.interval, Duration::from_secs(5));
            assert_eq!(
                application_settings_url(server).unwrap(),
                format!("{browser_base}/settings/applications/new")
            );
        }
        for private in [false, true] {
            let flow = GitHubDeviceFlow::begin_with(
                "https://github.company.test",
                "public-id",
                private,
                &CancellationToken::new(),
                &Tokens(|request: &TokenRequest<'_>, _: &CancellationToken| {
                    assert_eq!(
                        request.endpoint.as_str(),
                        "https://github.company.test/login/device/code"
                    );
                    assert_eq!(field(request, "client_id"), Some("public-id"));
                    assert_eq!(field(request, "scope"), private.then_some("repo"));
                    assert_eq!(field(request, "client_secret"), None);
                    let mut response: serde_json::Value =
                        serde_json::from_slice(&device()).unwrap();
                    response["verification_uri"] =
                        serde_json::json!("https://github.company.test/login/device");
                    Ok(bytes(response))
                }),
                &clock,
            )
            .unwrap();
            assert_eq!(
                flow.verification_url(),
                "https://github.company.test/login/device"
            );
            assert_eq!(flow.user_code(), "ABCD-1234");
        }
        assert_eq!(
            application_settings_url("https://github.company.test").unwrap(),
            "https://github.company.test/settings/applications/new"
        );
        for server in [
            "http://github.com",
            "https://token@github.com",
            "https://github.com/?secret=x",
            "https://api.github.com/api/v3",
            "https://github.com:8443",
        ] {
            assert!(application_settings_url(server).is_err());
        }
    }

    #[test]
    fn polling_respects_initial_interval_pending_and_server_slowdown_without_secret() {
        let clock = Clock::new();
        let calls = AtomicUsize::new(0);
        let started = clock.now();
        let result = start_flow(&clock, false)
            .finish_with(
                &CancellationToken::new(),
                &Tokens(|request: &TokenRequest<'_>, _: &CancellationToken| {
                    assert_eq!(
                        request.endpoint.as_str(),
                        "https://github.com/login/oauth/access_token"
                    );
                    assert_eq!(field(request, "grant_type"), Some(DEVICE_GRANT));
                    assert_eq!(field(request, "device_code"), Some("secret-device-code"));
                    assert_eq!(field(request, "client_secret"), None);
                    match calls.fetch_add(1, Ordering::SeqCst) {
                        0 => {
                            assert_eq!(clock.now().duration_since(started).as_secs(), 5);
                            Ok(bytes(serde_json::json!({"error":"authorization_pending"})))
                        }
                        1 => {
                            assert_eq!(clock.now().duration_since(started).as_secs(), 10);
                            Ok(bytes(
                                serde_json::json!({"error":"slow_down", "interval":17}),
                            ))
                        }
                        2 => {
                            assert_eq!(clock.now().duration_since(started).as_secs(), 27);
                            Ok(token(false))
                        }
                        _ => panic!("authorization polled after success"),
                    }
                }),
                &clock,
            )
            .unwrap();
        assert_eq!(result.access.expose_secret(), "secret-access");
        assert_eq!(clock.waits(), [5, 5, 17].map(Duration::from_secs));
    }

    #[test]
    fn slowdown_without_interval_adds_five_seconds_and_expiry_stops_requests() {
        let clock = Clock::new();
        let mut flow = start_flow(&clock, false);
        flow.deadline = clock.now() + Duration::from_secs(13);
        let calls = AtomicUsize::new(0);
        let result = flow.finish_with(
            &CancellationToken::new(),
            &Tokens(|_: &TokenRequest<'_>, _: &CancellationToken| {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(bytes(serde_json::json!({"error":"slow_down"})))
            }),
            &clock,
        );
        assert!(matches!(result, Err(RemoteError::CredentialsExpired)));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(clock.waits(), [5, 8].map(Duration::from_secs));
    }

    #[test]
    fn cancelling_during_wait_never_sends_another_poll() {
        let mut clock = Clock::new();
        clock.cancel_wait = true;
        let flow = start_flow(&clock, false);
        assert!(matches!(
            flow.finish_with(
                &CancellationToken::new(),
                &Tokens(|_: &TokenRequest<'_>, _: &CancellationToken| panic!(
                    "cancelled wait polled"
                )),
                &clock
            ),
            Err(RemoteError::Cancelled)
        ));
        let clock = Clock::new();
        let cancel = CancellationToken::new();
        let flow = start_flow(&clock, false);
        cancel.cancel();
        assert!(matches!(
            flow.finish_with(
                &cancel,
                &Tokens(|_: &TokenRequest<'_>, _: &CancellationToken| panic!(
                    "cancelled flow polled"
                )),
                &clock
            ),
            Err(RemoteError::Cancelled)
        ));
        assert!(clock.waits().is_empty());
    }

    #[test]
    fn real_wait_is_interrupted_by_cancellation() {
        let cancel = CancellationToken::new();
        let worker_cancel = cancel.clone();
        let (send, receive) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let _ = send.send(SystemClock.wait(Duration::from_secs(60), &worker_cancel));
        });
        cancel.cancel();
        assert_eq!(
            receive.recv_timeout(Duration::from_secs(3)).unwrap(),
            Err(RemoteError::Cancelled)
        );
        worker.join().unwrap();
    }

    #[test]
    fn cancellation_or_expiry_during_request_cannot_complete_authorization() {
        let never = Tokens(|_: &TokenRequest<'_>, _: &CancellationToken| {
            panic!("cancelled begin sent a request")
        });
        let cancel = CancellationToken::new();
        cancel.cancel();
        assert!(matches!(
            GitHubDeviceFlow::begin_with(
                "https://github.com",
                "public-id",
                false,
                &cancel,
                &never,
                &Clock::new()
            ),
            Err(RemoteError::Cancelled)
        ));
        for cancelled in [false, true] {
            let clock = Clock::new();
            let flow = start_flow(&clock, false);
            let calls = AtomicUsize::new(0);
            let result = flow.finish_with(
                &CancellationToken::new(),
                &Tokens(|_: &TokenRequest<'_>, cancel: &CancellationToken| {
                    calls.fetch_add(1, Ordering::SeqCst);
                    if cancelled {
                        cancel.cancel();
                    } else {
                        clock.state.lock().unwrap().0 += DEVICE_LIFETIME;
                    }
                    Ok(token(false))
                }),
                &clock,
            );
            let expected = if cancelled {
                RemoteError::Cancelled
            } else {
                RemoteError::CredentialsExpired
            };
            assert!(matches!(result, Err(error) if error == expected));
            assert_eq!(calls.load(Ordering::SeqCst), 1);
        }
    }

    #[test]
    fn network_timeouts_do_not_expire_device_codes_or_replace_credentials() {
        let timeout =
            Tokens(|_: &TokenRequest<'_>, _: &CancellationToken| Err(RemoteError::Timeout));
        assert!(matches!(
            GitHubDeviceFlow::begin_with(
                "https://github.com",
                "public-id",
                false,
                &CancellationToken::new(),
                &timeout,
                &Clock::new()
            ),
            Err(RemoteError::Timeout)
        ));
        for expires_during_request in [false, true] {
            let clock = Clock::new();
            let flow = start_flow(&clock, false);
            let result = flow.finish_with(
                &CancellationToken::new(),
                &Tokens(|_: &TokenRequest<'_>, _: &CancellationToken| {
                    if expires_during_request {
                        clock.state.lock().unwrap().0 += DEVICE_LIFETIME;
                    }
                    Err(RemoteError::Timeout)
                }),
                &clock,
            );
            let expected = if expires_during_request {
                RemoteError::CredentialsExpired
            } else {
                RemoteError::Timeout
            };
            assert!(matches!(result, Err(error) if error == expected));
        }
        let vault = Vault::default();
        let mut credentials = credentials(true);
        credentials.expires_at = Some(1);
        credentials
            .store(&vault, "personal", "github-account")
            .unwrap();
        let previous = vault.read("personal", "github-account").unwrap().unwrap();
        assert!(matches!(
            read_credentials_with(
                &vault,
                "personal",
                &account(),
                &CancellationToken::new(),
                &timeout
            ),
            Err(RemoteError::Timeout)
        ));
        assert_eq!(
            vault
                .read("personal", "github-account")
                .unwrap()
                .unwrap()
                .expose_secret(),
            previous.expose_secret()
        );
    }

    #[test]
    fn errors_are_fixed_and_never_show_device_codes_or_server_descriptions() {
        for (code, expected) in [
            ("access_denied", RemoteError::OAuthAccessDenied),
            ("device_flow_disabled", RemoteError::OAuthDeviceFlowDisabled),
            ("expired_token", RemoteError::CredentialsExpired),
            (
                "incorrect_client_credentials",
                RemoteError::InvalidConfiguration,
            ),
            ("secret-device-code", RemoteError::InvalidResponse),
        ] {
            let clock = Clock::new();
            let flow = start_flow(&clock, false);
            let result = flow.finish_with(&CancellationToken::new(), &Tokens(|_: &TokenRequest<'_>, _: &CancellationToken| {
                Ok(bytes(serde_json::json!({"error":code,"error_description":"secret-device-code secret-access"})))
            }), &clock);
            assert!(matches!(result, Err(error) if error == expected));
            assert!(!expected.to_string().contains("secret"));
        }
    }

    #[test]
    fn malformed_and_hostile_device_responses_are_rejected_before_browser_open() {
        for (key, value) in [
            (
                "verification_uri",
                serde_json::json!("https://evil.test/login/device"),
            ),
            (
                "verification_uri",
                serde_json::json!("http://github.com/login/device"),
            ),
            (
                "verification_uri",
                serde_json::json!("https://github.com/login/device?code=secret-device-code"),
            ),
            (
                "verification_uri",
                serde_json::json!("https://github.com/login/device#code"),
            ),
            (
                "verification_uri",
                serde_json::json!("https://token@github.com/login/device"),
            ),
            (
                "verification_uri",
                serde_json::json!("https://github.com/login/other"),
            ),
            ("user_code", serde_json::json!("ABCD\n1234")),
            ("device_code", serde_json::json!("")),
            ("expires_in", serde_json::json!(901)),
            ("expires_in", serde_json::json!(0)),
            ("interval", serde_json::json!(0)),
            ("interval", serde_json::json!(901)),
        ] {
            let mut response: serde_json::Value = serde_json::from_slice(&device()).unwrap();
            response[key] = value;
            assert!(matches!(
                GitHubDeviceFlow::begin_with(
                    "https://github.com",
                    "public-id",
                    false,
                    &CancellationToken::new(),
                    &Tokens(|_: &TokenRequest<'_>, _: &CancellationToken| Ok(bytes(
                        response.clone()
                    ))),
                    &Clock::new()
                ),
                Err(RemoteError::InvalidResponse)
            ));
        }
        assert!(matches!(
            GitHubDeviceFlow::begin_with(
                "https://github.com",
                "public-id",
                false,
                &CancellationToken::new(),
                &Tokens(
                    |_: &TokenRequest<'_>, _: &CancellationToken| Ok(Zeroizing::new(vec![
                        b'x';
                        MAX_TOKEN_BYTES
                            + 1
                    ]))
                ),
                &Clock::new()
            ),
            Err(RemoteError::ResponseTooLarge)
        ));
    }

    #[test]
    fn token_variants_accept_only_requested_scopes_and_complete_refresh_tuple() {
        for private in [false, true] {
            let expected = if private { token(true) } else { token(false) };
            assert!(parse_credentials(
                &expected,
                token_endpoint("https://github.com").unwrap(),
                "public-id".into(),
                private
            )
            .is_ok());
            for scope in ["gist", "repo gist", "repo,offline_access", "repo,repo"] {
                let mut response: serde_json::Value = serde_json::from_slice(&expected).unwrap();
                response["scope"] = serde_json::json!(scope);
                assert!(parse_credentials(
                    &bytes(response),
                    token_endpoint("https://github.com").unwrap(),
                    "public-id".into(),
                    private
                )
                .is_err());
            }
        }
        for (key, value) in [
            ("refresh_token", serde_json::json!(null)),
            ("refresh_token", serde_json::json!("")),
            ("refresh_token_expires_in", serde_json::json!(null)),
            ("expires_in", serde_json::json!(0)),
            ("token_type", serde_json::json!("basic")),
            ("access_token", serde_json::json!("secret\r\n")),
            ("scope", serde_json::json!("")),
        ] {
            let mut response: serde_json::Value = serde_json::from_slice(&token(true)).unwrap();
            response[key] = value;
            assert!(parse_credentials(
                &bytes(response),
                token_endpoint("https://github.com").unwrap(),
                "public-id".into(),
                true
            )
            .is_err());
        }
        let mut response: serde_json::Value = serde_json::from_slice(&token(true)).unwrap();
        response.as_object_mut().unwrap().remove("scope");
        assert!(parse_credentials(
            &bytes(response),
            token_endpoint("https://github.com").unwrap(),
            "public-id".into(),
            true
        )
        .is_ok());
        let response = bytes(
            serde_json::json!({"error":"authorization_pending", "access_token":"secret-access"}),
        );
        assert!(matches!(
            parse_credentials(
                &response,
                token_endpoint("https://github.com").unwrap(),
                "public-id".into(),
                false
            ),
            Err(RemoteError::InvalidResponse)
        ));
    }

    #[test]
    fn vault_credentials_are_context_bound_and_never_interpreted_as_a_personal_token() {
        let vault = Vault::default();
        let credentials = credentials(false);
        credentials
            .store(&vault, "personal", "github-account")
            .unwrap();
        let never = Tokens(|_: &TokenRequest<'_>, _: &CancellationToken| {
            panic!("nonexpiring token refreshed")
        });
        assert_eq!(
            read_credentials_with(
                &vault,
                "personal",
                &account(),
                &CancellationToken::new(),
                &never
            )
            .unwrap()
            .access
            .expose_secret(),
            "secret-access"
        );
        assert!(!serde_json::to_string(&account())
            .unwrap()
            .contains("secret-access"));
        let mut changed = account();
        changed.server_url = "https://api.github.com".into();
        assert!(read_credentials_with(
            &vault,
            "personal",
            &changed,
            &CancellationToken::new(),
            &never
        )
        .is_ok());
        changed = account();
        changed.server_url = "https://evil.test".into();
        assert!(matches!(
            read_credentials_with(
                &vault,
                "personal",
                &changed,
                &CancellationToken::new(),
                &never
            ),
            Err(RemoteError::InvalidConfiguration)
        ));
        changed = account();
        changed.authentication = Authentication::GitHubOAuth {
            client_id: "other-id".into(),
        };
        assert!(matches!(
            read_credentials_with(
                &vault,
                "personal",
                &changed,
                &CancellationToken::new(),
                &never
            ),
            Err(RemoteError::InvalidConfiguration)
        ));
        changed = account();
        changed.id = "other-account".into();
        assert!(matches!(
            read_credentials_with(
                &vault,
                "personal",
                &changed,
                &CancellationToken::new(),
                &never
            ),
            Err(RemoteError::InvalidConfiguration)
        ));
        assert!(matches!(
            read_credentials_with(
                &vault,
                "professional",
                &account(),
                &CancellationToken::new(),
                &never
            ),
            Err(RemoteError::InvalidConfiguration)
        ));
        changed = account();
        changed.authentication = Authentication::PersonalToken;
        assert!(matches!(
            oauth::account_client(&vault, "personal", &changed, &CancellationToken::new()),
            Err(RemoteError::InvalidConfiguration)
        ));
        changed = account();
        changed.provider = Provider::GitLab;
        assert_eq!(changed.validate(), Err(RemoteError::InvalidConfiguration));
    }

    #[test]
    fn expired_pair_refreshes_without_secret_and_preserves_rotation_after_cancellation() {
        let vault = Vault::default();
        let mut credentials = credentials(true);
        credentials.expires_at = Some(1);
        credentials
            .store(&vault, "personal", "github-account")
            .unwrap();
        let cancel = CancellationToken::new();
        let result = read_credentials_with(
            &vault,
            "personal",
            &account(),
            &cancel,
            &Tokens(|request: &TokenRequest<'_>, cancel: &CancellationToken| {
                assert_eq!(field(request, "grant_type"), Some("refresh_token"));
                assert_eq!(field(request, "refresh_token"), Some("secret-refresh"));
                assert_eq!(field(request, "client_secret"), None);
                assert_eq!(field(request, "scope"), None);
                let mut response: serde_json::Value = serde_json::from_slice(&token(true)).unwrap();
                response["access_token"] = serde_json::json!("rotated-access");
                response["refresh_token"] = serde_json::json!("rotated-refresh");
                cancel.cancel();
                Ok(bytes(response))
            }),
        );
        assert!(matches!(result, Err(RemoteError::Cancelled)));
        let restored = read_credentials_with(
            &vault,
            "personal",
            &account(),
            &CancellationToken::new(),
            &Tokens(|_: &TokenRequest<'_>, _: &CancellationToken| {
                panic!("fresh rotation refreshed")
            }),
        )
        .unwrap();
        assert_eq!(restored.access.expose_secret(), "rotated-access");
        assert_eq!(restored.refresh.unwrap().expose_secret(), "rotated-refresh");
    }

    #[test]
    fn logout_and_replacement_win_refresh_compare_and_swap() {
        for replace in [false, true] {
            let vault = Vault::default();
            let mut credentials = credentials(true);
            credentials.expires_at = Some(1);
            credentials
                .store(&vault, "personal", "github-account")
                .unwrap();
            let result = read_credentials_with(
                &vault,
                "personal",
                &account(),
                &CancellationToken::new(),
                &Tokens(|_: &TokenRequest<'_>, _: &CancellationToken| {
                    if replace {
                        vault
                            .store(
                                "personal",
                                "github-account",
                                &SecretToken::from("replacement"),
                            )
                            .unwrap();
                    } else {
                        vault.delete("personal", "github-account").unwrap();
                    }
                    Ok(token(true))
                }),
            );
            assert!(matches!(result, Err(RemoteError::CredentialsExpired)));
            assert_eq!(
                vault
                    .read("personal", "github-account")
                    .unwrap()
                    .as_ref()
                    .map(ExposeSecret::expose_secret),
                replace.then_some("replacement")
            );
        }
    }

    #[test]
    fn expired_refresh_token_fails_without_network_and_concurrent_reads_rotate_once() {
        let vault = Vault::default();
        let mut credentials = credentials(true);
        credentials.expires_at = Some(1);
        credentials.refresh_expires_at = Some(1);
        credentials
            .store(&vault, "personal", "github-account")
            .unwrap();
        assert!(matches!(
            read_credentials_with(
                &vault,
                "personal",
                &account(),
                &CancellationToken::new(),
                &Tokens(|_: &TokenRequest<'_>, _: &CancellationToken| panic!(
                    "expired refresh sent"
                ))
            ),
            Err(RemoteError::CredentialsExpired)
        ));
        credentials.refresh_expires_at = Some(oauth::now().unwrap() + 3600);
        credentials
            .store(&vault, "personal", "github-account")
            .unwrap();
        let calls = AtomicUsize::new(0);
        let transport = Tokens(|_: &TokenRequest<'_>, _: &CancellationToken| {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(token(true))
        });
        std::thread::scope(|scope| {
            let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
            for _ in 0..8 {
                let barrier = barrier.clone();
                let vault = &vault;
                let transport = &transport;
                scope.spawn(move || {
                    barrier.wait();
                    assert!(read_credentials_with(
                        vault,
                        "personal",
                        &account(),
                        &CancellationToken::new(),
                        transport
                    )
                    .is_ok());
                });
            }
        });
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    struct Api {
        body: Vec<u8>,
    }
    impl Transport for Api {
        fn get(&self, request: &HttpRequest, _: &CancellationToken) -> Result<HttpResponse> {
            assert_eq!(request.url.as_str(), "https://api.github.com/user");
            assert!(request
                .headers
                .iter()
                .any(|(name, value)| name.eq_ignore_ascii_case("authorization")
                    && value.expose_secret() == "Bearer secret-access"));
            Ok(HttpResponse {
                status: 200,
                headers: BTreeMap::new(),
                body: self.body.clone(),
            })
        }
    }
    #[test]
    fn provider_clients_shield_both_access_and_refresh_tokens_from_untrusted_json() {
        for secret in ["secret-access", "secret-refresh"] {
            let client = credentials(true)
                .provider_client(
                    Api {
                        body: format!("{{\"login\":\"{secret}\"}}").into_bytes(),
                    },
                    account(),
                )
                .unwrap();
            assert_eq!(
                client.account_identity(&CancellationToken::new()),
                Err(RemoteError::InvalidResponse)
            );
        }
        let client = credentials(true)
            .provider_client(
                Api {
                    body: br#"{"login":"alice","name":"Alice"}"#.to_vec(),
                },
                account(),
            )
            .unwrap();
        assert_eq!(
            client
                .account_identity(&CancellationToken::new())
                .unwrap()
                .unwrap()
                .username,
            "alice"
        );
    }
}
