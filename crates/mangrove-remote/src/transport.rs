use std::{collections::BTreeMap, fmt, time::Duration};

use base64::{engine::general_purpose::STANDARD, Engine};
use reqwest::{
    header::{HeaderName, HeaderValue},
    redirect::Policy,
    Client,
};
use secrecy::zeroize::{Zeroize, Zeroizing};
use url::Url;

use crate::{CancellationToken, ExposeSecret, RemoteError, Result, SecretToken};

pub const MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

pub struct HttpRequest {
    pub url: Url,
    pub headers: Vec<(String, SecretToken)>,
}

impl fmt::Debug for HttpRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpRequest")
            .field("url", &"[REDACTED]")
            .field("header_count", &self.headers.len())
            .finish()
    }
}

pub struct HttpResponse {
    pub status: u16,
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
}

impl fmt::Debug for HttpResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HttpResponse")
            .field("status", &self.status)
            .field("body_bytes", &self.body.len())
            .finish_non_exhaustive()
    }
}

/// A deliberately GET-only boundary, mockable without network access.
pub trait Transport: Send + Sync {
    fn get(&self, request: &HttpRequest, cancel: &CancellationToken) -> Result<HttpResponse>;
}

/// Run `get` on the caller's background executor. Cancellation drops the async
/// request/body future, rather than keeping a blocking socket until timeout.
pub struct HttpTransport;

impl HttpTransport {
    pub fn new() -> Result<Self> {
        Ok(Self)
    }
}

impl Transport for HttpTransport {
    fn get(&self, request: &HttpRequest, cancel: &CancellationToken) -> Result<HttpResponse> {
        cancel.check()?;
        validate_https(&request.url).map_err(|_| RemoteError::InvalidConfiguration)?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| RemoteError::Network)?;
        runtime.block_on(async {
            tokio::select! {
                biased;
                _ = cancel.cancelled() => Err(RemoteError::Cancelled),
                result = read_response(request) => {
                    cancel.check()?;
                    result
                },
            }
        })
    }
}

async fn read_response(request: &HttpRequest) -> Result<HttpResponse> {
    // Clients must live inside this runtime; no idle connection or TLS session
    // is retained across profiles, accounts or cancellation boundaries.
    let client = Client::builder()
        .https_only(true)
        .redirect(Policy::none())
        .referer(false)
        .timeout(REQUEST_TIMEOUT)
        .connect_timeout(Duration::from_secs(8))
        .user_agent("Mangrove/0.1 read-only")
        .build()
        .map_err(|_| RemoteError::Network)?;
    let mut builder = client.get(request.url.clone());
    for (name, value) in &request.headers {
        let name = HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| RemoteError::InvalidConfiguration)?;
        let mut value = HeaderValue::from_str(value.expose_secret())
            .map_err(|_| RemoteError::InvalidConfiguration)?;
        value.set_sensitive(true);
        builder = builder.header(name, value);
    }
    let mut response = builder.send().await.map_err(network_error)?;
    let status = response.status().as_u16();
    let mut headers = BTreeMap::new();
    // Forward only pagination/rate-limit headers, bounded independently of body.
    for name in [
        "link",
        "x-next-page",
        "x-ratelimit-remaining",
        "x-ratelimit-reset",
        "retry-after",
    ] {
        if let Some(value) = response.headers().get(name).and_then(|v| v.to_str().ok()) {
            if value.len() > 16 * 1024 {
                return Err(RemoteError::ResponseTooLarge);
            }
            headers.insert(name.into(), value.into());
        }
    }
    check_status(status, &headers)?;
    if response
        .content_length()
        .is_some_and(|n| n > MAX_RESPONSE_BYTES as u64)
    {
        return Err(RemoteError::ResponseTooLarge);
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(network_error)? {
        if body.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
            return Err(RemoteError::ResponseTooLarge);
        }
        body.extend_from_slice(&chunk);
    }
    reject_credential_echo(request, &headers, &mut body)?;
    Ok(HttpResponse {
        status,
        headers,
        body,
    })
}

// Hosting APIs may echo authorization strings in diagnostics, pagination or
// normal JSON fields. Reject those responses at the backend boundary, before
// any upstream content reaches a serializable model or the native UI.
fn reject_credential_echo(
    request: &HttpRequest,
    headers: &BTreeMap<String, String>,
    body: &mut Vec<u8>,
) -> Result<()> {
    let mut secrets = Vec::new();
    for (name, value) in &request.headers {
        let value = value.expose_secret();
        if name.eq_ignore_ascii_case("authorization") {
            secrets.push(SecretToken::from(value));
            if let Some(bearer) = value
                .strip_prefix("Bearer ")
                .or_else(|| value.strip_prefix("token "))
            {
                secrets.push(SecretToken::from(bearer));
            }
            if let Some(basic) = value.strip_prefix("Basic ") {
                secrets.push(SecretToken::from(basic));
                if let Ok(decoded) = STANDARD.decode(basic) {
                    let decoded = Zeroizing::new(decoded);
                    if let Ok(decoded) = std::str::from_utf8(&decoded) {
                        if let Some((_, token)) = decoded.split_once(':') {
                            secrets.push(SecretToken::from(token));
                        }
                    }
                }
            }
        } else if name.eq_ignore_ascii_case("private-token") {
            secrets.push(SecretToken::from(value));
        }
    }
    let contains = |value: &str| {
        secrets
            .iter()
            .any(|secret| contains_credential(value, secret.expose_secret()))
    };
    let unsafe_header = headers.values().any(|value| contains(value));
    let unsafe_raw = std::str::from_utf8(body).map_or_else(
        |_| {
            secrets.iter().any(|secret| {
                let bytes = secret.expose_secret().as_bytes();
                !bytes.is_empty() && body.windows(bytes.len()).any(|window| window == bytes)
            })
        },
        contains,
    );
    let unsafe_json = if !unsafe_raw {
        if let Ok(value) = serde_json::from_slice::<serde_json::Value>(body) {
            let unsafe_value = json_contains(&value, &contains);
            zeroize_json(value);
            unsafe_value
        } else {
            false
        }
    } else {
        false
    };
    if unsafe_header || unsafe_raw || unsafe_json {
        body.zeroize();
        Err(RemoteError::InvalidResponse)
    } else {
        Ok(())
    }
}

/// Also covers partially escaped URLs and either case of hexadecimal digits.
/// Two passes cover an encoded URL embedded inside another URL without allowing
/// an unbounded decoding loop on upstream content.
pub(crate) fn contains_credential(value: &str, secret: &str) -> bool {
    if secret.is_empty() {
        return false;
    }
    if value.contains(secret) {
        return true;
    }
    if !value.as_bytes().contains(&b'%') {
        return false;
    }
    let mut decoded = Zeroizing::new(value.as_bytes().to_vec());
    for _ in 0..2 {
        if !decoded.contains(&b'%') {
            break;
        }
        let mut read = 0;
        let mut write = 0;
        while read < decoded.len() {
            if decoded[read] == b'%' && read + 2 < decoded.len() {
                if let (Some(hi), Some(lo)) = (hex(decoded[read + 1]), hex(decoded[read + 2])) {
                    decoded[write] = hi * 16 + lo;
                    read += 3;
                    write += 1;
                    continue;
                }
            }
            decoded[write] = decoded[read];
            read += 1;
            write += 1;
        }
        decoded[write..].zeroize();
        decoded.truncate(write);
        if decoded
            .windows(secret.len())
            .any(|window| window == secret.as_bytes())
        {
            return true;
        }
    }
    false
}

fn json_contains(value: &serde_json::Value, contains: &impl Fn(&str) -> bool) -> bool {
    match value {
        serde_json::Value::String(value) => contains(value),
        serde_json::Value::Array(values) => {
            values.iter().any(|value| json_contains(value, contains))
        }
        serde_json::Value::Object(values) => values
            .iter()
            .any(|(key, value)| contains(key) || json_contains(value, contains)),
        _ => false,
    }
}

fn zeroize_json(value: serde_json::Value) {
    match value {
        serde_json::Value::String(mut value) => value.zeroize(),
        serde_json::Value::Array(values) => values.into_iter().for_each(zeroize_json),
        serde_json::Value::Object(values) => {
            for (mut key, value) in values {
                key.zeroize();
                zeroize_json(value);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(header: &str, value: &str) -> HttpRequest {
        HttpRequest {
            url: Url::parse("https://git.example/api/v4/projects").unwrap(),
            headers: vec![(header.into(), SecretToken::from(value))],
        }
    }

    #[test]
    fn debug_redacts_urls_headers_and_upstream_bodies() {
        let mut request = request("authorization", "Bearer secret-fixture");
        request.url.set_query(Some("access_token=secret-fixture"));
        assert!(!format!("{request:?}").contains("secret-fixture"));
        let response = HttpResponse {
            status: 200,
            headers: BTreeMap::from([("link".into(), "secret-fixture".into())]),
            body: b"secret-fixture".to_vec(),
        };
        assert!(!format!("{response:?}").contains("secret-fixture"));
        assert!(!format!("{:?}", request.headers[0].1).contains("secret-fixture"));
    }

    #[test]
    fn pagination_cannot_move_credentials_to_another_origin_or_escape_api() {
        let api = SafeApiUrl::new("https://git.example/deployment", "api/v4").unwrap();
        let good = "https://git.example/deployment/api/v4/projects?page=2";
        assert_eq!(api.validate_next(good).unwrap().as_str(), good);
        assert!(api.url("projects/team%2Frepo").is_ok());
        for unsafe_url in [
            "https://evil.example/deployment/api/v4/projects",
            "http://git.example/deployment/api/v4/projects",
            "https://git.example:8443/deployment/api/v4/projects",
            "https://token@git.example/deployment/api/v4/projects",
            "https://git.example/deployment/api/v40/projects",
            "https://git.example/deployment/api/v4/projects#token",
            "https://git.example/deployment/api/v4/x%2f..%2f..%2fadmin",
            "https://git.example/deployment/api/v4/x%252f..%252fadmin",
            "https://git.example/deployment/api/v4/x%5c..%5cadmin",
        ] {
            assert_eq!(
                api.validate_next(unsafe_url),
                Err(RemoteError::UnsafePagination)
            );
        }
    }

    #[test]
    fn echo_in_body_header_or_json_escape_never_crosses_the_boundary() {
        let request = request("authorization", "Bearer secret-fixture");
        for body in [
            br#"{"title":"secret-fixture"}"#.as_slice(),
            br#"{"title":"secret\u002dfixture"}"#.as_slice(),
            br#"{"secret-fixture":"ordinary"}"#.as_slice(),
        ] {
            let mut body = body.to_vec();
            assert_eq!(
                reject_credential_echo(&request, &BTreeMap::new(), &mut body),
                Err(RemoteError::InvalidResponse)
            );
            assert!(body.iter().all(|b| *b == 0));
        }
        let mut body = b"[]".to_vec();
        let headers = BTreeMap::from([(
            "link".into(),
            "https://git.example/api/v4/projects?token=secret-fixture".into(),
        )]);
        assert_eq!(
            reject_credential_echo(&request, &headers, &mut body),
            Err(RemoteError::InvalidResponse)
        );
        let mut safe_body = br#"{"title":"ordinary repository"}"#.to_vec();
        assert!(reject_credential_echo(&request, &BTreeMap::new(), &mut safe_body).is_ok());
    }

    #[test]
    fn basic_and_gitlab_secret_echoes_are_rejected() {
        let basic = STANDARD.encode("user:secret-fixture");
        let basic_request = request("authorization", &format!("Basic {basic}"));
        for text in ["secret-fixture".to_owned(), basic] {
            let mut body = text.into_bytes();
            assert_eq!(
                reject_credential_echo(&basic_request, &BTreeMap::new(), &mut body),
                Err(RemoteError::InvalidResponse)
            );
        }
        let request = request("private-token", "gitlab-fixture");
        let mut body = b"gitlab-fixture".to_vec();
        assert_eq!(
            reject_credential_echo(&request, &BTreeMap::new(), &mut body),
            Err(RemoteError::InvalidResponse)
        );
    }

    #[test]
    fn percent_encoded_credential_echoes_never_cross_the_boundary() {
        let request = request("private-token", "+/=secret-fixture");
        for encoded in [
            "%2b%2f%3dsecret-fixture",
            "%2B/%3dsecret-fixture",
            "%252b%252f%253dsecret-fixture",
        ] {
            let mut body = format!(r#"{{"url":"https://git.example/{encoded}"}}"#).into_bytes();
            assert_eq!(
                reject_credential_echo(&request, &BTreeMap::new(), &mut body),
                Err(RemoteError::InvalidResponse)
            );
            assert!(body.iter().all(|byte| *byte == 0));
            let mut body = b"[]".to_vec();
            let headers = BTreeMap::from([("link".into(), encoded.into())]);
            assert_eq!(
                reject_credential_echo(&request, &headers, &mut body),
                Err(RemoteError::InvalidResponse)
            );
        }
        let mut ordinary = br#"{"name":"project%20name"}"#.to_vec();
        assert!(reject_credential_echo(&request, &BTreeMap::new(), &mut ordinary).is_ok());
    }

    #[test]
    fn cancelled_request_never_starts_network_or_runtime() {
        let cancellation = CancellationToken::new();
        cancellation.cancel();
        assert_eq!(
            HttpTransport::new()
                .unwrap()
                .get(&request("authorization", "Bearer fixture"), &cancellation)
                .unwrap_err(),
            RemoteError::Cancelled
        );
    }
}

fn network_error(error: reqwest::Error) -> RemoteError {
    if error.is_timeout() {
        RemoteError::Timeout
    } else {
        RemoteError::Network
    }
}

pub(crate) fn header<'a>(headers: &'a BTreeMap<String, String>, name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

pub(crate) fn check_status(status: u16, headers: &BTreeMap<String, String>) -> Result<()> {
    if status == 401 {
        return Err(RemoteError::CredentialsExpired);
    }
    if status == 429
        || (status == 403
            && (header(headers, "x-ratelimit-remaining") == Some("0")
                || header(headers, "retry-after").is_some()))
    {
        return Err(RemoteError::RateLimited {
            retry_after_seconds: header(headers, "retry-after").and_then(|v| v.parse().ok()),
        });
    }
    if !(200..300).contains(&status) {
        return Err(RemoteError::Http { status });
    }
    Ok(())
}

#[derive(Clone)]
pub struct SafeApiUrl {
    base: Url,
    prefix: String,
}

impl fmt::Debug for SafeApiUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SafeApiUrl").finish_non_exhaustive()
    }
}

impl SafeApiUrl {
    /// Adds an API prefix relative to the configured server (including its
    /// deployment subpath). Empty prefix validates a server root for profiles.
    pub fn new(server_url: &str, api_prefix: &str) -> Result<Self> {
        let mut base = Url::parse(server_url).map_err(|_| RemoteError::InvalidConfiguration)?;
        validate_https(&base).map_err(|_| RemoteError::InvalidConfiguration)?;
        if base.query().is_some() || api_prefix.contains(['?', '#', '\\']) {
            return Err(RemoteError::InvalidConfiguration);
        }
        let path = format!(
            "{}/{}/",
            base.path().trim_end_matches('/'),
            api_prefix.trim_matches('/')
        );
        base.set_path(&path.replace("//", "/"));
        let prefix = base.path().to_owned();
        validate_path(&prefix).map_err(|_| RemoteError::InvalidConfiguration)?;
        Ok(Self { base, prefix })
    }

    pub fn url(&self, path: &str) -> Result<Url> {
        if path.starts_with('/') || path.contains(['#', '\\']) {
            return Err(RemoteError::InvalidConfiguration);
        }
        let url = self
            .base
            .join(path)
            .map_err(|_| RemoteError::InvalidConfiguration)?;
        self.validate_url(&url)?;
        Ok(url)
    }

    pub fn validate_next(&self, url: &str) -> Result<Url> {
        let url = Url::parse(url).map_err(|_| RemoteError::UnsafePagination)?;
        self.validate_url(&url)?;
        Ok(url)
    }

    fn validate_url(&self, url: &Url) -> Result<()> {
        validate_https(url)?;
        if url.origin() != self.base.origin() || !url.path().starts_with(&self.prefix) {
            return Err(RemoteError::UnsafePagination);
        }
        validate_path(url.path())
    }
}

fn validate_https(url: &Url) -> Result<()> {
    if url.scheme() != "https"
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        Err(RemoteError::UnsafePagination)
    } else {
        Ok(())
    }
}

fn validate_path(path: &str) -> Result<()> {
    let mut decoded = Vec::with_capacity(path.len());
    let mut iter = path.bytes();
    while let Some(byte) = iter.next() {
        if byte == b'%' {
            let hi = iter
                .next()
                .and_then(hex)
                .ok_or(RemoteError::UnsafePagination)?;
            let lo = iter
                .next()
                .and_then(hex)
                .ok_or(RemoteError::UnsafePagination)?;
            decoded.push(hi * 16 + lo);
        } else {
            decoded.push(byte);
        }
    }
    if decoded
        .iter()
        .any(|b| matches!(b, b'\\' | b'%' | 0..=31 | 127))
        || decoded
            .split(|b| *b == b'/')
            .any(|segment| segment == b"." || segment == b"..")
    {
        Err(RemoteError::UnsafePagination)
    } else {
        Ok(())
    }
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}
