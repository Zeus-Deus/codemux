//! Per-workspace virtual display manager (Phase 2 of display isolation).
//!
//! Spawns one headless X server (currently Xvfb) per workspace on demand and
//! hands out a `DISPLAY=:N` env var the caller injects into child processes.
//! When a workspace has a virtual display, agents can launch real Electron /
//! Chromium / Playwright-headed apps fully headed — they open inside the
//! hidden display that the user never sees (until a future "watch the agent"
//! pane connects via x11vnc).
//!
//! This is the same infrastructure that later enables computer-use: agents
//! drive real desktop apps with mouse/keyboard input on the hidden display.
//!
//! ## Design
//!
//! - `VirtualDisplayManager` is held as shared state (`Arc<Mutex<_>>` internally).
//! - `acquire(workspace_id)` returns the workspace's existing display or spawns
//!   a new Xvfb. Idempotent — repeated calls return the same display.
//! - `release(workspace_id)` kills the Xvfb with SIGTERM, 5s grace, SIGKILL.
//! - `shutdown_all()` on app exit kills every tracked display.
//! - `new()` runs an **orphan sweep** — stale `/tmp/.X<n>-lock` from a
//!   previous crashed Codemux gets unlinked so we can reuse the slot.
//!
//! ## Portability
//!
//! - Linux: full support when `Xvfb` is on PATH.
//! - macOS, Windows: `is_supported()` returns `false`; `acquire()` returns
//!   `Error::Unsupported`. Windows+WSL2 is handled later by detecting `/run/WSL`
//!   and still spawning Xvfb inside the Linux userspace. macOS goes through
//!   `xwfb-run` (future) or Lima VM.
//!
//! ## Tracked in `docs/plans/sandboxing.md` Phase 2.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Where Xvfb writes its lockfile for display N — `/tmp/.X{N}-lock`.
const X_LOCK_DIR: &str = "/tmp";
/// Where Xvfb binds its UNIX socket — `/tmp/.X11-unix/X{N}`.
const X_SOCKET_DIR: &str = "/tmp/.X11-unix";

/// Starting display number for allocation. Picked deliberately high to avoid
/// stepping on the host's main session (`:0`-`:2`) and common CI defaults
/// (`:99`). 2026-style cargo-culted display numbers are a common source of
/// "works on my machine but fails in the other workspace" bugs.
const DEFAULT_DISPLAY_START: u32 = 1000;
const DEFAULT_DISPLAY_MAX_TRIES: u32 = 200;

const DEFAULT_SCREEN_WIDTH: u32 = 1920;
const DEFAULT_SCREEN_HEIGHT: u32 = 1080;
const DEFAULT_SCREEN_DEPTH: u32 = 24;
const DEFAULT_DPI: u32 = 96;

/// TCP port range for x11vnc. 5900 is the canonical VNC port; we start at
/// 5910 to leave 5900-5909 for whatever real VNC servers the user already
/// runs, and probe upward.
const DEFAULT_VNC_PORT_START: u16 = 5910;
const DEFAULT_VNC_PORT_TRIES: u16 = 90;

const GRACEFUL_SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

#[derive(Debug)]
pub enum Error {
    Unsupported,
    XvfbNotFound,
    X11vncNotFound,
    XauthFailed,
    NoFreeDisplay { start: u32 },
    NoFreeVncPort { start: u16 },
    Spawn(std::io::Error),
    XvfbExitedEarly { display: u32 },
    XvfbStartupTimeout { display: u32 },
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Unsupported => write!(f, "virtual display not supported on this platform"),
            Error::XvfbNotFound => write!(f, "Xvfb binary not found on PATH"),
            Error::X11vncNotFound => write!(f, "x11vnc binary not found on PATH"),
            Error::XauthFailed => write!(f, "xauth failed to write cookie file"),
            Error::NoFreeDisplay { start } => {
                write!(f, "exhausted display number range starting at {start}")
            }
            Error::NoFreeVncPort { start } => {
                write!(f, "exhausted VNC port range starting at {start}")
            }
            Error::Spawn(e) => write!(f, "failed to spawn Xvfb: {e}"),
            Error::XvfbExitedEarly { display } => write!(
                f,
                "Xvfb exited before binding socket for display :{display}"
            ),
            Error::XvfbStartupTimeout { display } => write!(
                f,
                "Xvfb took too long to bind socket for display :{display}"
            ),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Spawn(e) => Some(e),
            _ => None,
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Spawn(e)
    }
}

#[derive(Debug, Clone)]
pub struct VirtualDisplayEnv {
    /// The `DISPLAY` value to inject into child env — e.g. `:1000`.
    pub display: String,
    /// Display number as an integer.
    pub display_number: u32,
    /// Path to the workspace's `XAUTHORITY` file containing the
    /// MIT-MAGIC-COOKIE-1 secret, or `None` if auth setup failed (we still
    /// return the display — graceful degrade).
    pub xauthority_path: Option<String>,
    /// Localhost TCP port where `x11vnc` is exposing the display, or `None`
    /// if VNC wasn't requested or `x11vnc` isn't installed.
    pub vnc_port: Option<u16>,
    /// Plain-text VNC password a viewer needs to connect, or `None` if VNC
    /// wasn't spawned. Not returned by `env_pairs()` — the frontend uses
    /// this to auto-fill a viewer connection without user input.
    pub vnc_password: Option<String>,
}

impl VirtualDisplayEnv {
    /// Env var pairs the caller should apply to every child process in this
    /// workspace's agent sandbox. Always includes `DISPLAY`; also includes
    /// `XAUTHORITY` when the cookie file exists so children can authenticate
    /// against the X server.
    pub fn env_pairs(&self) -> Vec<(String, String)> {
        let mut out = vec![("DISPLAY".to_string(), self.display.clone())];
        if let Some(path) = &self.xauthority_path {
            out.push(("XAUTHORITY".to_string(), path.clone()));
        }
        out
    }
}

/// Options passed to [`VirtualDisplayManager::acquire_with_options`].
#[derive(Debug, Clone, Default)]
pub struct AcquireOptions {
    /// When `true`, the manager also spawns `x11vnc` bound to `127.0.0.1` so
    /// the user can watch the agent's GUI. No-op (logs a warning) if
    /// `x11vnc` is not on PATH — the Xvfb still comes up normally.
    pub watch_vnc: bool,
}

/// Internal handle for one running Xvfb (+ optional x11vnc + xauth file).
struct XvfbHandle {
    display_number: u32,
    xvfb: Child,
    vnc: Option<Vnc>,
    xauthority_path: Option<PathBuf>,
}

struct Vnc {
    child: Child,
    port: u16,
    /// Plain-text password (we remember it so `env_from_handle` can surface
    /// it to the frontend; the on-disk file has the same value).
    password: String,
    /// Password file to unlink on release.
    passwd_file: PathBuf,
}

/// Manages per-workspace Xvfb instances.
pub struct VirtualDisplayManager {
    inner: Mutex<Inner>,
}

struct Inner {
    by_workspace: HashMap<String, XvfbHandle>,
}

impl Default for VirtualDisplayManager {
    fn default() -> Self {
        Self::new()
    }
}

impl VirtualDisplayManager {
    /// Construct an empty manager and perform a best-effort orphan sweep.
    ///
    /// On Linux, walks `/tmp/.X*-lock`, checks whether the PID recorded in
    /// each lock file is still alive, and unlinks stale lock + socket pairs
    /// left by a previous Codemux crash (or any other app). Safe to run
    /// concurrently with other Codemux instances — worst case a living
    /// process's lock is left alone.
    pub fn new() -> Self {
        let mgr = Self {
            inner: Mutex::new(Inner {
                by_workspace: HashMap::new(),
            }),
        };
        #[cfg(target_os = "linux")]
        {
            orphan_sweep();
        }
        mgr
    }

    /// Whether the host has everything required to spawn virtual displays.
    ///
    /// Today that means Linux + `Xvfb` on PATH. On macOS / Windows this
    /// returns `false` until the respective backends are implemented.
    pub fn is_supported() -> bool {
        #[cfg(target_os = "linux")]
        {
            find_executable("Xvfb").is_some()
        }
        #[cfg(not(target_os = "linux"))]
        {
            false
        }
    }

    /// Acquire a virtual display for the given workspace with default options
    /// (no VNC). See [`acquire_with_options`](Self::acquire_with_options).
    pub fn acquire(&self, workspace_id: &str) -> Result<VirtualDisplayEnv, Error> {
        self.acquire_with_options(workspace_id, AcquireOptions::default())
    }

    /// Acquire a virtual display for the given workspace.
    ///
    /// Idempotent: if the workspace already has a running display, returns
    /// the cached env (VNC is NOT re-evaluated — if you want to turn VNC on
    /// for an already-running display, release and re-acquire).
    ///
    /// On a cold acquire:
    /// 1. Finds a free display number starting at `:1000`.
    /// 2. Creates an `XAUTHORITY` file with a random MIT-MAGIC-COOKIE-1 so
    ///    only processes with the cookie can connect. Best-effort: if `xauth`
    ///    isn't on PATH or the file can't be written, we log and continue
    ///    without auth (same as the pre-2.5 behavior).
    /// 3. Spawns Xvfb with `-auth <file>` (when the cookie exists).
    /// 4. If `options.watch_vnc` is set and `x11vnc` is on PATH, spawns
    ///    `x11vnc` bound to `127.0.0.1:<free-port>` with the same cookie.
    ///    Missing `x11vnc` logs a warning and returns `vnc_port: None`.
    pub fn acquire_with_options(
        &self,
        workspace_id: &str,
        options: AcquireOptions,
    ) -> Result<VirtualDisplayEnv, Error> {
        if !Self::is_supported() {
            return Err(if cfg!(target_os = "linux") {
                Error::XvfbNotFound
            } else {
                Error::Unsupported
            });
        }

        let mut inner = self.inner.lock().expect("virtual display mutex poisoned");

        // If we have a cached handle, verify Xvfb is still alive. A mid-
        // session crash (segfault, OOM-killed, external `kill -9`) would
        // otherwise leave us returning a DISPLAY that no server is listening
        // on — agents would fail to connect with no explanation. Reap the
        // dead handle and fall through to re-acquire.
        if let Some(handle) = inner.by_workspace.get_mut(workspace_id) {
            if xvfb_is_alive(handle) {
                return Ok(env_from_handle(handle));
            }
            crate::diagnostics::stderr_line(&format!(
                "[codemux::virtual_display] Cached Xvfb for workspace {workspace_id} is dead; re-acquiring"
            ));
            if let Some(dead) = inner.by_workspace.remove(workspace_id) {
                // Don't bother waiting for a graceful kill — process is
                // already gone. Just clean up artifacts.
                cleanup_display_artifacts(dead.display_number);
                if let Some(auth) = &dead.xauthority_path {
                    let _ = std::fs::remove_file(auth);
                }
                if let Some(vnc) = &dead.vnc {
                    let _ = std::fs::remove_file(&vnc.passwd_file);
                }
            }
        }

        // Display numbers this manager already owns. Excluded from the
        // probe below so a second workspace can never be handed the same
        // number — even when the first workspace's Xvfb has silently died
        // (its `/tmp/.X<n>-lock` + socket are gone, so the filesystem probe
        // alone would consider the slot free and re-hand it out). The dead
        // handle stays parked under its workspace_id and gets reaped on
        // that workspace's next acquire.
        let reserved: HashSet<u32> = inner
            .by_workspace
            .values()
            .map(|h| h.display_number)
            .collect();

        // Try up to 4 display-number slots before giving up. The probe
        // (`find_free_display_number`) can race with another process that
        // binds the number between our probe and Xvfb's socket creation;
        // on failure we advance and try the next slot.
        let (xvfb, display_number, xauthority_path) = spawn_xvfb_with_retry(
            DEFAULT_DISPLAY_START,
            DEFAULT_DISPLAY_MAX_TRIES,
            &reserved,
        )?;

        let vnc = if options.watch_vnc {
            match create_vnc_password(display_number) {
                Ok((passwd_file, password)) => {
                    match spawn_x11vnc(display_number, xauthority_path.as_deref(), &passwd_file) {
                        Ok(mut v) => {
                            v.password = password;
                            v.passwd_file = passwd_file;
                            Some(v)
                        }
                        Err(e) => {
                            let _ = std::fs::remove_file(&passwd_file);
                            crate::diagnostics::stderr_line(&format!(
                                "[codemux::virtual_display] VNC requested but could not spawn x11vnc: {e}"
                            ));
                            None
                        }
                    }
                }
                Err(e) => {
                    crate::diagnostics::stderr_line(&format!(
                        "[codemux::virtual_display] VNC requested but could not generate password: {e}"
                    ));
                    None
                }
            }
        } else {
            None
        };

        let handle = XvfbHandle {
            display_number,
            xvfb,
            vnc,
            xauthority_path: xauthority_path.clone(),
        };
        let env = env_from_handle(&handle);
        inner.by_workspace.insert(workspace_id.to_string(), handle);

        Ok(env)
    }

    /// Look up the cached env for a workspace without spawning anything.
    /// Used by the `get_workspace_virtual_display` Tauri command so the
    /// frontend can discover the VNC port after acquire.
    pub fn env_for_workspace(&self, workspace_id: &str) -> Option<VirtualDisplayEnv> {
        let inner = self.inner.lock().ok()?;
        inner.by_workspace.get(workspace_id).map(env_from_handle)
    }

    /// Stop the Xvfb for this workspace if one is running. Idempotent.
    pub fn release(&self, workspace_id: &str) {
        let handle_opt = {
            let mut inner = self.inner.lock().expect("virtual display mutex poisoned");
            inner.by_workspace.remove(workspace_id)
        };
        if let Some(handle) = handle_opt {
            terminate_xvfb(handle);
        }
    }

    /// Kill every tracked Xvfb. Call on app exit.
    pub fn shutdown_all(&self) {
        let handles: Vec<XvfbHandle> = {
            let mut inner = self.inner.lock().expect("virtual display mutex poisoned");
            inner.by_workspace.drain().map(|(_, v)| v).collect()
        };
        for handle in handles {
            terminate_xvfb(handle);
        }
    }

    /// Number of active displays — useful for tests and observability.
    pub fn active_count(&self) -> usize {
        self.inner
            .lock()
            .map(|g| g.by_workspace.len())
            .unwrap_or(0)
    }
}

impl Drop for VirtualDisplayManager {
    fn drop(&mut self) {
        self.shutdown_all();
    }
}

// ──────────────────────────────────────────────────────────────────────────
// Internals
// ──────────────────────────────────────────────────────────────────────────

fn find_executable(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Probe `/tmp/.X{n}-lock` and `/tmp/.X11-unix/X{n}` starting at `start`,
/// returning the first N where both are absent.
///
/// This is a probe-with-race — between "probe says free" and "Xvfb binds
/// the socket" another process could claim N. The race window is narrow
/// and Xvfb itself will fail loudly if it happens; the caller retries.
/// The 2026-idiomatic alternative (`-displayfd <pipe>`) needs unsafe FD
/// plumbing and is deferred to a follow-up.
/// Integration-test-only shim for probing display numbers without spawning
/// Xvfb. Not part of the public API for app code — use `acquire` there.
#[doc(hidden)]
pub fn find_free_display_number_for_tests(start: u32, max_tries: u32) -> Option<u32> {
    find_free_display_number(start, max_tries, &HashSet::new())
}

/// Find a display number not in use. `reserved` holds numbers the caller
/// already owns (live or dead-but-still-tracked Xvfb handles) — those are
/// skipped even when the filesystem probe says the slot is free, so two
/// workspaces can never collide on a number.
pub(crate) fn find_free_display_number(
    start: u32,
    max_tries: u32,
    reserved: &HashSet<u32>,
) -> Option<u32> {
    for offset in 0..max_tries {
        let n = start + offset;
        if !reserved.contains(&n) && !display_number_in_use(n) {
            return Some(n);
        }
    }
    None
}

fn display_number_in_use(n: u32) -> bool {
    let lock = PathBuf::from(X_LOCK_DIR).join(format!(".X{n}-lock"));
    let socket = PathBuf::from(X_SOCKET_DIR).join(format!("X{n}"));
    lock.exists() || socket.exists()
}

#[cfg(target_os = "linux")]
fn spawn_xvfb(display: u32, xauth: Option<&Path>) -> Result<Child, Error> {
    // Canonical 2026 Xvfb flags for agent dev workflows:
    //   -screen 0 1920x1080x24: modern resolution; Electron/Chromium render crisp
    //   -dpi 96: matches Chromium's logical DPI baseline (120 produces fuzz)
    //   -noreset: don't tear down on last-client-disconnect; agents restart constantly
    //   -nolisten tcp: defense in depth even though it's the default
    //   +extension GLX +extension RANDR: required for Chromium WebGL + window resize
    //   -auth <file>: per-workspace MIT-MAGIC-COOKIE-1 (Phase 2.5) so only
    //     processes that inherit our `XAUTHORITY` can connect. Without this,
    //     anything on the host knowing `:N` could connect.
    //
    // Intentionally NOT passed:
    //   -ac: disables host-based access control globally — security hole.
    let mut cmd = Command::new("Xvfb");
    cmd.arg(format!(":{display}"))
        .arg("-screen")
        .arg("0")
        .arg(format!(
            "{DEFAULT_SCREEN_WIDTH}x{DEFAULT_SCREEN_HEIGHT}x{DEFAULT_SCREEN_DEPTH}"
        ))
        .arg("-dpi")
        .arg(DEFAULT_DPI.to_string())
        .arg("-noreset")
        .arg("-nolisten")
        .arg("tcp")
        .arg("+extension")
        .arg("GLX")
        .arg("+extension")
        .arg("RANDR");
    if let Some(auth_path) = xauth {
        cmd.arg("-auth").arg(auth_path);
    }
    cmd.stdout(Stdio::null())
        .stderr(Stdio::null())
        .stdin(Stdio::null());

    cmd.spawn().map_err(Error::from)
}

#[cfg(not(target_os = "linux"))]
fn spawn_xvfb(_display: u32, _xauth: Option<&Path>) -> Result<Child, Error> {
    Err(Error::Unsupported)
}

/// Directory holding cookie + VNC password for a given display.
///
/// Prefers `$XDG_RUNTIME_DIR/codemux/vd-<N>/` (per-user tmpfs, wiped on
/// logout, 0700) as recommended by freedesktop guidance. Falls back to
/// `/tmp/codemux-vd-<N>/` when `XDG_RUNTIME_DIR` is unset (sandboxed envs,
/// some distros without `pam_systemd`).
///
/// **Why NOT plain `/tmp`:** systemd-tmpfiles ages `/tmp` entries at 10
/// days by default on Arch/Omarchy/Fedora (`d /tmp 1777 root root 10d`).
/// A long-idle workspace whose cookie wasn't touched in 10 days would
/// wake up to a swept cookie file and mysterious "X client connection
/// failed" errors. `$XDG_RUNTIME_DIR` has no such ageing.
fn display_secrets_dir(display: u32) -> PathBuf {
    if let Some(xdg) = std::env::var_os("XDG_RUNTIME_DIR") {
        let p = PathBuf::from(xdg).join(crate::APP_DIR_NAME).join(format!("vd-{display}"));
        return p;
    }
    tmp_secrets_dir(display)
}

fn tmp_secrets_dir(display: u32) -> PathBuf {
    PathBuf::from(format!("/tmp/codemux-vd-{display}"))
}

fn xauthority_path_for(display: u32) -> PathBuf {
    display_secrets_dir(display).join("Xauthority")
}

fn vnc_password_path_for(display: u32) -> PathBuf {
    display_secrets_dir(display).join("vncpasswd")
}

/// Ensure the per-display secrets dir exists with 0700. If the preferred
/// `$XDG_RUNTIME_DIR`-based path can't be created (set but read-only,
/// over-quota, stale login session) we fall back to the `/tmp` path so
/// acquire still succeeds. The fallback is strictly less safe w.r.t.
/// systemd-tmpfiles ageing, but that's better than failing the whole
/// workspace for a misconfigured runtime dir.
fn ensure_secrets_dir(display: u32) -> std::io::Result<PathBuf> {
    let preferred = display_secrets_dir(display);
    match std::fs::create_dir_all(&preferred) {
        Ok(()) => {
            set_dir_mode_0700(&preferred);
            Ok(preferred)
        }
        Err(primary_err) => {
            let fallback = tmp_secrets_dir(display);
            if preferred == fallback {
                // XDG wasn't set; the failure is on /tmp itself — propagate.
                return Err(primary_err);
            }
            match std::fs::create_dir_all(&fallback) {
                Ok(()) => {
                    crate::diagnostics::stderr_line(&format!(
                        "[codemux::virtual_display] XDG_RUNTIME_DIR path {} unusable ({primary_err}); falling back to {}",
                        preferred.display(),
                        fallback.display()
                    ));
                    set_dir_mode_0700(&fallback);
                    Ok(fallback)
                }
                Err(fallback_err) => Err(fallback_err),
            }
        }
    }
}

fn set_dir_mode_0700(dir: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    }
    #[cfg(not(unix))]
    {
        let _ = dir;
    }
}

/// Fill `out` with OS entropy. Uses `rand::rngs::OsRng` which reads directly
/// from `getrandom(2)` / `/dev/urandom` — bypasses the `ThreadRng` ChaCha
/// userspace CSPRNG for this security-critical path so we don't depend on
/// `rand`'s future security policy changes.
fn fill_os_entropy(out: &mut [u8]) {
    use rand::RngCore;
    rand::rngs::OsRng.fill_bytes(out);
}

/// Generate a MIT-MAGIC-COOKIE-1 (128 random bits) and write it into an
/// Xauthority file for the given display using the `xauth` CLI. Returns
/// the file path on success.
#[cfg(target_os = "linux")]
fn create_xauth_cookie(display: u32) -> Result<PathBuf, Error> {
    ensure_secrets_dir(display)?;
    let path = xauthority_path_for(display);
    // Clean slate — `xauth add` appends; start fresh so stale entries don't
    // accumulate if a previous run crashed mid-write.
    let _ = std::fs::remove_file(&path);

    // Touch the file first so `xauth` finds it (it won't create the file
    // with just `-f`).
    std::fs::File::create(&path)?;
    // Tighten perms to 0600 — this file holds a secret.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }

    // 128 bits of cookie entropy → 32 hex chars, direct from OS.
    let mut bytes = [0u8; 16];
    fill_os_entropy(&mut bytes);
    let hex_cookie = bytes
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();

    let status = Command::new("xauth")
        .arg("-f")
        .arg(&path)
        .arg("add")
        .arg(format!(":{display}"))
        .arg("MIT-MAGIC-COOKIE-1")
        .arg(&hex_cookie)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .stdin(Stdio::null())
        .status()
        .map_err(Error::from)?;

    if !status.success() {
        let _ = std::fs::remove_file(&path);
        return Err(Error::XauthFailed);
    }

    Ok(path)
}

#[cfg(not(target_os = "linux"))]
fn create_xauth_cookie(_display: u32) -> Result<PathBuf, Error> {
    Err(Error::Unsupported)
}

/// Generate a random VNC password file for `x11vnc -passwdfile read:<path>`.
/// 16 URL-safe bytes → 22 char password, plenty of entropy for localhost-
/// only VNC. Returns `(path, password)` so the password can be surfaced to
/// the frontend (so a viewer component can connect without user input).
#[cfg(target_os = "linux")]
fn create_vnc_password(display: u32) -> Result<(PathBuf, String), Error> {
    ensure_secrets_dir(display)?;
    let path = vnc_password_path_for(display);
    let _ = std::fs::remove_file(&path);

    // 16 random bytes → url-safe base64 gives 22 chars.
    let mut bytes = [0u8; 16];
    fill_os_entropy(&mut bytes);
    // Keep this dependency-free: hex is simpler than base64 and the extra
    // 10 chars don't matter since we never type this manually.
    let password = bytes
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();

    std::fs::write(&path, password.as_bytes())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok((path, password))
}

#[cfg(not(target_os = "linux"))]
fn create_vnc_password(_display: u32) -> Result<(PathBuf, String), Error> {
    Err(Error::Unsupported)
}

/// Spawn `x11vnc` bound to 127.0.0.1 on the first free TCP port in a
/// sensible range, attached to the given display and gated by a random
/// per-display password.
///
/// Flags tuned for the ADE use case:
/// - `-localhost`: never bind to external interfaces; VNC over loopback only.
/// - `-passwdfile read:<path>`: plain-text password read at startup. The
///   file is in `$XDG_RUNTIME_DIR/codemux/vd-<N>/` (0600) so only this
///   user's processes can read it. Combined with `-localhost` this closes
///   the same-UID lateral-process case that `-nopw` left open.
/// - `-auth <cookie>`: Xvfb xauth cookie so x11vnc can connect to Xvfb.
/// - `-forever`: don't exit when a viewer disconnects; user may reconnect.
/// - `-quiet`: don't spam stdout during the agent's test runs.
/// - `-noxdamage`: still the recommended flag in 2026 for Chromium/Electron
///   rendering inside a virtual display.
#[cfg(target_os = "linux")]
fn spawn_x11vnc(
    display: u32,
    xauth: Option<&Path>,
    passwd_file: &Path,
) -> Result<Vnc, Error> {
    if find_executable("x11vnc").is_none() {
        return Err(Error::X11vncNotFound);
    }

    let port = find_free_tcp_port(DEFAULT_VNC_PORT_START, DEFAULT_VNC_PORT_TRIES)
        .ok_or(Error::NoFreeVncPort {
            start: DEFAULT_VNC_PORT_START,
        })?;

    let mut cmd = Command::new("x11vnc");
    cmd.arg("-display")
        .arg(format!(":{display}"))
        .arg("-rfbport")
        .arg(port.to_string())
        .arg("-localhost")
        .arg("-forever")
        .arg("-quiet")
        .arg("-noxdamage")
        .arg("-passwdfile")
        .arg(format!("read:{}", passwd_file.display()));
    if let Some(auth_path) = xauth {
        cmd.arg("-auth").arg(auth_path);
    }
    cmd.stdout(Stdio::null())
        .stderr(Stdio::null())
        .stdin(Stdio::null());

    let child = cmd.spawn().map_err(Error::from)?;

    Ok(Vnc {
        child,
        port,
        password: String::new(),
        passwd_file: passwd_file.to_path_buf(),
    })
}

#[cfg(not(target_os = "linux"))]
fn spawn_x11vnc(
    _display: u32,
    _xauth: Option<&Path>,
    _passwd_file: &Path,
) -> Result<Vnc, Error> {
    Err(Error::Unsupported)
}

fn find_free_tcp_port(start: u16, max_tries: u16) -> Option<u16> {
    for offset in 0..max_tries {
        let port = start.checked_add(offset)?;
        if std::net::TcpListener::bind(("127.0.0.1", port)).is_ok() {
            return Some(port);
        }
    }
    None
}

fn env_from_handle(handle: &XvfbHandle) -> VirtualDisplayEnv {
    VirtualDisplayEnv {
        display: format!(":{}", handle.display_number),
        display_number: handle.display_number,
        xauthority_path: handle
            .xauthority_path
            .as_ref()
            .map(|p| p.display().to_string()),
        vnc_port: handle.vnc.as_ref().map(|v| v.port),
        vnc_password: handle.vnc.as_ref().map(|v| v.password.clone()),
    }
}

/// True if the cached Xvfb for this handle is still running. Uses
/// `try_wait()` which reaps the child if it has exited and returns `Ok(Some(_))`
/// in that case. A `try_wait()` error is also treated as "dead" — the kernel
/// lost track of our child somehow, it can't be alive.
fn xvfb_is_alive(handle: &mut XvfbHandle) -> bool {
    matches!(handle.xvfb.try_wait(), Ok(None))
}

/// Cold-acquire path: probe for free displays starting at `start`, spawn
/// Xvfb, wait for its socket. Retries up to 4 times on the race where
/// `find_free_display_number` says a slot is free but Xvfb fails to bind
/// (another process raced us to the abstract socket, or the X lock file
/// race from the probe-then-spawn window triggered).
///
/// Returns `(child, display_number, xauthority_path)`.
fn spawn_xvfb_with_retry(
    start: u32,
    max_tries: u32,
    reserved: &HashSet<u32>,
) -> Result<(Child, u32, Option<PathBuf>), Error> {
    const MAX_RETRIES: u32 = 4;
    let mut last_err: Option<Error> = None;
    let mut probe_start = start;
    let mut probe_budget = max_tries;

    for _ in 0..MAX_RETRIES {
        let display_number =
            find_free_display_number(probe_start, probe_budget, reserved).ok_or_else(|| {
                Error::NoFreeDisplay {
                    start: DEFAULT_DISPLAY_START,
                }
            })?;

        // Best-effort xauth cookie per attempt. If earlier attempt left a
        // cookie file, ensure_secrets_dir reuses the dir; `create_xauth_cookie`
        // does `remove_file` so we don't accumulate entries.
        let xauthority_path = create_xauth_cookie(display_number).ok();

        match spawn_xvfb(display_number, xauthority_path.as_deref()) {
            Ok(child) => match wait_for_xvfb_socket(display_number, Duration::from_secs(5)) {
                Ok(()) => return Ok((child, display_number, xauthority_path)),
                Err(e) => {
                    // Socket didn't come up — kill whatever we spawned,
                    // clean the cookie, move to the next slot.
                    let mut child = child;
                    let _ = child.kill();
                    let _ = child.wait();
                    if let Some(path) = &xauthority_path {
                        let _ = std::fs::remove_file(path);
                    }
                    cleanup_display_artifacts(display_number);
                    last_err = Some(e);
                }
            },
            Err(e) => {
                if let Some(path) = &xauthority_path {
                    let _ = std::fs::remove_file(path);
                }
                last_err = Some(e);
            }
        }

        // Advance the probe past the display we just tried.
        probe_start = display_number + 1;
        probe_budget = probe_budget.saturating_sub(1);
        if probe_budget == 0 {
            break;
        }
    }

    Err(last_err.unwrap_or(Error::NoFreeDisplay {
        start: DEFAULT_DISPLAY_START,
    }))
}

/// Block until Xvfb has bound its socket, or give up.
fn wait_for_xvfb_socket(display: u32, timeout: Duration) -> Result<(), Error> {
    let socket = PathBuf::from(X_SOCKET_DIR).join(format!("X{display}"));
    let started = Instant::now();
    loop {
        if socket.exists() {
            return Ok(());
        }
        if started.elapsed() > timeout {
            return Err(Error::XvfbStartupTimeout { display });
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Send SIGTERM to a child, wait up to `grace`, then SIGKILL if still alive.
fn graceful_kill(child: &mut Child, grace: Duration) {
    #[cfg(unix)]
    {
        let pid = child.id() as i32;
        // SAFETY: std::process::Child owns the PID; sending SIGTERM to an
        // owned child is well-defined. The id() matches until `wait()` reaps.
        unsafe {
            libc::kill(pid, libc::SIGTERM);
        }
    }
    #[cfg(not(unix))]
    {
        let _ = child.kill();
    }

    let deadline = Instant::now() + grace;
    let mut reaped = false;
    while Instant::now() < deadline {
        match child.try_wait() {
            Ok(Some(_)) => {
                reaped = true;
                break;
            }
            Ok(None) => {
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(_) => break,
        }
    }

    if !reaped {
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// Kill Xvfb + any paired x11vnc with SIGTERM → 5s grace → SIGKILL. Also
/// unlinks stale lock/socket files, the xauth cookie, the VNC password
/// file, and removes the per-display secrets directory if empty.
fn terminate_xvfb(mut handle: XvfbHandle) {
    // Kill VNC first so it stops screenshotting a display that's about to
    // die. If we kill Xvfb first, x11vnc will spam "connection lost" lines.
    if let Some(mut vnc) = handle.vnc.take() {
        graceful_kill(&mut vnc.child, GRACEFUL_SHUTDOWN_GRACE);
        let _ = std::fs::remove_file(&vnc.passwd_file);
    }
    graceful_kill(&mut handle.xvfb, GRACEFUL_SHUTDOWN_GRACE);

    cleanup_display_artifacts(handle.display_number);

    if let Some(auth_path) = &handle.xauthority_path {
        let _ = std::fs::remove_file(auth_path);
    }
    // Best-effort: remove the per-display secrets dir. `remove_dir` refuses
    // to unlink a non-empty dir so we can't accidentally delete unrelated
    // user data even if somehow a file got in there.
    let _ = std::fs::remove_dir(display_secrets_dir(handle.display_number));
}

fn cleanup_display_artifacts(display: u32) {
    let lock = PathBuf::from(X_LOCK_DIR).join(format!(".X{display}-lock"));
    let socket = PathBuf::from(X_SOCKET_DIR).join(format!("X{display}"));
    let _ = std::fs::remove_file(&lock);
    let _ = std::fs::remove_file(&socket);
}

#[cfg(target_os = "linux")]
fn orphan_sweep() {
    orphan_sweep_in(Path::new(X_LOCK_DIR), Path::new(X_SOCKET_DIR), pid_is_live_xvfb);
}

/// Testable core of `orphan_sweep`: walks `lock_dir` for `.X<digits>-lock`
/// files, and for any whose recorded PID fails `pid_check`, unlinks both the
/// lock and the matching socket in `socket_dir`.
///
/// Exposed to tests so they can drive it against a temp dir without touching
/// the real `/tmp`.
#[cfg(target_os = "linux")]
pub(crate) fn orphan_sweep_in(
    lock_dir: &Path,
    socket_dir: &Path,
    pid_check: impl Fn(i32) -> bool,
) {
    let Ok(entries) = std::fs::read_dir(lock_dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name_str) = name.to_str() else {
            continue;
        };
        // Match `.X<digits>-lock`.
        let Some(rest) = name_str.strip_prefix(".X") else {
            continue;
        };
        let Some(num_str) = rest.strip_suffix("-lock") else {
            continue;
        };
        let Ok(display) = num_str.parse::<u32>() else {
            continue;
        };

        // Only consider locks in our default range. Don't touch `:0`–`:2` etc.
        if display < DEFAULT_DISPLAY_START {
            continue;
        }

        let lock_path = entry.path();
        let Ok(contents) = std::fs::read_to_string(&lock_path) else {
            continue;
        };
        let pid_str = contents.trim();
        let Ok(pid) = pid_str.parse::<i32>() else {
            // Malformed lock file — leave it alone.
            continue;
        };

        // If the PID is alive AND its command is Xvfb, assume a real Xvfb
        // still owns this display. Otherwise the lock is stale — either the
        // old Xvfb died (ESRCH on the PID) or the PID was recycled by an
        // unrelated process (comm != "Xvfb"). This closes the classic
        // `kill(pid, 0)`-only PID-reuse race that leaks locks forever.
        if pid_check(pid) {
            continue;
        }

        // Stale lock: unlink it + the socket.
        let _ = std::fs::remove_file(&lock_path);
        let socket = socket_dir.join(format!("X{display}"));
        let _ = std::fs::remove_file(&socket);
    }
}

/// Public test hook for `pid_is_live_xvfb`.
#[doc(hidden)]
#[cfg(target_os = "linux")]
pub fn pid_is_live_xvfb_for_tests(pid: i32) -> bool {
    pid_is_live_xvfb(pid)
}

/// True iff `pid` is alive AND `/proc/<pid>/comm` reads as `Xvfb` (within
/// the 15-char TASK_COMM_LEN limit).
#[cfg(target_os = "linux")]
fn pid_is_live_xvfb(pid: i32) -> bool {
    if !is_pid_alive(pid) {
        return false;
    }
    let comm_path = format!("/proc/{pid}/comm");
    match std::fs::read_to_string(&comm_path) {
        Ok(contents) => contents.trim() == "Xvfb",
        // ENOENT → process died between kill(0) and our read, treat as dead.
        Err(_) => false,
    }
}

#[cfg(target_os = "linux")]
fn is_pid_alive(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    // SAFETY: kill(pid, 0) is a well-defined process-existence probe; no
    // signal is delivered. Returns 0 on success (alive), -1 with ESRCH if gone.
    unsafe { libc::kill(pid, 0) == 0 }
}

// ──────────────────────────────────────────────────────────────────────────
// Tests
// ──────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_free_display_number_stops_when_none_available() {
        // max_tries=0 → immediate None regardless of state.
        assert_eq!(find_free_display_number(1000, 0, &HashSet::new()), None);
    }

    #[test]
    fn env_pairs_includes_display() {
        let env = VirtualDisplayEnv {
            display: ":1000".to_string(),
            display_number: 1000,
            xauthority_path: None,
            vnc_port: None,
            vnc_password: None,
        };
        let pairs = env.env_pairs();
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].0, "DISPLAY");
        assert_eq!(pairs[0].1, ":1000");
    }

    #[test]
    fn env_pairs_includes_xauthority_when_present() {
        let env = VirtualDisplayEnv {
            display: ":1042".to_string(),
            display_number: 1042,
            xauthority_path: Some(
                "/run/user/1000/codemux/vd-1042/Xauthority".to_string(),
            ),
            vnc_port: Some(5910),
            vnc_password: Some("deadbeef".to_string()),
        };
        let pairs = env.env_pairs();
        // vnc_password is intentionally NOT in env_pairs — only the frontend
        // uses it; we don't leak the VNC password to the agent's env.
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0], ("DISPLAY".to_string(), ":1042".to_string()));
        assert_eq!(
            pairs[1],
            (
                "XAUTHORITY".to_string(),
                "/run/user/1000/codemux/vd-1042/Xauthority".to_string()
            )
        );
    }

    #[test]
    fn display_secrets_dir_prefers_xdg_runtime_dir() {
        // Set XDG_RUNTIME_DIR and check we don't land in /tmp.
        // Process-scoped env — affects only this test, and tests are not
        // run in parallel via the default test harness for env-dependent
        // tests, but we still restore the original value to be safe.
        let orig = std::env::var_os("XDG_RUNTIME_DIR");
        // SAFETY: single-threaded in test harness for env mutations.
        unsafe {
            std::env::set_var("XDG_RUNTIME_DIR", "/run/user/1234");
        }
        let dir = display_secrets_dir(1042);
        assert_eq!(
            dir,
            std::path::PathBuf::from(format!(
                "/run/user/1234/{}/vd-1042",
                crate::APP_DIR_NAME
            ))
        );
        unsafe {
            match orig {
                Some(v) => std::env::set_var("XDG_RUNTIME_DIR", v),
                None => std::env::remove_var("XDG_RUNTIME_DIR"),
            }
        }
    }

    #[test]
    fn find_free_tcp_port_returns_something_in_range() {
        let base = 45_000;
        let port = find_free_tcp_port(base, 50).expect("should find a free port");
        assert!(port >= base && port < base + 50);
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn orphan_sweep_removes_stale_locks_and_keeps_live_ones() {
        use std::fs;
        // Build a synthetic /tmp + /tmp/.X11-unix layout so we don't touch
        // the real system paths.
        let tmp = tempfile::tempdir().expect("tempdir");
        let lock_dir = tmp.path();
        let socket_dir = tmp.path().join("X11-unix");
        fs::create_dir_all(&socket_dir).unwrap();

        // Lock 1: "stale" — a fake PID 999_999_999 we'll claim is dead.
        let stale_lock = lock_dir.join(".X1000-lock");
        let stale_sock = socket_dir.join("X1000");
        fs::write(&stale_lock, "999999999\n").unwrap();
        fs::write(&stale_sock, "").unwrap();

        // Lock 2: "live" — our current PID, we'll claim is alive.
        let live_lock = lock_dir.join(".X1001-lock");
        let live_sock = socket_dir.join("X1001");
        let our_pid = std::process::id() as i32;
        fs::write(&live_lock, format!("{our_pid}\n")).unwrap();
        fs::write(&live_sock, "").unwrap();

        // Lock 3: below our range (`:5` — don't touch real system locks).
        let out_of_range = lock_dir.join(".X5-lock");
        fs::write(&out_of_range, "1\n").unwrap();

        // Lock 4: malformed (non-numeric PID).
        let malformed = lock_dir.join(".X1002-lock");
        fs::write(&malformed, "not-a-pid\n").unwrap();

        orphan_sweep_in(lock_dir, &socket_dir, |pid| pid == our_pid);

        // Stale: unlinked.
        assert!(!stale_lock.exists(), "stale lock should be unlinked");
        assert!(!stale_sock.exists(), "stale socket should be unlinked");
        // Live: kept.
        assert!(live_lock.exists(), "live lock must be preserved");
        assert!(live_sock.exists(), "live socket must be preserved");
        // Out-of-range: kept (we never touch :0-:999).
        assert!(out_of_range.exists(), "out-of-range lock must not be touched");
        // Malformed: kept (can't confirm stale, so leave alone).
        assert!(malformed.exists(), "malformed lock must be preserved");
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn pid_is_live_xvfb_rejects_non_xvfb_processes() {
        // Current process is `codemux_lib-<hash>` (the test binary),
        // definitely not "Xvfb" — should return false even though it's alive.
        let our_pid = std::process::id() as i32;
        assert!(is_pid_alive(our_pid));
        assert!(
            !pid_is_live_xvfb(our_pid),
            "our own PID should not pass the Xvfb-name check"
        );
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn pid_is_live_xvfb_returns_false_for_dead_pids() {
        // PID 1 is always alive (init), but it's not Xvfb, so the stricter
        // check rejects it. A genuinely impossible PID also returns false.
        assert!(!pid_is_live_xvfb(1), "init is alive but not Xvfb");
        assert!(
            !pid_is_live_xvfb(0x7fff_ffff),
            "out-of-range PID must not be treated as live"
        );
        assert!(!pid_is_live_xvfb(-1));
    }

    // Unix-only: the whole XDG_RUNTIME_DIR fallback concept is freedesktop-
    // specific and doesn't apply to Windows (no XDG on Windows, no /tmp
    // semantic equivalent for ageing, no /proc for a guaranteed-unwritable
    // seed path). On Windows `create_dir_all("/proc/...")` happily creates
    // a relative directory in the test CWD and our "unwritable" assumption
    // breaks. Windows users hit the virtual_display path only through future
    // WSL2 integration, where this Linux code runs *inside* the distro.
    #[test]
    #[cfg(unix)]
    fn ensure_secrets_dir_falls_back_when_xdg_unwritable() {
        // Point XDG_RUNTIME_DIR at a read-only path; ensure_secrets_dir
        // should degrade to /tmp/codemux-vd-<N>/ rather than failing.
        let orig = std::env::var_os("XDG_RUNTIME_DIR");
        // SAFETY: we restore below; tests run single-threaded for env mutations.
        unsafe {
            std::env::set_var("XDG_RUNTIME_DIR", "/proc/self/attr/nonexistent-dir");
        }

        // Pick a display number unlikely to be in use on any dev box.
        let result = ensure_secrets_dir(42_999);

        unsafe {
            match orig {
                Some(v) => std::env::set_var("XDG_RUNTIME_DIR", v),
                None => std::env::remove_var("XDG_RUNTIME_DIR"),
            }
        }

        let dir = result.expect("fallback must succeed");
        assert!(
            dir.starts_with("/tmp"),
            "when XDG is unwritable we must fall back to /tmp, got {dir:?}"
        );
        // Clean up after ourselves.
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn manager_is_unsupported_when_xvfb_missing() {
        // On this test host we know Xvfb isn't installed.
        if find_executable("Xvfb").is_none() {
            assert!(!VirtualDisplayManager::is_supported());
            let mgr = VirtualDisplayManager::new();
            let err = mgr.acquire("ws-1").unwrap_err();
            match err {
                Error::XvfbNotFound | Error::Unsupported => {}
                other => panic!("expected XvfbNotFound/Unsupported, got {other:?}"),
            }
            assert_eq!(mgr.active_count(), 0);
        }
    }

    #[test]
    fn release_nonexistent_is_noop() {
        let mgr = VirtualDisplayManager::new();
        mgr.release("not-a-real-workspace");
        assert_eq!(mgr.active_count(), 0);
    }

    #[test]
    fn shutdown_all_on_empty_manager_is_noop() {
        let mgr = VirtualDisplayManager::new();
        mgr.shutdown_all();
        assert_eq!(mgr.active_count(), 0);
    }
}
