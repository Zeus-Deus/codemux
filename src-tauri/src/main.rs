// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if let Ok(true) = tauri::async_runtime::block_on(codemux_lib::cli::maybe_run_cli()) {
        return;
    }

    codemux_lib::run()
}
