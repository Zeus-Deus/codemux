// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::env;
use std::os::unix::net::UnixStream;
use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    // Generate unique startup ID to track multiple instances
    let startup_id = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();

    // Debug: Log startup info to help diagnose duplicate spawns
    #[cfg(debug_assertions)]
    {
        eprintln!("[DEBUG] ═══ Codemux starting [{}] ═══", startup_id);
        eprintln!("[DEBUG] Args: {:?}", env::args().collect::<Vec<_>>());

        // Check if launched by another process
        if let Ok(parent_pid) = env::var("CODEMUX_PARENT_PID") {
            eprintln!("[DEBUG] Launched by parent PID: {}", parent_pid);
        }

        // Check environment for agent mode
        if let Ok(agent_mode) = env::var("CODEMUX_AGENT_MODE") {
            eprintln!("[DEBUG] Agent mode: {}", agent_mode);
        }

        // Check what launched us
        if let Ok(parent) = env::var("PARENT_PROCESS") {
            eprintln!("[DEBUG] Parent process: {}", parent);
        }

        // Check if already have codemux running (socket exists)
        if let Some(socket_path) = codemux_lib::control::control_socket_path() {
            if socket_path.exists() {
                eprintln!(
                    "[DEBUG] WARNING: Control socket exists at {:?}",
                    socket_path
                );
            }
        }
    }

    match tauri::async_runtime::block_on(codemux_lib::cli::maybe_run_cli()) {
        Ok(true) => {
            // CLI command handled successfully; do not start the GUI.
            return;
        }
        Ok(false) => {
            // No CLI subcommand; proceed to start the GUI below.
        }
        Err(error) => {
            eprintln!("[codemux] CLI command failed: {error}");
            // On CLI failure, exit with a non-zero status instead of starting a new GUI instance.
            std::process::exit(1);
        }
    }

    // Basic single-instance guard: if a control socket exists and responds, assume another
    // Codemux instance is already running and exit instead of launching a second GUI.
    if let Some(socket_path) = codemux_lib::control::control_socket_path() {
        if let Ok(stream) = UnixStream::connect(&socket_path) {
            drop(stream);
            eprintln!(
                "[codemux] Existing Codemux instance detected via control socket at {:?}; exiting.",
                socket_path
            );
            return;
        }
    }

    codemux_lib::run()
}
