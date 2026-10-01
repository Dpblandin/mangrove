use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Mutex},
};

use mangrove_remote::{
    providers::{ChangeRequest, ProviderClient, Repository},
    Account, CancellationToken, ExposeSecret, HttpRequest, HttpResponse, Provider, RemoteError,
    Result, SecretToken, Transport,
};
use serde_json::{json, Value};

const TOKEN: &str = "test-secret-never-in-diagnostics";
const SHA: &str = "0123456789abcdef0123456789abcdef01234567";
type RecordedRequests = Arc<Mutex<Vec<(String, Vec<(String, String)>)>>>;

#[derive(Clone, Default)]
struct MockTransport {
    responses: Arc<Mutex<VecDeque<HttpResponse>>>,
    requests: RecordedRequests,
}

impl MockTransport {
    fn new(responses: Vec<HttpResponse>) -> Self {
        Self {
            responses: Arc::new(Mutex::new(responses.into())),
            ..Self::default()
        }
    }
    fn urls(&self) -> Vec<String> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .map(|(url, _)| url.clone())
            .collect()
    }
    fn credential(&self, name: &str) -> String {
        self.requests.lock().unwrap()[0]
            .1
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .unwrap()
            .1
            .clone()
    }
}

impl Transport for MockTransport {
    fn get(&self, request: &HttpRequest, cancel: &CancellationToken) -> Result<HttpResponse> {
        cancel.check()?;
        self.requests.lock().unwrap().push((
            request.url.to_string(),
            request
                .headers
                .iter()
                .map(|(name, secret)| (name.clone(), secret.expose_secret().to_owned()))
                .collect(),
        ));
        Ok(self
            .responses
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected extra request"))
    }
}

fn response(value: Value) -> HttpResponse {
    HttpResponse {
        status: 200,
        headers: BTreeMap::new(),
        body: serde_json::to_vec(&value).unwrap(),
    }
}

fn account(provider: Provider, server_url: &str) -> Account {
    Account {
        id: "account-1".into(),
        label: "work".into(),
        provider,
        server_url: server_url.into(),
        username: None,
    }
}

fn client(
    provider: Provider,
    server: &str,
    transport: MockTransport,
) -> ProviderClient<MockTransport> {
    ProviderClient::new(
        transport,
        account(provider, server),
        SecretToken::from(TOKEN.to_owned()),
    )
    .unwrap()
}

fn repo(full_name: &str) -> Repository {
    Repository {
        id: "42".into(),
        name: "app".into(),
        full_name: full_name.into(),
        web_url: None,
        clone_url: None,
    }
}

fn request() -> ChangeRequest {
    ChangeRequest {
        id: "7".into(),
        title: "change".into(),
        description: String::new(),
        author: "user".into(),
        state: "open".into(),
        source_branch: "topic".into(),
        target_branch: "main".into(),
        head_sha: Some(SHA.into()),
        web_url: None,
    }
}

#[test]
fn gitlab_reads_projects_merge_requests_discussion_threads_and_pipelines() {
    let transport = MockTransport::new(vec![
        response(
            json!([{"id":42,"name":"app","path_with_namespace":"group/app",
            "web_url":"https://gitlab.example/git/group/app","http_url_to_repo":"https://gitlab.example/git/group/app.git"}]),
        ),
        response(
            json!([{"id":700,"iid":7,"title":"Improve app","description":"Details","state":"opened",
            "author":{"username":"alice"},"source_branch":"topic","target_branch":"main","sha":SHA,
            "web_url":"https://gitlab.example/git/group/app/-/merge_requests/7"}]),
        ),
        response(json!([{"id":"thread-abc","individual_note":false,"notes":[
            {"id":11,"body":"Review","author":{"username":"alice"}},
            {"id":12,"body":"Reply","author":{"username":"bob"}}]}])),
        response(json!([{"id":51,"status":"success","ref":"topic","sha":SHA,
            "web_url":"https://gitlab.example/git/group/app/-/pipelines/51"}])),
    ]);
    let client = client(
        Provider::GitLab,
        "https://gitlab.example/git",
        transport.clone(),
    );
    let cancel = CancellationToken::new();
    let repository = client.repositories(None, &cancel).unwrap().items.remove(0);
    let request = client
        .pull_requests(&repository, None, &cancel)
        .unwrap()
        .items
        .remove(0);
    assert_eq!(
        request.id, "7",
        "GitLab must use project-scoped IID, not global ID"
    );
    let threads = client
        .discussions(&repository, &request, None, &cancel)
        .unwrap();
    assert_eq!(threads.items[0].comments.len(), 2);
    assert_eq!(threads.items[0].comments[1].body, "Reply");
    let statuses = client.statuses(&repository, SHA, None, &cancel).unwrap();
    assert_eq!(statuses.items[0].state, "success");
    assert!(transport.urls()[0].starts_with("https://gitlab.example/git/api/v4/projects?"));
    assert!(transport.urls()[2].contains("/projects/42/merge_requests/7/discussions?"));
    assert!(transport.urls()[3].contains(&format!("sha={SHA}")));
    assert!(transport.credential("PRIVATE-TOKEN") == TOKEN);
}

#[test]
fn github_reads_issue_and_review_comments_plus_checks_and_legacy_statuses() {
    let transport = MockTransport::new(vec![
        response(json!([{"id":42,"name":"app","full_name":"org/app",
            "html_url":"https://github.com/org/app","clone_url":"https://github.com/org/app.git"}])),
        response(
            json!([{"id":987,"number":7,"title":"Improve app","body":"Details","state":"open",
            "user":{"login":"alice"},"head":{"ref":"topic","sha":SHA},"base":{"ref":"main"},
            "html_url":"https://github.com/org/app/pull/7"}]),
        ),
        response(
            json!([{"id":11,"body":"General comment","user":{"login":"alice"},"html_url":"https://github.com/org/app/pull/7#issuecomment-11"}]),
        ),
        response(
            json!([{"id":12,"body":"Inline review","user":{"login":"bob"},"path":"main.rs","html_url":"https://github.com/org/app/pull/7#discussion_r12"}]),
        ),
        response(
            json!({"total_count":1,"check_runs":[{"id":13,"name":"tests","status":"completed","conclusion":"success",
            "output":{"title":"Tests passed"},"html_url":"https://github.com/org/app/runs/13"}]}),
        ),
        response(
            json!([{"id":14,"context":"ci/external","state":"failure","description":"Build failed","target_url":"https://ci.example/build/14"}]),
        ),
    ]);
    let client = client(Provider::GitHub, "https://github.com", transport.clone());
    let cancel = CancellationToken::new();
    let repository = client.repositories(None, &cancel).unwrap().items.remove(0);
    let request = client
        .pull_requests(&repository, None, &cancel)
        .unwrap()
        .items
        .remove(0);
    assert_eq!(request.id, "7");
    let comments = client
        .discussions(&repository, &request, None, &cancel)
        .unwrap();
    let reviews = client
        .discussions(&repository, &request, comments.next.as_ref(), &cancel)
        .unwrap();
    assert_eq!(reviews.items[0].comments[0].body, "Inline review");
    assert!(reviews.next.is_none());
    let checks = client.statuses(&repository, SHA, None, &cancel).unwrap();
    let legacy = client
        .statuses(&repository, SHA, checks.next.as_ref(), &cancel)
        .unwrap();
    assert_eq!(checks.items[0].state, "success");
    assert_eq!(legacy.items[0].name, "ci/external");
    assert_eq!(legacy.items[0].state, "failure");
    assert!(legacy.next.is_none());
    assert!(transport.urls()[0].starts_with("https://api.github.com/user/repos?"));
    assert!(transport.urls()[3].contains("/repos/org/app/pulls/7/comments?"));
    assert!(transport.urls()[5].contains("/statuses?"));
    assert!(transport.credential("Authorization") == format!("Bearer {TOKEN}"));
}

#[test]
fn github_enterprise_preserves_deployment_subpath() {
    let transport = MockTransport::new(vec![response(json!([]))]);
    let client = client(
        Provider::GitHub,
        "https://enterprise.example/git",
        transport.clone(),
    );
    client
        .repositories(None, &CancellationToken::new())
        .unwrap();
    assert!(transport.urls()[0].starts_with("https://enterprise.example/git/api/v3/user/repos?"));
}

fn cloud_page(values: Value) -> HttpResponse {
    response(json!({"pagelen":50,"values":values}))
}

#[test]
fn bitbucket_cloud_uses_workspace_api_and_distinct_api_token_auth() {
    let transport = MockTransport::new(vec![
        cloud_page(
            json!([{"uuid":"{repo-id}","name":"app","full_name":"workspace/app",
            "links":{"html":{"href":"https://bitbucket.org/workspace/app"},"clone":[{"name":"https","href":"https://bitbucket.org/workspace/app.git"}]}}]),
        ),
        cloud_page(
            json!([{"id":7,"title":"Improve app","description":"Details","state":"OPEN","author":{"display_name":"Alice"},
            "source":{"branch":{"name":"topic"},"commit":{"hash":SHA}},"destination":{"branch":{"name":"main"}},
            "links":{"html":{"href":"https://bitbucket.org/workspace/app/pull-requests/7"}}}]),
        ),
        cloud_page(
            json!([{"id":11,"user":{"display_name":"Bob"},"content":{"raw":"Review"},"links":{"html":{"href":"https://bitbucket.org/workspace/app/pull-requests/7#comment-11"}}}]),
        ),
        cloud_page(
            json!([{"key":"TEST","name":"unit tests","state":"SUCCESSFUL","description":"passed","url":"https://ci.example/build/1"}]),
        ),
    ]);
    let mut account = account(Provider::BitbucketCloud, "https://bitbucket.org/workspace");
    account.username = Some("person@example.com".into());
    let client = ProviderClient::new(
        transport.clone(),
        account,
        SecretToken::from(TOKEN.to_owned()),
    )
    .unwrap();
    let cancel = CancellationToken::new();
    let repository = client.repositories(None, &cancel).unwrap().items.remove(0);
    let request = client
        .pull_requests(&repository, None, &cancel)
        .unwrap()
        .items
        .remove(0);
    assert_eq!(
        client
            .discussions(&repository, &request, None, &cancel)
            .unwrap()
            .items[0]
            .comments[0]
            .body,
        "Review"
    );
    assert_eq!(
        client
            .statuses(&repository, SHA, None, &cancel)
            .unwrap()
            .items[0]
            .state,
        "success"
    );
    assert!(
        transport.urls()[0].starts_with("https://api.bitbucket.org/2.0/repositories/workspace?")
    );
    assert!(transport.credential("Authorization").starts_with("Basic "));
    let bearer_transport = MockTransport::new(vec![cloud_page(json!([]))]);
    client_bearer_cloud(&bearer_transport)
        .repositories(None, &cancel)
        .unwrap();
    assert!(bearer_transport.credential("Authorization") == format!("Bearer {TOKEN}"));
}

fn client_bearer_cloud(transport: &MockTransport) -> ProviderClient<MockTransport> {
    client(
        Provider::BitbucketCloud,
        "https://bitbucket.org/workspace",
        transport.clone(),
    )
}

fn server_page(values: Value) -> HttpResponse {
    response(json!({"size":1,"limit":50,"isLastPage":true,"start":0,"values":values}))
}

#[test]
fn bitbucket_server_reads_project_repos_nested_comments_and_repository_builds() {
    let transport = MockTransport::new(vec![
        server_page(
            json!([{"id":42,"slug":"app","name":"app","project":{"key":"PROJ"},
            "links":{"self":[{"href":"https://bb.example/bitbucket/projects/PROJ/repos/app/browse"}],"clone":[{"name":"http","href":"https://bb.example/bitbucket/scm/proj/app.git"}]}}]),
        ),
        server_page(
            json!([{"id":7,"title":"Improve app","description":"Details","state":"OPEN","author":{"user":{"displayName":"Alice"}},
            "fromRef":{"displayId":"topic","latestCommit":SHA},"toRef":{"displayId":"main"},"links":{"self":[{"href":"https://bb.example/bitbucket/projects/PROJ/repos/app/pull-requests/7"}]}}]),
        ),
        server_page(
            json!([{"id":900,"action":"OPENED"},{"id":901,"action":"COMMENTED","comment":{"id":11,"text":"Review","author":{"displayName":"Alice"},
            "comments":[{"id":12,"text":"Reply","author":{"displayName":"Bob"},"comments":[]}]}}]),
        ),
        server_page(
            json!([{"key":"CI","name":"tests","state":"INPROGRESS","url":"https://ci.example/build/1","description":"Running"}]),
        ),
    ]);
    let client = client(
        Provider::BitbucketServer,
        "https://bb.example/bitbucket",
        transport.clone(),
    );
    let cancel = CancellationToken::new();
    let repository = client.repositories(None, &cancel).unwrap().items.remove(0);
    assert_eq!(repository.full_name, "PROJ/app");
    let request = client
        .pull_requests(&repository, None, &cancel)
        .unwrap()
        .items
        .remove(0);
    let threads = client
        .discussions(&repository, &request, None, &cancel)
        .unwrap();
    assert_eq!(threads.items.len(), 1);
    assert_eq!(threads.items[0].comments[1].body, "Reply");
    assert_eq!(
        client
            .statuses(&repository, SHA, None, &cancel)
            .unwrap()
            .items[0]
            .state,
        "running"
    );
    assert!(transport.urls()[0].starts_with("https://bb.example/bitbucket/rest/api/1.0/repos?"));
    assert!(
        transport.urls()[3].contains(&format!("/projects/PROJ/repos/app/commits/{SHA}/builds?"))
    );
}

#[test]
fn each_provider_paginates_without_constructing_untrusted_origins() {
    for (provider, server, first, second) in [
        (
            Provider::GitLab,
            "https://gitlab.example",
            {
                let mut response = response(json!([]));
                response.headers.insert("X-Next-Page".into(), "2".into());
                response
            },
            response(json!([])),
        ),
        (
            Provider::GitHub,
            "https://github.com",
            {
                let mut response = response(json!([]));
                response.headers.insert("Link".into(), "<https://api.github.com/user/repos?sort=full_name&per_page=50&page=2>; rel=\"next\"".into());
                response
            },
            response(json!([])),
        ),
        (
            Provider::BitbucketCloud,
            "https://bitbucket.org/workspace",
            response(
                json!({"values":[],"next":"https://api.bitbucket.org/2.0/repositories/workspace?role=member&pagelen=50&page=2"}),
            ),
            cloud_page(json!([])),
        ),
        (
            Provider::BitbucketServer,
            "https://bb.example",
            response(json!({"values":[],"isLastPage":false,"nextPageStart":19})),
            server_page(json!([])),
        ),
    ] {
        let transport = MockTransport::new(vec![first, second]);
        let client = client(provider, server, transport.clone());
        let cancel = CancellationToken::new();
        let page = client.repositories(None, &cancel).unwrap();
        assert!(page.next.is_some());
        let next = client.repositories(page.next.as_ref(), &cancel).unwrap();
        assert!(next.next.is_none());
        assert!(transport.urls()[1].contains("page=2") || transport.urls()[1].contains("start=19"));
    }
}

#[test]
fn pagination_rejects_cross_origin_path_credentials_filters_and_oversized_pages() {
    for next in [
        "https://attacker.example/2.0/repositories/workspace?role=member&page=2",
        "http://api.bitbucket.org/2.0/repositories/workspace?role=member&page=2",
        "https://user:password@api.bitbucket.org/2.0/repositories/workspace?role=member&page=2",
        "https://api.bitbucket.org/2.0/repositories/other?role=member&page=2",
        "https://api.bitbucket.org/2.0/repositories/workspace?role=admin&page=2",
        "https://api.bitbucket.org/2.0/repositories/workspace?page=2",
        "https://api.bitbucket.org/2.0/repositories/workspace?role=member&pagelen=1000&page=2",
        "https://api.bitbucket.org/2.0/repositories/workspace?role=member&page=2&token=secret",
        "https://api.bitbucket.org/2.0/repositories/workspace?role=member&page=2&page=3",
        "https://api.bitbucket.org/2.0/repositories/workspace?role=member&pagelen=50",
    ] {
        let transport = MockTransport::new(vec![response(json!({"values":[],"next":next}))]);
        let result = client_bearer_cloud(&transport).repositories(None, &CancellationToken::new());
        assert!(
            matches!(result, Err(RemoteError::UnsafePagination)),
            "unsafe pagination accepted"
        );
        assert_eq!(transport.urls().len(), 1);
    }
}

#[test]
fn cursor_is_bound_to_account_resource_and_page_budget() {
    let transport = MockTransport::new(
        (1..=100)
            .map(|page| {
                let mut response = response(json!([]));
                response
                    .headers
                    .insert("x-next-page".into(), (page + 1).to_string());
                response
            })
            .collect(),
    );
    let client = client(
        Provider::GitLab,
        "https://gitlab.example",
        transport.clone(),
    );
    let cancel = CancellationToken::new();
    let mut page = client.repositories(None, &cancel).unwrap();
    assert!(matches!(
        client.pull_requests(&repo("org/app"), page.next.as_ref(), &cancel),
        Err(RemoteError::UnsafePagination)
    ));
    assert!(matches!(
        client.discussions(&repo("org/app"), &request(), page.next.as_ref(), &cancel),
        Err(RemoteError::UnsafePagination)
    ));
    let mut other_account = account(Provider::GitLab, "https://gitlab.example");
    other_account.id = "account-2".into();
    let other = ProviderClient::new(
        transport.clone(),
        other_account,
        SecretToken::from(TOKEN.to_owned()),
    )
    .unwrap();
    assert!(matches!(
        other.repositories(page.next.as_ref(), &cancel),
        Err(RemoteError::UnsafePagination)
    ));
    for _ in 1..100 {
        page = client.repositories(page.next.as_ref(), &cancel).unwrap();
    }
    assert!(matches!(
        client.repositories(page.next.as_ref(), &cancel),
        Err(RemoteError::UnsafePagination)
    ));
    assert_eq!(transport.urls().len(), 100);
}

#[test]
fn expired_revoked_rate_limited_and_permission_errors_do_not_expose_remote_bodies() {
    for (status, headers, expected) in [
        (401, BTreeMap::new(), RemoteError::CredentialsExpired),
        (403, BTreeMap::new(), RemoteError::Http { status: 403 }),
        (
            429,
            BTreeMap::from([("retry-after".into(), "60".into())]),
            RemoteError::RateLimited {
                retry_after_seconds: Some(60),
            },
        ),
        (
            403,
            BTreeMap::from([("x-ratelimit-remaining".into(), "0".into())]),
            RemoteError::RateLimited {
                retry_after_seconds: None,
            },
        ),
        (500, BTreeMap::new(), RemoteError::Http { status: 500 }),
    ] {
        let transport = MockTransport::new(vec![HttpResponse {
            status,
            headers,
            body: format!("revoked credential {TOKEN}").into_bytes(),
        }]);
        let error = client(Provider::GitHub, "https://github.com", transport)
            .repositories(None, &CancellationToken::new())
            .unwrap_err();
        assert_eq!(error, expected);
        assert!(!format!("{error:?} {error}").contains(TOKEN));
    }
}

#[test]
fn normalized_data_and_cursor_diagnostics_redact_secrets_and_reject_credential_links() {
    let mut response = response(
        json!([{"id":42,"name":format!("app {TOKEN}"),"full_name":"org/app",
        "html_url":format!("https://github.com/org/app?token={TOKEN}"),"clone_url":"https://user:password@github.com/org/app.git"}]),
    );
    response.headers.insert(
        "link".into(),
        "<https://api.github.com/user/repos?sort=full_name&per_page=50&page=2>; rel=\"next\""
            .into(),
    );
    let transport = MockTransport::new(vec![response]);
    let page = client(Provider::GitHub, "https://github.com", transport)
        .repositories(None, &CancellationToken::new())
        .unwrap();
    assert!(!format!("{page:?}").contains(TOKEN));
    assert!(page.items[0].name.contains("[redacted]"));
    assert!(page.items[0].web_url.is_none());
    assert!(page.items[0].clone_url.is_none());
    assert_eq!(
        format!("{:?}", page.next.unwrap()),
        "PageCursor([redacted])"
    );
}

#[test]
fn encoded_credentials_are_removed_from_normalized_text_and_url_fragments() {
    for encoded in [
        "%2b%2f%3dsecret-fixture",
        "%2B/%3dsecret-fixture",
        "%252b%252f%253dsecret-fixture",
    ] {
        let transport = MockTransport::new(vec![response(json!([{
            "id":42,"name":format!("app {encoded}"),"full_name":"org/app",
            "html_url":format!("https://github.com/org/app#{encoded}"),
            "clone_url":format!("https://github.com/org/{encoded}.git")
        }]))]);
        let client = ProviderClient::new(
            transport,
            account(Provider::GitHub, "https://github.com"),
            SecretToken::from("+/=secret-fixture"),
        )
        .unwrap();
        let page = client
            .repositories(None, &CancellationToken::new())
            .unwrap();
        assert!(!format!("{page:?}").contains(encoded));
        assert_eq!(page.items[0].name, "[redacted]");
        assert!(page.items[0].web_url.is_none());
        assert!(page.items[0].clone_url.is_none());
    }
}

#[test]
fn discussion_page_bounds_the_total_comments_across_threads() {
    for (threads, expected_ok) in [(100, true), (101, false)] {
        let values: Vec<_> = (0..threads)
            .map(|id| {
                json!({"id":id.to_string(),"notes":[
                    {"id":id * 2 + 1,"body":"First note","author":{"username":"alice"}},
                    {"id":id * 2 + 2,"body":"Reply","author":{"username":"bob"}}
                ]})
            })
            .collect();
        let transport = MockTransport::new(vec![response(json!(values))]);
        let result = client(Provider::GitLab, "https://gitlab.example", transport).discussions(
            &repo("org/app"),
            &request(),
            None,
            &CancellationToken::new(),
        );
        if expected_ok {
            let page = result.unwrap();
            assert_eq!(
                page.items.iter().map(|d| d.comments.len()).sum::<usize>(),
                200
            );
        } else {
            assert!(matches!(result, Err(RemoteError::ResponseTooLarge)));
        }
    }
}

#[test]
fn cancellation_invalid_json_and_configuration_fail_safely() {
    let transport = MockTransport::new(vec![HttpResponse {
        status: 200,
        headers: BTreeMap::new(),
        body: b"broken json".to_vec(),
    }]);
    let client = client(
        Provider::GitLab,
        "https://gitlab.example",
        transport.clone(),
    );
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert!(matches!(
        client.repositories(None, &cancel),
        Err(RemoteError::Cancelled)
    ));
    assert!(transport.urls().is_empty());
    assert!(matches!(
        client.repositories(None, &CancellationToken::new()),
        Err(RemoteError::InvalidResponse)
    ));
    for server in [
        "https://bitbucket.org",
        "https://bitbucket.org/workspace/extra",
        "https://evil.example/workspace",
    ] {
        assert!(ProviderClient::new(
            MockTransport::default(),
            account(Provider::BitbucketCloud, server),
            SecretToken::from(TOKEN.to_owned())
        )
        .is_err());
    }
    assert!(matches!(
        client.statuses(
            &repo("org/app"),
            "../../evil",
            None,
            &CancellationToken::new()
        ),
        Err(RemoteError::InvalidConfiguration)
    ));
}
