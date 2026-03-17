//! Lightweight durable breadcrumbs for crash/restart attribution (OpenFlow, native startup, etc.).
//! Only written in debug builds; paths under .codemux/ are ignored by Vite and the index watcher.

use std::env;
use std::io::Write;
use std::path::PathBuf;

#[cfg(debug_assertions)]
fn breadcrumb_log_path(name: &str) -> PathBuf {
    let cwd = env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let dot = cwd.join(".codemux");
    if dot.exists() || cwd.join("package.json").exists() {
        return dot.join(name);
    }
    cwd.join("..").join(".codemux").join(name)
}

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
fn native_global_log_path() -> Option<PathBuf> {
    let runtime_dir = env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .or_else(|| Some(std::env::temp_dir()))?;
    Some(runtime_dir.join("codemux-native-launches.log"))
}

#[cfg(debug_assertions)]
fn append_debug_log(path: &PathBuf, line: &str) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(f, "{}", line);
        let _ = f.flush();
    }
}

/// Appends a line to .codemux/openflow-breadcrumbs.log (debug only).
/// Use for run_created, agents_spawned, run_stopped, agent_exited.
pub fn openflow_breadcrumb(line: &str) {
    #[cfg(debug_assertions)]
    {
        let path = breadcrumb_log_path("openflow-breadcrumbs.log");
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let _ = writeln!(f, "[{}] {}", ts, line);
            let _ = f.flush();
        }
    }
}

/// Writes a durable native startup breadcrumb (debug builds only).
///
/// This is intended to correlate multi-process launch attempts and the
/// control-socket lifecycle. It writes to both:
/// - `.codemux/native-startup.log` (project-local)
/// - `$XDG_RUNTIME_DIR/codemux-native-launches.log` (global per-user)
pub fn native_startup_breadcrumb(line: &str) {
    #[cfg(debug_assertions)]
    {
        if let Some(path) = native_startup_log_path() {
            append_debug_log(&path, line);
        }
        if let Some(path) = native_global_log_path() {
            append_debug_log(&path, line);
        }
    }
}

/// Writes a line to stderr without panicking if the stream is closed.
pub fn stderr_line(line: &str) {
    let mut stderr = std::io::stderr().lock();
    let _ = writeln!(stderr, "{}", line);
    let _ = stderr.flush();
}
