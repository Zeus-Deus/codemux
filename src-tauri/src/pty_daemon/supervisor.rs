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
//! Exception to step 2: if the manifest's pid is alive and its socket accepts
//! our dial but `Hello` times out (or the peer hangs up mid-handshake), we do
//! **not** spawn. A replacement would unlink that daemon's socket and rewrite
//! the manifest, stranding every shell it still owns; the caller gets a
//! retry-only `Timeout`/`Closed` instead.
//!
//! The supervisor caches the connected `PtyDaemonClient` behind a small async
//! mutex so all subsequent Tauri calls share one live socket and an EOF can
//! evict that socket for reconnection.

use crate::pty_daemon::client::{PtyDaemonClient, PtyDaemonError};
use crate::pty_daemon::manifest::{manifest_path, read_manifest, socket_dir};
use crate::pty_daemon::protocol::PROTOCOL_VERSION;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::Mutex as AsyncMutex;

/// Globally-cached live client. Holding the mutex across initialization also
/// makes reconnect singleflight: a daemon restart cannot make every pane spawn
/// its own replacement connection.
static CLIENT: AsyncMutex<Option<Arc<PtyDaemonClient>>> = AsyncMutex::const_new(None);

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

    let mut slot = CLIENT.lock().await;
    if let Some(client) = live_cached_client(&mut slot) {
        return Ok(client);
    }

    let result = connect_or_spawn_daemon().await;
    if let Ok(client) = &result {
        *slot = Some(client.clone());
    }
    drop(slot);
    match result {
        Ok(client) => Ok(client),
        Err(failure) => {
            // A live daemon that merely stalled on the handshake is not a
            // crash. Counting it would open the circuit after three slow
            // handshakes, and the circuit-open error is whitelisted as
            // safe-to-fallback downstream — which would spawn in-process
            // duplicates of shells the daemon still owns.
            if failure.counts_toward_circuit {
                record_failure();
            }
            Err(failure.error)
        }
    }
}

/// Why `connect_or_spawn_daemon` could not hand back a client, plus whether
/// the crash circuit breaker should learn about it.
struct ConnectFailure {
    error: PtyDaemonError,
    /// `false` when a daemon process is alive and reachable but did not
    /// answer `Hello` in time. Nothing crashed, so the breaker must not
    /// count it; callers should simply retry later.
    counts_toward_circuit: bool,
}

impl From<PtyDaemonError> for ConnectFailure {
    /// Spawn/connect/socket-wait errors: genuine failures to bring up a
    /// daemon, so they count.
    fn from(error: PtyDaemonError) -> Self {
        Self {
            error,
            counts_toward_circuit: true,
        }
    }
}

/// Result of trying to adopt the daemon named by the manifest.
enum AdoptOutcome {
    Adopted(Arc<PtyDaemonClient>),
    /// Nothing usable is listening (no manifest, dead pid, refused dial,
    /// protocol mismatch). Spawning a replacement cannot orphan live shells.
    NoUsableDaemon,
    /// The pid is alive and its socket accepted our dial, but the handshake
    /// never completed. The daemon still owns every shell; a replacement
    /// would steal its socket + manifest and strand those shells forever.
    Unresponsive(PtyDaemonError),
}

/// Pure decision for a `Hello` result against a daemon whose pid is alive
/// and whose socket accepted our connection. Split out so the policy is
/// unit-testable without sockets.
enum HandshakeVerdict {
    Adopt {
        version: String,
    },
    /// Not adoptable, but definitively so: safe to spawn a replacement.
    Replace(String),
    /// Something answered the dial but never finished the handshake.
    Unresponsive(PtyDaemonError),
}

fn classify_adopt_handshake(
    result: Result<(u32, String, u32), PtyDaemonError>,
) -> HandshakeVerdict {
    match result {
        Ok((_pid, version, proto)) if proto == PROTOCOL_VERSION => {
            HandshakeVerdict::Adopt { version }
        }
        Ok((_pid, _version, proto)) => HandshakeVerdict::Replace(format!(
            "adopted daemon speaks protocol {proto}, expected {PROTOCOL_VERSION}; will not adopt"
        )),
        // `hello()` has a deadline (`DEFAULT_RPC_TIMEOUT`), so a stalled or
        // merely slow daemon now surfaces here as `Timeout`. `Closed` after a
        // successful connect means something accepted and then hung up —
        // also not proof that the daemon and its shells are gone. Mirror
        // `classify_client_acquisition_failure` in terminal/daemon_backed.rs:
        // both are retry-only.
        Err(error @ (PtyDaemonError::Timeout | PtyDaemonError::Closed)) => {
            HandshakeVerdict::Unresponsive(error)
        }
        // A daemon-side rejection or a malformed reply is a definitive
        // answer from whatever is listening; keep the pre-existing
        // "ignore and respawn" behavior.
        Err(error) => HandshakeVerdict::Replace(format!("adopt handshake failed: {error}")),
    }
}

/// Clone a cached client only while its reader is alive. `PtyDaemonClient`
/// marks EOF before draining pending requests, so observing `is_closed` is a
/// definitive eviction signal rather than a transient health guess.
fn live_cached_client(slot: &mut Option<Arc<PtyDaemonClient>>) -> Option<Arc<PtyDaemonClient>> {
    if slot.as_ref().is_some_and(|client| client.is_closed()) {
        eprintln!("[codemux::pty_daemon::supervisor] cached client closed; reconnecting");
        slot.take();
    }
    slot.clone()
}

async fn connect_or_spawn_daemon() -> Result<Arc<PtyDaemonClient>, ConnectFailure> {
    // Try adoption first. This also handles a daemon that merely closed our
    // old connection: reconnect to the still-live process and re-handshake.
    match try_adopt().await {
        AdoptOutcome::Adopted(client) => return Ok(client),
        AdoptOutcome::NoUsableDaemon => {}
        // Do NOT spawn. A fresh daemon would unlink the live daemon's socket
        // and overwrite its manifest; the old daemon keeps every shell and
        // agent but becomes unreachable, and the new daemon's empty `List`
        // would make hydration fresh-spawn a duplicate of each session.
        // Surface the retry-only error instead so the caller waits it out.
        AdoptOutcome::Unresponsive(error) => {
            eprintln!(
                "[codemux::pty_daemon::supervisor] live daemon unresponsive ({error}); \
                 not spawning a replacement, caller should retry"
            );
            return Err(ConnectFailure {
                error,
                counts_toward_circuit: false,
            });
        }
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
        ))
        .into());
    }
    Ok(client)
}

async fn try_adopt() -> AdoptOutcome {
    let Some(manifest) = read_manifest() else {
        return AdoptOutcome::NoUsableDaemon;
    };
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
            return AdoptOutcome::NoUsableDaemon;
        }
    }
    let client = match PtyDaemonClient::connect(&manifest.socket_path).await {
        Ok(c) => c,
        Err(error) => {
            eprintln!("[codemux::pty_daemon::supervisor] adopt connect failed: {error}");
            return AdoptOutcome::NoUsableDaemon;
        }
    };
    match classify_adopt_handshake(client.hello().await) {
        HandshakeVerdict::Adopt { version } => {
            eprintln!(
                "[codemux::pty_daemon::supervisor] adopted daemon pid={} version={version}",
                manifest.pid
            );
            AdoptOutcome::Adopted(client)
        }
        HandshakeVerdict::Replace(reason) => {
            eprintln!("[codemux::pty_daemon::supervisor] {reason}");
            // TODO(phase-2): graceful shutdown + respawn on protocol
            // mismatch. For now we just ignore the old daemon and spawn a
            // fresh one, which means the old PTYs are orphaned. Acceptable
            // for the MVP because protocol bumps will be rare.
            AdoptOutcome::NoUsableDaemon
        }
        HandshakeVerdict::Unresponsive(error) => {
            eprintln!(
                "[codemux::pty_daemon::supervisor] adopt handshake failed on live pid {}: {error}",
                manifest.pid
            );
            AdoptOutcome::Unresponsive(error)
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

    // The WebKitGTK renderer transport vars configure the app's own webview;
    // the daemon has no webview and every PTY child it spawns would inherit
    // them. Strip here so the whole daemon subtree starts clean.
    for key in crate::webview_tuning::RENDERER_ENV_VARS {
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
        PtyDaemonError::Daemon("could not determine socket dir (HOME unset?)".to_string())
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

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::net::UnixStream;

    #[test]
    fn matching_protocol_is_adopted() {
        match classify_adopt_handshake(Ok((42, "1.2.3".into(), PROTOCOL_VERSION))) {
            HandshakeVerdict::Adopt { version } => assert_eq!(version, "1.2.3"),
            _ => panic!("matching protocol must adopt"),
        }
    }

    #[test]
    fn protocol_mismatch_keeps_replace_behavior() {
        match classify_adopt_handshake(Ok((42, "1.2.3".into(), PROTOCOL_VERSION + 1))) {
            HandshakeVerdict::Replace(reason) => assert!(reason.contains("will not adopt")),
            _ => panic!("protocol mismatch must remain a replace decision"),
        }
    }

    #[test]
    fn timeout_and_closed_on_live_daemon_are_unresponsive_not_replace() {
        // These two are the only variants a stalled-but-alive daemon can
        // produce after `connect` succeeded; replacing it would orphan its
        // shells and duplicate every session on the next hydration.
        assert!(matches!(
            classify_adopt_handshake(Err(PtyDaemonError::Timeout)),
            HandshakeVerdict::Unresponsive(PtyDaemonError::Timeout)
        ));
        assert!(matches!(
            classify_adopt_handshake(Err(PtyDaemonError::Closed)),
            HandshakeVerdict::Unresponsive(PtyDaemonError::Closed)
        ));
    }

    #[test]
    fn definitive_handshake_errors_still_replace() {
        assert!(matches!(
            classify_adopt_handshake(Err(PtyDaemonError::Daemon("nope".into()))),
            HandshakeVerdict::Replace(_)
        ));
        assert!(matches!(
            classify_adopt_handshake(Err(PtyDaemonError::Io(std::io::Error::other("boom")))),
            HandshakeVerdict::Replace(_)
        ));
    }

    #[test]
    fn spawn_and_connect_errors_count_toward_circuit_but_unresponsive_does_not() {
        let spawn_failure: ConnectFailure = PtyDaemonError::Daemon("spawn failed".into()).into();
        assert!(spawn_failure.counts_toward_circuit);
        let io_failure: ConnectFailure =
            PtyDaemonError::Io(std::io::Error::other("refused")).into();
        assert!(io_failure.counts_toward_circuit);

        let unresponsive = match AdoptOutcome::Unresponsive(PtyDaemonError::Timeout) {
            AdoptOutcome::Unresponsive(error) => ConnectFailure {
                error,
                counts_toward_circuit: false,
            },
            _ => unreachable!(),
        };
        assert!(!unresponsive.counts_toward_circuit);
        assert!(matches!(unresponsive.error, PtyDaemonError::Timeout));
    }

    /// End-to-end over a real socket pair: a peer that accepts the dial but
    /// never answers `Hello` must classify as unresponsive, and the client
    /// must stay open (the daemon is alive, only slow) so a retry can reuse
    /// the same connection instead of stealing the socket.
    #[tokio::test]
    async fn stalled_hello_over_socket_is_unresponsive_and_leaves_client_open() {
        let (client_stream, _silent_daemon) = UnixStream::pair().unwrap();
        let client = PtyDaemonClient::from_test_stream(client_stream, Duration::from_millis(50));
        match classify_adopt_handshake(client.hello().await) {
            HandshakeVerdict::Unresponsive(PtyDaemonError::Timeout) => {}
            HandshakeVerdict::Unresponsive(other) => panic!("expected Timeout, got {other}"),
            HandshakeVerdict::Adopt { .. } => panic!("silent peer must not be adopted"),
            HandshakeVerdict::Replace(reason) => {
                panic!("silent peer must not be replaced: {reason}")
            }
        }
        assert!(
            !client.is_closed(),
            "a timed-out hello must not evict the connection"
        );
    }

    #[tokio::test]
    async fn disconnected_cache_entry_is_evicted_and_replacement_is_reused() {
        let (client_stream, daemon_stream) = UnixStream::pair().unwrap();
        let disconnected = PtyDaemonClient::from_test_stream(client_stream, Duration::from_secs(1));
        let mut slot = Some(disconnected.clone());
        assert!(Arc::ptr_eq(
            &live_cached_client(&mut slot).expect("initial live client"),
            &disconnected,
        ));

        // Simulate the daemon/socket disappearing, then wait until the client
        // reader has observed EOF and published its liveness flag.
        drop(daemon_stream);
        tokio::time::timeout(Duration::from_secs(1), async {
            while !disconnected.is_closed() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("client should observe disconnect");
        assert!(live_cached_client(&mut slot).is_none());
        assert!(slot.is_none(), "closed cache entry must be evicted");

        // A subsequent initialization installs a connection to the new server,
        // and later callers reuse that replacement rather than the dead Arc.
        let (replacement_stream, _replacement_daemon) = UnixStream::pair().unwrap();
        let replacement =
            PtyDaemonClient::from_test_stream(replacement_stream, Duration::from_secs(1));
        slot = Some(replacement.clone());
        let cached = live_cached_client(&mut slot).expect("replacement client");
        assert!(Arc::ptr_eq(&cached, &replacement));
        assert!(!Arc::ptr_eq(&cached, &disconnected));
    }
}
