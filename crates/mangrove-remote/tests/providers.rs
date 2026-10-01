use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Mutex},
};

use mangrove_remote::{
    providers::{AccountIdentity, ChangeRequest, PageCursor, ProviderClient, Repository},
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
        authentication: mangrove_remote::Authentication::PersonalToken,
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
        ssh_clone_url: None,
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
fn connected_identity_uses_the_authenticated_user_without_listing_repositories() {
    for (provider, server, user, expected_url, expected) in [
        (
            Provider::GitLab,
            "https://gitlab.example/git",
            json!({"id":42,"username":"alice","name":"Alice Martin",
                "avatar_url":"https://avatar.example/alice.png","email":"private@example.com"}),
            "https://gitlab.example/git/api/v4/user",
            AccountIdentity {
                username: "alice".into(),
                display_name: "Alice Martin".into(),
            },
        ),
        (
            Provider::GitHub,
            "https://github.com",
            json!({"id":42,"login":"octocat","name":null,
                "avatar_url":"https://avatar.example/octocat.png"}),
            "https://api.github.com/user",
            AccountIdentity {
                username: "octocat".into(),
                display_name: "octocat".into(),
            },
        ),
        (
            Provider::GitHub,
            "https://github.example",
            json!({"id":42,"login":"octocat","name":"Mona Lisa"}),
            "https://github.example/api/v3/user",
            AccountIdentity {
                username: "octocat".into(),
                display_name: "Mona Lisa".into(),
            },
        ),
    ] {
        let transport = MockTransport::new(vec![response(user)]);
        let identity = client(provider, server, transport.clone())
            .account_identity(&CancellationToken::new())
            .unwrap();
        assert_eq!(identity, Some(expected));
        assert_eq!(transport.urls(), [expected_url]);
    }
}

#[test]
fn bitbucket_connections_keep_repository_permissions_without_claiming_a_user() {
    for (provider, server, page, endpoint) in [
        (
            Provider::BitbucketCloud,
            "https://bitbucket.org/team",
            json!({"values":[],"next":"https://api.bitbucket.org/2.0/repositories/team?role=member&pagelen=50&page=2"}),
            "https://api.bitbucket.org/2.0/repositories/team?role=member&pagelen=50",
        ),
        (
            Provider::BitbucketServer,
            "https://bitbucket.example/git",
            json!({"values":[],"isLastPage":false,"nextPageStart":50}),
            "https://bitbucket.example/git/rest/api/1.0/repos?permission=REPO_READ&limit=50",
        ),
    ] {
        let transport = MockTransport::new(vec![response(page)]);
        let identity = client(provider, server, transport.clone())
            .account_identity(&CancellationToken::new())
            .unwrap();
        assert_eq!(identity, None);
        assert_eq!(transport.urls(), [endpoint]);
    }
}

#[test]
fn connected_identity_rejects_invalid_or_credential_echoed_usernames() {
    for user in [
        json!([]),
        json!({"username":null}),
        json!({"username":"  "}),
        json!({"username":"alice\nadmin"}),
        json!({"username":"a".repeat(257)}),
        json!({"username":format!("user-{TOKEN}")}),
    ] {
        let error = client(
            Provider::GitLab,
            "https://gitlab.com",
            MockTransport::new(vec![response(user)]),
        )
        .account_identity(&CancellationToken::new())
        .unwrap_err();
        assert_eq!(error, RemoteError::InvalidResponse);
        assert!(!format!("{error:?} {error}").contains(TOKEN));
    }
    let error = ProviderClient::new(
        MockTransport::new(vec![response(
            json!({"login":"%252b%252f%253dsecret-fixture"}),
        )]),
        account(Provider::GitHub, "https://github.com"),
        SecretToken::from("+/=secret-fixture"),
    )
    .unwrap()
    .account_identity(&CancellationToken::new())
    .unwrap_err();
    assert_eq!(error, RemoteError::InvalidResponse);
}

#[test]
fn connected_display_names_are_bounded_and_never_echo_credentials() {
    for name in [
        format!("Name {TOKEN}"),
        "test-secret-never-in-\ndiagnostics".into(),
    ] {
        let identity = client(
            Provider::GitLab,
            "https://gitlab.com",
            MockTransport::new(vec![response(json!({"username":"alice","name":name}))]),
        )
        .account_identity(&CancellationToken::new())
        .unwrap()
        .unwrap();
        assert_eq!(identity.display_name, "alice");
        assert!(!format!("{identity:?}").contains(TOKEN));
    }
    let identity = client(
        Provider::GitHub,
        "https://github.com",
        MockTransport::new(vec![response(
            json!({"login":"alice","name":"é".repeat(100)}),
        )]),
    )
    .account_identity(&CancellationToken::new())
    .unwrap()
    .unwrap();
    assert_eq!(identity.display_name, "é".repeat(64));
}

#[test]
fn connected_identity_rejects_revocation_oversized_bodies_and_cancellation() {
    for (status, body, expected) in [
        (
            401,
            TOKEN.as_bytes().to_vec(),
            RemoteError::CredentialsExpired,
        ),
        (
            403,
            TOKEN.as_bytes().to_vec(),
            RemoteError::Http { status: 403 },
        ),
        (
            200,
            vec![b' '; 4 * 1024 * 1024 + 1],
            RemoteError::ResponseTooLarge,
        ),
        (200, b"invalid json".to_vec(), RemoteError::InvalidResponse),
    ] {
        let error = client(
            Provider::GitLab,
            "https://gitlab.com",
            MockTransport::new(vec![HttpResponse {
                status,
                headers: BTreeMap::new(),
                body,
            }]),
        )
        .account_identity(&CancellationToken::new())
        .unwrap_err();
        assert_eq!(error, expected);
        assert!(!format!("{error:?} {error}").contains(TOKEN));
    }
    let transport = MockTransport::default();
    let cancel = CancellationToken::new();
    cancel.cancel();
    assert_eq!(
        client(Provider::GitLab, "https://gitlab.com", transport.clone()).account_identity(&cancel),
        Err(RemoteError::Cancelled)
    );
    assert!(transport.urls().is_empty());

    struct CancelDuringRead;
    impl Transport for CancelDuringRead {
        fn get(&self, _request: &HttpRequest, cancel: &CancellationToken) -> Result<HttpResponse> {
            cancel.cancel();
            Ok(response(json!({"username":"alice","name":"Alice"})))
        }
    }
    assert_eq!(
        ProviderClient::new(
            CancelDuringRead,
            account(Provider::GitLab, "https://gitlab.com"),
            SecretToken::from(TOKEN),
        )
        .unwrap()
        .account_identity(&CancellationToken::new()),
        Err(RemoteError::Cancelled)
    );
}

#[test]
fn gitlab_reads_projects_merge_requests_discussion_threads_and_pipelines() {
    let transport = MockTransport::new(vec![
        response(
            json!([{"id":42,"name":"app","path_with_namespace":"group/app",
            "web_url":"https://gitlab.example/git/group/app","http_url_to_repo":"https://gitlab.example/git/group/app.git",
            "ssh_url_to_repo":"git@gitlab.example:group/app.git"}]),
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
    assert_eq!(
        repository.clone_url.as_deref(),
        Some("https://gitlab.example/git/group/app.git")
    );
    assert_eq!(
        repository.ssh_clone_url.as_deref(),
        Some("git@gitlab.example:group/app.git")
    );
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
            "html_url":"https://github.com/org/app","clone_url":"https://github.com/org/app.git",
            "ssh_url":"git@github.com:org/app.git"}])),
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
    assert_eq!(
        repository.clone_url.as_deref(),
        Some("https://github.com/org/app.git")
    );
    assert_eq!(
        repository.ssh_clone_url.as_deref(),
        Some("git@github.com:org/app.git")
    );
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
            "links":{"html":{"href":"https://bitbucket.org/workspace/app"},"clone":[
            {"name":"https","href":"https://bitbucket.org/workspace/app.git"},
            {"name":"ssh","href":"ssh://git@bitbucket.org/workspace/app.git"}]}}]),
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
    assert_eq!(
        repository.clone_url.as_deref(),
        Some("https://bitbucket.org/workspace/app.git")
    );
    assert_eq!(
        repository.ssh_clone_url.as_deref(),
        Some("ssh://git@bitbucket.org/workspace/app.git")
    );
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
            "links":{"self":[{"href":"https://bb.example/bitbucket/projects/PROJ/repos/app/browse"}],"clone":[
            {"name":"http","href":"https://bb.example/bitbucket/scm/proj/app.git"},
            {"name":"ssh","href":"ssh://git@bb.example:7999/proj/app.git"}]}}]),
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
    assert_eq!(
        repository.clone_url.as_deref(),
        Some("https://bb.example/bitbucket/scm/proj/app.git")
    );
    assert_eq!(
        repository.ssh_clone_url.as_deref(),
        Some("ssh://git@bb.example:7999/proj/app.git")
    );
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
fn gitlab_hides_pending_deletions_without_stopping_pagination_or_hiding_archived_projects() {
    let mut first = response(json!([
        {"id":1,"name":"dated","path_with_namespace":"group/dated","marked_for_deletion_on":"2026-10-08"},
        {"id":2,"name":"legacy","path_with_namespace":"group/legacy","marked_for_deletion_at":"2026-10-08"},
        {"id":3,"name":"renamed","path_with_namespace":"group/renamed-deletion_scheduled-3"},
        {"id":4,"name":"child","path_with_namespace":"mcp-deletion_scheduled-141/subgroup/child","marked_for_deletion_on":null},
        {"id":5,"name":"nested","path_with_namespace":"group/skills-deletion_scheduled-142/nested","marked_for_deletion_on":null,"marked_for_deletion_at":null}
    ]));
    first.headers.insert("X-Next-Page".into(), "2".into());
    let visible = json!([
        {"id":6,"name":"archived","path_with_namespace":"group/archived","archived":true,"marked_for_deletion_on":null,"marked_for_deletion_at":null},
        {"id":7,"name":"ordinary","path_with_namespace":"group/ordinary"},
        {"id":8,"name":"word","path_with_namespace":"deletion_scheduled-tools/word","marked_for_deletion_on":""},
        {"id":9,"name":"nonnumeric","path_with_namespace":"group-deletion_scheduled-custom/nonnumeric"},
        {"id":10,"name":"continued","path_with_namespace":"group/continued-deletion_scheduled-123-backup"},
        {"id":11,"name":"no-id","path_with_namespace":"group/no-id-deletion_scheduled-"},
        {"id":12,"name":"no-base","path_with_namespace":"-deletion_scheduled-123/no-base"}
    ]);
    let transport = MockTransport::new(vec![first, response(visible)]);
    let client = client(Provider::GitLab, "https://gitlab.com", transport.clone());
    let cancel = CancellationToken::new();
    let first = client.repositories(None, &cancel).unwrap();
    assert!(first.items.is_empty());
    assert!(first.next.is_some());
    let second = client.repositories(first.next.as_ref(), &cancel).unwrap();
    assert_eq!(
        second
            .items
            .iter()
            .map(|repo| repo.id.as_str())
            .collect::<Vec<_>>(),
        ["6", "7", "8", "9", "10", "11", "12"]
    );
    assert!(second.next.is_none());
    assert_eq!(
        transport.urls(),
        [
            "https://gitlab.com/api/v4/projects?membership=true&per_page=50",
            "https://gitlab.com/api/v4/projects?membership=true&per_page=50&page=2"
        ]
    );
}

#[test]
fn gitlab_deletion_markers_do_not_hide_repositories_on_other_providers() {
    let full_name = "group-deletion_scheduled-141/app-deletion_scheduled-42";
    for (provider, server, page) in [
        (
            Provider::GitHub,
            "https://github.com",
            response(json!([{
                "id":42,"name":"app","full_name":full_name,"path_with_namespace":full_name,
                "marked_for_deletion_on":"2026-10-08"
            }])),
        ),
        (
            Provider::BitbucketCloud,
            "https://bitbucket.org/workspace",
            cloud_page(json!([{
                "uuid":"{repo-id}","name":"app","full_name":full_name,"path_with_namespace":full_name,
                "marked_for_deletion_at":"2026-10-08"
            }])),
        ),
        (
            Provider::BitbucketServer,
            "https://bb.example",
            server_page(json!([{
                "id":42,"name":"app","slug":"app-deletion_scheduled-42",
                "project":{"key":"group-deletion_scheduled-141"},"path_with_namespace":full_name,
                "marked_for_deletion_on":"2026-10-08","marked_for_deletion_at":"2026-10-08"
            }])),
        ),
    ] {
        let transport = MockTransport::new(vec![page]);
        let page = client(provider, server, transport.clone())
            .repositories(None, &CancellationToken::new())
            .unwrap();
        assert_eq!(page.items.len(), 1, "{provider:?}");
        assert_eq!(page.items[0].full_name, full_name, "{provider:?}");
        assert!(page.next.is_none());
        assert_eq!(transport.urls().len(), 1);
    }
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
fn gitlab_paginates_links_with_server_defaults_and_route_parameters() {
    // Projects/MR/pipeline parameters observed from public GitLab.com responses;
    // discussion route parameters come from GitLab's Discussions API definition.
    for server in ["https://gitlab.com", "https://gitlab.example/git"] {
        for (path, query) in [
            (
                "projects",
                concat!(
                    "imported=false&include_hidden=false&membership=true&order_by=created_at&",
                    "owned=false&repository_checksum_failed=false&simple=false&sort=desc&",
                    "starred=false&statistics=false&wiki_checksum_failed=false&",
                    "with_custom_attributes=false&with_issues_enabled=false&with_merge_requests_enabled=false"
                ).to_owned(),
            ),
            (
                "projects/42/merge_requests",
                "id=42&order_by=created_at&sort=desc&state=opened&with_labels_details=false&with_merge_status_recheck=false".into(),
            ),
            (
                "projects/42/merge_requests/7/discussions",
                "id=42&noteable_id=7".into(),
            ),
            (
                "projects/42/pipelines",
                format!("id=42&order_by=id&sha={SHA}&sort=desc"),
            ),
        ] {
            let page_url = |page| format!("{server}/api/v4/{path}?{query}&page={page}&per_page=50");
            let transport = MockTransport::new(vec![
                gitlab_link_response(&page_url(2)),
                gitlab_link_response(&page_url(3)),
                response(json!([])),
            ]);
            let client = client(Provider::GitLab, server, transport.clone());
            let cancel = CancellationToken::new();
            let read = |cursor| gitlab_next_page(&client, path, cursor, &cancel);
            let next = read(None).unwrap().unwrap();
            let next = read(Some(&next)).unwrap().unwrap();
            assert!(read(Some(&next)).unwrap().is_none());
            assert_eq!(&transport.urls()[1..], &[page_url(2), page_url(3)]);
        }
    }
}

fn gitlab_link_response(next: &str) -> HttpResponse {
    let mut first = response(json!([]));
    first
        .headers
        .insert("Link".into(), format!("<{next}>; rel=\"next\""));
    first
}

fn gitlab_next_page(
    client: &ProviderClient<MockTransport>,
    path: &str,
    cursor: Option<&PageCursor>,
    cancel: &CancellationToken,
) -> Result<Option<PageCursor>> {
    match path {
        "projects" => client.repositories(cursor, cancel).map(|p| p.next),
        "projects/42/merge_requests" => client
            .pull_requests(&repo("org/app"), cursor, cancel)
            .map(|p| p.next),
        "projects/42/merge_requests/7/discussions" => client
            .discussions(&repo("org/app"), &request(), cursor, cancel)
            .map(|p| p.next),
        "projects/42/pipelines" => client
            .statuses(&repo("org/app"), SHA, cursor, cancel)
            .map(|p| p.next),
        _ => unreachable!("unknown GitLab fixture"),
    }
}

#[test]
fn gitlab_pagination_rejects_changed_filters_defaults_and_route_parameters() {
    for (path, query) in [
        ("projects", "membership=false&page=2".into()),
        ("projects", "order_by=created_at&page=2".into()),
        ("projects", "membership=true&owned=true&page=2".into()),
        ("projects", "membership=true&sort=asc&page=2".into()),
        ("projects", "membership=true&order_by=id&page=2".into()),
        ("projects", "membership=true&unknown=false&page=2".into()),
        (
            "projects",
            "membership=true&owned=false&owned=true&page=2".into(),
        ),
        ("projects", "membership=true&id=42&page=2".into()),
        ("projects", "membership=true&per_page=100&page=2".into()),
        ("projects", format!("membership=true&token={TOKEN}&page=2")),
        (
            "projects/42/merge_requests",
            "id=43&state=opened&page=2".into(),
        ),
        (
            "projects/42/merge_requests",
            "id=42&state=all&page=2".into(),
        ),
        (
            "projects/42/merge_requests",
            "state=opened&with_merge_status_recheck=true&page=2".into(),
        ),
        (
            "projects/42/merge_requests/7/discussions",
            "id=43&noteable_id=7&page=2".into(),
        ),
        (
            "projects/42/merge_requests/7/discussions",
            "id=42&noteable_id=8&page=2".into(),
        ),
        (
            "projects/42/merge_requests/7/discussions",
            "id=42&sort=desc&page=2".into(),
        ),
        ("projects/42/pipelines", format!("id=43&sha={SHA}&page=2")),
        ("projects/42/pipelines", "id=42&sha=abcdef0&page=2".into()),
        (
            "projects/42/pipelines",
            format!("sha={SHA}&order_by=created_at&page=2"),
        ),
    ] {
        let next = format!("https://gitlab.example/git/api/v4/{path}?{query}");
        let transport = MockTransport::new(vec![gitlab_link_response(&next)]);
        let client = client(
            Provider::GitLab,
            "https://gitlab.example/git",
            transport.clone(),
        );
        assert!(
            matches!(
                gitlab_next_page(&client, path, None, &CancellationToken::new()),
                Err(RemoteError::UnsafePagination)
            ),
            "unsafe GitLab pagination accepted: {path}?{query}"
        );
        assert_eq!(transport.urls().len(), 1);
    }
}

#[test]
fn gitlab_server_parameters_remain_unchanged_after_the_first_link() {
    for query in [
        "membership=true&sort=asc&page=3&per_page=50",
        "membership=true&page=3&per_page=50",
    ] {
        let transport = MockTransport::new(vec![
            gitlab_link_response(
                "https://gitlab.com/api/v4/projects?membership=true&sort=desc&page=2&per_page=50",
            ),
            gitlab_link_response(&format!("https://gitlab.com/api/v4/projects?{query}")),
        ]);
        let client = client(Provider::GitLab, "https://gitlab.com", transport.clone());
        let cancel = CancellationToken::new();
        let next = client.repositories(None, &cancel).unwrap().next.unwrap();
        assert!(matches!(
            client.repositories(Some(&next), &cancel),
            Err(RemoteError::UnsafePagination)
        ));
        assert_eq!(transport.urls().len(), 2);
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
        "html_url":format!("https://github.com/org/app?token={TOKEN}"),"clone_url":"https://user:password@github.com/org/app.git",
        "ssh_url":format!("git@github.com:org/{TOKEN}.git")}]),
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
    assert!(page.items[0].ssh_clone_url.is_none());
    assert_eq!(
        format!("{:?}", page.next.unwrap()),
        "PageCursor([redacted])"
    );
}

#[test]
fn advertised_https_clone_links_follow_the_clone_contract_without_restricting_web_links() {
    const WEB: &str = "http://example.com/team/app?view=history#readme";
    const SSH: &str = "git@example.com:team/app.git";
    const HTTPS: &str = "https://example.com/team/app.git";
    for (provider, server) in [
        (Provider::GitLab, "https://gitlab.com"),
        (Provider::GitHub, "https://github.com"),
        (Provider::BitbucketCloud, "https://bitbucket.org/team"),
        (Provider::BitbucketServer, "https://bitbucket.example"),
    ] {
        for remote in [
            HTTPS,
            "http://example.com/team/app.git",
            "https://example.com/team/app.git?branch=main",
            "https://example.com/team/app.git#main",
            "https://user@example.com/team/app.git",
            "https://user:password@example.com/team/app.git",
            "https://example.com/team/app.git\n",
            "https://example.com/team/app%0Aheader.git",
            "https://example.com/team/app%00.git",
            "https://example.com/",
            "https://example.com:0/team/app.git",
        ] {
            let repository = match provider {
                Provider::GitLab => json!({
                    "id":42,"name":"app","path_with_namespace":"team/app",
                    "web_url":WEB,"http_url_to_repo":remote,"ssh_url_to_repo":SSH
                }),
                Provider::GitHub => json!({
                    "id":42,"name":"app","full_name":"team/app",
                    "html_url":WEB,"clone_url":remote,"ssh_url":SSH
                }),
                Provider::BitbucketCloud => json!({
                    "uuid":"{repo-id}","name":"app","full_name":"team/app",
                    "links":{"html":{"href":WEB},"clone":[
                        {"name":"https","href":remote},{"name":"ssh","href":SSH}
                    ]}
                }),
                Provider::BitbucketServer => json!({
                    "id":42,"name":"app","slug":"app","project":{"key":"TEAM"},
                    "links":{"self":[{"href":WEB}],"clone":[
                        {"name":"http","href":remote},{"name":"ssh","href":SSH}
                    ]}
                }),
            };
            let page = match provider {
                Provider::GitLab | Provider::GitHub => json!([repository]),
                Provider::BitbucketCloud => json!({"values":[repository]}),
                Provider::BitbucketServer => {
                    json!({"values":[repository],"isLastPage":true})
                }
            };
            let repository = client(provider, server, MockTransport::new(vec![response(page)]))
                .repositories(None, &CancellationToken::new())
                .unwrap()
                .items
                .remove(0);
            assert_eq!(
                repository.clone_url.as_deref(),
                (remote == HTTPS).then_some(HTTPS),
                "{provider:?}: {remote}"
            );
            assert_eq!(repository.web_url.as_deref(), Some(WEB));
            assert_eq!(repository.ssh_clone_url.as_deref(), Some(SSH));
        }
    }
}

#[test]
fn ssh_clone_links_reject_local_transports_options_credentials_and_shell_paths() {
    for remote in [
        "https://github.com/org/app.git",
        "file:///tmp/app",
        "git@github.com:-oProxyCommand=id",
        "ssh://git:password@github.com/org/app.git",
        "git@github.com:org/app.git\n--config=evil",
        "git@github.com:org/app;touch.git",
        "ssh://git@github.com/org/app.git?token=secret",
        "ssh://git@github.com:0/org/app.git",
    ] {
        let repository = client(
            Provider::GitHub,
            "https://github.com",
            MockTransport::new(vec![response(json!([{
                "id":42,"name":"app","full_name":"org/app",
                "clone_url":"https://github.com/org/app.git","ssh_url":remote
            }]))]),
        )
        .repositories(None, &CancellationToken::new())
        .unwrap()
        .items
        .remove(0);
        assert!(repository.ssh_clone_url.is_none(), "{remote}");
        assert_eq!(
            repository.clone_url.as_deref(),
            Some("https://github.com/org/app.git")
        );
    }
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
