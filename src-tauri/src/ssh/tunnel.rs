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

/// The remote shell command the tunnel runs over SSH.
///
/// This is the heart of "close the app, the host keeps working, reopen
/// reattaches." Instead of `exec`-ing the daemon in the foreground (which
/// tied the daemon's life to the SSH connection — closing the app or a WiFi
/// flap killed every host-side agent), it:
///
/// 1. **Reuses** a still-running daemon. If `<socket>.pid` names a live
///    process and the socket exists, a detached daemon from a previous
///    session is already serving — we just hold the `-L` forward open
///    (`exec sleep`) so the client can reconnect to it. `client.list()` on
///    the desktop then finds the surviving sessions and reattaches.
/// 2. Otherwise **spawns the daemon detached** — `setsid` (or `nohup` on
///    hosts without it, e.g. macOS) with stdio redirected — so it survives
///    SSH channel close (no SIGHUP, own session), then holds the forward.
///
/// The daemon's own idle-reaper (1h with zero sessions) handles cleanup of
/// an abandoned detached daemon, so this never leaks indefinitely.
pub fn build_remote_command(remote_socket: &str, binary: &str) -> String {
    // `SOCK` is a Codemux-generated `/tmp/codemux-ptyd-<hex>.sock` path
    // (no shell metacharacters), so single-quoting is safe. `BIN` may be
    // `$HOME/.local/bin/codemux-remote` and must stay expandable, so it's
    // double-quoted.
    format!(
        "SOCK='{remote_socket}' ; BIN=\"{binary}\" ; PIDF=\"$SOCK.pid\" ; LOG=\"$SOCK.log\" ; \
         alive=0 ; \
         if [ -S \"$SOCK\" ] && [ -f \"$PIDF\" ] ; then \
           pid=$(cat \"$PIDF\" 2>/dev/null) ; \
           if [ -n \"$pid\" ] && kill -0 \"$pid\" 2>/dev/null ; then alive=1 ; fi ; \
         fi ; \
         if [ \"$alive\" -eq 0 ] ; then \
           mkdir -p \"$(dirname \"$SOCK\")\" ; rm -f \"$SOCK\" ; \
           if command -v setsid >/dev/null 2>&1 ; then \
             setsid \"$BIN\" pty-daemon --socket \"$SOCK\" </dev/null >>\"$LOG\" 2>&1 & \
           else \
             nohup \"$BIN\" pty-daemon --socket \"$SOCK\" </dev/null >>\"$LOG\" 2>&1 & \
           fi ; \
           i=0 ; while [ ! -S \"$SOCK\" ] && [ \"$i\" -lt 100 ] ; do i=$((i+1)) ; sleep 0.1 ; done ; \
         fi ; \
         exec sleep 2147483647",
        remote_socket = remote_socket,
        binary = binary,
    )
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
        // Remote command: reuse a live detached daemon or spawn one
        // detached, then hold the forward open. See `build_remote_command`.
        build_remote_command(opts.remote_socket, opts.remote_binary),
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

    // ── Persistence-aware remote command (the "survive app close + reattach"
    //    mechanism). These lock in the contract that lets a host agent
    //    outlive the SSH connection and be reused on reconnect. ──

    #[test]
    fn remote_command_reuses_a_live_daemon_via_pidfile_probe() {
        let cmd = build_remote_command("/tmp/codemux-ptyd-abc.sock", "codemux-remote");
        // Liveness probe: the pidfile + kill -0 check is what tells a
        // reconnect "a detached daemon is already serving — don't respawn,
        // just hold the forward." Losing this regresses reattach into
        // "fresh daemon, sessions lost."
        assert!(cmd.contains("$SOCK.pid"), "must probe the per-socket pidfile");
        assert!(cmd.contains("kill -0"), "must verify the pid is alive");
        // The socket path + binary still appear (same as the old command);
        // the daemon is invoked via the `$BIN` var so the literal is
        // `"$BIN" pty-daemon --socket "$SOCK"`.
        assert!(cmd.contains("/tmp/codemux-ptyd-abc.sock"));
        assert!(cmd.contains("BIN=\"codemux-remote\""));
        assert!(cmd.contains("pty-daemon --socket \"$SOCK\""));
    }

    #[test]
    fn remote_command_spawns_detached_so_it_survives_ssh_close() {
        let cmd = build_remote_command("/tmp/codemux-ptyd-abc.sock", "codemux-remote");
        // The daemon must detach from the SSH session — otherwise the
        // channel-close SIGHUP kills every host agent when the app quits.
        // setsid (Linux) with a nohup fallback (macOS) is the contract.
        assert!(cmd.contains("setsid"), "prefer setsid to detach the daemon");
        assert!(
            cmd.contains("nohup"),
            "fall back to nohup on hosts without setsid (e.g. macOS)"
        );
        // stdin detached so the daemon can't be wedged on a closed channel.
        assert!(cmd.contains("</dev/null"));
    }

    #[test]
    fn remote_command_holds_the_forward_open() {
        let cmd = build_remote_command("/tmp/codemux-ptyd-abc.sock", "codemux-remote");
        // The SSH `-L` forward only lives as long as the remote command
        // does. Since the daemon is now detached (not the foreground exec),
        // we hold the forward with a long-lived `exec sleep`.
        assert!(cmd.contains("exec sleep"), "must hold the -L forward open");
    }

    #[test]
    fn remote_command_keeps_binary_expandable() {
        // The bootstrap installs to ~/.local/bin, so the binary path is
        // often `$HOME/.local/bin/codemux-remote` and MUST stay shell-
        // expandable (double-quoted, not single-quoted) or the daemon never
        // starts. The socket is a fixed /tmp path, safe to single-quote.
        let cmd =
            build_remote_command("/tmp/codemux-ptyd-abc.sock", "$HOME/.local/bin/codemux-remote");
        assert!(
            cmd.contains("BIN=\"$HOME/.local/bin/codemux-remote\""),
            "binary must be double-quoted so $HOME expands: {cmd}"
        );
        assert!(
            cmd.contains("SOCK='/tmp/codemux-ptyd-abc.sock'"),
            "socket path is single-quoted (no expansion needed)"
        );
    }
}
