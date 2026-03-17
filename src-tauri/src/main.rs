// Prevents additional console window on Windows in release, DO NOT REMOVE!!
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::env;
use std::io::Write;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(debug_assertions)]
fn native_startup_log_path() -> Option<PathBuf> {
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

#[cfg(debug_assertions)]
fn native_startup_log(line: &str) {
    if let Some(path) = native_startup_log_path() {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            let _ = writeln!(f, "{}", line);
            let _ = f.flush();
        }
    }
}

#[cfg(not(debug_assertions))]
fn native_startup_log(_line: &str) {}

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

    let pid = std::process::id();
    let parent_pid = env::var("CODEMUX_PARENT_PID").ok();
    let socket_existed = codemux_lib::control::control_socket_path()
        .map(|p| p.exists())
        .unwrap_or(false);
    #[cfg(debug_assertions)]
    native_startup_log(&format!(
        "[{}] startup_id={} pid={} parent_pid={:?} argv={:?} socket_existed={}",
        chrono_timestamp(),
        startup_id,
        pid,
        parent_pid,
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
        Ok(true) => {
            #[cfg(debug_assertions)]
            native_startup_log(&format!(
                "[{}] startup_id={} outcome=cli_handled",
                chrono_timestamp(),
                startup_id
            ));
            return;
        }
        Ok(false) => {}
        Err(error) => {
            codemux_lib::diagnostics::stderr_line(&format!(
                "[codemux] CLI command failed: {error}"
            ));
            #[cfg(debug_assertions)]
            native_startup_log(&format!(
                "[{}] startup_id={} outcome=cli_error error={}",
                chrono_timestamp(),
                startup_id,
                error
            ));
            std::process::exit(1);
        }
    }

    if let Some(socket_path) = codemux_lib::control::control_socket_path() {
        if let Ok(stream) = UnixStream::connect(&socket_path) {
            drop(stream);
            codemux_lib::diagnostics::stderr_line(&format!(
                "[codemux] Existing Codemux instance detected via control socket at {:?}; exiting.",
                socket_path
            ));
            #[cfg(debug_assertions)]
            native_startup_log(&format!(
                "[{}] startup_id={} outcome=single_instance_exit",
                chrono_timestamp(),
                startup_id
            ));
            return;
        }
    }

    #[cfg(debug_assertions)]
    native_startup_log(&format!(
        "[{}] startup_id={} outcome=run_gui",
        chrono_timestamp(),
        startup_id
    ));

    #[cfg(debug_assertions)]
    {
        let default = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            native_startup_log(&format!(
                "[{}] outcome=panic payload={:?}",
                chrono_timestamp(),
                info
            ));
            default(info);
        }));
    }

    #[cfg(all(debug_assertions, unix))]
    native_signal_log::install();

    codemux_lib::run();

    #[cfg(debug_assertions)]
    native_startup_log(&format!(
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
