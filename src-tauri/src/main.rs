// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::env;
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(all(target_os = "windows", not(debug_assertions)))]
fn hide_console_for_release_build() {
    // Raw FFI to kernel32 + user32 — kept inline to avoid pulling in winapi/
    // windows-sys as an explicit dep (both are already in the transitive tree
    // but we don't depend on them directly).
    use std::ffi::c_void;
    type HWND = *mut c_void;
    type BOOL = i32;
    const SW_HIDE: i32 = 0;
    extern "system" {
        fn AllocConsole() -> BOOL;
        fn GetConsoleWindow() -> HWND;
        fn ShowWindow(hwnd: HWND, ncmdshow: i32) -> BOOL;
    }
    unsafe {
        // AllocConsole returns FALSE if the process already has a console;
        // either way we just need a console *handle* to hide.
        let _ = AllocConsole();
        let hwnd = GetConsoleWindow();
        if !hwnd.is_null() {
            let _ = ShowWindow(hwnd, SW_HIDE);
        }
    }
}

#[cfg(all(debug_assertions, unix))]
mod native_signal_log {
    use std::env;
    use std::fs::OpenOptions;
    use std::os::unix::io::AsRawFd;
    use std::path::PathBuf;

    static mut NATIVE_LOG_FD: i32 = -1;

    fn path() -> Option<PathBuf> {
        let cwd = env::current_dir().ok()?;
        let dot = cwd.join(".codemux");
        if dot.exists() || cwd.join("package.json").exists() {
            return Some(dot.join("native-startup.log"));
        }
        let dot_alt = cwd.join("..").join(".codemux");
        if dot_alt.exists() {
            return Some(dot_alt.join("native-startup.log"));
        }
        Some(dot.join("native-startup.log"))
    }

    extern "C" fn handle_signal(sig: i32) {
        let msg: &[u8] = match sig {
            libc::SIGTERM => b"native_signal sig=TERM\n",
            libc::SIGINT => b"native_signal sig=INT\n",
            libc::SIGHUP => b"native_signal sig=HUP\n",
            _ => b"native_signal sig=?\n",
        };
        let fd = unsafe { NATIVE_LOG_FD };
        if fd >= 0 {
            unsafe { libc::write(fd, msg.as_ptr() as *const _, msg.len()); }
        }
        unsafe { libc::signal(sig, libc::SIG_DFL); }
        unsafe { libc::raise(sig); }
    }

    fn handler_as_sighandler_t() -> libc::sighandler_t {
        handle_signal as *const () as libc::sighandler_t
    }

    pub fn install() {
        let path = match path() {
            Some(p) => p,
            None => return,
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(f) = OpenOptions::new().create(true).append(true).open(&path) {
            let fd = f.as_raw_fd();
            std::mem::forget(f);
            unsafe { NATIVE_LOG_FD = fd; }
            unsafe {
                libc::signal(libc::SIGTERM, handler_as_sighandler_t());
                libc::signal(libc::SIGINT, handler_as_sighandler_t());
                libc::signal(libc::SIGHUP, handler_as_sighandler_t());
            }
        }
    }
}

fn main() {
    // Generate unique startup ID to track multiple instances
    let startup_id = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();

    // Make the startup ID available to code that runs after tauri setup.
    env::set_var("CODEMUX_STARTUP_ID", startup_id.to_string());

    let pid = std::process::id();
    let cwd = env::current_dir()
        .ok()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "<unknown>".to_string());
    let parent_pid = env::var("CODEMUX_PARENT_PID").ok();
    let socket_existed = codemux_lib::control::control_socket_path()
        .map(|p| p.exists())
        .unwrap_or(false);
    #[cfg(debug_assertions)]
    codemux_lib::diagnostics::native_startup_breadcrumb(&format!(
        "[{}] startup_id={} pid={} parent_pid={:?} cwd={} argv={:?} socket_existed={}",
        chrono_timestamp(),
        startup_id,
        pid,
        parent_pid,
        cwd,
        env::args().collect::<Vec<_>>(),
        socket_existed
    ));

    // Debug: Log startup info to help diagnose duplicate spawns
    #[cfg(debug_assertions)]
    {
        codemux_lib::diagnostics::stderr_line(&format!(
            "[DEBUG] ═══ Codemux starting [{}] ═══",
            startup_id
        ));
        codemux_lib::diagnostics::stderr_line(&format!(
            "[DEBUG] Args: {:?}",
            env::args().collect::<Vec<_>>()
        ));

        if let Ok(pp) = env::var("CODEMUX_PARENT_PID") {
            codemux_lib::diagnostics::stderr_line(&format!(
                "[DEBUG] Launched by parent PID: {}",
                pp
            ));
        }
        if let Ok(agent_mode) = env::var("CODEMUX_AGENT_MODE") {
            codemux_lib::diagnostics::stderr_line(&format!(
                "[DEBUG] Agent mode: {}",
                agent_mode
            ));
        }
        if let Ok(parent) = env::var("PARENT_PROCESS") {
            codemux_lib::diagnostics::stderr_line(&format!(
                "[DEBUG] Parent process: {}",
                parent
            ));
        }
        if let Some(socket_path) = codemux_lib::control::control_socket_path() {
            if socket_path.exists() {
                codemux_lib::diagnostics::stderr_line(&format!(
                    "[DEBUG] WARNING: Control socket exists at {:?}",
                    socket_path
                ));
            }
        }
    }

    match tauri::async_runtime::block_on(codemux_lib::cli::maybe_run_cli()) {
        Ok(codemux_lib::cli::CliOutcome::Handled) => {
            #[cfg(debug_assertions)]
            codemux_lib::diagnostics::native_startup_breadcrumb(&format!(
                "[{}] startup_id={} outcome=cli_handled",
                chrono_timestamp(),
                startup_id
            ));
            return;
        }
        Ok(codemux_lib::cli::CliOutcome::RunServe(opts)) => {
            // `codemux serve` is a long-lived foreground process, not a
            // control round-trip. Run it on the main thread (mirroring how the
            // GUI's `run()` is invoked below) — it boots the full backend
            // headless and blocks until a shutdown signal, then exits. The
            #[cfg(debug_assertions)]
            codemux_lib::diagnostics::native_startup_breadcrumb(&format!(
                "[{}] startup_id={} outcome=run_serve",
                chrono_timestamp(),
                startup_id
            ));
            if let Err(error) = codemux_lib::run_serve(opts) {
                codemux_lib::diagnostics::stderr_line(&format!(
                    "[codemux serve] {error}"
                ));
                std::process::exit(1);
            }
            return;
        }
        Ok(codemux_lib::cli::CliOutcome::LaunchGui) => {}
        Err(error) => {
            codemux_lib::diagnostics::stderr_line(&format!(
                "[codemux] CLI command failed: {error}"
            ));
            #[cfg(debug_assertions)]
            codemux_lib::diagnostics::native_startup_breadcrumb(&format!(
                "[{}] startup_id={} outcome=cli_error error={}",
                chrono_timestamp(),
                startup_id,
                error
            ));
            std::process::exit(1);
        }
    }

    if codemux_lib::control::control_server_is_running() {
        let socket_path = codemux_lib::control::control_socket_path();
        codemux_lib::diagnostics::stderr_line(&format!(
            "[codemux] Existing Codemux instance detected via control endpoint at {:?}; exiting.",
            socket_path
        ));
        #[cfg(debug_assertions)]
        codemux_lib::diagnostics::native_startup_breadcrumb(&format!(
            "[{}] startup_id={} outcome=single_instance_exit",
            chrono_timestamp(),
            startup_id
        ));
        return;
    }

    #[cfg(debug_assertions)]
    codemux_lib::diagnostics::native_startup_breadcrumb(&format!(
        "[{}] startup_id={} outcome=run_gui",
        chrono_timestamp(),
        startup_id
    ));

    #[cfg(debug_assertions)]
    {
        let default = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            codemux_lib::diagnostics::native_startup_breadcrumb(&format!(
                "[{}] outcome=panic payload={:?}",
                chrono_timestamp(),
                info
            ));
            default(info);
        }));
    }

    #[cfg(all(debug_assertions, unix))]
    native_signal_log::install();

    // Allocate a hidden console for ConPTY children to inherit.
    // Release builds use `windows_subsystem = "windows"` (no console attached),
    // which causes the Windows pseudoconsole API to allocate a fallback visible
    // console window per spawned child. portable-pty 0.8.1 honors the
    // pseudoconsole attribute correctly when the parent has *some* console;
    // by allocating one here and immediately hiding it, ConPTY children inherit
    // the hidden console handle and no popup appears. This is the same pattern
    // node-pty / VS Code use for Windows ConPTY in GUI-subsystem hosts.
    //
    // Skipped in debug builds because cargo run inherits the developer's
    // terminal; allocating a second console would orphan keystrokes and SW_HIDE
    // would hide the wrong window.
    #[cfg(all(target_os = "windows", not(debug_assertions)))]
    hide_console_for_release_build();

    codemux_lib::run();

    #[cfg(debug_assertions)]
    codemux_lib::diagnostics::native_startup_breadcrumb(&format!(
        "[{}] startup_id={} outcome=run_returned",
        chrono_timestamp(),
        startup_id
    ));
}

#[cfg(debug_assertions)]
fn chrono_timestamp() -> String {
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    t.to_string()
}
