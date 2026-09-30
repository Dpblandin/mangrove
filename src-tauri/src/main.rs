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
        ])
        .run(tauri::generate_context!())
        .expect("Unable to start Mangrove");
}
