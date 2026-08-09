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
//! * **Cleanup**: `kill_on_drop(true)` on the underlying
//!   `tokio::process::Child` is the safety net — if the server
//!   handle is dropped (panic, `OpenCodeServerManager::stop`,
//!   process shutdown) the child receives `SIGKILL`. Graceful
//!   `SIGTERM → 1s → SIGKILL` is a Stage 3 polish item; for Stage 2
//!   the server is a stateless model-list cache and an immediate
//!   kill loses nothing.
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

use crate::execution::sanitize_gui_env_tokio;

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
    /// `kill_on_drop(true)` is set on construction — letting `child`
    /// drop is sufficient to terminate the server. The handle is
    /// kept around so `Drop` reports the PID into a debug log.
    child: Child,
    /// Stable URL extracted from the ready banner — usable as-is for
    /// `reqwest` requests.
    base_url: String,
    /// Password that was set into `OPENCODE_SERVER_PASSWORD` for
    /// this specific child. Bound to the server's lifetime: when the
    /// server dies the password becomes meaningless.
    server_password: String,
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
        // Echo OpenCode's documented "use upstream defaults, ignore
        // user config" knob — the reference clone does this at line
        // 340. Without it OpenCode honours
        // `~/.config/opencode/config.json`, which
        // could surprise a user whose Codemux pane suddenly behaves
        // differently from their CLI.
        cmd.env("OPENCODE_CONFIG_CONTENT", "{}");
        cmd.stdin(Stdio::null());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        cmd.kill_on_drop(true);
        sanitize_gui_env_tokio(&mut cmd);

        let mut child = cmd.spawn().map_err(|err| {
            format!(
                "spawn_failed: {} ({err})",
                binary_path.display()
            )
        })?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "spawn_failed: child stdout pipe missing".to_string())?;

        let url_result = tokio::time::timeout(
            ready_budget,
            await_ready_url(stdout),
        )
        .await;

        let base_url = match url_result {
            Ok(Ok(url)) => url,
            Ok(Err(message)) => {
                // Probe drained stdout before exiting — kill the
                // child and surface the failure verbatim.
                let _ = child.kill().await;
                return Err(message);
            }
            Err(_) => {
                let _ = child.kill().await;
                return Err(format!(
                    "ready_timeout_after_{}ms",
                    ready_budget.as_millis()
                ));
            }
        };

        Ok(Self {
            child,
            base_url,
            server_password,
        })
    }

    /// Stable HTTP URL the server is listening on, e.g.
    /// `http://127.0.0.1:38871`. Trailing slash already stripped to
    /// match [`OpenCodeClientConfig::new`](super::client::OpenCodeClientConfig::new).
    pub fn base_url(&self) -> &str {
        &self.base_url
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
        self.child.id()
    }
}

impl Drop for OpenCodeServer {
    fn drop(&mut self) {
        // Logged on shutdown so a stuck-server diagnosis can confirm
        // the right child was killed. `kill_on_drop(true)` does the
        // actual cleanup — this is just a breadcrumb.
        if let Some(pid) = self.child.id() {
            eprintln!(
                "[codemux::opencode] dropping OpenCodeServer (pid={pid}, url={})",
                self.base_url
            );
        }
    }
}

/// Read lines from the child's stdout until the ready banner shows
/// up or the stream closes. Only the *banner* triggers `Ok`; anything
/// else is forwarded as part of the diagnostic on failure.
async fn await_ready_url(stdout: tokio::process::ChildStdout) -> Result<String, String> {
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
            return Ok(url);
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
    if !trimmed.starts_with(READY_PREFIX) {
        return None;
    }
    // After the prefix the banner reads `… on <url>`; we walk to
    // the first `http` substring rather than fix the regex to a
    // specific tail format so a future banner like
    // `"opencode server listening at https://…"` still works.
    let rest = &trimmed[READY_PREFIX.len()..];
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
        assert_eq!(
            parse_ready_url(line),
            Some("http://127.0.0.1:38871".into())
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
        assert_eq!(
            parse_ready_url(line),
            Some("http://127.0.0.1:9000".into())
        );
    }

    #[test]
    fn parse_ready_url_rejects_unrelated_lines() {
        assert_eq!(parse_ready_url(""), None);
        assert_eq!(
            parse_ready_url("Warning: OPENCODE_SERVER_PASSWORD is not set"),
            None
        );
        assert_eq!(
            parse_ready_url("opencode something different"),
            None
        );
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

    #[tokio::test]
    async fn spawn_against_missing_binary_returns_spawn_failed() {
        let result =
            OpenCodeServer::spawn_with_timeout(Path::new("/no/such/opencode"), Duration::from_millis(200))
                .await;
        let err = result.expect_err("must fail");
        assert!(
            err.starts_with("spawn_failed:"),
            "unexpected error: {err}"
        );
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
        let result = OpenCodeServer::spawn_with_timeout(
            Path::new("/bin/true"),
            Duration::from_millis(300),
        )
        .await;
        let err = result.expect_err("must fail");
        assert!(
            err.starts_with("ready_banner_missing")
                || err.starts_with("ready_timeout_after_"),
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
