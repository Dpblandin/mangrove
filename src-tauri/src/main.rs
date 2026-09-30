#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::Arc;
#[cfg(feature = "app-benchmark")]
use std::{path::PathBuf, time::Instant};

use mangrove_core::{model::*, Result, Workspace};
use tauri::{Manager, State};

/// Benchmark commands can write only to an absolute path fixed at process launch.
/// They are compiled only with `app-benchmark` and require opt-in at process launch.
#[cfg(feature = "app-benchmark")]
struct AppBenchmark {
    output: Option<PathBuf>,
    started: Instant,
    started_unix_ms: f64,
    launch_overhead_ms: f64,
}

#[cfg(feature = "app-benchmark")]
#[tauri::command]
fn app_benchmark_phase(
    app: tauri::AppHandle,
    state: State<'_, AppBenchmark>,
    phase: String,
) -> Result<()> {
    let output = state.output.as_ref().ok_or("Benchmark mode is disabled")?;
    if ![
        "startup",
        "index",
        "selection",
        "syntax",
        "heavy_diff",
        "diff_scroll",
        "close_diff",
        "history_scroll",
        "idle",
        "done",
    ]
    .contains(&phase.as_str())
    {
        return Err("Unknown benchmark phase".into());
    }
    if phase == "startup" {
        // Direct benchmark launches must be visible and foregrounded like GPUI's
        // cx.activate(true). A suspended WebView cannot provide real rAF timings.
        #[cfg(target_os = "macos")]
        app.show().map_err(|e| e.to_string())?;
        let window = app
            .get_webview_window("main")
            .ok_or("Benchmark window is missing")?;
        let monitor = window
            .primary_monitor()
            .map_err(|e| e.to_string())?
            .ok_or("Benchmark primary monitor is missing")?;
        let scale = monitor.scale_factor();
        window
            .set_position(tauri::LogicalPosition::new(
                monitor.position().x as f64 / scale
                    + (monitor.size().width as f64 / scale - 1_440.0) / 2.0,
                monitor.position().y as f64 / scale
                    + (monitor.size().height as f64 / scale - 920.0) / 2.0,
            ))
            .map_err(|e| e.to_string())?;
        window.show().map_err(|e| e.to_string())?;
        window.set_focus().map_err(|e| e.to_string())?;
        let webview: &tauri::Webview = window.as_ref();
        webview.show().map_err(|e| e.to_string())?;
        webview.set_focus().map_err(|e| e.to_string())?;
    }
    std::fs::write(output.with_extension("phase"), phase).map_err(|e| e.to_string())
}

#[cfg(feature = "app-benchmark")]
#[tauri::command]
fn app_benchmark_report(
    app: tauri::AppHandle,
    state: State<'_, AppBenchmark>,
    report: String,
) -> Result<()> {
    let output = state.output.as_ref().ok_or("Benchmark mode is disabled")?;
    if report.len() > 512 * 1_024 {
        return Err("Benchmark report exceeds its bound".into());
    }
    let mut report: serde_json::Value =
        serde_json::from_str(&report).map_err(|_| "Invalid benchmark report")?;
    let time_origin = report
        .get("time_origin_unix_ms")
        .and_then(|v| v.as_f64())
        .filter(|v| v.is_finite())
        .ok_or("Missing benchmark navigation time origin")?;
    // Add native initialization, WebView creation and JS loading to navigation timings.
    // Do not reconstruct this origin from the report's arrival: preference
    // restoration, serialization and IPC happen after the final JS timestamp.
    let offset = time_origin - state.started_unix_ms + state.launch_overhead_ms;
    if let Some(timings) = report.get_mut("timings_ms").and_then(|v| v.as_object_mut()) {
        for (name, samples) in timings {
            if name.starts_with("startup_") {
                if let Some(samples) = samples.as_array_mut() {
                    for sample in samples {
                        if let Some(value) = sample.as_f64() {
                            *sample = serde_json::json!(value + offset);
                        }
                    }
                }
            }
        }
    }
    report["launch_overhead_ms"] = serde_json::json!(state.launch_overhead_ms);
    report["elapsed_ms"] = serde_json::json!(
        state.started.elapsed().as_secs_f64() * 1_000.0 + state.launch_overhead_ms
    );
    let temporary = output.with_extension("tmp");
    let json = serde_json::to_vec_pretty(&report).map_err(|e| e.to_string())?;
    std::fs::write(&temporary, json).map_err(|e| e.to_string())?;
    std::fs::rename(&temporary, output).map_err(|e| e.to_string())?;
    std::fs::write(output.with_extension("phase"), "done").map_err(|e| e.to_string())?;
    app.exit(0);
    Ok(())
}

async fn blocking<T: Send + 'static>(
    task: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    tauri::async_runtime::spawn_blocking(task)
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn open_repository(
    state: State<'_, Arc<Workspace>>,
    path: String,
    solo_refs: Option<Vec<String>>,
) -> Result<RepositoryInfo> {
    let workspace = state.inner().clone();
    blocking(move || workspace.open_repository_with_solo(&path, &solo_refs.unwrap_or_default()))
        .await
}

#[tauri::command]
async fn fetch_repository(
    state: State<'_, Arc<Workspace>>,
    session_id: String,
) -> Result<FetchResult> {
    let workspace = state.inner().clone();
    blocking(move || workspace.fetch_repository(&session_id)).await
}

#[tauri::command]
async fn repository_names(paths: Vec<String>) -> Result<Vec<RepositoryName>> {
    blocking(move || mangrove_core::repository_names(&paths)).await
}

#[tauri::command]
async fn index_status(
    state: State<'_, Arc<Workspace>>,
    session_id: String,
) -> Result<IndexProgress> {
    let workspace = state.inner().clone();
    blocking(move || workspace.index_status(&session_id)).await
}

#[tauri::command]
async fn list_commits(
    state: State<'_, Arc<Workspace>>,
    session_id: String,
    offset: usize,
    limit: usize,
    query: String,
) -> Result<CommitPage> {
    let workspace = state.inner().clone();
    blocking(move || workspace.list_commits(&session_id, offset, limit, &query)).await
}

#[tauri::command]
async fn commit_position(
    state: State<'_, Arc<Workspace>>,
    session_id: String,
    sha: String,
) -> Result<Option<usize>> {
    let workspace = state.inner().clone();
    blocking(move || workspace.commit_position(&session_id, &sha)).await
}

#[tauri::command]
async fn commit_details(
    state: State<'_, Arc<Workspace>>,
    session_id: String,
    sha: String,
) -> Result<CommitDetails> {
    let workspace = state.inner().clone();
    blocking(move || workspace.commit_details(&session_id, &sha)).await
}

#[tauri::command]
async fn file_diff(
    state: State<'_, Arc<Workspace>>,
    session_id: String,
    sha: String,
    file_index: usize,
) -> Result<FileDiff> {
    let workspace = state.inner().clone();
    blocking(move || workspace.file_diff(&session_id, &sha, file_index)).await
}

#[tauri::command]
async fn file_content(
    state: State<'_, Arc<Workspace>>,
    session_id: String,
    sha: String,
    file_key: String,
) -> Result<FileContent> {
    let workspace = state.inner().clone();
    blocking(move || workspace.file_content(&session_id, &sha, &file_key)).await
}

#[tauri::command]
async fn file_blame(
    state: State<'_, Arc<Workspace>>,
    session_id: String,
    sha: String,
    file_key: String,
) -> Result<FileBlame> {
    let workspace = state.inner().clone();
    blocking(move || workspace.file_blame(&session_id, &sha, &file_key)).await
}

#[tauri::command]
async fn file_revision_diff(
    state: State<'_, Arc<Workspace>>,
    session_id: String,
    sha: String,
    file_key: String,
) -> Result<FileDiff> {
    let workspace = state.inner().clone();
    blocking(move || workspace.file_revision_diff(&session_id, &sha, &file_key)).await
}

#[tauri::command]
async fn file_history(
    state: State<'_, Arc<Workspace>>,
    session_id: String,
    sha: String,
    file_key: String,
    cursor: Option<String>,
) -> Result<FileHistoryPage> {
    let workspace = state.inner().clone();
    blocking(move || workspace.file_history(&session_id, &sha, &file_key, cursor.as_deref())).await
}

#[tauri::command]
async fn working_tree_status(
    state: State<'_, Arc<Workspace>>,
    session_id: String,
) -> Result<WorkingTreeStatus> {
    let workspace = state.inner().clone();
    blocking(move || workspace.working_tree_status(&session_id)).await
}

#[tauri::command]
async fn working_tree_diff(
    state: State<'_, Arc<Workspace>>,
    session_id: String,
    area: String,
    file_key: String,
) -> Result<FileDiff> {
    let workspace = state.inner().clone();
    blocking(move || workspace.working_tree_diff(&session_id, &area, &file_key)).await
}

#[tauri::command]
async fn list_worktrees(
    state: State<'_, Arc<Workspace>>,
    session_id: String,
) -> Result<WorktreeList> {
    let workspace = state.inner().clone();
    blocking(move || workspace.list_worktrees(&session_id)).await
}

#[tauri::command]
async fn list_stashes(state: State<'_, Arc<Workspace>>, session_id: String) -> Result<StashList> {
    let workspace = state.inner().clone();
    blocking(move || workspace.list_stashes(&session_id)).await
}

#[tauri::command]
async fn stash_details(
    state: State<'_, Arc<Workspace>>,
    session_id: String,
    sha: String,
) -> Result<StashDetails> {
    let workspace = state.inner().clone();
    blocking(move || workspace.stash_details(&session_id, &sha)).await
}

#[tauri::command]
async fn stash_diff(
    state: State<'_, Arc<Workspace>>,
    session_id: String,
    sha: String,
    area: String,
    file_key: String,
) -> Result<FileDiff> {
    let workspace = state.inner().clone();
    blocking(move || workspace.stash_diff(&session_id, &sha, &area, &file_key)).await
}

fn main() {
    #[cfg(feature = "app-benchmark")]
    let started = Instant::now();
    #[cfg(feature = "app-benchmark")]
    let started_unix_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64()
        * 1_000.0;
    #[cfg(feature = "app-benchmark")]
    let launch_overhead_ms = std::env::var("MANGROVE_BENCH_LAUNCH_UNIX_MS")
        .ok()
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|v| v.is_finite())
        .map(|launch| started_unix_ms - launch)
        .unwrap_or_default()
        .max(0.0);
    #[cfg(feature = "app-benchmark")]
    let benchmark_output = std::env::var_os("MANGROVE_BENCH_OUTPUT").map(PathBuf::from);
    let builder = tauri::Builder::default();
    #[cfg(feature = "app-benchmark")]
    let builder = {
        let mut builder = builder;
        if let Some(output) = &benchmark_output {
            assert!(output.is_absolute(), "Benchmark output must be absolute");
            let repository = std::env::var("MANGROVE_BENCH_REPO")
                .expect("Benchmark mode requires an explicit repository");
            assert!(
                PathBuf::from(&repository).is_absolute(),
                "Benchmark repository must be absolute"
            );
            let samples = std::env::var("MANGROVE_BENCH_SAMPLES")
                .ok()
                .and_then(|v| v.parse::<usize>().ok())
                .unwrap_or(30)
                .clamp(2, 100);
            let config = serde_json::json!({ "repository": repository, "samples": samples });
            builder = builder.append_invoke_initialization_script(format!(
                "window.__MANGROVE_BENCHMARK__ = {config};"
            ));
        }
        builder.manage(AppBenchmark {
            output: benchmark_output,
            started,
            started_unix_ms,
            launch_overhead_ms,
        })
    };
    builder
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            #[cfg(feature = "app-benchmark")]
            app.add_capability(include_str!("../benchmark/capabilities.json"))?;
            #[cfg(feature = "app-benchmark")]
            let cache = {
                let benchmark = app.state::<AppBenchmark>();
                if benchmark.output.is_some() {
                    let cache = std::env::var_os("MANGROVE_BENCH_CACHE")
                        .map(PathBuf::from)
                        .ok_or_else(|| std::io::Error::other("Benchmark cache is required"))?;
                    if !cache.is_absolute() {
                        return Err(
                            std::io::Error::other("Benchmark cache must be absolute").into()
                        );
                    }
                    cache
                } else {
                    app.path().app_cache_dir()?.join("repositories")
                }
            };
            #[cfg(not(feature = "app-benchmark"))]
            let cache = app.path().app_cache_dir()?.join("repositories");
            let workspace = Workspace::new(cache).map_err(std::io::Error::other)?;
            app.manage(Arc::new(workspace));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            open_repository,
            fetch_repository,
            repository_names,
            index_status,
            list_commits,
            commit_position,
            commit_details,
            file_diff,
            file_content,
            file_history,
            file_blame,
            file_revision_diff,
            working_tree_status,
            working_tree_diff,
            list_worktrees,
            list_stashes,
            stash_details,
            stash_diff,
            #[cfg(feature = "app-benchmark")]
            app_benchmark_phase,
            #[cfg(feature = "app-benchmark")]
            app_benchmark_report,
        ])
        .run(tauri::generate_context!())
        .expect("Unable to start Mangrove");
}
