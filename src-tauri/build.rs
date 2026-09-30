fn main() {
    println!("cargo:rerun-if-changed=benchmark/capabilities.json");
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&[
            "open_repository",
            "fetch_repository",
            "repository_names",
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
            "list_worktrees",
            "list_stashes",
            "stash_details",
            "stash_diff",
            #[cfg(feature = "app-benchmark")]
            "app_benchmark_phase",
            #[cfg(feature = "app-benchmark")]
            "app_benchmark_report",
        ]),
    ))
    .expect("Unable to prepare the desktop permissions");
}
