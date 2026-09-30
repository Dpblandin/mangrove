//! Aggregated, content-free file-view timings. Source repository paths, SHAs and authors are not emitted.
//! cargo run --release --locked -p mangrove-core --example file_benchmark -- /repo HEAD path mode
//! mode: content | history | blame | baseline. Measure process peak RSS with /usr/bin/time -l on macOS.
use mangrove_core::Workspace;
use serde_json::json;
use std::{
    env,
    fmt::Write,
    thread,
    time::{Duration, Instant},
};

fn main() -> Result<(), String> {
    let args: Vec<_> = env::args().skip(1).collect();
    if args.len() != 4 {
        return Err(
            "Usage: file_benchmark /repo SHA|HEAD path content|history|blame|baseline".into(),
        );
    }
    let repository = git2::Repository::open(&args[0]).map_err(|e| e.to_string())?;
    let sha = repository
        .revparse_single(&args[1])
        .and_then(|object| object.peel_to_commit())
        .map_err(|e| e.to_string())?
        .id()
        .to_string();
    let key = args[2]
        .as_bytes()
        .iter()
        .fold(String::new(), |mut key, byte| {
            write!(&mut key, "{byte:02x}").unwrap();
            key
        });
    drop(repository);
    let cache = tempfile::tempdir().map_err(|e| e.to_string())?;
    let workspace = Workspace::new(cache.path())?;
    let info = workspace.open_repository(&args[0])?;
    let started = Instant::now();
    loop {
        let progress = workspace.index_status(&info.id)?;
        if let Some(error) = progress.error {
            return Err(error);
        }
        if progress.complete {
            break;
        }
        if started.elapsed() > Duration::from_secs(300) {
            return Err("Index timeout".into());
        }
        thread::sleep(Duration::from_millis(10));
    }
    let commits = workspace.index_status(&info.id)?.indexed;
    let mut times = Vec::new();
    let mut first = 0.;
    let mut rows = 0;
    let mut truncated = false;
    for sample in 0..31 {
        let start = Instant::now();
        match args[3].as_str() {
            "content" => {
                let value = workspace.file_content(&info.id, &sha, &key)?;
                rows = value.lines.len();
                truncated = value.truncated;
            }
            "history" => {
                let value = workspace.file_history(&info.id, &sha, &key, None)?;
                rows = value.entries.len();
                truncated = value.next_cursor.is_some() || value.limitation.is_some();
            }
            "blame" => {
                let value = workspace.file_blame(&info.id, &sha, &key)?;
                rows = value.groups.len();
                truncated = value.limitation.is_some();
            }
            "baseline" => (),
            _ => return Err("Unknown mode".into()),
        }
        let elapsed = start.elapsed().as_secs_f64() * 1000.;
        if sample == 0 {
            first = elapsed;
        } else {
            times.push(elapsed);
        }
    }
    times.sort_by(f64::total_cmp);
    println!("{}", serde_json::to_string_pretty(&json!({ "commits": commits, "operation": args[3], "firstMs": first, "p50Ms": times[14], "p95Ms": times[28], "samples": times.len(), "rows": rows, "limited": truncated, "scope": "Native engine only; no UI, IPC or syntax worker. Fresh application cache, warm OS cache. No source content, path, SHA or author emitted." })).map_err(|e| e.to_string())?);
    Ok(())
}
