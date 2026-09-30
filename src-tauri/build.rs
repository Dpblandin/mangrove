fn main() {
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&[
            "open_repository",
            "index_status",
            "list_commits",
            "commit_position",
            "commit_details",
            "file_diff",
        ]),
    ))
    .expect("Unable to prepare the read-only desktop permissions");
}
