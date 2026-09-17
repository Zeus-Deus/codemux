fn main() {
    // Generate the app context here, in the build script, rather than via the
    // `tauri::generate_context!()` proc macro. The macro writes cached files
    // (the decoded window icon) into OUT_DIR *while* the lib is compiling, and
    // the lib tracks them as inputs, so the next cargo invocation always saw a
    // "changed" input and rebuilt the entire crate. In CI that meant the
    // Windows job compiled everything twice (~6.5 min per run). Doing the
    // codegen inside the build script writes those files before compilation
    // starts, so the fingerprint stays fresh.
    //
    // `dev`/`test` semantics are identical to the macro's defaults: `dev` comes
    // from tauri's `custom-protocol` feature via `cargo:dev`, and `test` is
    // false (we never passed `test = true` to the macro).
    tauri_build::try_build(
        tauri_build::Attributes::new().codegen(tauri_build::CodegenContext::new()),
    )
    .expect("tauri-build failed");
}
