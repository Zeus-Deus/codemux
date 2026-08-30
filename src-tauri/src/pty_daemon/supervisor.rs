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
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::OnceCell;

/// Globally-cached client. Initialized lazily by `ensure_daemon`.
static CLIENT: OnceCell<Arc<PtyDaemonClient>> = OnceCell::const_new();

/// Crash circuit breaker state.
///
/// We track the most recent `ensure_daemon` failure timestamps. If `CRASH_BUDGET`
/// failures land within `CRASH_WINDOW`, the circuit opens and `circuit_is_open`
/// returns true until the process restarts. The spawn paths consult this and
/// silently fall back to the in-process PTY path so the user always gets a
/// working terminal, even if the daemon is fundamentally broken on their
/// system.
///
/// The circuit is intentionally *one-shot per process lifetime*: once tripped,
/// it stays tripped. A user who hits this likely has a deeper environmental
/// problem (no permissions in `$HOME`, the daemon binary is missing, etc.)
/// and our auto-retry would just burn battery. Restarting the app gives them
/// a fresh chance.
const CRASH_BUDGET: usize = 3;
const CRASH_WINDOW: Duration = Duration::from_secs(60);

static CIRCUIT_OPEN: AtomicBool = AtomicBool::new(false);
static FAILURE_TIMESTAMPS: Mutex<Vec<Instant>> = Mutex::new(Vec::new());
static TOTAL_FAILURES: AtomicU64 = AtomicU64::new(0);

/// True if the crash circuit breaker has tripped this process lifetime.
pub fn circuit_is_open() -> bool {
    CIRCUIT_OPEN.load(Ordering::Relaxed)
}

/// Total number of `ensure_daemon` failures observed this process lifetime.
/// Used by diagnostics + tests. Cheap atomic read.
#[allow(dead_code)]
pub fn total_failures() -> u64 {
    TOTAL_FAILURES.load(Ordering::Relaxed)
}

/// Record a failure. Trips the circuit if we exceed the budget within the
/// window. Returns `true` if this failure tripped the circuit.
fn record_failure() -> bool {
    TOTAL_FAILURES.fetch_add(1, Ordering::Relaxed);
    let now = Instant::now();
    let mut guard = FAILURE_TIMESTAMPS.lock().unwrap_or_else(|e| e.into_inner());
    // Evict failures older than the window so we only count recent ones.
    guard.retain(|t| now.duration_since(*t) <= CRASH_WINDOW);
    guard.push(now);
    if guard.len() >= CRASH_BUDGET && !CIRCUIT_OPEN.swap(true, Ordering::SeqCst) {
        eprintln!(
            "[codemux::pty_daemon::supervisor] crash circuit OPEN: {} ensure_daemon \
             failures within {:?}; further PTY spawns will use the in-process path \
             until the app restarts",
            guard.len(),
            CRASH_WINDOW
        );
        return true;
    }
    false
}

/// Reset the circuit breaker. Tests use this; production code does not.
/// Public (not `#[cfg(test)]`) so the integration test in
/// `tests/pty_daemon_circuit_breaker.rs` can call it — `#[cfg(test)]`
/// only enables items for the crate's own `cargo test` build, not for
/// out-of-tree integration test binaries.
#[doc(hidden)]
pub fn reset_circuit() {
    CIRCUIT_OPEN.store(false, Ordering::SeqCst);
    FAILURE_TIMESTAMPS.lock().unwrap().clear();
    TOTAL_FAILURES.store(0, Ordering::Relaxed);
}

/// Return a connected client, spawning + adopting as needed. Cheap on the
/// second call.
///
/// Errors here are counted against the crash circuit breaker. If we trip
/// the breaker, subsequent calls **fast-fail** with a sentinel error so
/// callers can drop to the in-process fallback without paying the spawn
/// or socket-timeout cost again.
pub async fn ensure_daemon() -> Result<Arc<PtyDaemonClient>, PtyDaemonError> {
    if circuit_is_open() {
        return Err(PtyDaemonError::Daemon(
            "circuit breaker open: too many recent failures, using in-process fallback".into(),
        ));
    }
    let result = CLIENT
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
        .cloned();
    if result.is_err() {
        record_failure();
    }
    result
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

    // The WebKitGTK renderer transport vars configure the app's own webview
    // and the pinned sidecar path belongs to this install; the daemon needs
    // neither, and every PTY child it spawns would inherit them. Strip here so
    // the whole daemon subtree starts clean (the leaf spawn strips again for
    // daemons adopted from an older app process).
    for key in crate::terminal::app_process_only_env_vars() {
        cmd.env_remove(key);
    }

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
