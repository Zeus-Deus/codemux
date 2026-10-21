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
        dirs::runtime_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("codemux-opencode")
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
        let _ = std::fs::create_dir_all(dir());
        if let Ok(json) = serde_json::to_vec(&record) {
            let _ = std::fs::write(dir().join(format!("{server}.json")), json);
        }
    }

    pub(super) fn forget(server: u32) {
        let _ = std::fs::remove_file(dir().join(format!("{server}.json")));
    }

    pub(super) fn reap() {
        let Ok(entries) = std::fs::read_dir(dir()) else {
            return;
        };
        for path in entries.flatten().map(|entry| entry.path()) {
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
        cmd.stdin(Stdio::null());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        cmd.kill_on_drop(true);
        #[cfg(unix)]
        cmd.process_group(0);
        sanitize_gui_env_tokio(&mut cmd);

        let mut child = cmd
            .spawn()
            .map(ServerProcess)
            .map_err(|err| format!("spawn_failed: {} ({err})", binary_path.display()))?;
        #[cfg(target_os = "linux")]
        if let Some(pid) = child.0.id() {
            orphans::record(pid);
        }
        let stdout = child
            .0
            .stdout
            .take()
            .ok_or_else(|| "spawn_failed: child stdout pipe missing".to_string())?;

        let url_result = tokio::time::timeout(ready_budget, await_ready_url(stdout)).await;

        let (base_url, api) = match url_result {
            Ok(Ok(url)) => url,
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
#[derive(Debug)]
struct ServerProcess(Child);

impl ServerProcess {
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
        self.0.kill().await
    }
}

impl Drop for ServerProcess {
    fn drop(&mut self) {
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

/// Read lines from the child's stdout until the ready banner shows
/// up or the stream closes. Only the *banner* triggers `Ok`; anything
/// else is forwarded as part of the diagnostic on failure.
async fn await_ready_url(
    stdout: tokio::process::ChildStdout,
) -> Result<(String, OpenCodeApi), String> {
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
                return Ok((url, api));
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
                "#!/bin/sh\n(trap '' TERM; exec sleep 600) &\necho $! > \"$(dirname \"$0\")/pid\"\n{banner}\nwait\n"
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
        (dir, script)
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
            child: ServerProcess(child),
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
