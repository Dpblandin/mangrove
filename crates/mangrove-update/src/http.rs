//! GET requests confined to GitHub over HTTPS, redirections included.
use crate::{Result, UpdateError};
use reqwest::{
    blocking::{Client, Response},
    header::ACCEPT,
    redirect::Policy,
    StatusCode,
};
use std::{
    io::{self, Read},
    time::Duration,
};
use url::Url;

const USER_AGENT: &str = concat!("Mangrove/", env!("CARGO_PKG_VERSION"), " updater");
const MAX_REDIRECTS: usize = 5;
/// Bounds the wait for the headers and for each read of the body, not the
/// whole download.
const TIMEOUT: Duration = Duration::from_secs(30);

/// GitHub's API, its release pages and the storage its downloads redirect to.
pub(crate) fn is_github(url: &Url) -> bool {
    url.scheme() == "https"
        && url.port().is_none()
        && url.username().is_empty()
        && url.password().is_none()
        && url.host_str().is_some_and(|host| {
            host == "github.com"
                || host.ends_with(".github.com")
                || host.ends_with(".githubusercontent.com")
        })
}

pub(crate) fn get(url: &Url, accept: &str) -> Result<Response> {
    if !is_github(url) {
        return Err(UpdateError::InvalidResponse);
    }
    let client = Client::builder()
        .https_only(true)
        .redirect(Policy::custom(|attempt| {
            if attempt.previous().len() > MAX_REDIRECTS {
                attempt.error("too many redirections")
            } else if is_github(attempt.url()) {
                attempt.follow()
            } else {
                attempt.stop()
            }
        }))
        .referer(false)
        .connect_timeout(Duration::from_secs(10))
        .timeout(TIMEOUT)
        .user_agent(USER_AGENT)
        .build()?;
    let response = client.get(url.clone()).header(ACCEPT, accept).send()?;
    let status = response.status();
    let exhausted = response
        .headers()
        .get("x-ratelimit-remaining")
        .is_some_and(|remaining| remaining == "0");
    if status == StatusCode::TOO_MANY_REQUESTS || (status == StatusCode::FORBIDDEN && exhausted) {
        return Err(UpdateError::RateLimited);
    }
    if !status.is_success() {
        return Err(UpdateError::Http(status.as_u16()));
    }
    Ok(response)
}

/// A small body, refused above `limit` bytes.
pub(crate) fn read_body(response: Response, limit: u64) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    response
        .take(limit + 1)
        .read_to_end(&mut body)
        .map_err(read_error)?;
    if body.len() as u64 > limit {
        return Err(UpdateError::InvalidResponse);
    }
    Ok(body)
}

/// A body that stopped arriving. The blocking client wraps its own errors,
/// timeouts included, in `io::Error`s of another kind.
pub(crate) fn read_error(error: io::Error) -> UpdateError {
    let timed_out = error.kind() == io::ErrorKind::TimedOut
        || error
            .get_ref()
            .and_then(|inner| inner.downcast_ref::<reqwest::Error>())
            .is_some_and(reqwest::Error::is_timeout);
    if timed_out {
        UpdateError::Timeout
    } else {
        UpdateError::Network
    }
}

#[cfg(test)]
mod tests {
    use super::is_github;
    use url::Url;

    #[test]
    fn only_github_over_https_is_reached() {
        for url in [
            "https://api.github.com/repos/Dpblandin/mangrove/releases/latest",
            "https://github.com/Dpblandin/mangrove/releases/download/v0.4.0/a.dmg",
            "https://release-assets.githubusercontent.com/github-production-release-asset/1",
            "https://objects.githubusercontent.com/github-production-release-asset-2e65be/1",
        ] {
            assert!(is_github(&Url::parse(url).unwrap()), "{url}");
        }
        for url in [
            "http://github.com/Dpblandin/mangrove",
            "https://github.com:8443/Dpblandin/mangrove",
            "https://user@github.com/Dpblandin/mangrove",
            "https://github.com.example.com/",
            "https://evilgithub.com/",
            "https://githubusercontent.com.example/",
            "https://example.com/github.com",
        ] {
            assert!(!is_github(&Url::parse(url).unwrap()), "{url}");
        }
    }
}
