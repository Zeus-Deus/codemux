fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let response_file = std::path::Path::new(&std::env::var("OUT_DIR").unwrap())
            .join("test-common-controls-v6.rsp");
        std::fs::write(
            &response_file,
            r#"/MANIFEST:EMBED
/MANIFESTDEPENDENCY:"type='win32' name='Microsoft.Windows.Common-Controls' version='6.0.0.0' processorArchitecture='amd64' publicKeyToken='6595b64144ccf1df' language='*'"
"#,
        )
        .expect("failed to write Windows test linker response file");
        println!("cargo:rustc-link-arg-tests=@{}", response_file.display());
    }
    tauri_build::build()
}
