fn main() {
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&[
            "open_repository",
            "fetch_repository",
            "index_status",
            "list_commits",
            "commit_position",
            "commit_details",
            "file_diff",
            "file_content",
            "file_history",
            "file_blame",
            "file_revision_diff",
            "working_tree_status",
            "working_tree_diff",
        ]),
    ))
    .expect("Unable to prepare the desktop permissions");
}
