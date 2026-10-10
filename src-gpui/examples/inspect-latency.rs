//! Measure Git reads and syntax analysis separately for a real commit.
//! cargo run --release --locked --manifest-path src-gpui/Cargo.toml \
//!   --example inspect-latency -- /path/to/repo COMMIT [FILE...]
//! Excludes UI rendering, queueing, scrolling and network operations.

#[allow(dead_code)]
#[path = "../src/diff.rs"]
mod diff;
#[allow(dead_code)]
#[path = "../src/syntax.rs"]
mod syntax;

use mangrove_core::{Workspace, model::PreviewLimit};
use serde_json::json;
use std::{env, time::Instant};

fn main() -> Result<(), String> {
    let mut args = env::args().skip(1);
    let repository = args
        .next()
        .ok_or("Expected repository path and commit SHA")?;
    let sha = args.next().ok_or("Expected commit SHA")?;
    let paths: Vec<_> = args.collect();
    let cache = env::temp_dir().join(format!("mangrove-inspect-latency-{}", std::process::id()));
    let engine = Workspace::new(&cache)?;
    let repository = engine.open_repository(&repository)?;
    let started = Instant::now();
    let details = engine.commit_details(&repository.id, &sha)?;
    let details_ms = started.elapsed().as_secs_f64() * 1000.;
    for path in &paths {
        if !details.files.iter().any(|file| &file.path == path) {
            return Err(format!(
                "File is not in this commit's displayed list: {path}"
            ));
        }
    }
    let mut files = Vec::new();
    for file in &details.files {
        if !paths.is_empty() && !paths.contains(&file.path) {
            continue;
        }
        let started = Instant::now();
        // The limit of a file opened with the default preferences.
        let limit = PreviewLimit::from_mib(8);
        let diff = engine.file_diff(&repository.id, &sha, file.index, limit)?;
        let read_ms = started.elapsed().as_secs_f64() * 1000.;
        let started = Instant::now();
        let tokens =
            syntax::cached_diff_tokens_cancellable(&file.path, &diff, || false).unwrap_or_default();
        let syntax_ms = started.elapsed().as_secs_f64() * 1000.;
        let started = Instant::now();
        let cached_tokens =
            syntax::cached_diff_tokens_cancellable(&file.path, &diff, || false).unwrap_or_default();
        let syntax_cached_ms = started.elapsed().as_secs_f64() * 1000.;
        files.push(json!({
            "path": file.path,
            "read_ms": read_ms,
            "syntax_ms": syntax_ms,
            "syntax_cached_ms": syntax_cached_ms,
            "syntax_cache_reused": std::sync::Arc::ptr_eq(&tokens, &cached_tokens),
            "hunks": diff.hunks.len(),
            "lines": diff.hunks.iter().map(|h| h.lines.len()).sum::<usize>(),
            "tokens": tokens.iter().map(Vec::len).sum::<usize>(),
            "token_rows": tokens.len(),
            "truncated": diff.truncated,
            "oversized_bytes": diff.oversized.map(|oversized| oversized.size),
        }));
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "commit": details.sha,
            "file_count": details.file_count,
            "details_ms": details_ms,
            "files": files,
            "note": "Git read and syntax CPU latency only; excludes UI, scheduling and scrolling."
        }))
        .map_err(|e| e.to_string())?
    );
    // Indexing uses a worker; let it finish before deleting this probe's cache.
    let started = Instant::now();
    loop {
        let progress = engine.index_status(&repository.id)?;
        if let Some(error) = progress.error {
            return Err(error);
        }
        if progress.complete {
            break;
        }
        if started.elapsed() > std::time::Duration::from_secs(30) {
            return Err("Timed out waiting for index cleanup".into());
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    drop(engine);
    std::fs::remove_dir_all(cache).map_err(|e| e.to_string())?;
    Ok(())
}
