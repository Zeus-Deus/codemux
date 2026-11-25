//! Manifest file — single source of truth for the daemon's local
//! endpoint and bearer secret.
//!
//! On daemon boot, `serve` writes `<state_dir>/manifest.json`
//! containing the endpoint URL it bound, a freshly generated 32-byte
//! bearer secret, its pid, and the host id. Every other piece of code
//! (the MCP subcommand, the desktop's eventual `--host` SSH tunnel,
//! `serve status`) reads this file to find out where the daemon is
//! and what secret to present.
//!
//! File mode is `0600` and the containing directory is `0700` —
//! same-user filesystem trust, matching `gh`/`ssh-agent`/`docker`
//! conventions. On a single-user VPS this is the right level. On
//! multi-tenant hosts the user is expected to put `<state_dir>` on
//! a per-user-mode mount.

use std::path::Path;

use rand::RngCore;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    /// e.g. `http://127.0.0.1:54231`
    pub endpoint: String,
    /// 32-byte bearer secret encoded as URL-safe base64 (no padding).
    /// Every HTTP request to the daemon must carry this as
    /// `Authorization: Bearer <secret>`.
    pub secret: String,
    /// Daemon PID. `serve status` uses this to check liveness.
    pub pid: u32,
    /// RFC 3339 UTC timestamp of when the daemon booted.
    pub started_at: String,
    /// Stable host identifier (currently `hostname()`).
    pub host_id: String,
    /// Owner identity if known. Always `None` in v1 — populated when
    /// a future cloud relay forwards an authenticated user identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_id: Option<String>,
}

impl Manifest {
    pub fn new(endpoint: String, host_id: String) -> Self {
        Self {
            endpoint,
            secret: generate_secret(),
            pid: std::process::id(),
            started_at: chrono::Utc::now().to_rfc3339(),
            host_id,
            owner_id: None,
        }
    }
}

/// Write the manifest atomically to disk with mode 0600 and ensure
/// the parent directory is mode 0700. Replaces any existing file at
/// that path.
pub fn write(path: &Path, manifest: &Manifest) -> Result<(), String> {
    let parent = path.parent().ok_or_else(|| {
        "manifest path has no parent directory".to_string()
    })?;
    let name = path
        .file_name()
        .ok_or_else(|| "manifest path has no file name".to_string())?;
    let dir =
        super::config::open_private_dir(parent).map_err(|e| format!("create state dir: {e}"))?;

    let payload = serde_json::to_vec_pretty(manifest)
        .map_err(|e| format!("serialise manifest: {e}"))?;

    // The unique 0600 file and final rename are both relative to the same
    // verified directory handle. A stale legacy temp symlink is never opened.
    use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt};
    use cap_std::fs::{OpenOptions, OpenOptionsExt};
    use std::io::Write;
    let temp = format!(".manifest-{}.tmp", uuid::Uuid::new_v4());
    let mut options = OpenOptions::new();
    options
        .write(true)
        .create_new(true)
        .follow(FollowSymlinks::No)
        .mode(0o600);
    let result = (|| {
        let mut file = dir
            .open_with(&temp, &options)
            .map_err(|e| format!("open manifest tmpfile: {e}"))?;
        file.write_all(&payload)
            .map_err(|e| format!("write manifest: {e}"))?;
        file.sync_all()
            .map_err(|e| format!("fsync manifest: {e}"))?;
        drop(file);
        dir.rename(&temp, &dir, Path::new(name))
            .map_err(|e| format!("rename manifest into place: {e}"))
    })();
    if result.is_err() {
        let _ = dir.remove_file(&temp);
    }
    result
}

/// Read a private manifest through its verified state directory. Returns
/// `Ok(None)` when the directory or file is absent, and an error for unsafe
/// state, a symlink, an overexposed file, or malformed contents.
pub fn read(path: &Path) -> Result<Option<Manifest>, String> {
    use cap_fs_ext::{FollowSymlinks, OpenOptionsFollowExt};
    use cap_std::fs::{OpenOptions, OpenOptionsExt};
    use std::io::{ErrorKind, Read};
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    const MAX_MANIFEST_BYTES: u64 = 64 * 1024;

    let parent = path
        .parent()
        .ok_or_else(|| "manifest path has no state directory".to_string())?;
    let name = path
        .file_name()
        .ok_or_else(|| "manifest path has no file name".to_string())?;
    let Some(dir) = super::config::open_existing_private_dir(parent)
        .map_err(|e| format!("open manifest state directory: {e}"))?
    else {
        return Ok(None);
    };
    let mut options = OpenOptions::new();
    // Opening a FIFO for reading blocks until a writer connects. Reject it
    // from the opened handle without letting an invalid manifest stall CLI.
    options
        .read(true)
        .follow(FollowSymlinks::No)
        .custom_flags(libc::O_NONBLOCK);
    let file = match dir.open_with(name, &options) {
        Ok(file) => file.into_std(),
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(format!("open manifest: {e}")),
    };
    let metadata = file.metadata().map_err(|e| format!("stat manifest: {e}"))?;
    if !metadata.is_file()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.permissions().mode() & 0o077 != 0
    {
        return Err("manifest must be a regular owner-only file owned by this user".into());
    }
    if metadata.len() > MAX_MANIFEST_BYTES {
        return Err("manifest exceeds its size limit".into());
    }
    let mut bytes = Vec::new();
    file.take(MAX_MANIFEST_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("read manifest: {e}"))?;
    if bytes.len() as u64 > MAX_MANIFEST_BYTES {
        return Err("manifest exceeds its size limit".into());
    }
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|e| format!("parse manifest: {e}"))
}

/// Remove the manifest if present. Used on clean shutdown so a stale
/// manifest doesn't outlive the daemon.
pub fn remove(path: &Path) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("remove manifest: {e}")),
    }
}

/// Check whether a given pid is alive on this host. Used to decide
/// whether an existing manifest belongs to a still-running daemon
/// (singleton check) or is just leftover from a crash.
pub fn pid_alive(pid: u32) -> bool {
    #[cfg(unix)]
    {
        // `kill -0` returns 0 if the process exists and we can signal
        // it. ESRCH (no such process) is the negative case we care
        // about. EPERM (exists but we can't signal) still means alive.
        let pid = pid as libc::pid_t;
        // SAFETY: kill(pid, 0) is a syscall with no memory side
        // effects; only the return value matters.
        let rc = unsafe { libc::kill(pid, 0) };
        if rc == 0 {
            return true;
        }
        let err = std::io::Error::last_os_error();
        // EPERM means "exists, can't signal" — still alive.
        err.raw_os_error() == Some(libc::EPERM)
    }
    #[cfg(not(unix))]
    {
        let _ = pid;
        false
    }
}

fn generate_secret() -> String {
    let mut buf = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut buf);
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buf)
}

fn _host_id_placeholder() -> String {
    hostname::get()
        .ok()
        .and_then(|h| h.into_string().ok())
        .unwrap_or_else(|| "unknown-host".into())
}

pub fn current_host_id() -> String {
    _host_id_placeholder()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use std::os::unix::fs::PermissionsExt;

    fn private_tempdir() -> TempDir {
        tempfile::Builder::new()
            .permissions(std::fs::Permissions::from_mode(0o700))
            .tempdir()
            .unwrap()
    }

    #[test]
    fn write_and_read_roundtrip() {
        let dir = private_tempdir();
        let path = dir.path().join("manifest.json");
        let manifest = Manifest::new("http://127.0.0.1:1234".into(), "test-host".into());
        write(&path, &manifest).unwrap();

        let loaded = read(&path).unwrap().unwrap();
        assert_eq!(loaded.endpoint, manifest.endpoint);
        assert_eq!(loaded.secret, manifest.secret);
        assert_eq!(loaded.host_id, manifest.host_id);
        assert_eq!(loaded.owner_id, None);
    }

    #[test]
    fn read_missing_returns_none() {
        let dir = private_tempdir();
        let path = dir.path().join("nope.json");
        assert!(read(&path).unwrap().is_none());
    }

    #[test]
    fn read_missing_state_does_not_create_it() {
        let dir = private_tempdir();
        let state = dir.path().join("missing-state");
        assert!(read(&state.join("manifest.json")).unwrap().is_none());
        assert!(!state.exists());
    }

    #[test]
    fn read_rejects_symlinked_and_overexposed_manifest() {
        let state = private_tempdir();
        let outside = private_tempdir();
        let target = outside.path().join("outside.json");
        let manifest = Manifest::new("http://127.0.0.1:1".into(), "h".into());
        write(&target, &manifest).unwrap();
        let path = state.path().join("manifest.json");
        std::os::unix::fs::symlink(&target, &path).unwrap();
        assert!(read(&path).is_err());
        assert_eq!(read(&target).unwrap().unwrap().secret, manifest.secret);

        std::fs::remove_file(&path).unwrap();
        std::fs::write(&path, serde_json::to_vec(&manifest).unwrap()).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(read(&path).unwrap_err().contains("owner-only"));
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(read(&path).unwrap().unwrap().secret, manifest.secret);
    }

    #[test]
    fn read_rejects_oversized_manifest() {
        let state = private_tempdir();
        let path = state.path().join("manifest.json");
        std::fs::write(&path, vec![b'x'; 64 * 1024 + 1]).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(read(&path).unwrap_err().contains("size limit"));
    }

    #[test]
    fn read_rejects_fifo_without_waiting_for_a_writer() {
        use std::os::unix::ffi::OsStrExt;
        use std::sync::mpsc;
        use std::time::Duration;

        let state = private_tempdir();
        let path = state.path().join("manifest.json");
        let name = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        // SAFETY: `name` is a valid NUL-terminated path in this test's private directory.
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);

        let (sender, receiver) = mpsc::channel();
        let reader = std::thread::spawn(move || sender.send(read(&path)).unwrap());
        let result = receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("reading a FIFO manifest must not block");
        assert!(result.unwrap_err().contains("regular owner-only"));
        reader.join().unwrap();
    }

    #[test]
    fn remove_idempotent() {
        let dir = private_tempdir();
        let path = dir.path().join("manifest.json");
        // Removing a non-existent manifest is fine.
        remove(&path).unwrap();
        // After writing, remove works.
        let manifest = Manifest::new("http://127.0.0.1:1".into(), "h".into());
        write(&path, &manifest).unwrap();
        assert!(path.exists());
        remove(&path).unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn manifest_file_mode_is_0600() {
        let dir = private_tempdir();
        let path = dir.path().join("manifest.json");
        let m = Manifest::new("http://127.0.0.1:1".into(), "h".into());
        write(&path, &m).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "manifest must be 0600, got {mode:o}");
    }

    #[test]
    fn stale_temp_symlink_cannot_redirect_manifest_write() {
        let state = private_tempdir();
        let outside = private_tempdir();
        let target = outside.path().join("unrelated.txt");
        std::fs::write(&target, b"keep this file").unwrap();
        let path = state.path().join("manifest.json");
        let legacy_temp = path.with_extension("json.tmp");
        std::os::unix::fs::symlink(&target, &legacy_temp).unwrap();
        std::os::unix::fs::symlink(&target, &path).unwrap();

        let manifest = Manifest::new("http://127.0.0.1:1".into(), "h".into());
        write(&path, &manifest).unwrap();

        assert_eq!(std::fs::read(&target).unwrap(), b"keep this file");
        assert!(std::fs::symlink_metadata(&legacy_temp)
            .unwrap()
            .file_type()
            .is_symlink());
        assert!(!std::fs::symlink_metadata(&path)
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(read(&path).unwrap().unwrap().secret, manifest.secret);
    }

    #[test]
    fn secrets_are_unique_across_writes() {
        let m1 = Manifest::new("a".into(), "h".into());
        let m2 = Manifest::new("a".into(), "h".into());
        assert_ne!(m1.secret, m2.secret, "secrets must not collide");
        assert_eq!(m1.secret.len(), 43, "32 bytes URL-safe base64 unpadded is 43 chars");
    }

    #[test]
    fn pid_alive_for_self() {
        let me = std::process::id();
        assert!(pid_alive(me), "our own pid must be alive");
    }

    #[test]
    fn pid_alive_for_bogus_pid_is_false() {
        // PID 0 is the kernel scheduler on Linux; kill(0, 0) returns
        // success but means "all processes in our pgroup," which isn't
        // what we want to test. Pick a value that's almost certainly
        // not in use.
        let very_high_pid = i32::MAX as u32 - 1;
        assert!(!pid_alive(very_high_pid));
    }
}
