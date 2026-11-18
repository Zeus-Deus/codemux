//! OpenCode HTTP server lifecycle.
//!
//! Wraps `opencode serve` as a long-lived child process whose URL is
//! discovered by reading the `"opencode server listening on http://…"`
//! banner that the binary prints to stdout (verified live against
//! OpenCode 1.14.31 on 2026-05-01; the reference clone reads the
//! same line at `apps/server/src/provider/opencodeRuntime.ts:389` —
//! note: stdout, NOT stderr; the original spec's "parse from stderr"
//! line was wrong).
//!
//! # Design
//!
//! * **Port**: forwarded as `--port=0` so OpenCode picks a free
//!   loopback port itself. Avoids the bind-then-release race window
//!   that `TcpListener::bind("127.0.0.1:0")`-then-drop introduces.
//!   The actual URL (with port) comes back in the ready banner.
//! * **Auth**: a fresh 32-char alphanumeric password is generated per
//!   spawn and exported as `OPENCODE_SERVER_PASSWORD`. Every Codemux
//!   request then attaches `Authorization: Basic
//!   base64("opencode:${password}")`. Without the env var OpenCode
//!   prints `"server is unsecured"` and accepts any caller — even on
//!   loopback that's a footgun if the user has another tool poking
//!   at random localhost ports, so we always pin the password.
//! * **Cleanup**: on Unix the server leads its own process group, so MCP
//!   servers and tool subprocesses it starts are stopped with it. Shutdown
//!   sends TERM to the group, allows a short grace period, sends KILL, then
//!   reaps the leader; Drop (including cancelled startup) KILLs the group.
//!   Windows shutdown uses `taskkill /T /F`; Windows Drop still kills only
//!   the direct child. After a crash, the next spawn kills the group of any
//!   recorded server whose Codemux owner is gone (Linux; see `orphans`).
//!   Descendants that detach into their own group/session escape.
//! * **GUI sanitation**: env is sanitised through
//!   `sanitize_gui_env_tokio` so an OpenCode binary
//!   that inadvertently launches a GUI helper does not pop windows
//!   on the host display.
//!
//! # Stage scope
//!
//! Stage 2 only spawns the server long enough to harvest a model
//! list. Stage 3 will reuse this same lifecycle for streaming
//! `session.promptAsync` turns; the public API
//! ([`OpenCodeServer::base_url`] + [`server_password`]) is the same
//! either way.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use rand::distributions::{Alphanumeric, DistString};
#[cfg(unix)]
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};

use super::api::OpenCodeApi;
use crate::execution::sanitize_gui_env_tokio;

/// Upstream defaults plus one rule: every chat registers its Codemux tools
/// as a `codemux_<id>` MCP server on this shared server, and only that chat's
/// session is granted `codemux_<id>_*`, so other chats are not offered them.
/// Agent rules in the user's config take precedence over this, so it is not
/// what isolates chats: the gateway accepts a call only from the session its
/// credential is bound to.
pub(crate) const CONFIG_CONTENT: &str = r#"{"permission":{"codemux_*":"deny"}}"#;

/// Crash recovery. Each server is recorded with the Codemux process that owns
/// it; a later spawn kills the process group of any recorded server whose
/// owner is gone. Live owners (another Codemux, `codemux serve`) are left alone,
/// and start times guard against PID reuse.
#[cfg(target_os = "linux")]
mod orphans {
    use std::path::PathBuf;

    #[derive(serde::Serialize, serde::Deserialize)]
    struct Record {
        server: u32,
        server_start: u64,
        owner: u32,
        owner_start: u64,
    }

    fn dir() -> PathBuf {
        #[cfg(test)]
        return std::env::temp_dir().join(format!("codemux-opencode-test-{}", std::process::id()));
        #[cfg(not(test))]
        match dirs::runtime_dir() {
            Some(runtime) => runtime.join("codemux-opencode"),
            // The shared temp dir needs a per-user name; `private` refuses a
            // directory another user created first.
            None => std::env::temp_dir()
                .join(format!("codemux-opencode-{}", unsafe { libc::getuid() })),
        }
    }

    /// Records are trusted only from a real directory this user owns that
    /// nobody else can write, and from regular files this user owns: a forged
    /// record would make Codemux kill an unrelated process group.
    fn private(path: &std::path::Path, directory: bool) -> bool {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        std::fs::symlink_metadata(path).is_ok_and(|meta| {
            let kind = if directory { meta.is_dir() } else { meta.is_file() };
            kind && meta.uid() == unsafe { libc::getuid() }
                && meta.permissions().mode() & 0o022 == 0
        })
    }

    /// The record directory, created private when missing.
    fn ensure_dir() -> Option<PathBuf> {
        use std::os::unix::fs::DirBuilderExt;
        let dir = dir();
        let _ = std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&dir);
        private(&dir, true).then_some(dir)
    }

    /// `starttime` from `/proc/<pid>/stat`: field 22, the 20th after `comm`.
    fn start_ticks(pid: u32) -> Option<u64> {
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
        stat.rsplit_once(')')?.1.split_whitespace().nth(19)?.parse().ok()
    }

    pub(super) fn record(server: u32) {
        let owner = std::process::id();
        let (Some(server_start), Some(owner_start)) = (start_ticks(server), start_ticks(owner))
        else {
            return;
        };
        let record = Record { server, server_start, owner, owner_start };
        let (Some(dir), Ok(json)) = (ensure_dir(), serde_json::to_vec(&record)) else {
            return;
        };
        // Written whole, then renamed into place: `reap` never reads half a
        // record.
        let staging = dir.join(format!(".{server}.json.tmp"));
        if std::fs::write(&staging, json).is_ok()
            && std::fs::rename(&staging, dir.join(format!("{server}.json"))).is_err()
        {
            let _ = std::fs::remove_file(&staging);
        }
    }

    pub(super) fn forget(server: u32) {
        let _ = std::fs::remove_file(dir().join(format!("{server}.json")));
    }

    pub(super) fn reap() {
        reap_in(&dir());
    }

    fn reap_in(dir: &std::path::Path) {
        if !private(dir, true) {
            return;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for path in entries.flatten().map(|entry| entry.path()) {
            if path.extension().is_none_or(|ext| ext != "json") {
                continue;
            }
            if !private(&path, false) {
                continue;
            }
            let Some(record) = std::fs::read(&path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Record>(&bytes).ok())
            else {
                let _ = std::fs::remove_file(&path);
                continue;
            };
            let server_alive = start_ticks(record.server) == Some(record.server_start);
            if server_alive && start_ticks(record.owner) == Some(record.owner_start) {
                continue;
            }
            if server_alive {
                super::signal_group(record.server, libc::SIGKILL);
            }
            let _ = std::fs::remove_file(&path);
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn reaps_only_servers_whose_owner_is_gone() {
            let spawn = || {
                use std::os::unix::process::CommandExt;
                std::process::Command::new("/bin/sleep")
                    .arg("600")
                    .process_group(0)
                    .spawn()
                    .unwrap()
            };
            let (mut orphan, mut owned) = (spawn(), spawn());
            let write = |server: u32, owner: u32, owner_start: u64| {
                let record = Record {
                    server,
                    server_start: start_ticks(server).unwrap(),
                    owner,
                    owner_start,
                };
                std::fs::create_dir_all(dir()).unwrap();
                std::fs::write(
                    dir().join(format!("{server}.json")),
                    serde_json::to_vec(&record).unwrap(),
                )
                .unwrap();
            };
            let me = std::process::id();
            // An owner start time that never matches: that owner crashed.
            write(orphan.id(), me, u64::MAX);
            write(owned.id(), me, start_ticks(me).unwrap());
            reap();
            let status = orphan.wait().unwrap();
            assert!(!status.success(), "the orphan's group was killed");
            assert!(!dir().join(format!("{}.json", orphan.id())).exists());
            assert!(owned.try_wait().unwrap().is_none(), "a live owner's server stays");
            assert!(dir().join(format!("{}.json", owned.id())).exists());
            forget(owned.id());
            let _ = owned.kill();
            let _ = owned.wait();
        }

        #[test]
        fn ignores_records_another_user_could_have_planted() {
            use std::os::unix::fs::PermissionsExt;
            use std::os::unix::process::CommandExt;
            // A group leader, so a trusted record naming it would kill it.
            let mut victim = std::process::Command::new("/bin/sleep")
                .arg("600")
                .process_group(0)
                .spawn()
                .unwrap();
            let forged = serde_json::to_vec(&Record {
                server: victim.id(),
                server_start: start_ticks(victim.id()).unwrap(),
                owner: u32::MAX,
                owner_start: 0,
            })
            .unwrap();
            let scratch = tempfile::tempdir().unwrap();
            // A shared, writable directory another user could have created.
            let shared = scratch.path().join("shared");
            std::fs::create_dir(&shared).unwrap();
            std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o777)).unwrap();
            std::fs::write(shared.join("1.json"), &forged).unwrap();
            reap_in(&shared);
            assert!(shared.join("1.json").exists(), "an untrusted directory is not read");
            // A symlink to a private directory is not trusted either.
            let real = scratch.path().join("real");
            std::fs::create_dir(&real).unwrap();
            std::fs::write(real.join("1.json"), &forged).unwrap();
            let link = scratch.path().join("link");
            std::os::unix::fs::symlink(&real, &link).unwrap();
            reap_in(&link);
            assert!(real.join("1.json").exists(), "a symlinked directory is not read");
            // A record file another user can write is skipped.
            std::fs::set_permissions(real.join("1.json"), std::fs::Permissions::from_mode(0o666))
                .unwrap();
            reap_in(&real);
            assert!(real.join("1.json").exists(), "a writable record is not trusted");
            assert!(victim.try_wait().unwrap().is_none(), "the victim survives");
            let _ = victim.kill();
            let _ = victim.wait();
        }
    }
}

/// Banner prefix OpenCode emits on stdout once the HTTP server is
/// accepting connections. Verified live against OpenCode 1.14.31
/// (the binary's actual output: `"opencode server listening on
/// http://127.0.0.1:38871"`). Pinned to match
/// `OPENCODE_SERVER_READY_PREFIX` at
/// `/tmp/<reference>/apps/server/src/provider/opencodeRuntime.ts:36`.
pub const READY_PREFIX: &str = "opencode server listening";

/// Default budget for the spawn → ready handshake. OpenCode usually
/// prints the banner inside ~500 ms; 10 s is a generous cap so a
/// debug-build binary or a busy machine still succeeds. Past this
/// budget [`OpenCodeServer::spawn`] returns an error string the
/// command surface forwards verbatim.
pub const READY_TIMEOUT: Duration = Duration::from_secs(10);

/// Bound the direct child's termination/reap so GUI quit cannot hang
/// indefinitely if the OS fails to complete process termination.
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

/// Time the server's process group gets to exit after TERM before KILL.
#[cfg(unix)]
const TERMINATE_GRACE: Duration = Duration::from_secs(1);

/// Length of the random server password. 32 alphanumeric characters
/// = ~190 bits of entropy, vastly more than enough to make a
/// loopback brute-force pointless. Locked here so tests can pin the
/// shape without recomputing the constant.
const SERVER_PASSWORD_LEN: usize = 32;

/// Live handle to a running `opencode serve` child.
///
/// Drop the value to kill the server. Hold it for the duration of
/// any session that talks to its HTTP endpoint.
#[derive(Debug)]
pub struct OpenCodeServer {
    /// Owned until explicit termination/reap. Drop is only a fallback,
    /// not proof that teardown completed.
    child: ServerProcess,
    /// Stable URL extracted from the ready banner — usable as-is for
    /// `reqwest` requests.
    base_url: String,
    api: OpenCodeApi,
    /// Password that was set into `OPENCODE_SERVER_PASSWORD` for
    /// this specific child. Bound to the server's lifetime: when the
    /// server dies the password becomes meaningless.
    server_password: String,
}

// Task-scoped scheduling gate. It delays actual cleanup without replacing
// the production kill/wait or its result, and disappears with its owner.
#[cfg(test)]
tokio::task_local! {
    pub(crate) static SHUTDOWN_GATE: (std::sync::Arc<tokio::sync::Notify>, std::sync::Arc<tokio::sync::Notify>);
}

impl OpenCodeServer {
    /// Spawn `opencode serve` with a fresh random password and
    /// resolve once the server prints its ready banner.
    ///
    /// Returns `Err` on:
    ///
    /// * spawn failure (`opencode` not on PATH or unable to launch);
    /// * the binary exiting before printing the banner;
    /// * the banner not arriving inside [`READY_TIMEOUT`].
    pub async fn spawn(binary_path: &Path) -> Result<Self, String> {
        Self::spawn_with_timeout(binary_path, READY_TIMEOUT).await
    }

    /// Same as [`spawn`](Self::spawn) but exposes the timeout so
    /// tests can pin a tight budget without waiting 10 s on every
    /// failure path.
    pub async fn spawn_with_timeout(
        binary_path: &Path,
        ready_budget: Duration,
    ) -> Result<Self, String> {
        let server_password =
            Alphanumeric.sample_string(&mut rand::thread_rng(), SERVER_PASSWORD_LEN);

        #[cfg(target_os = "linux")]
        orphans::reap();
        // NOTE: this is a single long-lived server shared by every OpenCode
        // chat session, so per-session workspace env (CODEMUX_WORKSPACE_ID,
        // CODEMUX_PANE_ID, …) can NOT be injected here the way it is for the
        // per-session Claude/Codex sidecars — one env would be wrong for all
        // but one workspace. OpenCode agents therefore rely on the
        // control-layer cwd fallback (`resolve_workspace_id_by_cwd` in
        // control.rs) to route `codemux browser open` to the right workspace.
        let mut cmd = Command::new(binary_path);
        cmd.args(["serve", "--hostname=127.0.0.1", "--port=0"]);
        cmd.env("OPENCODE_SERVER_PASSWORD", &server_password);
        // v2 prefers OPENCODE_PASSWORD over the legacy alias. Bind both to
        // this child so an ambient CLI/service password cannot override it.
        cmd.env("OPENCODE_PASSWORD", &server_password);
        // Echo OpenCode's documented "use upstream defaults, ignore
        // user config" knob — the reference clone does this at line
        // 340. Without it OpenCode honours
        // `~/.config/opencode/config.json`, which
        // could surprise a user whose Codemux pane suddenly behaves
        // differently from their CLI.
        cmd.env("OPENCODE_CONFIG_CONTENT", CONFIG_CONTENT);
        // The shared server runs every OpenCode chat's tools.
        cmd.env(crate::control_identity::AGENT_PROCESS_ENV, "opencode");
        cmd.stdin(Stdio::null());
        cmd.stdout(Stdio::piped());
        // Nothing reads stderr, and an unread pipe would block the server
        // once full — and every OpenCode chat with it.
        cmd.stderr(Stdio::null());
        cmd.kill_on_drop(true);
        #[cfg(unix)]
        cmd.process_group(0);
        sanitize_gui_env_tokio(&mut cmd);

        let mut child = cmd
            .spawn()
            .map(ServerProcess::new)
            .map_err(|err| format!("spawn_failed: {} ({err})", binary_path.display()))?;
        if let Some(pid) = child.0.id() {
            #[cfg(target_os = "linux")]
            orphans::record(pid);
            crate::control_identity::register_agent_process(pid);
        }
        let stdout = child
            .0
            .stdout
            .take()
            .ok_or_else(|| "spawn_failed: child stdout pipe missing".to_string())?;
        let url_result = Self::read_banner(&mut child, stdout, ready_budget).await;
        let (base_url, api) = match url_result {
            Ok(Ok(found)) => found,
            Ok(Err(message)) => {
                // Probe drained stdout before exiting — kill the
                // child and surface the failure verbatim.
                let _ = tokio::time::timeout(SHUTDOWN_TIMEOUT, child.terminate()).await;
                return Err(message);
            }
            Err(_) => {
                let _ = tokio::time::timeout(SHUTDOWN_TIMEOUT, child.terminate()).await;
                return Err(format!(
                    "ready_timeout_after_{}ms",
                    ready_budget.as_millis()
                ));
            }
        };

        Ok(Self {
            child,
            base_url,
            api,
            server_password,
        })
    }

    /// Wait up to `budget` for the ready banner on `stdout`, and leave the
    /// rest of stdout draining, owned by `child` so it stops with the
    /// process however startup ends — even if this future is dropped.
    #[cfg(unix)]
    async fn read_banner(
        child: &mut ServerProcess,
        stdout: tokio::process::ChildStdout,
        budget: Duration,
    ) -> Result<Result<(String, OpenCodeApi), String>, tokio::time::error::Elapsed> {
        tokio::time::timeout(budget, async {
            let (base_url, api, rest) = await_ready_url(stdout).await?;
            // Keep reading stdout after the banner too: dropping the reader
            // closes the pipe, and the server's next write to stdout would
            // fail instead.
            child.1 = Some(stdout_drain::Drain::start(rest));
            Ok((base_url, api))
        })
        .await
    }

    /// Windows: the pipe is read on a thread of its own from the start —
    /// banner and drain alike — because tokio's pipe reads run on a
    /// blocking thread nothing can cancel. The drain is owned by `child`
    /// before the first await.
    #[cfg(windows)]
    async fn read_banner(
        child: &mut ServerProcess,
        stdout: tokio::process::ChildStdout,
        budget: Duration,
    ) -> Result<Result<(String, OpenCodeApi), String>, tokio::time::error::Elapsed> {
        let (drain, banner) = match stdout_drain::Drain::start(stdout) {
            Ok(started) => started,
            Err(e) => return Ok(Err(format!("spawn_failed: read stdout ({e})"))),
        };
        child.1 = Some(drain);
        tokio::time::timeout(budget, async {
            banner
                .await
                .unwrap_or_else(|_| Err("ready_banner_missing; child stdout closed".to_string()))
        })
        .await
    }

    /// Stable HTTP URL the server is listening on, e.g.
    /// `http://127.0.0.1:38871`. Trailing slash already stripped to
    /// match [`OpenCodeClientConfig::new`](super::client::OpenCodeClientConfig::new).
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn api(&self) -> OpenCodeApi {
        self.api
    }

    /// Server password used for HTTP Basic auth. Always matches the
    /// `OPENCODE_SERVER_PASSWORD` exported into the child's env at
    /// spawn time.
    pub fn server_password(&self) -> &str {
        &self.server_password
    }

    /// Best-effort PID of the running child. `None` after the child
    /// has been reaped.
    pub fn pid(&self) -> Option<u32> {
        self.child.0.id()
    }

    /// Terminate the owned process group/tree and reap its direct child.
    /// Idempotent after a completed wait; Drop remains the error fallback.
    pub async fn shutdown(&mut self) -> Result<(), String> {
        #[cfg(test)]
        if let Ok((entered, release)) = SHUTDOWN_GATE.try_with(Clone::clone) {
            entered.notify_one();
            release.notified().await;
        }
        #[cfg(target_os = "linux")]
        let pid = self.child.0.id();
        tokio::time::timeout(SHUTDOWN_TIMEOUT, self.child.terminate())
            .await
            .map_err(|_| "opencode_shutdown_timeout".to_string())?
            .map_err(|error| format!("opencode_shutdown_failed: {error}"))?;
        #[cfg(target_os = "linux")]
        if let Some(pid) = pid {
            orphans::forget(pid);
        }
        Ok(())
    }
}

impl Drop for OpenCodeServer {
    fn drop(&mut self) {
        // Logged on shutdown so a stuck-server diagnosis can confirm
        // the right child was killed. `ServerProcess` does the actual
        // cleanup — this is just a breadcrumb.
        if let Some(pid) = self.child.0.id() {
            eprintln!(
                "[codemux::opencode] dropping OpenCodeServer (pid={pid}, url={})",
                self.base_url
            );
        }
    }
}

/// The spawned `opencode serve` leader. On Unix it leads its own process
/// group; every group signal below is sent while the leader is still
/// unreaped (`Child::id` is `Some`), so its PID — the group ID — cannot have
/// been recycled for an unrelated group.
///
/// It also owns the drain of its stdout after the ready banner. The drain
/// lives and dies with the process: it is stopped on terminate and on drop,
/// so a descendant still holding the pipe open cannot keep the drain (and
/// the pipe) alive past the server.
#[derive(Debug)]
struct ServerProcess(Child, Option<stdout_drain::Drain>);

impl ServerProcess {
    fn new(child: Child) -> Self {
        Self(child, None)
    }

    fn stop_drain(&mut self) {
        if let Some(drain) = self.1.take() {
            drain.stop();
        }
    }

    /// TERM the group, wait up to [`TERMINATE_GRACE`] for the leader, KILL
    /// the group (descendants may ignore TERM or outlive the leader), reap.
    async fn terminate(&mut self) -> std::io::Result<()> {
        #[cfg(unix)]
        if let Some(pid) = self.0.id() {
            signal_group(pid, libc::SIGTERM);
            let _ = tokio::time::timeout(TERMINATE_GRACE, leader_exited(pid)).await;
            signal_group(pid, libc::SIGKILL);
        }
        #[cfg(windows)]
        if let (Some(pid), Some(Ok(handle))) = (
            self.0.id(),
            self.0.raw_handle().map(|raw| {
                unsafe { std::os::windows::io::BorrowedHandle::borrow_raw(raw) }
                    .try_clone_to_owned()
            }),
        ) {
            // taskkill finds the tree by PID. The job owns a duplicate of the
            // process handle, so the PID cannot be reused even if this future
            // is cancelled (SHUTDOWN_TIMEOUT) and the child dropped first.
            let _ = tokio::task::spawn_blocking(move || {
                crate::agent_browser::kill_pid(pid);
                drop(handle);
            })
            .await;
        }
        let result = self.0.kill().await;
        self.stop_drain();
        result
    }
}

impl Drop for ServerProcess {
    fn drop(&mut self) {
        self.stop_drain();
        // Runs before the inner Child drops, so the leader is still
        // unreaped; `kill_on_drop` then kills and reaps the leader.
        #[cfg(unix)]
        if let Some(pid) = self.0.id() {
            signal_group(pid, libc::SIGKILL);
        }
    }
}

#[cfg(unix)]
fn signal_group(pid: u32, signal: libc::c_int) {
    // ESRCH just means the whole group is already gone.
    unsafe { libc::kill(-(pid as libc::pid_t), signal) };
}

/// Resolve once the leader has exited, without reaping it (`WNOWAIT`).
#[cfg(target_os = "linux")]
async fn leader_exited(pid: u32) {
    loop {
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        let result = unsafe {
            libc::waitid(
                libc::P_PID,
                pid as libc::id_t,
                &mut info,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        };
        if result != 0 || unsafe { info.si_pid() } != 0 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Elsewhere on Unix, always allow the full grace period.
#[cfg(all(unix, not(target_os = "linux")))]
async fn leader_exited(_pid: u32) {
    std::future::pending::<()>().await
}

/// Read and discard a child's output until it closes, so the child never
/// blocks on a full pipe.
/// Reads and discards the server's stdout after the ready banner, so the
/// server never blocks or fails on a full or closed pipe, and stops with the
/// server even if a descendant still holds the pipe's write end.
mod stdout_drain {
    /// Unix: an async task, which aborting stops outright.
    #[cfg(unix)]
    #[derive(Debug)]
    pub(super) struct Drain(tokio::task::AbortHandle);

    #[cfg(unix)]
    impl Drain {
        pub(super) fn start(mut rest: tokio::io::BufReader<tokio::process::ChildStdout>) -> Self {
            Self(
                tokio::spawn(async move {
                    let _ = tokio::io::copy(&mut rest, &mut tokio::io::sink()).await;
                })
                .abort_handle(),
            )
        }

        pub(super) fn stop(self) {
            self.0.abort();
        }

        /// Whether the drain has stopped reading.
        #[cfg(test)]
        pub(super) fn finished(&self) -> impl Fn() -> bool {
            let handle = self.0.clone();
            move || handle.is_finished()
        }
    }

    /// The longest startup line parsed for the banner, and the most startup
    /// output kept for an error message.
    #[cfg(any(windows, test))]
    const MAX_LINE: usize = 4096;
    #[cfg(any(windows, test))]
    const MAX_CAPTURED: usize = 8192;

    /// Read startup output line by line until the ready banner. Lines are
    /// scanned as bytes: a line longer than [`MAX_LINE`] is skipped to its
    /// end without being parsed, and the rest are decoded leniently, so a
    /// multibyte character cut by a buffer boundary never ends startup.
    /// Memory stays bounded however chatty the output is.
    #[cfg(any(windows, test))]
    pub(super) fn scan_banner(
        reader: &mut impl std::io::BufRead,
        stop: &std::sync::atomic::AtomicBool,
    ) -> Result<(String, super::OpenCodeApi), String> {
        let mut captured = String::new();
        let mut line: Vec<u8> = Vec::with_capacity(256);
        let mut overlong = false;
        loop {
            if stop.load(std::sync::atomic::Ordering::SeqCst) {
                return Err("ready_banner_missing; stopped".to_string());
            }
            let chunk = match reader.fill_buf() {
                Ok([]) | Err(_) => {
                    return Err(format!(
                        "ready_banner_missing; child stdout closed. captured:\n{}",
                        captured.trim()
                    ))
                }
                Ok(chunk) => chunk,
            };
            let (part, ends_line) = match chunk.iter().position(|&b| b == b'\n') {
                Some(i) => (&chunk[..=i], true),
                None => (chunk, false),
            };
            let used = part.len();
            if !overlong {
                if line.len() + used > MAX_LINE + 1 {
                    // Too long to be the banner; skip to the end of the line.
                    overlong = true;
                    line.clear();
                } else {
                    line.extend_from_slice(part);
                }
            }
            reader.consume(used);
            if !ends_line {
                continue;
            }
            if !std::mem::take(&mut overlong) {
                let text = String::from_utf8_lossy(&line);
                let text = text.trim_end_matches(['\r', '\n']);
                if let Some(found) = super::parse_ready_url(text)
                    .and_then(|url| super::OpenCodeApi::from_ready_banner(text).map(|api| (url, api)))
                {
                    return Ok(found);
                }
                if captured.len() < MAX_CAPTURED {
                    if !captured.is_empty() {
                        captured.push('\n');
                    }
                    captured.push_str(text);
                }
            }
            line.clear();
        }
    }

    /// The banner the reader thread found, or why it found none.
    #[cfg(windows)]
    pub(super) type Banner = tokio::sync::oneshot::Receiver<Result<(String, super::OpenCodeApi), String>>;

    /// Windows: the pipe is read on a thread of its own — the banner, then
    /// everything after it. Stopping sets a flag that keeps the thread from
    /// starting another read, then cancels the read in progress with
    /// `CancelIoEx` until the thread is seen to exit.
    #[cfg(windows)]
    #[derive(Debug)]
    pub(super) struct Drain {
        stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
        done: std::sync::Arc<std::sync::atomic::AtomicBool>,
        /// Wakes the cancel thread; dropping it does too.
        stopping: std::sync::mpsc::Sender<()>,
    }

    #[cfg(windows)]
    impl Drain {
        pub(super) fn start(stdout: tokio::process::ChildStdout) -> Result<(Self, Banner), String> {
            use std::io::Read;
            use std::sync::atomic::{AtomicBool, Ordering};
            use std::sync::Arc;

            let handle = stdout.into_owned_handle().map_err(|e| e.to_string())?;
            let reader = std::fs::File::from(handle);
            let pipe = reader.try_clone().map_err(|e| e.to_string())?;
            let stop = Arc::new(AtomicBool::new(false));
            let done = Arc::new(AtomicBool::new(false));

            // Cancellation is in place before the first read: a thread that
            // waits for the stop, then keeps cancelling any pending read until
            // the reader is seen to exit. A read begun just after one cancel
            // is caught by the next; there is no deadline, because the reader
            // always exits once its read fails. If it cannot start, the drain
            // does not start either.
            let (stopping, stopped) = std::sync::mpsc::channel::<()>();
            let (cancel_stop, cancel_done) = (stop.clone(), done.clone());
            #[cfg(test)]
            let canceller_exited = super::DRAIN_PROBE
                .try_with(|probe| probe.lock().unwrap().canceller_exited.clone())
                .ok()
                .flatten();
            let canceller = std::thread::Builder::new()
                .name("opencode-stdout-cancel".into())
                .spawn(move || {
                    use std::os::windows::io::AsRawHandle;
                    // A stop, or the drain being dropped without one.
                    let _ = stopped.recv();
                    cancel_stop.store(true, Ordering::SeqCst);
                    let mut wait = std::time::Duration::from_millis(5);
                    while !cancel_done.load(Ordering::SeqCst) {
                        // SAFETY: a live handle owned by `pipe`; a null
                        // OVERLAPPED cancels every read pending on it.
                        unsafe {
                            windows_sys::Win32::System::IO::CancelIoEx(pipe.as_raw_handle(), std::ptr::null())
                        };
                        std::thread::sleep(wait);
                        wait = (wait * 2).min(std::time::Duration::from_millis(250));
                    }
                    #[cfg(test)]
                    if let Some(exited) = canceller_exited {
                        exited.store(true, Ordering::SeqCst);
                    }
                })
                .map_err(|e| e.to_string())?;
            #[cfg(test)]
            let pause = super::DRAIN_PROBE
                .try_with(|probe| {
                    let mut probe = probe.lock().unwrap();
                    probe.done = Some(done.clone());
                    (probe.pause_before_drain_read.take(), probe.fail_reader_spawn)
                })
                .ok();
            #[cfg(test)]
            let (pause, fail_reader_spawn) = match pause {
                Some((pause, fail)) => (pause, fail),
                None => (None, false),
            };
            let (banner_tx, banner_rx) = tokio::sync::oneshot::channel();
            let (thread_stop, thread_done) = (stop.clone(), done.clone());
            let reader_thread = move || {
                let mut reader = std::io::BufReader::new(reader);
                // The banner first; its buffers live only inside the scan.
                let result = scan_banner(&mut reader, &thread_stop);
                let found = result.is_ok();
                let _ = banner_tx.send(result);
                if !found {
                    thread_done.store(true, Ordering::SeqCst);
                    return;
                }
                // Then everything after it.
                let mut buf = [0u8; 8192];
                while !thread_stop.load(Ordering::SeqCst) {
                    #[cfg(test)]
                    if let Some((arrived, release)) = pause.as_ref() {
                        arrived.wait();
                        release.wait();
                    }
                    match reader.read(&mut buf) {
                        Ok(0) | Err(_) => break,
                        Ok(_) => {}
                    }
                }
                thread_done.store(true, Ordering::SeqCst);
            };
            let reader_builder = std::thread::Builder::new().name("opencode-stdout".into());
            #[cfg(test)]
            let spawned = if fail_reader_spawn {
                drop(reader_thread);
                Err(std::io::Error::other("simulated reader spawn failure"))
            } else {
                reader_builder.spawn(reader_thread)
            };
            #[cfg(not(test))]
            let spawned = reader_builder.spawn(reader_thread);
            if let Err(e) = spawned {
                // No reader will ever set `done`: set it here, wake the cancel
                // thread so it sees that and exits, and wait for it, so a
                // failed start leaves no thread or pipe handle behind.
                done.store(true, Ordering::SeqCst);
                drop(stopping);
                let _ = canceller.join();
                return Err(e.to_string());
            }

            Ok((Self { stop, done, stopping }, banner_rx))
        }

        pub(super) fn stop(self) {
            // The flag keeps the reader from starting another read; the
            // cancel thread, already running, releases the one in progress.
            self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
            let _ = self.stopping.send(());
        }

        /// Whether the reader thread has exited.
        #[cfg(test)]
        pub(super) fn finished(&self) -> impl Fn() -> bool {
            let done = self.done.clone();
            move || done.load(std::sync::atomic::Ordering::SeqCst)
        }
    }
}

/// Test hook for the Windows reader thread: it hands over the thread's
/// "exited" flag, so a test can watch it after startup itself has been
/// dropped, and can pause the thread just before its first drain read.
#[cfg(all(test, windows))]
#[derive(Default)]
pub(crate) struct DrainProbe {
    pub(crate) done: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    pub(crate) pause_before_drain_read: Option<(std::sync::Arc<std::sync::Barrier>, std::sync::Arc<std::sync::Barrier>)>,
    /// Make the reader thread's spawn fail, after the cancel thread started.
    pub(crate) fail_reader_spawn: bool,
    /// Set by the cancel thread as it exits.
    pub(crate) canceller_exited: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
}

#[cfg(all(test, windows))]
tokio::task_local! {
    pub(crate) static DRAIN_PROBE: std::sync::Arc<std::sync::Mutex<DrainProbe>>;
}

/// Read lines from the child's stdout until the ready banner shows
/// up or the stream closes. Only the *banner* triggers `Ok`; anything
/// else is forwarded as part of the diagnostic on failure. On success the
/// rest of the stream is handed back so the caller keeps draining it.
#[cfg(unix)]
async fn await_ready_url(
    stdout: tokio::process::ChildStdout,
) -> Result<(String, OpenCodeApi, BufReader<tokio::process::ChildStdout>), String> {
    let mut reader = BufReader::new(stdout).lines();
    let mut buffered = String::new();
    while let Some(line) = reader
        .next_line()
        .await
        .map_err(|err| format!("stdout_read_error: {err}"))?
    {
        if !buffered.is_empty() {
            buffered.push('\n');
        }
        buffered.push_str(&line);
        if let Some(url) = parse_ready_url(&line) {
            if let Some(api) = OpenCodeApi::from_ready_banner(&line) {
                return Ok((url, api, reader.into_inner()));
            }
        }
    }
    Err(format!(
        "ready_banner_missing; child stdout closed. captured:\n{}",
        buffered.trim()
    ))
}

/// Pure parser for a single stdout line. Returns `Some(url)` when
/// the line matches the OpenCode ready banner (verified shape:
/// `"opencode server listening on http://127.0.0.1:38871"`).
///
/// Pulled out so the regex can be unit-tested without spawning a
/// real binary. Mirrors `parseServerUrlFromOutput` at
/// `/tmp/<reference>/apps/server/src/provider/opencodeRuntime.ts:145-154`.
pub fn parse_ready_url(line: &str) -> Option<String> {
    let trimmed = line.trim_start();
    let rest = trimmed
        .strip_prefix(READY_PREFIX)
        .or_else(|| trimmed.strip_prefix("server listening"))?;
    // OpenCode 1.x includes the binary name; 2.x prints only
    // `server listening`. Both then include `on <url>`.
    // Find the first HTTP substring to preserve the legacy `at` form.
    let http_idx = rest.find("http")?;
    let after_http = &rest[http_idx..];
    let end = after_http
        .find(|c: char| c.is_whitespace())
        .unwrap_or(after_http.len());
    let url: String = after_http[..end].trim_end_matches('/').to_string();
    if url.is_empty() {
        None
    } else {
        Some(url)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ready_prefix_matches_reference_constant() {
        // Pinned: changing this value silently desyncs us from the
        // reference impl and breaks ready detection on any opencode
        // build that follows the existing banner format.
        assert_eq!(READY_PREFIX, "opencode server listening");
    }

    #[test]
    fn parse_ready_url_extracts_http_loopback() {
        let line = "opencode server listening on http://127.0.0.1:38871";
        assert_eq!(parse_ready_url(line), Some("http://127.0.0.1:38871".into()));
    }

    #[test]
    fn parse_ready_url_accepts_opencode_v2_banner() {
        assert_eq!(
            parse_ready_url("server listening on http://127.0.0.1:60977"),
            Some("http://127.0.0.1:60977".into())
        );
    }

    #[test]
    fn parse_ready_url_handles_https_and_trailing_slash() {
        let line = "opencode server listening at https://example.test:8080/";
        assert_eq!(
            parse_ready_url(line),
            Some("https://example.test:8080".into())
        );
    }

    #[test]
    fn parse_ready_url_strips_leading_indent() {
        // Some logger banners prepend a timestamp pad. Trim leading
        // whitespace so the prefix match still hits.
        let line = "   opencode server listening on http://127.0.0.1:9000";
        assert_eq!(parse_ready_url(line), Some("http://127.0.0.1:9000".into()));
    }

    #[test]
    fn parse_ready_url_rejects_unrelated_lines() {
        assert_eq!(parse_ready_url(""), None);
        assert_eq!(
            parse_ready_url("Warning: OPENCODE_SERVER_PASSWORD is not set"),
            None
        );
        assert_eq!(parse_ready_url("opencode something different"), None);
        // Prefix matches but no URL anywhere.
        assert_eq!(
            parse_ready_url("opencode server listening on (no url)"),
            None
        );
    }

    #[test]
    fn server_password_constant_is_long_enough() {
        // Pinned: lowering this below 32 chars without a deliberate
        // change of plan would silently weaken loopback security on
        // every spawn. ~190 bits at 32 alphanumerics.
        assert_eq!(SERVER_PASSWORD_LEN, 32);
    }

    /// Fake `opencode serve` that starts a TERM-ignoring descendant (an MCP
    /// server or tool subprocess stand-in) and records its PID.
    #[cfg(target_os = "linux")]
    fn fake_server_with_descendant(banner: bool) -> (tempfile::TempDir, std::path::PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("opencode");
        let banner = if banner {
            "echo 'opencode server listening on http://127.0.0.1:1'"
        } else {
            ""
        };
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\n[ \"$1\" = probe ] && exit 0\n(trap '' TERM; exec sleep 600) &\necho $! > \"$(dirname \"$0\")/pid\"\n{banner}\nwait\n"
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        wait_until_executable(&script);
        (dir, script)
    }

    /// A sibling test thread that forks while the script's write handle is
    /// open keeps that handle until it execs, so executing the fresh script
    /// can fail with ETXTBSY. Once one exec succeeds no writer is left.
    #[cfg(target_os = "linux")]
    fn wait_until_executable(script: &Path) {
        const ETXTBSY: i32 = 26;
        for _ in 0..50 {
            match std::process::Command::new(script).arg("probe").status() {
                Err(error) if error.raw_os_error() == Some(ETXTBSY) => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                result => {
                    assert!(result.unwrap().success(), "fixture probe failed");
                    return;
                }
            }
        }
        panic!("fixture script stayed busy: {}", script.display());
    }

    /// Wait for the descendant to stop; on failure kill it (we started it)
    /// so a failing run does not leak a `sleep 600`.
    #[cfg(target_os = "linux")]
    async fn assert_descendant_stopped(dir: &Path) {
        let pid: i32 = std::fs::read_to_string(dir.join("pid"))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        // A reparented zombie awaiting its new parent's reap has stopped.
        let running = || {
            std::fs::read_to_string(format!("/proc/{pid}/stat"))
                .ok()
                .and_then(|stat| stat.rsplit_once(") ").map(|(_, rest)| !rest.starts_with('Z')))
                .unwrap_or(false)
        };
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while running() && std::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let survived = running();
        if survived {
            unsafe { libc::kill(pid, libc::SIGKILL) };
        }
        assert!(!survived, "OpenCode descendant {pid} survived server cleanup");
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn shutdown_stops_server_descendants() {
        let (dir, script) = fake_server_with_descendant(true);
        let mut server = OpenCodeServer::spawn_with_timeout(&script, Duration::from_secs(5))
            .await
            .unwrap();
        server.shutdown().await.unwrap();
        assert_eq!(server.pid(), None);
        assert_descendant_stopped(dir.path()).await;
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn failed_startup_stops_server_descendants() {
        let (dir, script) = fake_server_with_descendant(false);
        let error = OpenCodeServer::spawn_with_timeout(&script, Duration::from_secs(2))
            .await
            .unwrap_err();
        assert!(error.starts_with("ready_timeout_after_"), "{error}");
        assert_descendant_stopped(dir.path()).await;
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn drop_stops_server_descendants() {
        let (dir, script) = fake_server_with_descendant(true);
        drop(
            OpenCodeServer::spawn_with_timeout(&script, Duration::from_secs(5))
                .await
                .unwrap(),
        );
        assert_descendant_stopped(dir.path()).await;
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn cancelled_startup_stops_server_descendants() {
        let (dir, script) = fake_server_with_descendant(false);
        let mut startup = Box::pin(OpenCodeServer::spawn(&script));
        tokio::select! {
            result = &mut startup => panic!("startup should still be pending: {result:?}"),
            started = tokio::time::timeout(Duration::from_secs(5), async {
                while !std::fs::read_to_string(dir.path().join("pid"))
                    .is_ok_and(|pid| !pid.trim().is_empty())
                {
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            }) => started.expect("fixture did not start"),
        }
        drop(startup);
        assert_descendant_stopped(dir.path()).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn shutdown_reaps_owned_child_before_return() {
        let child = Command::new("/bin/sleep")
            .arg("600")
            .kill_on_drop(true)
            .process_group(0)
            .spawn()
            .unwrap();
        let mut server = OpenCodeServer {
            child: ServerProcess::new(child),
            base_url: "http://127.0.0.1:1".into(),
            api: super::super::api::OpenCodeApi::Legacy,
            server_password: "synthetic".into(),
        };
        assert!(server.pid().is_some());
        server.shutdown().await.unwrap();
        assert_eq!(server.pid(), None, "Shutdown must wait for the owned child");
        server.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn spawn_against_missing_binary_returns_spawn_failed() {
        let result = OpenCodeServer::spawn_with_timeout(
            Path::new("/no/such/opencode"),
            Duration::from_millis(200),
        )
        .await;
        let err = result.expect_err("must fail");
        assert!(err.starts_with("spawn_failed:"), "unexpected error: {err}");
    }

    // Unix-only: relies on `/bin/true` which doesn't exist on Windows.
    // The spawn_failed path on Windows is already covered by
    // `spawn_against_missing_binary_returns_spawn_failed` above.
    #[cfg(unix)]
    #[tokio::test]
    async fn spawn_against_non_opencode_binary_times_out_or_reports_missing_banner() {
        // `/bin/true` exits 0 with no stdout — should surface as
        // either "ready_banner_missing" or "ready_timeout_…",
        // depending on the schedule of the readline-vs-exit race.
        // Both shapes are acceptable Stage 2 errors.
        let result =
            OpenCodeServer::spawn_with_timeout(Path::new("/bin/true"), Duration::from_millis(300))
                .await;
        let err = result.expect_err("must fail");
        assert!(
            err.starts_with("ready_banner_missing") || err.starts_with("ready_timeout_after_"),
            "unexpected error: {err}"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_chatty_server_is_never_blocked_on_its_own_output() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let done = dir.path().join("done");
        let script = dir.path().join("fake-opencode");
        // Far more than a pipe buffer on both streams, after the banner;
        // `set -e` makes a failed write (a closed pipe) skip the marker.
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\n\
                 set -e\n\
                 echo 'opencode server listening on http://127.0.0.1:1'\n\
                 head -c 1048576 /dev/zero >&2\n\
                 head -c 1048576 /dev/zero\n\
                 touch '{}'\n\
                 sleep 30\n",
                done.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

        let mut server = OpenCodeServer::spawn_with_timeout(&script, Duration::from_secs(5))
            .await
            .expect("the banner is enough to start");
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !done.exists() && std::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let finished = done.exists();
        server.shutdown().await.unwrap();
        assert!(finished, "the server blocked writing its own output");
    }

    // Linux only: `setsid` puts a descendant outside the server's process
    // group, where the group kill on shutdown does not reach it.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn the_stdout_drain_stops_with_the_server_even_if_a_descendant_keeps_the_pipe() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("stray.pid");
        let script = dir.path().join("fake-opencode");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\n\
                 echo 'opencode server listening on http://127.0.0.1:1'\n\
                 setsid sleep 60 &\n\
                 echo $! > '{}'\n\
                 exec sleep 60\n",
                pid_file.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

        let mut server = OpenCodeServer::spawn_with_timeout(&script, Duration::from_secs(5))
            .await
            .expect("the banner is enough to start");
        let drain = server.child.1.as_ref().expect("the drain is owned by the process").finished();
        // Shut down only once the descendant has left the server's session
        // (and so its process group) while holding the pipe.
        let escaped = |pid_file: &Path| -> bool {
            let Some(pid) = std::fs::read_to_string(pid_file).ok().and_then(|p| p.trim().parse::<u32>().ok())
            else {
                return false;
            };
            // Field 6 of /proc/<pid>/stat is the session id; the command
            // name before it is parenthesised and may contain spaces.
            std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|stat| {
                stat.rsplit_once(')')
                    .and_then(|(_, rest)| rest.split_whitespace().nth(3))
                    .is_some_and(|sid| sid == pid.to_string())
            })
        };
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !escaped(&pid_file) && std::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(escaped(&pid_file), "precondition: the descendant left the session");
        server.shutdown().await.unwrap();

        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while !drain() && std::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let stopped = drain();
        if let Ok(pid) = std::fs::read_to_string(&pid_file) {
            if let Ok(pid) = pid.trim().parse::<i32>() {
                // SAFETY: plain kill(2) on the stray child this test started.
                unsafe { libc::kill(pid, libc::SIGKILL) };
            }
        }
        assert!(stopped, "the drain outlived the server");
    }

    // Windows: tokio's own pipe reads cannot be cancelled, so the drain
    // has its own thread; stopping it must release a read that is pending
    // while the server still holds the pipe's write end.
    #[cfg(windows)]
    #[tokio::test]
    async fn stopping_the_drain_releases_a_pending_read_while_the_writer_lives() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("fake-opencode.cmd");
        std::fs::write(
            &script,
            "@echo off\r\necho opencode server listening on http://127.0.0.1:1\r\nping -n 60 127.0.0.1 >nul\r\n",
        )
        .unwrap();

        let mut server = OpenCodeServer::spawn_with_timeout(&script, Duration::from_secs(10))
            .await
            .expect("the banner is enough to start");
        let drain = server.child.1.as_ref().expect("the drain is owned by the process").finished();
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(!drain(), "precondition: the drain is waiting on the open pipe");

        // Stop only the drain: the server, and its write end, stay alive.
        server.child.stop_drain();
        let deadline = std::time::Instant::now() + Duration::from_secs(3);
        while !drain() && std::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let released = drain();
        server.shutdown().await.unwrap();
        assert!(released, "the pending read kept the drain thread alive");
    }

    // Windows: startup dropped while waiting for the banner, with another
    // process still holding the pipe's write end, must release the read.
    #[cfg(windows)]
    #[tokio::test]
    async fn dropping_startup_before_the_banner_releases_the_read() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("fake-opencode.cmd");
        // No banner; a `start /b` child inherits stdout and outlives the
        // script, so the pipe stays open after the server process is gone.
        std::fs::write(
            &script,
            "@echo off\r\nstart \"\" /b ping -n 20 127.0.0.1\r\nping -n 60 127.0.0.1 >nul\r\n",
        )
        .unwrap();

        let probe = std::sync::Arc::new(std::sync::Mutex::new(DrainProbe::default()));
        let startup = DRAIN_PROBE.scope(
            probe.clone(),
            OpenCodeServer::spawn_with_timeout(&script, Duration::from_secs(60)),
        );
        // Drop startup mid-wait.
        assert!(tokio::time::timeout(Duration::from_secs(2), startup).await.is_err());

        let done = probe.lock().unwrap().done.clone().expect("the reader started");
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !done.load(std::sync::atomic::Ordering::SeqCst) && std::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        assert!(done.load(std::sync::atomic::Ordering::SeqCst), "the read outlived the dropped startup");
    }

    // Windows: the reader passed its stop check and is about to read when
    // the drain is stopped; the read it then issues must still be released
    // while the writer stays open.
    #[cfg(windows)]
    #[tokio::test]
    async fn a_read_issued_after_the_stop_is_still_released() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("fake-opencode.cmd");
        std::fs::write(
            &script,
            "@echo off\r\necho opencode server listening on http://127.0.0.1:1\r\nping -n 60 127.0.0.1 >nul\r\n",
        )
        .unwrap();
        let arrived = std::sync::Arc::new(std::sync::Barrier::new(2));
        let release = std::sync::Arc::new(std::sync::Barrier::new(2));
        let probe = std::sync::Arc::new(std::sync::Mutex::new(DrainProbe {
            pause_before_drain_read: Some((arrived.clone(), release.clone())),
            ..DrainProbe::default()
        }));

        let mut server = DRAIN_PROBE
            .scope(probe.clone(), OpenCodeServer::spawn_with_timeout(&script, Duration::from_secs(10)))
            .await
            .expect("the banner is enough to start");
        let done = probe.lock().unwrap().done.clone().unwrap();

        // The reader is past its stop check, just before reading.
        tokio::task::spawn_blocking({
            let arrived = arrived.clone();
            move || arrived.wait()
        })
        .await
        .unwrap();
        server.child.stop_drain();
        // Let the cancel loop run with nothing pending, then let the reader
        // issue its read.
        tokio::time::sleep(Duration::from_millis(300)).await;
        tokio::task::spawn_blocking(move || release.wait()).await.unwrap();

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !done.load(std::sync::atomic::Ordering::SeqCst) && std::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let released = done.load(std::sync::atomic::Ordering::SeqCst);
        server.shutdown().await.unwrap();
        assert!(released, "a read issued after the stop kept the reader alive");
    }

    #[test]
    fn the_banner_is_found_after_long_and_multibyte_startup_lines() {
        let stop = std::sync::atomic::AtomicBool::new(false);
        // A line whose multibyte characters cross every 4 KiB boundary, one
        // far longer than any banner, invalid UTF-8, then the banner.
        let mut input = "é".repeat(3000).into_bytes();
        input.push(b'\n');
        input.extend("x".repeat(100_000).into_bytes());
        input.push(b'\n');
        input.extend(b"\xff\xfe not utf-8\r\n");
        input.extend(b"opencode server listening on http://127.0.0.1:4096\n");
        input.extend(b"after the banner\n");
        // A tiny buffer, so every line spans many reads.
        let mut reader = std::io::BufReader::with_capacity(7, std::io::Cursor::new(input));

        let (url, _api) = stdout_drain::scan_banner(&mut reader, &stop).expect("the banner");
        assert_eq!(url, "http://127.0.0.1:4096");
    }

    #[test]
    fn no_banner_reports_what_startup_printed_within_a_cap() {
        let stop = std::sync::atomic::AtomicBool::new(false);
        let input = "some log line\n".repeat(10_000);
        let mut reader = std::io::BufReader::new(std::io::Cursor::new(input.into_bytes()));
        let err = stdout_drain::scan_banner(&mut reader, &stop).unwrap_err();
        assert!(err.starts_with("ready_banner_missing"), "{err}");
        assert!(err.contains("some log line"));
        assert!(err.len() < 9_000, "the diagnostic is capped: {} bytes", err.len());
    }

    // Windows: if the reader thread cannot start after the cancel thread
    // has, starting fails and the cancel thread (with its pipe handle) has
    // exited — observed directly, not inferred from the error. Startup runs
    // on a worker thread, so a stuck join would trip the deadline instead of
    // hanging the test.
    #[cfg(windows)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_reader_that_cannot_start_leaves_no_cancel_thread_behind() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("fake-opencode.cmd");
        std::fs::write(
            &script,
            "@echo off\r\necho opencode server listening on http://127.0.0.1:1\r\nping -n 60 127.0.0.1 >nul\r\n",
        )
        .unwrap();
        let exited = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let probe = std::sync::Arc::new(std::sync::Mutex::new(DrainProbe {
            fail_reader_spawn: true,
            canceller_exited: Some(exited.clone()),
            ..DrainProbe::default()
        }));
        // The path moves into the task (`tokio::spawn` needs `'static`); the
        // temp dir outlives it, as the task is awaited below.
        let startup = tokio::spawn(DRAIN_PROBE.scope(probe, async move {
            OpenCodeServer::spawn_with_timeout(&script, Duration::from_secs(10)).await
        }));
        let started = tokio::time::timeout(Duration::from_secs(10), startup)
            .await
            .expect("a failed reader start must not hang on its cancel thread")
            .unwrap();
        let err = started.err().expect("startup fails when the reader cannot start");
        assert!(err.contains("read stdout"), "unexpected error: {err}");
        assert!(exited.load(std::sync::atomic::Ordering::SeqCst), "the cancel thread has exited");
    }

    /// Tests in this module that rely on the live `opencode` binary
    /// share the `opencode_path` serial-test label with the sibling
    /// modules so the discovery PATH-mutator can't race against
    /// them under parallel cargo runs.
    use serial_test::serial;

    #[tokio::test]
    #[serial(opencode_path)]
    async fn spawn_live_binary_when_present() {
        // Live smoke: when `opencode` is on PATH, exercise the full
        // spawn → ready → drop pipeline against the real binary.
        // Skipped on machines without it so CI / fresh worktrees
        // stay green.
        let Ok(binary_path) = which::which("opencode") else {
            eprintln!(
                "[opencode::server] skipping live spawn smoke — \
                 opencode not on PATH"
            );
            return;
        };

        let server = OpenCodeServer::spawn(&binary_path)
            .await
            .expect("live opencode must spawn");

        // URL should look like `http://127.0.0.1:<port>`.
        assert!(
            server.base_url().starts_with("http://127.0.0.1:"),
            "unexpected base_url: {}",
            server.base_url()
        );
        assert_eq!(server.server_password().len(), SERVER_PASSWORD_LEN);
        assert!(server.pid().is_some(), "child should still be live");

        // Ping it via reqwest using the published password — this
        // confirms the env var actually took effect (otherwise
        // OpenCode would 401 even with the right password, because
        // it would expect no auth at all).
        let client = reqwest::Client::new();
        let resp = client
            .get(format!("{}/", server.base_url()))
            .basic_auth("opencode", Some(server.server_password()))
            .send()
            .await
            .expect("ping must succeed");
        assert!(
            resp.status().is_success() || resp.status().as_u16() == 404,
            "unexpected ping status: {}",
            resp.status()
        );

        // Drop kills the child via kill_on_drop — verify by
        // grabbing the pid first then asserting the process is gone
        // shortly after the drop.
        let pid = server.pid().expect("pid present");
        drop(server);

        // Give kill_on_drop a beat to deliver SIGKILL.
        tokio::time::sleep(Duration::from_millis(200)).await;

        // /proc/<pid> existing and being one of ours is the
        // canonical Linux check. On macOS / Windows we accept
        // success without verification.
        #[cfg(target_os = "linux")]
        {
            let status_path = format!("/proc/{pid}/status");
            assert!(
                !std::path::Path::new(&status_path).exists(),
                "opencode server should be reaped after drop, but \
                 /proc/{pid} still exists"
            );
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = pid;
        }
    }
}
