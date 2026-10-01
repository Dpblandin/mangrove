//! Compare local Git reads with and without a pending synthetic API request.
//! Run against the generated fixture, never a confidential repository.
use mangrove_core::Workspace;
use mangrove_remote::{
    providers::ProviderClient, Account, CancellationToken, HttpRequest, HttpResponse, Provider,
    RemoteError, SecretToken, Transport,
};
use serde_json::{json, Value};
use std::{
    env,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    thread,
    time::{Duration, Instant},
};

struct DelayedTransport {
    began: mpsc::Sender<()>,
    pending: Arc<AtomicBool>,
}
impl Transport for DelayedTransport {
    fn get(
        &self,
        _: &HttpRequest,
        cancel: &CancellationToken,
    ) -> mangrove_remote::Result<HttpResponse> {
        self.pending.store(true, Ordering::SeqCst);
        let _ = self.began.send(());
        while !cancel.is_cancelled() {
            thread::sleep(Duration::from_millis(2));
        }
        self.pending.store(false, Ordering::SeqCst);
        Err(RemoteError::Cancelled)
    }
}
fn milliseconds(elapsed: Duration) -> f64 {
    elapsed.as_secs_f64() * 1000.
}
fn summary(mut samples: Vec<f64>) -> Value {
    samples.sort_by(f64::total_cmp);
    json!({"p50Ms": samples[samples.len()/2], "p95Ms": samples[(samples.len()*95/100).min(samples.len()-1)], "samples":samples.len()})
}
fn workload(path: &str, pending: Option<&AtomicBool>) -> Result<Value, String> {
    let cache = tempfile::tempdir().map_err(|_| "Temporary cache unavailable")?;
    let workspace = Workspace::new(cache.path())?;
    let started = Instant::now();
    let repository = workspace.open_repository(path)?;
    let open_ms = milliseconds(started.elapsed());
    while !workspace.index_status(&repository.id)?.complete {
        if started.elapsed() > Duration::from_secs(120) {
            return Err("Index timeout".into());
        }
        thread::sleep(Duration::from_millis(1));
    }
    let page = workspace.list_commits(&repository.id, 0, 50, "")?;
    let sha = &page.commits.first().ok_or("Fixture has no commits")?.sha;
    let details = workspace.commit_details(&repository.id, sha)?;
    let file = details.files.first().ok_or("Fixture has no changed file")?;
    let mut search = Vec::new();
    let mut diff = Vec::new();
    for _ in 0..50 {
        let started = Instant::now();
        workspace.list_commits(&repository.id, 0, 50, "fixture")?;
        search.push(milliseconds(started.elapsed()));
        let started = Instant::now();
        workspace.file_diff(&repository.id, sha, file.index)?;
        diff.push(milliseconds(started.elapsed()));
        if pending.is_some_and(|pending| !pending.load(Ordering::SeqCst)) {
            return Err("Remote request was no longer pending".into());
        }
    }
    Ok(
        json!({"coldOpenMs":open_ms,"commits":page.total,"search":summary(search),"diff":summary(diff)}),
    )
}
fn main() -> Result<(), String> {
    let path = env::args()
        .nth(1)
        .ok_or("Usage: local_isolation_benchmark /path/to/synthetic/fixture")?;
    let baseline = workload(&path, None)?;
    let (began, ready) = mpsc::channel();
    let pending = Arc::new(AtomicBool::new(false));
    let cancel = CancellationToken::new();
    let remote_cancel = cancel.clone();
    let remote_pending = pending.clone();
    let remote = thread::spawn(move || {
        let client = ProviderClient::new(
            DelayedTransport {
                began,
                pending: remote_pending,
            },
            Account {
                id: "synthetic".into(),
                label: "Synthetic".into(),
                provider: Provider::GitLab,
                server_url: "https://gitlab.example.invalid".into(),
                username: None,
            },
            SecretToken::from("synthetic-test-token"),
        )?;
        client.repositories(None, &remote_cancel)
    });
    ready
        .recv_timeout(Duration::from_secs(5))
        .map_err(|_| "Synthetic request did not start")?;
    let enriched = workload(&path, Some(&pending));
    let cancelling = Instant::now();
    cancel.cancel();
    let result = remote.join().map_err(|_| "Remote worker failed")?;
    if !matches!(result, Err(RemoteError::Cancelled)) {
        return Err("Cancellation failed".into());
    }
    let report = json!({
        "baseline":baseline,"pendingSyntheticApi":enriched?,"cancellationMs":milliseconds(cancelling.elapsed()),
        "note":"Backend-only, 50 warm search/diff samples per condition and one cold open per condition; synthetic delayed GET, no real network, excludes native rendering. Filesystem cache is warm; this demonstrates isolation, not a statistical performance guarantee."
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&report).map_err(|_| "Report encoding failed")?
    );
    Ok(())
}
