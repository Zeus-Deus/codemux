fn main() {
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rustc-link-arg-tests=/DELAYLOAD:comctl32.dll");
        println!("cargo:rustc-link-arg-tests=delayimp.lib");
    }
    tauri_build::build()
}
