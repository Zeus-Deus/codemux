//! Adoption + spawn-detached for the PTY daemon.
//!
//! Boot flow on Tauri startup:
//!
//! 1. Read manifest. If present, dial the socket and send `Hello`. If the
//!    handshake succeeds and the protocol version matches, **adopt** —
//!    reuse the daemon. PTYs from the previous run are still alive.
//! 2. Otherwise, spawn a fresh `codemux pty-daemon` process **detached**
//!    (Unix: `setsid`; Windows: `DETACHED_PROCESS`), wait for it to write
//!    its manifest, then dial.
//!
//! The supervisor caches the connected `PtyDaemonClient` in a `OnceCell`
//! so all subsequent Tauri calls share one socket.

use crate::pty_daemon::client::{PtyDaemonClient, PtyDaemonError};
use crate::pty_daemon::manifest::{manifest_path, read_manifest, socket_dir};
use crate::pty_daemon::protocol::PROTOCOL_VERSION;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::OnceCell;

/// Globally-cached client. Initialized lazily by `ensure_daemon`.
static CLIENT: OnceCell<Arc<PtyDaemonClient>> = OnceCell::const_new();

/// Return a connected client, spawning + adopting as needed. Cheap on the
/// second call.
pub async fn ensure_daemon() -> Result<Arc<PtyDaemonClient>, PtyDaemonError> {
    CLIENT
        .get_or_try_init(|| async {
            // Try adoption first.
            if let Some(client) = try_adopt().await {
                return Ok(client);
            }
            // No usable daemon; spawn one.
            let socket_path = spawn_daemon_detached().await?;
            // Poll for the socket to appear (the daemon races against us).
            wait_for_socket(&socket_path, Duration::from_secs(5)).await?;
            let client = PtyDaemonClient::connect(&socket_path).await?;
            // Sanity-check the handshake.
            let (_pid, _ver, proto) = client.hello().await?;
            if proto != PROTOCOL_VERSION {
                return Err(PtyDaemonError::Daemon(format!(
                    "freshly spawned daemon speaks protocol {proto}, expected {PROTOCOL_VERSION}"
                )));
            }
            Ok(client)
        })
        .await
        .cloned()
}

async fn try_adopt() -> Option<Arc<PtyDaemonClient>> {
    let manifest = read_manifest()?;
    // Cheap liveness check: kill(pid, 0). On Unix, returns 0 if the process
    // exists. If it's a different process with our recycled pid, the Hello
    // handshake will fail and we'll fall through to a fresh spawn.
    #[cfg(unix)]
    {
        let ret = unsafe { libc::kill(manifest.pid as i32, 0) };
        if ret != 0 {
            eprintln!(
                "[codemux::pty_daemon::supervisor] manifest pid {} not alive, ignoring",
                manifest.pid
            );
            return None;
        }
    }
    let client = match PtyDaemonClient::connect(&manifest.socket_path).await {
        Ok(c) => c,
        Err(error) => {
            eprintln!(
                "[codemux::pty_daemon::supervisor] adopt connect failed: {error}"
            );
            return None;
        }
    };
    match client.hello().await {
        Ok((_pid, ver, proto)) => {
            if proto != PROTOCOL_VERSION {
                eprintln!(
                    "[codemux::pty_daemon::supervisor] adopted daemon speaks protocol \
                     {proto}, expected {PROTOCOL_VERSION}; will not adopt"
                );
                // TODO(phase-2): graceful shutdown + respawn. For now we
                // just ignore the old daemon and spawn a fresh one, which
                // means the old PTYs are orphaned. Acceptable for the MVP
                // because protocol bumps will be rare.
                return None;
            }
            eprintln!(
                "[codemux::pty_daemon::supervisor] adopted daemon pid={} version={ver}",
                manifest.pid
            );
            Some(client)
        }
        Err(error) => {
            eprintln!(
                "[codemux::pty_daemon::supervisor] adopt handshake failed: {error}"
            );
            None
        }
    }
}

async fn spawn_daemon_detached() -> Result<PathBuf, PtyDaemonError> {
    let socket_path = choose_socket_path()?;

    // Make sure the socket dir exists so the daemon's bind doesn't have
    // to race to create it.
    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let current_exe = std::env::current_exe()?;

    let mut cmd = std::process::Command::new(&current_exe);
    cmd.arg("pty-daemon")
        .arg("--socket")
        .arg(&socket_path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // setsid → new process group + session → fully detached from the
        // Tauri app's controlling terminal. When the app exits, the kernel
        // does NOT send SIGHUP to the daemon (it's in its own session).
        unsafe {
            cmd.pre_exec(|| {
                // SAFETY: setsid is async-signal-safe.
                if libc::setsid() == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // DETACHED_PROCESS = 0x00000008, CREATE_NEW_PROCESS_GROUP = 0x00000200
        cmd.creation_flags(0x00000008 | 0x00000200);
    }

    let child = cmd.spawn()?;
    eprintln!(
        "[codemux::pty_daemon::supervisor] spawned daemon pid={} socket={:?}",
        child.id(),
        socket_path
    );
    // We intentionally don't keep the Child handle — we want this to be a
    // grandchild that survives us. Dropping `child` does NOT kill the
    // process; std::process::Child only kills on drop if you call
    // `.kill()` first.

    Ok(socket_path)
}

fn choose_socket_path() -> Result<PathBuf, PtyDaemonError> {
    let dir = socket_dir().ok_or_else(|| {
        PtyDaemonError::Daemon(
            "could not determine socket dir (HOME unset?)".to_string(),
        )
    })?;
    // Mirror superset's short-name strategy. macOS sun_path is 104 bytes;
    // we use a short fixed name under the per-build data dir so we stay
    // well under that limit.
    Ok(dir.join("ptyd.sock"))
}

async fn wait_for_socket(path: &PathBuf, deadline: Duration) -> Result<(), PtyDaemonError> {
    let start = std::time::Instant::now();
    while start.elapsed() < deadline {
        if path.exists() {
            // Give the daemon a beat to actually call bind() after creating
            // the file — listener.accept races against our connect.
            tokio::time::sleep(Duration::from_millis(50)).await;
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    Err(PtyDaemonError::Daemon(format!(
        "daemon socket {:?} did not appear within {:?}",
        path, deadline
    )))
}

/// Returns the manifest path for diagnostics surfaces (settings panel,
/// debug commands). Returns `None` if the data dir can't be located.
pub fn diagnostics_manifest_path() -> Option<PathBuf> {
    manifest_path()
}
