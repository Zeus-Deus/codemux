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
//! <state_dir>/logs/           # detached worktree-setup logs
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

/// Create `dir` (and any missing parents) owner-only, and tighten it to
/// 0700 when it already exists. The state dir is what keeps the registry,
/// setup logs and daemon secret away from other local users, and one-shot
/// CLI calls create it as well as `serve`, so every entry point uses this.
/// Only `dir` itself is chmodded; existing parents are left alone.
///
/// Creating the dir is fatal; tightening an existing one is best-effort, so
/// a `--state-dir` this user does not own (or a mount that rejects chmod)
/// still works as it did before, with a warning on stderr.
pub fn ensure_private_dir(dir: &std::path::Path) -> std::io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
    // A bare relative file name has an empty parent: the current directory,
    // which is not ours to chmod.
    if dir.as_os_str().is_empty() {
        return Ok(());
    }
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    tighten(dir, std::fs::Permissions::from_mode(0o700));
    Ok(())
}

/// Best-effort chmod of a state path that may predate the owner-only modes.
pub(crate) fn tighten(path: &std::path::Path, perms: std::fs::Permissions) {
    if let Err(e) = std::fs::set_permissions(path, perms) {
        eprintln!("[codemux-remote] could not restrict permissions on {}: {e}", path.display());
    }
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

/// Logs of worktree setup commands that `worktree create` ran detached
/// because no daemon was up to own them.
pub fn logs_dir(state_dir: &std::path::Path) -> PathBuf {
    state_dir.join("logs")
}
