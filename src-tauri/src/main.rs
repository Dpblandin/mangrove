#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::Arc;

use mangrove_core::{model::*, Result, Workspace};
use tauri::{Manager, State};

async fn blocking<T: Send + 'static>(
    task: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    tauri::async_runtime::spawn_blocking(task)
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn open_repository(state: State<'_, Arc<Workspace>>, path: String) -> Result<RepositoryInfo> {
    let workspace = state.inner().clone();
    blocking(move || workspace.open_repository(&path)).await
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

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let cache = app.path().app_cache_dir()?.join("repositories");
            let workspace = Workspace::new(cache).map_err(std::io::Error::other)?;
            app.manage(Arc::new(workspace));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            open_repository,
            fetch_repository,
            index_status,
            list_commits,
            commit_position,
            commit_details,
            file_diff,
            working_tree_status,
            working_tree_diff,
        ])
        .run(tauri::generate_context!())
        .expect("Unable to start Mangrove");
}
