//! Aggregate-only measurements. Never print source paths, refs, authors or contents.
use git2::{Oid, Repository};
use mangrove_core::{model::UpstreamState, Result, Workspace};
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
        if start.elapsed() > Duration::from_secs(600) {
            return Err("Indexation trop longue.".into());
        }
        thread::sleep(Duration::from_millis(1));
    }
}

fn milliseconds(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

fn distribution(mut times: Vec<f64>) -> Value {
    if times.is_empty() {
        return Value::Null;
    }
    let total: f64 = times.iter().sum();
    times.sort_by(f64::total_cmp);
    let at = |quantile: f64| times[((times.len() - 1) as f64 * quantile).round() as usize];
    json!({
        "pairs": times.len(),
        "totalMs": total,
        "p50Ms": at(0.5),
        "p95Ms": at(0.95),
        "p99Ms": at(0.99),
        "maxMs": times[times.len() - 1],
        "over100Ms": times.iter().filter(|time| **time > 100.0).count(),
        "over1s": times.iter().filter(|time| **time > 1000.0).count(),
    })
}

/// Reads every pair in 100 ms calls, like the interface's background chunks.
fn chunked(workspace: &Workspace, id: &str, pairs: &[(String, String)]) -> Result<Value> {
    let started = Instant::now();
    let mut remaining = pairs.to_vec();
    let (mut calls, mut longest) = (0, Duration::ZERO);
    while !remaining.is_empty() {
        let call = Instant::now();
        let counts =
            workspace.ahead_behind(id, &remaining, call + Duration::from_millis(100), &|| false)?;
        longest = longest.max(call.elapsed());
        calls += 1;
        remaining = remaining
            .into_iter()
            .zip(counts)
            .filter_map(|(pair, count)| count.is_none().then_some(pair))
            .collect();
    }
    Ok(json!({
        "totalMs": milliseconds(started.elapsed()),
        "calls": calls,
        "longestCallMs": milliseconds(longest),
    }))
}

fn main() -> Result<()> {
    let path = env::args()
        .nth(1)
        .ok_or("Usage: tracking_benchmark /path/to/repo")?;
    let cache = tempfile::tempdir().map_err(|error| error.to_string())?;
    let workspace = Workspace::new(cache.path())?;
    let info = workspace.open_repository(&path)?;
    // The two indexing workers would otherwise compete with the counts.
    finish(&workspace, &info.id)?;
    let mut resolution = Vec::new();
    for _ in 0..30 {
        let start = Instant::now();
        workspace.branch_tracking(&info.id)?;
        resolution.push(milliseconds(start.elapsed()));
    }
    let list = workspace.branch_tracking(&info.id)?;
    let pairs: Vec<_> = list
        .branches
        .iter()
        .filter_map(|branch| match &branch.state {
            UpstreamState::Tracking { sha } => Some((branch.sha.clone(), sha.clone())),
            _ => None,
        })
        .collect();

    // Each pair alone, on one repository as within a chunk, grouped by size.
    let repository = Repository::open(&path).map_err(|error| error.to_string())?;
    let mut groups: [Vec<f64>; 3] = Default::default();
    for (local, upstream) in &pairs {
        let (local, upstream) = (
            Oid::from_str(local).map_err(|error| error.to_string())?,
            Oid::from_str(upstream).map_err(|error| error.to_string())?,
        );
        let start = Instant::now();
        let (ahead, behind) = repository
            .graph_ahead_behind(local, upstream)
            .map_err(|error| error.to_string())?;
        let time = milliseconds(start.elapsed());
        groups[match ahead + behind {
            0..=50 => 0,
            51..=10_000 => 1,
            _ => 2,
        }]
        .push(time);
    }
    let all = groups.iter().flatten().copied().collect();
    let [close, far, deep] = groups.map(distribution);

    let cold = chunked(&workspace, &info.id, &pairs)?;
    let warm = chunked(&workspace, &info.id, &pairs)?;
    // A fetch moves some upstreams: one pair in fifty gets a new upstream.
    let moved: Vec<_> = pairs
        .iter()
        .enumerate()
        .map(|(index, (local, upstream))| {
            if index % 50 != 0 {
                return Ok((local.clone(), upstream.clone()));
            }
            let commit = Oid::from_str(upstream)
                .and_then(|id| repository.find_commit(id))
                .map_err(|error| error.to_string())?;
            Ok((
                local.clone(),
                commit.parent_id(0).unwrap_or(commit.id()).to_string(),
            ))
        })
        .collect::<Result<_>>()?;
    let after_fetch = chunked(&workspace, &info.id, &moved)?;
    let commits = workspace.list_commits(&info.id, 0, 1, "")?.total;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "commits": commits,
            "localBranches": info.references.iter().filter(|reference| reference.kind == "local").count(),
            "tracked": list.branches.len(),
            "countedPairs": pairs.len(),
            "shallow": list.shallow,
            "resolution": distribution(resolution),
            "perPair": { "all": distribution(all), "upTo50": close, "upTo10000": far, "over10000": deep },
            "chunkedCold": cold,
            "chunkedWarm": warm,
            "chunkedAfterFetch": after_fetch,
            "note": "Backend only; index complete before counting, warm filesystem cache, temporary application cache outside source. Resolution: 30 samples. perPair: libgit2 graph_ahead_behind per pair on one repository, grouped by ahead + behind. Chunked: Workspace::ahead_behind in 100 ms calls; after fetch, one upstream in fifty replaced by its parent; no source identifiers or content."
        }))
        .map_err(|error| error.to_string())?
    );
    Ok(())
}
