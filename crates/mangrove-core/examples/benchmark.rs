//! Run with: cargo run --release -p mangrove-core --example benchmark -- /path/to/repo
//! All cache writes use a fresh OS temporary directory outside the source repo.

use std::{
    env, thread,
    time::{Duration, Instant},
};

use mangrove_core::Workspace;
use serde_json::json;

fn milliseconds(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

fn main() -> Result<(), String> {
    let path = env::args()
        .nth(1)
        .ok_or("Usage: benchmark /path/to/repository")?;
    let cache = tempfile::tempdir().map_err(|error| error.to_string())?;
    let workspace = Workspace::new(cache.path())?;
    let cold_started = Instant::now();
    let repository = workspace.open_repository(&path)?;
    let cold_open_ms = milliseconds(cold_started.elapsed());
    let first_page = loop {
        let page = workspace.list_commits(&repository.id, 0, 256, "")?;
        if !page.commits.is_empty() || page.index.complete {
            break page;
        }
        if cold_started.elapsed() > Duration::from_secs(300) {
            return Err("Timed out waiting for the first batch.".into());
        }
        thread::sleep(Duration::from_millis(1));
    };
    let first_batch_ms = milliseconds(cold_started.elapsed());
    loop {
        let progress = workspace.index_status(&repository.id)?;
        if let Some(error) = progress.error {
            return Err(error);
        }
        if progress.complete {
            break;
        }
        if cold_started.elapsed() > Duration::from_secs(300) {
            return Err("Timed out waiting for the full index.".into());
        }
        thread::sleep(Duration::from_millis(1));
    }
    let full_index_ms = milliseconds(cold_started.elapsed());
    let page = workspace.list_commits(&repository.id, 0, 256, "")?;
    let mut queries = vec!["fix".to_owned(), "feat".to_owned(), "cache".to_owned()];
    if let Some(commit) = page.commits.first() {
        queries.push(commit.sha[..8].into());
        queries.push(commit.author.clone());
    }
    let mut search = Vec::new();
    for query in queries {
        let mut elapsed = Vec::with_capacity(30);
        let mut matches = 0;
        for _ in 0..30 {
            let started = Instant::now();
            matches = workspace
                .list_commits(&repository.id, 0, 256, &query)?
                .total;
            elapsed.push(milliseconds(started.elapsed()));
        }
        elapsed.sort_by(f64::total_cmp);
        search.push(json!({
            "query": query,
            "matches": matches,
            "p50Ms": elapsed[14],
            "p95Ms": elapsed[28],
            "samples": elapsed.len(),
        }));
    }
    let mut detail_and_diff_ms = None;
    let mut diff_lines = 0;
    if let Some(commit) = page.commits.first() {
        let started = Instant::now();
        let details = workspace.commit_details(&repository.id, &commit.sha)?;
        if let Some(file) = details.files.first() {
            let diff = workspace.file_diff(&repository.id, &commit.sha, file.index)?;
            diff_lines = diff
                .hunks
                .iter()
                .map(|hunk| hunk.lines.len())
                .sum::<usize>();
        }
        detail_and_diff_ms = Some(milliseconds(started.elapsed()));
    }
    drop(workspace);
    let workspace = Workspace::new(cache.path())?;
    let warm_started = Instant::now();
    let warm = workspace.open_repository(&path)?;
    let warm_page = workspace.list_commits(&warm.id, 0, 256, "")?;
    let warm_open_and_page_ms = milliseconds(warm_started.elapsed());
    if !warm.index.complete || warm_page.total != page.total {
        return Err("Warm cache was not reused.".into());
    }
    let result = json!({
        "commits": page.total,
        "coldOpenMs": cold_open_ms,
        "firstBatchMs": first_batch_ms,
        "firstBatchRows": first_page.commits.len(),
        "fullIndexMs": full_index_ms,
        "warmOpenAndPageMs": warm_open_and_page_ms,
        "detailsAndFirstDiffMs": detail_and_diff_ms,
        "diffLines": diff_lines,
        "search": search,
        "note": "Backend only, warm filesystem cache, one synthetic or local repository; excludes UI rendering and network.",
    });
    println!(
        "{}",
        serde_json::to_string_pretty(&result).map_err(|error| error.to_string())?
    );
    Ok(())
}
