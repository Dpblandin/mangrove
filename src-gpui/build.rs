fn main() {
    println!("cargo::rerun-if-changed=packaging/windows");
    println!("cargo::rerun-if-changed=assets/icons/icon.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile(
            "packaging/windows/mangrove.rc",
            embed_resource::ParamsIncludeDirs(["packaging/windows", "assets/icons"]),
        )
        .manifest_required()
        .unwrap();
    }
}
