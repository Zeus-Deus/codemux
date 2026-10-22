//! Small source-level canaries for command wiring that is awkward to exercise
//! without constructing a full Tauri application.

use std::fs;
use std::path::PathBuf;

fn source_root() -> PathBuf {
    std::env::current_dir().expect("cwd")
}

fn read(relative: &str) -> String {
    let path = source_root().join(relative);
    fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()))
}

#[test]
fn lib_registers_quit_app_command() {
    let source = read("src/lib.rs");
    assert!(source.contains("commands::quit_app"));
}

#[test]
fn quit_app_uses_tauri_graceful_exit() {
    let source = read("src/commands/mod.rs");
    let (_, after_function) = source
        .split_once("pub fn quit_app")
        .expect("quit_app function");
    let body = after_function
        .split_once("\npub fn ")
        .map(|(body, _)| body)
        .unwrap_or(after_function);
    assert!(body.contains("app.exit"));
}
