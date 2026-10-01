//! Read-only REST adapters. Cursors remain opaque and are bound to one account/resource.

use std::fmt;

use base64::{engine::general_purpose::STANDARD, Engine};
use secrecy::zeroize::Zeroizing;
use serde_json::Value;
use url::Url;

use crate::{
    transport::{check_status, contains_credential, header, MAX_RESPONSE_BYTES},
    Account, CancellationToken, ExposeSecret, HttpRequest, HttpResponse, Provider, RemoteError,
    Result, SafeApiUrl, SecretToken, Transport,
};

const PAGE_SIZE: u64 = 50;
const MAX_PAGES: u16 = 100;
const MAX_ITEMS: usize = 200;
const MAX_TEXT: usize = 32_768;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Repository {
    pub id: String,
    pub name: String,
    pub full_name: String,
    pub web_url: Option<String>,
    pub clone_url: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangeRequest {
    pub id: String,
    pub title: String,
    pub description: String,
    pub author: String,
    pub state: String,
    pub source_branch: String,
    pub target_branch: String,
    pub head_sha: Option<String>,
    pub web_url: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Comment {
    pub id: String,
    pub author: String,
    pub body: String,
    pub web_url: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Discussion {
    pub id: String,
    pub comments: Vec<Comment>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CiStatus {
    pub id: String,
    pub name: String,
    pub state: String,
    pub description: String,
    pub web_url: Option<String>,
}

#[derive(Clone, Debug)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next: Option<PageCursor>,
}

/// Not serializable: a pagination URL is never written into profile settings.
#[derive(Clone)]
pub struct PageCursor {
    url: Url,
    resource: String,
    account_id: String,
    pages: u16,
    phase: Phase,
}

impl fmt::Debug for PageCursor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PageCursor([redacted])")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Main,
    ReviewComments,
    CommitStatuses,
}

pub struct ProviderClient<T: Transport> {
    transport: T,
    account: Account,
    token: SecretToken,
    base: SafeApiUrl,
    cloud_workspace: Option<String>,
}

impl<T: Transport> ProviderClient<T> {
    pub fn new(transport: T, account: Account, token: SecretToken) -> Result<Self> {
        account.validate()?;
        if token.expose_secret().is_empty()
            || token.expose_secret().len() > 16 * 1024
            || token.expose_secret().chars().any(char::is_control)
        {
            return Err(RemoteError::InvalidConfiguration);
        }
        let server =
            Url::parse(&account.server_url).map_err(|_| RemoteError::InvalidConfiguration)?;
        let (base, cloud_workspace) = match account.provider {
            Provider::GitLab => (SafeApiUrl::new(&account.server_url, "api/v4")?, None),
            Provider::GitHub => {
                if matches!(server.host_str(), Some("github.com" | "api.github.com")) {
                    if !matches!(server.path(), "" | "/") {
                        return Err(RemoteError::InvalidConfiguration);
                    }
                    (SafeApiUrl::new("https://api.github.com", "")?, None)
                } else {
                    (SafeApiUrl::new(&account.server_url, "api/v3")?, None)
                }
            }
            Provider::BitbucketCloud => {
                if server.scheme() != "https"
                    || !server.username().is_empty()
                    || server.password().is_some()
                    || server.port().is_some()
                    || server.query().is_some()
                    || server.fragment().is_some()
                {
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
                validate_segment(workspace)?;
                if let Some(username) = &account.username {
                    if username.is_empty()
                        || username.contains(':')
                        || username.chars().any(char::is_control)
                    {
                        return Err(RemoteError::InvalidConfiguration);
                    }
                }
                (
                    SafeApiUrl::new("https://api.bitbucket.org", "2.0")?,
                    Some(workspace.to_owned()),
                )
            }
            Provider::BitbucketServer => {
                (SafeApiUrl::new(&account.server_url, "rest/api/1.0")?, None)
            }
        };
        Ok(Self {
            transport,
            account,
            token,
            base,
            cloud_workspace,
        })
    }

    pub fn repositories(
        &self,
        cursor: Option<&PageCursor>,
        cancel: &CancellationToken,
    ) -> Result<Page<Repository>> {
        let first = match self.account.provider {
            Provider::GitLab => self.endpoint("projects", &[("membership", "true")])?,
            Provider::GitHub => self.endpoint("user/repos", &[("sort", "full_name")])?,
            Provider::BitbucketCloud => self.endpoint(
                &format!(
                    "repositories/{}",
                    encode(self.cloud_workspace.as_deref().unwrap_or_default())
                ),
                &[("role", "member")],
            )?,
            Provider::BitbucketServer => self.endpoint("repos", &[("permission", "REPO_READ")])?,
        };
        let raw = self.fetch(&first, &first, cursor, Phase::Main, cancel)?;
        let items = raw
            .items
            .iter()
            .map(|v| self.repository(v))
            .collect::<Result<_>>()?;
        Ok(Page {
            items,
            next: raw.next,
        })
    }

    pub fn pull_requests(
        &self,
        repository: &Repository,
        cursor: Option<&PageCursor>,
        cancel: &CancellationToken,
    ) -> Result<Page<ChangeRequest>> {
        let path = self.repository_path(repository)?;
        let (suffix, state) = match self.account.provider {
            Provider::GitLab => ("merge_requests", "opened"),
            Provider::GitHub => ("pulls", "open"),
            Provider::BitbucketCloud => ("pullrequests", "OPEN"),
            Provider::BitbucketServer => ("pull-requests", "OPEN"),
        };
        let first = self.endpoint(&format!("{path}/{suffix}"), &[("state", state)])?;
        let raw = self.fetch(&first, &first, cursor, Phase::Main, cancel)?;
        let items = raw
            .items
            .iter()
            .map(|v| self.change_request(v))
            .collect::<Result<_>>()?;
        Ok(Page {
            items,
            next: raw.next,
        })
    }

    pub fn discussions(
        &self,
        repository: &Repository,
        request: &ChangeRequest,
        cursor: Option<&PageCursor>,
        cancel: &CancellationToken,
    ) -> Result<Page<Discussion>> {
        numeric_id(&request.id)?;
        let path = self.repository_path(repository)?;
        let id = &request.id;
        let suffix = match self.account.provider {
            Provider::GitLab => format!("merge_requests/{id}/discussions"),
            Provider::GitHub => format!("issues/{id}/comments"),
            Provider::BitbucketCloud => format!("pullrequests/{id}/comments"),
            Provider::BitbucketServer => format!("pull-requests/{id}/activities"),
        };
        let first = self.endpoint(&format!("{path}/{suffix}"), &[])?;
        let phase = cursor.map_or(Phase::Main, |c| c.phase);
        let current = if phase == Phase::ReviewComments && self.account.provider == Provider::GitHub
        {
            self.endpoint(&format!("{path}/pulls/{id}/comments"), &[])?
        } else if phase == Phase::Main {
            first.clone()
        } else {
            return Err(RemoteError::UnsafePagination);
        };
        let raw = self.fetch(&first, &current, cursor, phase, cancel)?;
        let mut items = Vec::new();
        let mut comment_count = 0;
        for v in &raw.items {
            if let Some(discussion) = self.discussion(v)? {
                comment_count += discussion.comments.len();
                if comment_count > MAX_ITEMS {
                    return Err(RemoteError::ResponseTooLarge);
                }
                items.push(discussion);
            }
        }
        let mut next = raw.next;
        if next.is_none() && self.account.provider == Provider::GitHub && phase == Phase::Main {
            next = Some(self.transition(
                &first,
                self.endpoint(&format!("{path}/pulls/{id}/comments"), &[])?,
                cursor,
                Phase::ReviewComments,
            )?);
        }
        Ok(Page { items, next })
    }

    /// GitHub checks and legacy statuses form two consecutive paginated collections.
    pub fn statuses(
        &self,
        repository: &Repository,
        sha: &str,
        cursor: Option<&PageCursor>,
        cancel: &CancellationToken,
    ) -> Result<Page<CiStatus>> {
        if !(7..=64).contains(&sha.len()) || !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(RemoteError::InvalidConfiguration);
        }
        let path = self.repository_path(repository)?;
        let first = match self.account.provider {
            Provider::GitLab => self.endpoint(&format!("{path}/pipelines"), &[("sha", sha)])?,
            Provider::GitHub => self.endpoint(&format!("{path}/commits/{sha}/check-runs"), &[])?,
            Provider::BitbucketCloud => {
                self.endpoint(&format!("{path}/commit/{sha}/statuses"), &[])?
            }
            Provider::BitbucketServer => {
                self.endpoint(&format!("{path}/commits/{sha}/builds"), &[])?
            }
        };
        let phase = cursor.map_or(Phase::Main, |c| c.phase);
        let current = if phase == Phase::CommitStatuses && self.account.provider == Provider::GitHub
        {
            self.endpoint(&format!("{path}/commits/{sha}/statuses"), &[])?
        } else if phase == Phase::Main {
            first.clone()
        } else {
            return Err(RemoteError::UnsafePagination);
        };
        let raw = self.fetch(&first, &current, cursor, phase, cancel)?;
        let items = raw
            .items
            .iter()
            .map(|v| self.ci_status(v, phase))
            .collect::<Result<_>>()?;
        let mut next = raw.next;
        if next.is_none() && self.account.provider == Provider::GitHub && phase == Phase::Main {
            next = Some(self.transition(
                &first,
                self.endpoint(&format!("{path}/commits/{sha}/statuses"), &[])?,
                cursor,
                Phase::CommitStatuses,
            )?);
        }
        Ok(Page { items, next })
    }

    fn endpoint(&self, path: &str, query: &[(&str, &str)]) -> Result<Url> {
        let mut url = self.base.url(path)?;
        let mut pairs = url.query_pairs_mut();
        for (key, value) in query {
            pairs.append_pair(key, value);
        }
        match self.account.provider {
            Provider::GitLab | Provider::GitHub => {
                pairs.append_pair("per_page", "50");
            }
            Provider::BitbucketCloud => {
                pairs.append_pair("pagelen", "50");
            }
            Provider::BitbucketServer => {
                pairs.append_pair("limit", "50");
            }
        }
        drop(pairs);
        Ok(url)
    }

    fn repository_path(&self, repository: &Repository) -> Result<String> {
        if self.account.provider == Provider::GitLab {
            numeric_id(&repository.id)?;
            return Ok(format!("projects/{}", repository.id));
        }
        let (owner, name) = repository
            .full_name
            .split_once('/')
            .ok_or(RemoteError::InvalidResponse)?;
        validate_segment(owner)?;
        validate_segment(name)?;
        Ok(match self.account.provider {
            Provider::GitHub => format!("repos/{}/{}", encode(owner), encode(name)),
            Provider::BitbucketCloud => format!("repositories/{}/{}", encode(owner), encode(name)),
            Provider::BitbucketServer => {
                format!("projects/{}/repos/{}", encode(owner), encode(name))
            }
            Provider::GitLab => unreachable!(),
        })
    }

    fn headers(&self) -> Vec<(String, SecretToken)> {
        let token = self.token.expose_secret();
        let (name, value) = match self.account.provider {
            Provider::GitLab => ("PRIVATE-TOKEN", token.to_owned()),
            Provider::BitbucketCloud if self.account.username.is_some() => (
                "Authorization",
                format!("Basic {}", self.basic_credential().unwrap().as_str()),
            ),
            _ => ("Authorization", format!("Bearer {token}")),
        };
        let accept = if self.account.provider == Provider::GitHub {
            "application/vnd.github+json"
        } else {
            "application/json"
        };
        let mut headers = vec![
            (name.to_owned(), SecretToken::from(value)),
            ("Accept".to_owned(), SecretToken::from(accept.to_owned())),
        ];
        if self.account.provider == Provider::GitHub {
            headers.push((
                "X-GitHub-Api-Version".to_owned(),
                SecretToken::from("2022-11-28".to_owned()),
            ));
        }
        headers
    }

    fn fetch(
        &self,
        resource: &Url,
        first: &Url,
        cursor: Option<&PageCursor>,
        phase: Phase,
        cancel: &CancellationToken,
    ) -> Result<Page<Value>> {
        cancel.check()?;
        let (url, pages) = if let Some(cursor) = cursor {
            if cursor.resource != resource.as_str()
                || cursor.account_id != self.account.id
                || cursor.phase != phase
                || cursor.pages >= MAX_PAGES
            {
                return Err(RemoteError::UnsafePagination);
            }
            (
                self.validate_next(first, cursor.url.as_str())?,
                cursor.pages,
            )
        } else {
            (first.clone(), 0)
        };
        let response = self.transport.get(
            &HttpRequest {
                url: url.clone(),
                headers: self.headers(),
            },
            cancel,
        )?;
        cancel.check()?;
        check_status(response.status, &response.headers)?;
        if response.body.len() > MAX_RESPONSE_BYTES {
            return Err(RemoteError::ResponseTooLarge);
        }
        let json: Value =
            serde_json::from_slice(&response.body).map_err(|_| RemoteError::InvalidResponse)?;
        let values =
            if self.account.provider == Provider::GitHub && url.path().ends_with("/check-runs") {
                json.get("check_runs")
            } else if matches!(
                self.account.provider,
                Provider::BitbucketCloud | Provider::BitbucketServer
            ) {
                json.get("values")
            } else {
                Some(&json)
            };
        let items = values
            .and_then(Value::as_array)
            .ok_or(RemoteError::InvalidResponse)?;
        if items.len() > MAX_ITEMS {
            return Err(RemoteError::ResponseTooLarge);
        }
        let next_url = self.next_url(&url, &response, &json)?;
        let next = next_url.map(|url| PageCursor {
            url,
            resource: resource.to_string(),
            account_id: self.account.id.clone(),
            pages: pages + 1,
            phase,
        });
        Ok(Page {
            items: items.clone(),
            next,
        })
    }

    fn transition(
        &self,
        resource: &Url,
        url: Url,
        cursor: Option<&PageCursor>,
        phase: Phase,
    ) -> Result<PageCursor> {
        let pages = cursor.map_or(1, |c| c.pages + 1);
        if pages >= MAX_PAGES {
            return Err(RemoteError::UnsafePagination);
        }
        Ok(PageCursor {
            url,
            resource: resource.to_string(),
            account_id: self.account.id.clone(),
            pages,
            phase,
        })
    }

    fn next_url(
        &self,
        current: &Url,
        response: &HttpResponse,
        json: &Value,
    ) -> Result<Option<Url>> {
        let next = match self.account.provider {
            Provider::GitLab => {
                if let Some(link) = header(&response.headers, "link").and_then(next_link) {
                    Some(link.to_owned())
                } else if let Some(page) =
                    header(&response.headers, "x-next-page").filter(|v| !v.is_empty())
                {
                    numeric_id(page).map_err(|_| RemoteError::UnsafePagination)?;
                    Some(with_query(current, "page", page).to_string())
                } else {
                    None
                }
            }
            Provider::GitHub => header(&response.headers, "link")
                .and_then(next_link)
                .map(str::to_owned),
            Provider::BitbucketCloud => match json.get("next") {
                Some(Value::String(next)) => Some(next.clone()),
                None | Some(Value::Null) => None,
                _ => return Err(RemoteError::InvalidResponse),
            },
            Provider::BitbucketServer => match json.get("isLastPage").and_then(Value::as_bool) {
                Some(true) => None,
                Some(false) => {
                    let start = json
                        .get("nextPageStart")
                        .and_then(Value::as_u64)
                        .ok_or(RemoteError::InvalidResponse)?;
                    let previous = current
                        .query_pairs()
                        .find(|(k, _)| k == "start")
                        .and_then(|(_, v)| v.parse::<u64>().ok())
                        .unwrap_or(0);
                    if start <= previous {
                        return Err(RemoteError::UnsafePagination);
                    }
                    Some(with_query(current, "start", &start.to_string()).to_string())
                }
                None => return Err(RemoteError::InvalidResponse),
            },
        };
        next.map(|next| {
            let url = self.validate_next(current, &next)?;
            if url == *current {
                Err(RemoteError::UnsafePagination)
            } else {
                Ok(url)
            }
        })
        .transpose()
    }

    fn validate_next(&self, current: &Url, next: &str) -> Result<Url> {
        if next.len() > 4096 || self.contains_credential(next) {
            return Err(RemoteError::UnsafePagination);
        }
        let url = self.base.validate_next(next)?;
        if url.path() != current.path() || url.fragment().is_some() {
            return Err(RemoteError::UnsafePagination);
        }
        let mut keys = std::collections::HashSet::new();
        for (key, value) in url.query_pairs() {
            if !keys.insert(key.to_string()) {
                return Err(RemoteError::UnsafePagination);
            }
            if matches!(
                key.as_ref(),
                "page" | "start" | "per_page" | "pagelen" | "limit"
            ) {
                let number = value
                    .parse::<u64>()
                    .map_err(|_| RemoteError::UnsafePagination)?;
                if matches!(key.as_ref(), "per_page" | "pagelen" | "limit")
                    && !(1..=PAGE_SIZE).contains(&number)
                {
                    return Err(RemoteError::UnsafePagination);
                }
            } else if matches!(key.as_ref(), "cursor" | "after") {
                if value.len() > 2048 {
                    return Err(RemoteError::UnsafePagination);
                }
            } else if !current.query_pairs().any(|(k, v)| k == key && v == value) {
                return Err(RemoteError::UnsafePagination);
            }
        }
        // Pagination may adjust only its own parameters, never the state/SHA/role filter.
        for (key, value) in current.query_pairs() {
            if !matches!(
                key.as_ref(),
                "page" | "start" | "cursor" | "after" | "per_page" | "pagelen" | "limit"
            ) && !url.query_pairs().any(|(k, v)| k == key && v == value)
            {
                return Err(RemoteError::UnsafePagination);
            }
        }
        Ok(url)
    }

    fn text(&self, value: &Value, path: &str) -> String {
        let raw = value
            .pointer(path)
            .and_then(Value::as_str)
            .unwrap_or_default();
        let token = self.token.expose_secret();
        let mut safe = raw
            .replace(token, "[redacted]")
            .replace(&encode(token), "[redacted]");
        if let Some(basic) = self.basic_credential() {
            safe = safe.replace(basic.as_str(), "[redacted]");
        }
        if self.contains_credential(&safe) {
            safe = "[redacted]".into();
        }
        safe.chars()
            .take(MAX_TEXT)
            .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
            .collect()
    }

    fn id(&self, value: &Value, path: &str) -> Result<String> {
        let id = match value.pointer(path) {
            Some(Value::String(_)) => self.text(value, path),
            Some(Value::Number(number)) => {
                number.as_u64().map(|v| v.to_string()).unwrap_or_default()
            }
            _ => String::new(),
        };
        if id.is_empty() {
            Err(RemoteError::InvalidResponse)
        } else {
            Ok(id)
        }
    }

    fn link(&self, value: &Value, path: &str) -> Option<String> {
        let raw = value.pointer(path).and_then(Value::as_str)?;
        if self.contains_credential(raw) {
            return None;
        }
        let url = Url::parse(raw).ok()?;
        if !matches!(url.scheme(), "https" | "http")
            || !url.username().is_empty()
            || url.password().is_some()
            || url
                .query_pairs()
                .any(|(_, value)| value.contains(self.token.expose_secret()))
            || url.query().is_some_and(|q| {
                q.to_ascii_lowercase().contains("token")
                    || q.to_ascii_lowercase().contains("password")
            })
        {
            return None;
        }
        Some(url.to_string())
    }

    fn contains_credential(&self, value: &str) -> bool {
        let token = self.token.expose_secret();
        if contains_credential(value, token) {
            return true;
        }
        self.basic_credential()
            .is_some_and(|basic| contains_credential(value, basic.as_str()))
    }

    fn basic_credential(&self) -> Option<Zeroizing<String>> {
        if self.account.provider != Provider::BitbucketCloud {
            return None;
        }
        let username = self.account.username.as_deref()?;
        let token = self.token.expose_secret();
        Some(Zeroizing::new(STANDARD.encode(
            Zeroizing::new(format!("{username}:{token}")).as_bytes(),
        )))
    }

    fn repository(&self, v: &Value) -> Result<Repository> {
        let (id, name, full_name, web_url, clone_url) = match self.account.provider {
            Provider::GitLab => (
                self.id(v, "/id")?,
                self.text(v, "/name"),
                self.text(v, "/path_with_namespace"),
                self.link(v, "/web_url"),
                self.link(v, "/http_url_to_repo"),
            ),
            Provider::GitHub => (
                self.id(v, "/id")?,
                self.text(v, "/name"),
                self.text(v, "/full_name"),
                self.link(v, "/html_url"),
                self.link(v, "/clone_url"),
            ),
            Provider::BitbucketCloud => (
                self.id(v, "/uuid")?,
                self.text(v, "/name"),
                self.text(v, "/full_name"),
                self.link(v, "/links/html/href"),
                v.pointer("/links/clone")
                    .and_then(Value::as_array)
                    .and_then(|links| {
                        links
                            .iter()
                            .find(|link| link.get("name").and_then(Value::as_str) == Some("https"))
                            .and_then(|link| self.link(link, "/href"))
                    }),
            ),
            Provider::BitbucketServer => (
                self.id(v, "/id")?,
                self.text(v, "/name"),
                format!("{}/{}", self.text(v, "/project/key"), self.text(v, "/slug")),
                self.link(v, "/links/self/0/href"),
                v.pointer("/links/clone")
                    .and_then(Value::as_array)
                    .and_then(|links| {
                        links
                            .iter()
                            .find(|link| link.get("name").and_then(Value::as_str) == Some("http"))
                            .and_then(|link| self.link(link, "/href"))
                    }),
            ),
        };
        if name.is_empty() || full_name.is_empty() {
            return Err(RemoteError::InvalidResponse);
        }
        Ok(Repository {
            id,
            name,
            full_name,
            web_url,
            clone_url,
        })
    }

    fn change_request(&self, v: &Value) -> Result<ChangeRequest> {
        let (id_path, description, author, state, source, target, sha_path, web_path) =
            match self.account.provider {
                Provider::GitLab => (
                    "/iid",
                    "/description",
                    "/author/username",
                    "/state",
                    "/source_branch",
                    "/target_branch",
                    "/sha",
                    "/web_url",
                ),
                Provider::GitHub => (
                    "/number",
                    "/body",
                    "/user/login",
                    "/state",
                    "/head/ref",
                    "/base/ref",
                    "/head/sha",
                    "/html_url",
                ),
                Provider::BitbucketCloud => (
                    "/id",
                    "/description",
                    "/author/display_name",
                    "/state",
                    "/source/branch/name",
                    "/destination/branch/name",
                    "/source/commit/hash",
                    "/links/html/href",
                ),
                Provider::BitbucketServer => (
                    "/id",
                    "/description",
                    "/author/user/displayName",
                    "/state",
                    "/fromRef/displayId",
                    "/toRef/displayId",
                    "/fromRef/latestCommit",
                    "/links/self/0/href",
                ),
            };
        let id = self.id(v, id_path)?;
        numeric_id(&id).map_err(|_| RemoteError::InvalidResponse)?;
        let sha = self.text(v, sha_path);
        Ok(ChangeRequest {
            id,
            title: self.text(v, "/title"),
            description: self.text(v, description),
            author: self.text(v, author),
            state: self.text(v, state).to_ascii_lowercase(),
            source_branch: self.text(v, source),
            target_branch: self.text(v, target),
            head_sha: if sha.is_empty() { None } else { Some(sha) },
            web_url: self.link(v, web_path),
        })
    }

    fn discussion(&self, v: &Value) -> Result<Option<Discussion>> {
        match self.account.provider {
            Provider::GitLab => {
                let notes = v
                    .get("notes")
                    .and_then(Value::as_array)
                    .ok_or(RemoteError::InvalidResponse)?;
                if notes.len() > MAX_ITEMS {
                    return Err(RemoteError::ResponseTooLarge);
                }
                let comments = notes
                    .iter()
                    .map(|note| self.comment(note))
                    .collect::<Result<_>>()?;
                Ok(Some(Discussion {
                    id: self.id(v, "/id")?,
                    comments,
                }))
            }
            Provider::BitbucketServer => {
                let Some(comment) = v.get("comment").filter(|c| c.is_object()) else {
                    return Ok(None);
                };
                let mut comments = Vec::new();
                self.server_comments(comment, &mut comments, 0)?;
                Ok(Some(Discussion {
                    id: self.id(comment, "/id")?,
                    comments,
                }))
            }
            _ => {
                let comment = self.comment(v)?;
                Ok(Some(Discussion {
                    id: comment.id.clone(),
                    comments: vec![comment],
                }))
            }
        }
    }

    fn server_comments(&self, v: &Value, comments: &mut Vec<Comment>, depth: usize) -> Result<()> {
        if comments.len() >= MAX_ITEMS || depth > 10 {
            return Err(RemoteError::ResponseTooLarge);
        }
        comments.push(self.comment(v)?);
        if let Some(replies) = v.get("comments").and_then(Value::as_array) {
            for reply in replies {
                self.server_comments(reply, comments, depth + 1)?;
            }
        }
        Ok(())
    }

    fn comment(&self, v: &Value) -> Result<Comment> {
        let (author, body, web) = match self.account.provider {
            Provider::GitLab => ("/author/username", "/body", "/web_url"),
            Provider::GitHub => ("/user/login", "/body", "/html_url"),
            Provider::BitbucketCloud => ("/user/display_name", "/content/raw", "/links/html/href"),
            Provider::BitbucketServer => ("/author/displayName", "/text", "/links/self/0/href"),
        };
        Ok(Comment {
            id: self.id(v, "/id")?,
            author: self.text(v, author),
            body: self.text(v, body),
            web_url: self.link(v, web),
        })
    }

    fn ci_status(&self, v: &Value, phase: Phase) -> Result<CiStatus> {
        let (id, name, raw_state, description, web) = match self.account.provider {
            Provider::GitLab => (
                self.id(v, "/id")?,
                self.text(v, "/name"),
                self.text(v, "/status"),
                self.text(v, "/ref"),
                self.link(v, "/web_url"),
            ),
            Provider::GitHub if phase == Phase::Main => {
                let conclusion = self.text(v, "/conclusion");
                (
                    self.id(v, "/id")?,
                    self.text(v, "/name"),
                    if conclusion.is_empty() {
                        self.text(v, "/status")
                    } else {
                        conclusion
                    },
                    self.text(v, "/output/title"),
                    self.link(v, "/html_url"),
                )
            }
            Provider::GitHub => (
                self.id(v, "/id")?,
                self.text(v, "/context"),
                self.text(v, "/state"),
                self.text(v, "/description"),
                self.link(v, "/target_url"),
            ),
            Provider::BitbucketCloud | Provider::BitbucketServer => (
                self.id(v, "/key")?,
                self.text(v, "/name"),
                self.text(v, "/state"),
                self.text(v, "/description"),
                self.link(v, "/url"),
            ),
        };
        let state = match raw_state.to_ascii_lowercase().as_str() {
            "success" | "successful" => "success",
            "failed" | "failure" | "error" | "timed_out" | "action_required" => "failure",
            "running" | "in_progress" | "inprogress" => "running",
            "created"
            | "waiting_for_resource"
            | "preparing"
            | "pending"
            | "queued"
            | "requested"
            | "waiting" => "pending",
            "canceled" | "cancelled" | "stopped" => "cancelled",
            "skipped" | "neutral" => "skipped",
            "manual" => "manual",
            _ => "unknown",
        }
        .to_owned();
        Ok(CiStatus {
            name: if name.is_empty() {
                format!("Pipeline {id}")
            } else {
                name
            },
            id,
            state,
            description,
            web_url: web,
        })
    }
}

fn next_link(link: &str) -> Option<&str> {
    link.split(',').find_map(|part| {
        let (target, attributes) = part.trim().split_once('>')?;
        if attributes
            .split(';')
            .any(|v| matches!(v.trim(), "rel=\"next\"" | "rel=next"))
        {
            target.strip_prefix('<')
        } else {
            None
        }
    })
}

fn with_query(url: &Url, key: &str, value: &str) -> Url {
    let mut next = url.clone();
    let existing: Vec<_> = url
        .query_pairs()
        .filter(|(k, _)| k != key)
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    next.set_query(None);
    next.query_pairs_mut()
        .extend_pairs(existing)
        .append_pair(key, value);
    next
}

fn numeric_id(value: &str) -> Result<()> {
    if value.is_empty() || value.len() > 20 || !value.bytes().all(|b| b.is_ascii_digit()) {
        Err(RemoteError::InvalidConfiguration)
    } else {
        Ok(())
    }
}

fn validate_segment(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 512
        || matches!(value, "." | "..")
        || value.contains('/')
        || value.contains('\\')
        || value.chars().any(char::is_control)
    {
        Err(RemoteError::InvalidConfiguration)
    } else {
        Ok(())
    }
}

fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}
