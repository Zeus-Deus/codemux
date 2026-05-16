//! SSH-tunneled PtyDaemonClient.
//!
//! `spawn_ssh_tunnel` opens an `ssh -L <local>:<remote>` and starts
//! `codemux-remote pty-daemon` on the remote in the same SSH
//! invocation. The returned `TunnelHandle` exposes the local Unix
//! socket path the existing `PtyDaemonClient::connect(&path)` dials
//! exactly as it does for the in-app daemon. The client never has
//! to know it's actually talking over SSH.
//!
//! Lifecycle:
//!
//! - The SSH process is the source of truth. While it lives, the
//!   tunnel works; when it dies, the local socket file goes stale.
//! - `TunnelHandle::shutdown()` kills the SSH process and removes
//!   the local socket file. Dropping the handle without shutdown is
//!   a leak (intentional in some flows — e.g. detaching a tunnel
//!   you want to outlive this process — but the supervisor should
//!   prefer explicit shutdown).
//! - Reconnect on transient SSH failure is the caller's job for now.
//!   We don't auto-retry from inside the handle because the right
//!   policy depends on intent (a push-then-detach vs. an
//!   interactive session want different reconnect cadences).

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::process::{Child, Command};
use tokio::time::sleep;

/// Live tunnel — the SSH process plus the local socket path the
/// `PtyDaemonClient` should connect to.
pub struct TunnelHandle {
    ssh_process: Child,
    local_socket: PathBuf,
}

impl TunnelHandle {
    /// Local socket path the `PtyDaemonClient` should dial.
    pub fn local_socket(&self) -> &Path {
        &self.local_socket
    }

    /// PID of the underlying `ssh` process. Useful for telemetry +
    /// crash reports.
    pub fn ssh_pid(&self) -> Option<u32> {
        self.ssh_process.id()
    }

    /// Kill the SSH process and clean up the local socket. Idempotent.
    pub async fn shutdown(mut self) {
        let _ = self.ssh_process.kill().await;
        // SSH cleans up the remote-side socket on disconnect; we own
        // the local end.
        let _ = std::fs::remove_file(&self.local_socket);
    }
}

pub struct TunnelOptions<'a> {
    pub ssh_target: &'a str,
    /// Where the daemon should bind its socket on the remote side.
    /// Defaults to `/tmp/codemux-ptyd-<rand>.sock` per call.
    pub remote_socket: &'a str,
    /// Where the SSH tunnel should expose that socket locally.
    /// Defaults to a temp file per call.
    pub local_socket: &'a Path,
    /// Path to the `codemux-remote` binary on the remote. Defaults
    /// to whatever's first on `PATH`; the bootstrap step installs
    /// to `~/.local/bin/codemux-remote` which is on the default
    /// PATH for most shells.
    pub remote_binary: &'a str,
}

/// Build the ssh argv we use to spawn the tunneled daemon. Extracted
/// so tests can assert the exact flags without forking ssh.
pub fn build_tunnel_argv(opts: &TunnelOptions<'_>) -> Vec<String> {
    vec![
        "-o".into(),
        "BatchMode=yes".into(),
        "-o".into(),
        "ServerAliveInterval=30".into(),
        "-o".into(),
        "ServerAliveCountMax=3".into(),
        "-o".into(),
        "ExitOnForwardFailure=yes".into(),
        // Tear down both ends if the local socket file already
        // exists from a stale prior run. Without this, ssh will
        // refuse to bind and exit before the daemon ever starts.
        "-o".into(),
        "StreamLocalBindUnlink=yes".into(),
        // -L local:remote — forward the local Unix socket to the
        // remote Unix socket the daemon binds.
        "-L".into(),
        format!("{}:{}", opts.local_socket.display(), opts.remote_socket),
        opts.ssh_target.into(),
        // Remote command: ensure the socket dir exists, then run
        // the daemon. The daemon binds and serves until ssh dies.
        format!(
            "rm -f {remote_socket} ; mkdir -p \"$(dirname {remote_socket})\" ; \
             exec {binary} pty-daemon --socket {remote_socket}",
            remote_socket = opts.remote_socket,
            binary = opts.remote_binary,
        ),
    ]
}

/// Open the tunnel. Returns once the SSH process is alive AND the
/// local socket exists (or the timeout fires). Failure modes:
///
/// - SSH process exits immediately (bad target, auth failure,
///   ExitOnForwardFailure tripped) → we observe via `try_wait`.
/// - SSH process is alive but the socket never appears (binary
///   missing, daemon crash on startup) → timeout-driven failure.
///
/// On either failure the SSH process is killed before we return so
/// we don't leak a zombie process.
pub async fn spawn_ssh_tunnel(
    opts: TunnelOptions<'_>,
    spawn_timeout: Duration,
) -> Result<TunnelHandle, String> {
    let argv = build_tunnel_argv(&opts);
    let mut cmd = Command::new("ssh");
    for arg in &argv {
        cmd.arg(arg);
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("failed to spawn ssh: {e}"))?;

    let deadline = std::time::Instant::now() + spawn_timeout;
    loop {
        // If SSH has already exited, the tunnel can't possibly come
        // up. Capture stderr for the error message.
        if let Ok(Some(status)) = child.try_wait() {
            let mut stderr = String::new();
            if let Some(mut err_stream) = child.stderr.take() {
                use tokio::io::AsyncReadExt;
                let _ = err_stream.read_to_string(&mut stderr).await;
            }
            return Err(format!(
                "ssh exited before tunnel came up (status={status}): {}",
                stderr.trim()
            ));
        }
        if opts.local_socket.exists() {
            // Small grace beat so the daemon's listener is fully
            // up before we hand the path to a client.
            sleep(Duration::from_millis(50)).await;
            return Ok(TunnelHandle {
                ssh_process: child,
                local_socket: opts.local_socket.to_path_buf(),
            });
        }
        if std::time::Instant::now() >= deadline {
            let _ = child.kill().await;
            return Err(format!(
                "tunnel did not come up within {:?} (local socket {:?} never appeared)",
                spawn_timeout, opts.local_socket
            ));
        }
        sleep(Duration::from_millis(100)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn options() -> (PathBuf, TunnelOptions<'static>) {
        let path = PathBuf::from("/tmp/codemux-test-local.sock");
        // Leak the PathBuf into a static-lifetime reference via Box::leak
        // so the test closure can borrow it. Acceptable because each
        // test runs once and the leak is bounded.
        let leaked: &'static Path = Box::leak(path.clone().into_boxed_path());
        let opts = TunnelOptions {
            ssh_target: "user@host",
            remote_socket: "/tmp/codemux-ptyd-abc.sock",
            local_socket: leaked,
            remote_binary: "codemux-remote",
        };
        (path, opts)
    }

    #[test]
    fn build_tunnel_argv_locks_in_required_ssh_flags() {
        let (_path, opts) = options();
        let argv = build_tunnel_argv(&opts);
        // Critical flags. Losing any of these silently regresses the
        // tunnel's reliability:
        // - BatchMode prevents hangs on a password prompt
        // - ExitOnForwardFailure makes tunnel-binding failures hard
        //   errors instead of "ssh is alive but useless"
        // - StreamLocalBindUnlink unblocks a re-bind after a stale
        //   local socket from a previous run
        // - ServerAlive keeps the tunnel from going stale under NAT
        for must_have in [
            "BatchMode=yes",
            "ExitOnForwardFailure=yes",
            "StreamLocalBindUnlink=yes",
            "ServerAliveInterval=30",
        ] {
            assert!(
                argv.iter().any(|a| a == must_have),
                "missing required flag: {must_have} (argv={argv:?})"
            );
        }
    }

    #[test]
    #[allow(non_snake_case)]
    fn build_tunnel_argv_uses_dash_L_for_forwarding() {
        let (_path, opts) = options();
        let argv = build_tunnel_argv(&opts);
        // Find the `-L` arg + the spec right after it.
        let l_idx = argv.iter().position(|a| a == "-L").expect("has -L");
        let spec = &argv[l_idx + 1];
        assert!(
            spec.contains(":"),
            "spec must be local:remote, got {spec}"
        );
        assert!(spec.contains("codemux-test-local.sock"));
        assert!(spec.contains("codemux-ptyd-abc.sock"));
    }

    #[test]
    fn build_tunnel_argv_puts_remote_command_last() {
        let (_path, opts) = options();
        let argv = build_tunnel_argv(&opts);
        let last = argv.last().unwrap();
        // The remote command must include the binary + pty-daemon
        // subcommand + matching socket path. A drift here is
        // exactly the kind of bug a quick visual diff would miss.
        assert!(last.contains("codemux-remote"));
        assert!(last.contains("pty-daemon"));
        assert!(last.contains("/tmp/codemux-ptyd-abc.sock"));
        assert!(last.contains("exec "));
    }

    #[test]
    fn build_tunnel_argv_places_target_before_command() {
        let (_path, opts) = options();
        let argv = build_tunnel_argv(&opts);
        let target_idx = argv.iter().position(|a| a == "user@host").unwrap();
        let last_idx = argv.len() - 1;
        assert!(
            target_idx < last_idx,
            "target must come before the remote command"
        );
    }
}
