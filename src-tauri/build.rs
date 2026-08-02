fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        embed_resource::compile_for_tests("test-common-controls-v6.rc", embed_resource::NONE)
            .manifest_required()
            .expect("failed to embed Common Controls v6 manifest in Windows tests");
    }
    tauri_build::build()
}
