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

/// Writes a line to stderr without panicking if the stream is closed.
pub fn stderr_line(line: &str) {
    let mut stderr = std::io::stderr().lock();
    let _ = writeln!(stderr, "{}", line);
    let _ = stderr.flush();
}
