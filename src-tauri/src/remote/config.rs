//! State-dir + manifest path resolution.
//!
//! The daemon's whole on-disk footprint lives under one directory,
//! by default `~/.local/share/codemux-remote/`. Everything is
//! addressable as `<state_dir>/<thing>`:
//!
//! ```text
//! <state_dir>/manifest.json   # endpoint + secret + pid + started_at
//! <state_dir>/codemux.db      # SQLite workspace registry
//! <state_dir>/workspaces/     # worktree storage
//! <state_dir>/serve.log       # daemonized-mode log
//! ```

use std::path::PathBuf;

/// Default state directory for `codemux-remote serve`.
///
/// `XDG_DATA_HOME` / `~/.local/share/codemux-remote` on Linux.
/// `~/Library/Application Support/codemux-remote` on macOS.
/// Falls back to `~/.codemux-remote` if `dirs::data_dir()` returns
/// `None` (extremely unusual).
pub fn default_state_dir() -> PathBuf {
    if let Some(data_dir) = dirs::data_dir() {
        return data_dir.join("codemux-remote");
    }
    if let Some(home) = dirs::home_dir() {
        return home.join(".codemux-remote");
    }
    // Last resort. Anyone hitting this has bigger problems than where
    // their state directory lives.
    PathBuf::from(".codemux-remote")
}

/// Manifest file inside a given state-dir.
pub fn manifest_path(state_dir: &std::path::Path) -> PathBuf {
    state_dir.join("manifest.json")
}

/// SQLite workspace registry inside a given state-dir.
pub fn database_path(state_dir: &std::path::Path) -> PathBuf {
    state_dir.join("codemux.db")
}

/// Worktree storage root inside a given state-dir. The daemon checks
/// each new workspace's worktree out under
/// `<state_dir>/workspaces/<workspace_id>/`.
pub fn workspaces_root(state_dir: &std::path::Path) -> PathBuf {
    state_dir.join("workspaces")
}

/// Serve mode log file (only used when daemonized).
pub fn serve_log_path(state_dir: &std::path::Path) -> PathBuf {
    state_dir.join("serve.log")
}
