//! Bitbucket Cloud browser authorization with a user-configured OAuth consumer.
//! Cloud requires its consumer secret for code exchange and refresh; it is never
//! embedded in the application or serialized into account preferences. Blocking
//! entry points belong on a background executor.
use std::{net::TcpListener, time::Instant};

use secrecy::zeroize::{Zeroize, Zeroizing};
use serde::{Deserialize, Serialize};
use url::Url;

use crate::{
    oauth::{self, HttpTokenTransport, TokenRequest, TokenTransport, MAX_TOKEN_BYTES},
    Account, Authentication, CancellationToken, CredentialVault, ExposeSecret, Provider,
    ProviderClient, RemoteError, Result, SafeApiUrl, SecretToken, Transport,
};

pub(crate) const TOKEN_ENDPOINT: &str = "https://bitbucket.org/site/oauth2/access_token";
const VAULT_PREFIX: &str = "mangrove-bitbucket-oauth-v1:";
const MAX_ACCESS_LIFETIME: u64 = 24 * 3600;

/// Consumer registration belongs to workspace settings, not personal settings.
pub fn application_settings_url(server_url: &str) -> Result<String> {
    let workspace = cloud_workspace(server_url)?;
    Ok(format!(
        "https://bitbucket.org/{workspace}/workspace/settings/api"
    ))
}

pub(crate) fn cloud_workspace(server_url: &str) -> Result<String> {
    SafeApiUrl::new(server_url, "")?;
    let server = Url::parse(server_url).map_err(|_| RemoteError::InvalidConfiguration)?;
    if server.port().is_some() {
        return Err(RemoteError::InvalidConfiguration);
    }
    let path = server.path().trim_matches('/');
    let workspace = match server.host_str() {
        Some("bitbucket.org") => path,
        Some("api.bitbucket.org") => path
            .strip_prefix("2.0/repositories/")
            .ok_or(RemoteError::InvalidConfiguration)?,
        _ => return Err(RemoteError::InvalidConfiguration),
    };
    // Workspace slugs are one ASCII segment. Reject encoded delimiters and
    // deployment paths rather than constructing a registration URL from them.
    if workspace.is_empty()
        || workspace.len() > 256
        || !workspace
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(RemoteError::InvalidConfiguration);
    }
    Ok(workspace.into())
}

pub struct BitbucketOAuth {
    listener: TcpListener,
    authorization_url: Zeroizing<String>,
    client_id: String,
    consumer_secret: SecretToken,
    workspace: String,
    state: SecretToken,
    deadline: Instant,
}
impl BitbucketOAuth {
    pub fn begin(server_url: &str, client_id: &str, consumer_secret: SecretToken) -> Result<Self> {
        oauth::validate_client_id(client_id)?;
        if !oauth::valid_secret(consumer_secret.expose_secret()) {
            return Err(RemoteError::InvalidConfiguration);
        }
        let workspace = cloud_workspace(server_url)?;
        let state = oauth::random_secret()?;
        let mut authorize =
            SafeApiUrl::new("https://bitbucket.org", "site/oauth2")?.url("authorize")?;
        authorize
            .query_pairs_mut()
            .append_pair("client_id", client_id)
            .append_pair("redirect_uri", oauth::REDIRECT_URI)
            .append_pair("response_type", "code")
            .append_pair("state", state.expose_secret());
        // Cloud grants the consumer's configured permissions. A scope query
        // cannot reduce them; instructions require account/repository/pullrequest.
        let listener = oauth::callback_listener()?;
        Ok(Self {
            listener,
            authorization_url: Zeroizing::new(authorize.into()),
            client_id: client_id.into(),
            consumer_secret,
            workspace,
            state,
            deadline: Instant::now() + oauth::AUTHORIZATION_TIMEOUT,
        })
    }

    pub fn authorization_url(&self) -> &str {
        &self.authorization_url
    }

    pub fn finish(self, cancel: &CancellationToken) -> Result<BitbucketCredentials> {
        self.finish_with(cancel, &HttpTokenTransport)
    }

    fn finish_with(
        self,
        cancel: &CancellationToken,
        transport: &dyn TokenTransport,
    ) -> Result<BitbucketCredentials> {
        let code = oauth::authorization_code(self.listener, &self.state, self.deadline, cancel)?;
        cancel.check()?;
        let endpoint = Url::parse(TOKEN_ENDPOINT).map_err(|_| RemoteError::InvalidConfiguration)?;
        let bytes = transport.post(
            &TokenRequest {
                endpoint: &endpoint,
                basic_auth: Some((&self.client_id, &self.consumer_secret)),
                fields: vec![
                    ("grant_type", SecretToken::from("authorization_code")),
                    ("code", code),
                    ("redirect_uri", SecretToken::from(oauth::REDIRECT_URI)),
                ],
            },
            cancel,
        )?;
        cancel.check()?;
        parse_credentials(&bytes, self.client_id, self.consumer_secret, self.workspace)
    }
}

pub struct BitbucketCredentials {
    access: SecretToken,
    refresh: SecretToken,
    consumer_secret: SecretToken,
    expires_at: u64,
    client_id: String,
    workspace: String,
}
impl BitbucketCredentials {
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
            != (Authentication::BitbucketOAuth {
                client_id: self.client_id.clone(),
            })
            || cloud_workspace(&account.server_url)? != self.workspace
        {
            return Err(RemoteError::InvalidConfiguration);
        }
        Ok(
            ProviderClient::new(transport, account, self.access_token())?
                .protect_secret(&self.refresh)
                .protect_secret(&self.consumer_secret),
        )
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
            refresh_token: &'a str,
            consumer_secret: &'a str,
            expires_at: u64,
            client_id: &'a str,
            workspace: &'a str,
            profile_id: &'a str,
            account_id: &'a str,
        }
        let json = Zeroizing::new(
            serde_json::to_string(&Stored {
                access_token: self.access.expose_secret(),
                refresh_token: self.refresh.expose_secret(),
                consumer_secret: self.consumer_secret.expose_secret(),
                expires_at: self.expires_at,
                client_id: &self.client_id,
                workspace: &self.workspace,
                profile_id,
                account_id,
            })
            .map_err(|_| RemoteError::InvalidResponse)?,
        );
        check_size(json.len().saturating_add(VAULT_PREFIX.len()))?;
        Ok(SecretToken::from(format!(
            "{VAULT_PREFIX}{}",
            json.as_str()
        )))
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredCredentials {
    access_token: String,
    refresh_token: String,
    consumer_secret: String,
    expires_at: u64,
    client_id: String,
    workspace: String,
    profile_id: String,
    account_id: String,
}
impl Drop for StoredCredentials {
    fn drop(&mut self) {
        self.access_token.zeroize();
        self.refresh_token.zeroize();
        self.consumer_secret.zeroize();
    }
}
pub(crate) fn is_credential_packet(token: &SecretToken) -> bool {
    token.expose_secret().starts_with(VAULT_PREFIX)
}

fn decode_credentials(
    packet: &SecretToken,
    account: &Account,
    profile_id: &str,
) -> Result<BitbucketCredentials> {
    check_size(packet.expose_secret().len())?;
    let json = packet
        .expose_secret()
        .strip_prefix(VAULT_PREFIX)
        .ok_or(RemoteError::CredentialsExpired)?;
    let stored: StoredCredentials =
        serde_json::from_str(json).map_err(|_| RemoteError::CredentialsExpired)?;
    let Authentication::BitbucketOAuth { client_id } = &account.authentication else {
        return Err(RemoteError::InvalidConfiguration);
    };
    let workspace = cloud_workspace(&account.server_url)?;
    if account.provider != Provider::BitbucketCloud
        || &stored.client_id != client_id
        || stored.workspace != workspace
        || stored.profile_id != profile_id
        || stored.account_id != account.id
        || !oauth::valid_secret(&stored.access_token)
        || !oauth::valid_secret(&stored.refresh_token)
        || !oauth::valid_secret(&stored.consumer_secret)
        || stored.expires_at == 0
    {
        return Err(RemoteError::InvalidConfiguration);
    }
    Ok(BitbucketCredentials {
        access: SecretToken::from(stored.access_token.as_str()),
        refresh: SecretToken::from(stored.refresh_token.as_str()),
        consumer_secret: SecretToken::from(stored.consumer_secret.as_str()),
        expires_at: stored.expires_at,
        client_id: client_id.clone(),
        workspace,
    })
}

pub(crate) fn read_credentials_with(
    vault: &dyn CredentialVault,
    profile_id: &str,
    account: &Account,
    cancel: &CancellationToken,
    transport: &dyn TokenTransport,
) -> Result<BitbucketCredentials> {
    account.validate()?;
    crate::model::validate_id(profile_id)?;
    cancel.check()?;
    let lock = oauth::refresh_lock(profile_id, &account.id)?;
    let _guard = oauth::lock_refresh(&lock, cancel)?;
    let previous = vault
        .read(profile_id, &account.id)?
        .ok_or(RemoteError::CredentialsExpired)?;
    let credentials = decode_credentials(&previous, account, profile_id)?;
    if credentials.expires_at > oauth::now()?.saturating_add(30) {
        cancel.check()?;
        return Ok(credentials);
    }
    let endpoint = Url::parse(TOKEN_ENDPOINT).map_err(|_| RemoteError::InvalidConfiguration)?;
    let bytes = transport.post(
        &TokenRequest {
            endpoint: &endpoint,
            basic_auth: Some((&credentials.client_id, &credentials.consumer_secret)),
            fields: vec![
                ("grant_type", SecretToken::from("refresh_token")),
                ("refresh_token", credentials.refresh.clone()),
            ],
        },
        cancel,
    )?;
    let rotated = parse_credentials(
        &bytes,
        credentials.client_id,
        credentials.consumer_secret,
        credentials.workspace,
    )?;
    // A received refresh rotation must survive view cancellation. Atomic
    // comparison prevents replacing a logged-out or newly connected account.
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
    refresh_token: String,
    token_type: String,
    expires_in: u64,
    #[serde(alias = "scopes")]
    scope: Option<String>,
}
impl Drop for TokenResponse {
    fn drop(&mut self) {
        self.access_token.zeroize();
        self.refresh_token.zeroize();
    }
}
fn check_size(size: usize) -> Result<()> {
    if size > MAX_TOKEN_BYTES {
        Err(RemoteError::ResponseTooLarge)
    } else {
        Ok(())
    }
}
fn parse_credentials(
    bytes: &[u8],
    client_id: String,
    consumer_secret: SecretToken,
    workspace: String,
) -> Result<BitbucketCredentials> {
    check_size(bytes.len())?;
    let response: TokenResponse =
        serde_json::from_slice(bytes).map_err(|_| RemoteError::InvalidResponse)?;
    // RFC 6749 allows scope omission when equal to the configured grant. When
    // returned, require identity/PR access and reject any additional permission.
    // pullrequest already implies repository in Bitbucket's OAuth scope model.
    let scopes_valid = response.scope.as_ref().is_none_or(|scope| {
        let scopes = scope.split_whitespace().collect::<Vec<_>>();
        scopes.contains(&"account")
            && scopes.contains(&"pullrequest")
            && scopes
                .iter()
                .all(|scope| matches!(*scope, "account" | "repository" | "pullrequest"))
    });
    if !response.token_type.eq_ignore_ascii_case("bearer")
        || !oauth::valid_secret(&response.access_token)
        || !oauth::valid_secret(&response.refresh_token)
        || response.expires_in == 0
        || response.expires_in > MAX_ACCESS_LIFETIME
        || !scopes_valid
    {
        return Err(RemoteError::InvalidResponse);
    }
    Ok(BitbucketCredentials {
        access: SecretToken::from(response.access_token.as_str()),
        refresh: SecretToken::from(response.refresh_token.as_str()),
        consumer_secret,
        expires_at: oauth::now()?
            .checked_add(response.expires_in)
            .ok_or(RemoteError::InvalidResponse)?,
        client_id,
        workspace,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{HttpRequest, HttpResponse};
    use std::{
        collections::BTreeMap,
        io::{Read, Write},
        net::Ipv4Addr,
        sync::{
            atomic::{AtomicUsize, Ordering},
            Arc, Mutex,
        },
        time::Duration,
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
            let mut current = self.0.lock().unwrap();
            if current
                .as_ref()
                .is_some_and(|current| current.expose_secret() == previous.expose_secret())
            {
                *current = Some(next.clone());
                Ok(true)
            } else {
                Ok(false)
            }
        }
    }
    fn bytes(value: serde_json::Value) -> Zeroizing<Vec<u8>> {
        Zeroizing::new(serde_json::to_vec(&value).unwrap())
    }
    fn token() -> Zeroizing<Vec<u8>> {
        bytes(serde_json::json!({
            "access_token":"fixture-access", "refresh_token":"fixture-refresh",
            "token_type":"bearer", "expires_in":3600, "scopes":"account repository pullrequest"
        }))
    }
    fn account() -> Account {
        Account {
            id: "bitbucket-account".into(),
            label: "Bitbucket".into(),
            provider: Provider::BitbucketCloud,
            server_url: "https://bitbucket.org/workspace".into(),
            username: None,
            authentication: Authentication::BitbucketOAuth {
                client_id: "public-id".into(),
            },
        }
    }
    fn credentials() -> BitbucketCredentials {
        parse_credentials(
            &token(),
            "public-id".into(),
            SecretToken::from("fixture-consumer-secret"),
            "workspace".into(),
        )
        .unwrap()
    }
    fn field<'a>(request: &'a TokenRequest<'_>, key: &str) -> Option<&'a str> {
        request
            .fields
            .iter()
            .find(|(name, _)| *name == key)
            .map(|(_, value)| value.expose_secret())
    }

    #[test]
    fn configuration_rejects_foreign_origins_paths_secrets_and_other_editions() {
        for server in [
            "https://bitbucket.org/workspace",
            "https://api.bitbucket.org/2.0/repositories/workspace/",
        ] {
            assert_eq!(
                application_settings_url(server).unwrap(),
                "https://bitbucket.org/workspace/workspace/settings/api"
            );
        }
        for server in [
            "http://bitbucket.org/workspace",
            "https://secret@bitbucket.org/workspace",
            "https://evil.example/workspace",
            "https://bitbucket.org:8443/workspace",
            "https://bitbucket.org",
            "https://api.bitbucket.org/workspace",
            "https://api.bitbucket.org/2.0/repositories/workspace/extra",
            "https://bitbucket.org/workspace?token=secret",
            "https://bitbucket.org/workspace#secret",
            "https://bitbucket.org/workspace%2fextra",
            "https://bitbucket.org/a/b",
        ] {
            assert!(application_settings_url(server).is_err());
            let mut changed = account();
            changed.server_url = server.into();
            assert_eq!(changed.validate(), Err(RemoteError::InvalidConfiguration));
        }
        for provider in [
            Provider::GitHub,
            Provider::GitLab,
            Provider::BitbucketServer,
        ] {
            let mut changed = account();
            changed.provider = provider;
            assert_eq!(changed.validate(), Err(RemoteError::InvalidConfiguration));
        }
        assert!(matches!(
            BitbucketOAuth::begin(
                "https://bitbucket.org/workspace",
                "public-id",
                SecretToken::from("")
            ),
            Err(RemoteError::InvalidConfiguration)
        ));
    }

    #[test]
    fn token_grants_require_identity_and_pr_access_without_added_permissions() {
        for scope in [
            "account repository pullrequest",
            "pullrequest account",
            "repository account pullrequest",
        ] {
            let mut response: serde_json::Value = serde_json::from_slice(&token()).unwrap();
            response["scopes"] = serde_json::json!(scope);
            assert!(parse_credentials(
                &bytes(response),
                "id".into(),
                SecretToken::from("secret"),
                "workspace".into()
            )
            .is_ok());
        }
        for (key, value) in [
            (
                "scopes",
                serde_json::json!("account repository pullrequest repository:write"),
            ),
            (
                "scopes",
                serde_json::json!("account repository pullrequest repository:admin"),
            ),
            (
                "scopes",
                serde_json::json!("account repository pullrequest email"),
            ),
            (
                "scopes",
                serde_json::json!("account repository pullrequest webhook"),
            ),
            ("scopes", serde_json::json!("repository pullrequest")),
            ("scopes", serde_json::json!("account repository")),
            ("scopes", serde_json::json!("")),
            ("token_type", serde_json::json!("basic")),
            ("access_token", serde_json::json!("secret\r\n")),
            ("refresh_token", serde_json::json!("")),
            ("refresh_token", serde_json::json!("x".repeat(4097))),
            ("expires_in", serde_json::json!(0)),
            ("expires_in", serde_json::json!(u64::MAX)),
        ] {
            let mut response: serde_json::Value = serde_json::from_slice(&token()).unwrap();
            response[key] = value;
            assert!(parse_credentials(
                &bytes(response),
                "id".into(),
                SecretToken::from("secret"),
                "workspace".into()
            )
            .is_err());
        }
        let mut response: serde_json::Value = serde_json::from_slice(&token()).unwrap();
        response.as_object_mut().unwrap().remove("scopes");
        assert!(parse_credentials(
            &bytes(response.clone()),
            "id".into(),
            SecretToken::from("secret"),
            "workspace".into()
        )
        .is_ok());
        response["scope"] = serde_json::json!("account repository pullrequest:write");
        assert!(parse_credentials(
            &bytes(response),
            "id".into(),
            SecretToken::from("secret"),
            "workspace".into()
        )
        .is_err());
        assert!(matches!(
            parse_credentials(
                &vec![b' '; MAX_TOKEN_BYTES + 1],
                "id".into(),
                SecretToken::from("secret"),
                "workspace".into()
            ),
            Err(RemoteError::ResponseTooLarge)
        ));
    }

    #[test]
    fn vault_packet_cannot_move_between_profiles_accounts_workspaces_or_authentication_modes() {
        let vault = Vault::default();
        credentials()
            .store(&vault, "personal", "bitbucket-account")
            .unwrap();
        let never = Tokens(|_: &TokenRequest<'_>, _: &CancellationToken| {
            panic!("fresh or mismatched credentials contacted server")
        });
        let cancel = CancellationToken::new();
        let current =
            read_credentials_with(&vault, "personal", &account(), &cancel, &never).unwrap();
        assert_eq!(current.access.expose_secret(), "fixture-access");
        for changed in [
            Account {
                id: "different-account".into(),
                ..account()
            },
            Account {
                server_url: "https://bitbucket.org/another-workspace".into(),
                ..account()
            },
            Account {
                authentication: Authentication::BitbucketOAuth {
                    client_id: "different-client".into(),
                },
                ..account()
            },
        ] {
            assert!(matches!(
                read_credentials_with(&vault, "personal", &changed, &cancel, &never),
                Err(RemoteError::InvalidConfiguration)
            ));
        }
        assert!(matches!(
            read_credentials_with(&vault, "professional", &account(), &cancel, &never),
            Err(RemoteError::InvalidConfiguration)
        ));
        let changed = Account {
            authentication: Authentication::PersonalToken,
            ..account()
        };
        assert!(matches!(
            oauth::account_client(&vault, "personal", &changed, &cancel),
            Err(RemoteError::InvalidConfiguration)
        ));
        let metadata = serde_json::to_string(&account()).unwrap();
        for secret in [
            "fixture-access",
            "fixture-refresh",
            "fixture-consumer-secret",
        ] {
            assert!(!metadata.contains(secret));
        }
    }

    #[test]
    fn refresh_rotation_is_single_and_retained_after_view_cancel_while_logout_wins() {
        let vault = Vault::default();
        let mut expired = credentials();
        expired.expires_at = 1;
        expired
            .store(&vault, "personal", "bitbucket-account")
            .unwrap();
        let calls = AtomicUsize::new(0);
        let cancel = CancellationToken::new();
        let tokens = Tokens(|request: &TokenRequest<'_>, cancel: &CancellationToken| {
            calls.fetch_add(1, Ordering::SeqCst);
            let (key, secret) = request.basic_auth.unwrap();
            assert_eq!(key, "public-id");
            assert_eq!(secret.expose_secret(), "fixture-consumer-secret");
            assert_eq!(field(request, "grant_type"), Some("refresh_token"));
            assert_eq!(field(request, "refresh_token"), Some("fixture-refresh"));
            assert_eq!(field(request, "client_secret"), None);
            cancel.cancel();
            let mut response: serde_json::Value = serde_json::from_slice(&token()).unwrap();
            response["access_token"] = serde_json::json!("rotated-access");
            response["refresh_token"] = serde_json::json!("rotated-refresh");
            Ok(bytes(response))
        });
        assert!(matches!(
            read_credentials_with(&vault, "personal", &account(), &cancel, &tokens),
            Err(RemoteError::Cancelled)
        ));
        let fresh = read_credentials_with(
            &vault,
            "personal",
            &account(),
            &CancellationToken::new(),
            &tokens,
        )
        .unwrap();
        assert_eq!(fresh.access.expose_secret(), "rotated-access");
        assert_eq!(fresh.refresh.expose_secret(), "rotated-refresh");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        expired
            .store(&vault, "personal", "bitbucket-account")
            .unwrap();
        let delete = Tokens(|_: &TokenRequest<'_>, _: &CancellationToken| {
            vault.delete("personal", "bitbucket-account").unwrap();
            Ok(token())
        });
        assert!(matches!(
            read_credentials_with(
                &vault,
                "personal",
                &account(),
                &CancellationToken::new(),
                &delete
            ),
            Err(RemoteError::CredentialsExpired)
        ));
        assert!(vault
            .read("personal", "bitbucket-account")
            .unwrap()
            .is_none());
    }

    #[test]
    fn concurrent_reads_share_one_refresh_and_cancelled_reads_do_not_contact_server() {
        let vault = Arc::new(Vault::default());
        let mut expired = credentials();
        expired.expires_at = 1;
        expired
            .store(vault.as_ref(), "personal", "bitbucket-account")
            .unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        std::thread::scope(|scope| {
            for _ in 0..4 {
                let vault = vault.clone();
                let calls = calls.clone();
                scope.spawn(move || {
                    let tokens = Tokens(|_: &TokenRequest<'_>, _: &CancellationToken| {
                        calls.fetch_add(1, Ordering::SeqCst);
                        std::thread::sleep(Duration::from_millis(20));
                        Ok(token())
                    });
                    assert!(read_credentials_with(
                        vault.as_ref(),
                        "personal",
                        &account(),
                        &CancellationToken::new(),
                        &tokens
                    )
                    .is_ok());
                });
            }
        });
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        let cancel = CancellationToken::new();
        cancel.cancel();
        let never = Tokens(|_: &TokenRequest<'_>, _: &CancellationToken| {
            panic!("cancelled refresh contacted server")
        });
        assert!(matches!(
            read_credentials_with(vault.as_ref(), "personal", &account(), &cancel, &never),
            Err(RemoteError::Cancelled)
        ));
    }

    #[test]
    fn real_loopback_ignores_wrong_state_and_sends_secret_only_at_token_exchange() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        listener.set_nonblocking(true).unwrap();
        let address = listener.local_addr().unwrap();
        let flow = BitbucketOAuth {
            listener,
            authorization_url: Zeroizing::new(String::new()),
            client_id: "public-id".into(),
            consumer_secret: SecretToken::from("fixture-consumer-secret"),
            workspace: "workspace".into(),
            state: SecretToken::from("expected-state"),
            deadline: Instant::now() + Duration::from_secs(5),
        };
        let worker = std::thread::spawn(move || {
            flow.finish_with(
                &CancellationToken::new(),
                &Tokens(|request: &TokenRequest<'_>, _: &CancellationToken| {
                    let (key, secret) = request.basic_auth.unwrap();
                    assert_eq!(key, "public-id");
                    assert_eq!(secret.expose_secret(), "fixture-consumer-secret");
                    assert_eq!(field(request, "code"), Some("fixture-code"));
                    assert_eq!(field(request, "redirect_uri"), Some(oauth::REDIRECT_URI));
                    assert_eq!(field(request, "grant_type"), Some("authorization_code"));
                    assert_eq!(field(request, "client_secret"), None);
                    Ok(token())
                }),
            )
        });
        for (state, status) in [("wrong-state", "400"), ("expected-state", "200")] {
            let mut stream = std::net::TcpStream::connect(address).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            write!(stream,"GET /oauth/callback?code=fixture-code&state={state} HTTP/1.1\r\nHost: 127.0.0.1:47321\r\n\r\n").unwrap();
            let mut response = String::new();
            stream.read_to_string(&mut response).unwrap();
            assert!(response.starts_with(&format!("HTTP/1.1 {status}")));
            assert!(!response.contains("fixture-code"));
            assert!(!response.contains("fixture-consumer-secret"));
        }
        assert_eq!(
            worker.join().unwrap().unwrap().access.expose_secret(),
            "fixture-access"
        );
    }

    #[test]
    fn oauth_identity_uses_bearer_even_with_username_and_masks_all_credential_echoes() {
        struct Identity(bool);
        impl Transport for Identity {
            fn get(&self, request: &HttpRequest, _: &CancellationToken) -> Result<HttpResponse> {
                assert_eq!(request.url.as_str(), "https://api.bitbucket.org/2.0/user");
                assert_eq!(
                    request
                        .headers
                        .iter()
                        .find(|(name, _)| name == "Authorization")
                        .unwrap()
                        .1
                        .expose_secret(),
                    "Bearer fixture-access"
                );
                Ok(HttpResponse {status:200,headers:BTreeMap::new(),body:serde_json::to_vec(&if self.0 {
                    serde_json::json!({"nickname":"alice","display_name":"Alice QA","uuid":"{uuid}"})
                } else {
                    serde_json::json!({"display_name":"Alice QA","uuid":"{uuid}"})
                }).unwrap()})
            }
        }
        let cancel = CancellationToken::new();
        for nickname in [true, false] {
            let account = Account {
                username: Some("prior-identity".into()),
                ..account()
            };
            let identity = credentials()
                .provider_client(Identity(nickname), account)
                .unwrap()
                .account_identity(&cancel)
                .unwrap()
                .unwrap();
            assert_eq!(identity.username, if nickname { "alice" } else { "{uuid}" });
            assert_eq!(identity.display_name, "Alice QA");
        }
        struct Echo;
        impl Transport for Echo {
            fn get(&self, _: &HttpRequest, _: &CancellationToken) -> Result<HttpResponse> {
                Ok(HttpResponse {status:200,headers:BTreeMap::new(),body:serde_json::to_vec(&serde_json::json!({"values":[{
                    "uuid":"{repo}","name":"fixture-consumer-secret and fixture-refresh",
                    "full_name":"workspace/fixture-consumer-secret","links":{
                        "html":{"href":"https://bitbucket.org/workspace/fixture-consumer-secret"}
                    }
                }]})).unwrap()})
            }
        }
        let page = credentials()
            .provider_client(Echo, account())
            .unwrap()
            .repositories(None, &cancel)
            .unwrap();
        let rendered = format!("{page:?}");
        assert!(!rendered.contains("fixture-consumer-secret"));
        assert!(!rendered.contains("fixture-refresh"));
        assert!(page.items[0].web_url.is_none());
    }

    #[test]
    fn consumer_basic_auth_never_leaves_the_fixed_bitbucket_token_endpoint() {
        let secret = SecretToken::from("fixture-consumer-secret");
        for endpoint in [
            "https://evil.example/site/oauth2/access_token",
            "https://bitbucket.org/site/oauth2/authorize",
            "https://api.bitbucket.org/site/oauth2/access_token",
            "https://bitbucket.org/site/oauth2/access_token?redirect=evil",
            "https://bitbucket.org:8443/site/oauth2/access_token",
            "http://bitbucket.org/site/oauth2/access_token",
        ] {
            let endpoint = Url::parse(endpoint).unwrap();
            assert!(matches!(
                HttpTokenTransport.post(
                    &TokenRequest {
                        endpoint: &endpoint,
                        basic_auth: Some(("public-id", &secret)),
                        fields: vec![("grant_type", SecretToken::from("refresh_token"))],
                    },
                    &CancellationToken::new()
                ),
                Err(RemoteError::InvalidConfiguration)
            ));
        }
    }

    #[test]
    fn failed_refresh_does_not_replace_the_account_credentials() {
        let vault = Vault::default();
        let mut expired = credentials();
        expired.expires_at = 1;
        expired
            .store(&vault, "personal", "bitbucket-account")
            .unwrap();
        let previous = vault
            .read("personal", "bitbucket-account")
            .unwrap()
            .unwrap();
        for error in [
            RemoteError::Network,
            RemoteError::Timeout,
            RemoteError::CredentialsExpired,
        ] {
            let fail = Tokens(|_: &TokenRequest<'_>, _: &CancellationToken| Err(error.clone()));
            assert!(
                matches!(read_credentials_with(&vault,"personal",&account(),&CancellationToken::new(),&fail),Err(actual) if actual==error)
            );
            let current = vault
                .read("personal", "bitbucket-account")
                .unwrap()
                .unwrap();
            assert_eq!(current.expose_secret(), previous.expose_secret());
        }
    }
}
