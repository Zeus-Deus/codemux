//! On-disk manifest that lets a freshly-started Tauri app discover a still-
//! running `codemux pty-daemon` from a previous run and adopt it instead of
//! spawning a duplicate.
//!
//! Layout (Linux): `~/.local/share/codemux[-dev]/pty-daemon-manifest.json`.
//!
//! The manifest is intentionally tiny — just enough to find the daemon. The
//! protocol's `Hello` handshake validates that the process at `pid` is
//! actually our daemon at the expected version; the manifest itself is just
//! a hint that may be stale.
//!
//! Writes are atomic (`tempfile` + rename) so a crash mid-write never leaves
//! a half-truncated file the next adoption attempt would choke on.

use serde::{Deserialize, Serialize};
use std::fs;
use std::io::{ErrorKind, Write};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DaemonManifest {
    /// PID of the running `codemux pty-daemon` process.
    pub pid: u32,
    /// Absolute path to the daemon's listening socket (Unix sockets only,
    /// for now — Windows named-pipe support tracked in `mod.rs`).
    pub socket_path: PathBuf,
    /// Daemon binary version (matches `CARGO_PKG_VERSION`). Drives the
    /// "your daemon is older than your app, restart it" path.
    pub daemon_version: String,
    /// Wire protocol version (see `protocol::PROTOCOL_VERSION`).
    pub protocol_version: u32,
    /// Unix epoch seconds. Diagnostic only.
    pub started_at: i64,
}

/// Returns the canonical manifest path under the per-build data dir.
///
/// Debug builds use the `codemux-dev` data dir (see `lib.rs::APP_DIR_NAME`)
/// so a locally-running dev build doesn't clobber the release build's
/// daemon manifest. Tests can override the parent dir via the
/// `CODEMUX_PTY_DAEMON_DIR` env var.
pub fn manifest_path() -> Option<PathBuf> {
    if let Ok(override_dir) = std::env::var("CODEMUX_PTY_DAEMON_DIR") {
        return Some(PathBuf::from(override_dir).join("pty-daemon-manifest.json"));
    }
    let data_dir = dirs::data_local_dir()?.join(crate::APP_DIR_NAME);
    Some(data_dir.join("pty-daemon-manifest.json"))
}

/// Returns the directory the daemon should put its socket in. Same parent
/// as the manifest, so cleanup is one `rm -r` away.
pub fn socket_dir() -> Option<PathBuf> {
    manifest_path().and_then(|p| p.parent().map(|p| p.to_path_buf()))
}

pub fn read_manifest() -> Option<DaemonManifest> {
    let path = manifest_path()?;
    match fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text).ok(),
        Err(error) if error.kind() == ErrorKind::NotFound => None,
        Err(error) => {
            eprintln!(
                "[codemux::pty_daemon::manifest] failed to read {:?}: {error}",
                path
            );
            None
        }
    }
}

/// Atomic write: serialize to a sibling tempfile, fsync, rename.
///
/// We can't use `tempfile::NamedTempFile::persist` here because it may fail
/// across filesystems; we control both source and target so a plain
/// `fs::rename` on the same directory is fine and atomic on POSIX.
pub fn write_manifest(manifest: &DaemonManifest) -> std::io::Result<()> {
    let path = manifest_path().ok_or_else(|| {
        std::io::Error::new(
            ErrorKind::Other,
            "could not determine manifest path (HOME unset?)",
        )
    })?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(manifest)
        .map_err(|e| std::io::Error::new(ErrorKind::Other, e))?;
    let tmp = path.with_extension("json.tmp");
    {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(json.as_bytes())?;
        f.sync_all()?;
    }
    fs::rename(&tmp, &path)?;
    Ok(())
}

/// Best-effort manifest deletion. Called on clean daemon shutdown.
pub fn remove_manifest() {
    if let Some(path) = manifest_path() {
        let _ = fs::remove_file(path);
    }
}
