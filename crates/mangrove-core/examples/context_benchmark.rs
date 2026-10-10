//! Aggregate-only measurements. Never print source paths, refs, authors or contents.
use mangrove_core::{model::PreviewLimit, Result, Workspace};
use serde_json::{json, Value};
use std::{
    env, thread,
    time::{Duration, Instant},
};

fn finish(workspace: &Workspace, id: &str) -> Result<()> {
    let start = Instant::now();
    loop {
        let state = workspace.index_status(id)?;
        if let Some(error) = state.error {
            return Err(error);
        }
        if state.complete {
            return Ok(());
        }
        if start.elapsed() > Duration::from_secs(300) {
            return Err("Indexation trop longue.".into());
        }
        thread::sleep(Duration::from_millis(1));
    }
}
fn samples(mut operation: impl FnMut(usize) -> Result<usize>) -> Result<Value> {
    let mut times = Vec::new();
    let mut count = 0;
    for sample in 0..30 {
        let start = Instant::now();
        count = operation(sample)?;
        times.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    let first = times[0];
    times.sort_by(f64::total_cmp);
    Ok(
        json!({"firstMs": first, "p50Ms": times[14], "p95Ms": times[28], "samples": 30, "lastCount": count}),
    )
}
fn main() -> Result<()> {
    let path = env::args()
        .nth(1)
        .ok_or("Usage: context_benchmark /path/to/repo")?;
    let cache = tempfile::tempdir().map_err(|error| error.to_string())?;
    let workspace = Workspace::new(cache.path())?;
    let info = workspace.open_repository(&path)?;
    finish(&workspace, &info.id)?;
    let count = workspace.list_commits(&info.id, 0, 1, "")?.total;
    let worktrees = samples(|_| Ok(workspace.list_worktrees(&info.id)?.worktrees.len()))?;
    let stashes = samples(|_| Ok(workspace.list_stashes(&info.id)?.stashes.len()))?;
    let list = workspace.list_stashes(&info.id)?;
    let mut stash_details = Value::Null;
    let mut stash_diff = Value::Null;
    if let Some(stash) = list.stashes.first() {
        stash_details = samples(|_| {
            Ok(workspace
                .stash_details(&info.id, &stash.sha)?
                .groups
                .iter()
                .map(|group| group.file_count)
                .sum())
        })?;
        let detail = workspace.stash_details(&info.id, &stash.sha)?;
        if let Some((group, file)) = detail
            .groups
            .iter()
            .find_map(|group| group.files.first().map(|file| (group, file)))
        {
            stash_diff = samples(|_| {
                Ok(workspace
                    .stash_diff(
                        &info.id,
                        &stash.sha,
                        &group.area,
                        &file.file_key,
                        PreviewLimit::MIN,
                    )?
                    .hunks
                    .iter()
                    .map(|hunk| hunk.lines.len())
                    .sum())
            })?;
        }
    }
    let branches: Vec<_> = info
        .references
        .iter()
        .filter(|reference| matches!(reference.kind.as_str(), "local" | "remote"))
        .map(|reference| reference.name.clone())
        .collect();
    let mut solo = Value::Null;
    if branches.len() >= 2 {
        let one = vec![branches[0].clone()];
        let two = vec![branches[0].clone(), branches.last().unwrap().clone()];
        solo = samples(|sample| {
            let filtered = workspace
                .open_repository_with_solo(&path, if sample % 2 == 0 { &one } else { &two })?;
            finish(&workspace, &filtered.id)?;
            Ok(workspace.list_commits(&filtered.id, 0, 128, "")?.total)
        })?;
    }
    println!("{}", serde_json::to_string_pretty(&json!({"commits": count, "worktrees": worktrees, "stashes": stashes, "stashDetails": stash_details, "stashDiff": stash_diff, "soloIndexAndPage": solo, "note": "Backend only; 30 samples, warm filesystem cache, temporary application cache outside source, metadata reused for alternating Solo filters (requires two distinct branch refs); no source identifiers or content."})).map_err(|e| e.to_string())?);
    Ok(())
}
