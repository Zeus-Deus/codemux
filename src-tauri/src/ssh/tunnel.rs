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

use std::fs;
use std::os::unix::fs::{DirBuilderExt, FileTypeExt, MetadataExt};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::time::{sleep, timeout};

use super::exec::{sh_quote, validate_ssh_target};
use super::tunnel_paths::{
    forward_socket_path, is_private_directory, probe_remote_socket, verified_ssh_args, LocalSocket,
    DIRECTORY_CHECKS,
};
use crate::pty_daemon::{PtyDaemonClient, PROTOCOL_VERSION};

const TUNNEL_READY: &str = "codemux:pty-tunnel-ready:v1";
const PROBE_TIMEOUT: Duration = Duration::from_secs(15);
const PROBE_CLEANUP_TIMEOUT: Duration = Duration::from_secs(3);
const INSTALLED_PROBE_POLL_LIMIT: u32 = 200;

pub(super) fn has_private_socket_parent(path: &Path) -> bool {
    path.parent().is_some_and(is_private_directory)
}

pub(super) fn prepare_local_socket(path: &Path) -> Result<(), String> {
    let parent = path.parent().ok_or("tunnel socket needs a private directory")?;
    match fs::DirBuilder::new().mode(0o700).create(parent) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(format!("create tunnel directory {parent:?}: {error}")),
    }
    if !has_private_socket_parent(path) {
        return Err(format!("tunnel directory {parent:?} must be owned by this user and mode 0700"));
    }
    Ok(())
}

fn local_socket_ready(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok_and(|metadata| {
        metadata.file_type().is_socket()
            && metadata.uid() == unsafe { libc::geteuid() }
            && metadata.mode() & 0o777 == 0o600
    })
}

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
        if has_private_socket_parent(&self.local_socket) {
            let _ = fs::remove_file(&self.local_socket);
        }
    }
}

pub struct TunnelOptions<'a> {
    pub ssh_target: &'a str,
    /// Where the daemon should bind its socket on the remote side.
    /// The production path is inside a private per-workspace directory.
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
    let binary = match binary.strip_prefix("$HOME/") {
        Some(relative) => format!("\"$HOME\"/{}", sh_quote(relative)),
        None => sh_quote(binary),
    };
    let script = format!(
        "umask 077\nUID_NUMBER=$(id -u) || exit 1\nSOCK={}\nBIN={}\n{}{}",
        sh_quote(remote_socket),
        binary,
        DIRECTORY_CHECKS,
        r#"
DIR=$(dirname "$SOCK") || exit 1
PIDF="$SOCK.pid"
LOG="$SOCK.log"
if ! private_directory "$DIR"; then
    printf '%s\n' 'Codemux tunnel requires an owner-only socket directory' >&2
    exit 1
fi
lock_socket_path "$SOCK.start.fd" || exit 1
if ! daemon_alive "$SOCK"; then
    rm -f "$SOCK" "$PIDF" || exit 1
    if command -v setsid >/dev/null 2>&1; then
        setsid "$BIN" pty-daemon --socket "$SOCK" </dev/null >>"$LOG" 2>&1 9>&- &
    else
        nohup "$BIN" pty-daemon --socket "$SOCK" </dev/null >>"$LOG" 2>&1 9>&- &
    fi
fi
i=0
while ! daemon_alive "$SOCK"; do
    [ "$i" -lt 100 ] || exit 1
    i=$((i+1))
    sleep 0.1
done
unlock_socket_path || exit 1
printf '%s\n' 'codemux:pty-tunnel-ready:v1'
exec sleep 2147483647"#,
    );
    format!("exec sh -c {}", sh_quote(&script))
}

/// Build the ssh argv we use to spawn the tunneled daemon. Extracted
/// so tests can assert the exact flags without forking ssh.
pub fn build_tunnel_argv(opts: &TunnelOptions<'_>) -> Vec<String> {
    build_forward_argv(
        opts.ssh_target,
        opts.remote_socket,
        opts.local_socket,
        build_remote_command(opts.remote_socket, opts.remote_binary),
    )
}

fn build_forward_argv(
    ssh_target: &str,
    remote_socket: &str,
    local_socket: &Path,
    remote_command: String,
) -> Vec<String> {
    let mut args: Vec<_> = verified_ssh_args().into_iter().map(String::from).collect();
    args.extend([
        "-o".into(),
        "ServerAliveInterval=30".into(),
        "-o".into(),
        "ServerAliveCountMax=3".into(),
        "-o".into(),
        "ExitOnForwardFailure=yes".into(),
        // Rebind a stale socket only inside our private local directory.
        "-o".into(),
        "StreamLocalBindUnlink=yes".into(),
        "-o".into(),
        "StreamLocalBindMask=0177".into(),
        // -L local:remote — forward the local Unix socket to the
        // remote Unix socket the daemon binds.
        "-L".into(),
        format!("{}:{remote_socket}", local_socket.display()),
        "--".into(),
        ssh_target.into(),
        remote_command,
    ]);
    args
}

fn build_existing_daemon_command(remote_socket: &str) -> String {
    let script = format!(
        "UID_NUMBER=$(id -u) || exit 1\nSOCK={}\n{}{}",
        sh_quote(remote_socket),
        DIRECTORY_CHECKS,
        r#"
DIR=$(dirname "$SOCK") || exit 1
if ! private_directory "$DIR"; then
    # The old flat socket can be probed in the trusted sticky temp root.
    # Recheck its ownership and mode in daemon_alive before using it.
    [ "$DIR" = "$(absolute_dir /tmp)" ] && trusted_directory "$DIR" || exit 1
fi
daemon_alive "$SOCK" || exit 1
printf '%s\n' 'codemux:pty-tunnel-ready:v1'
# This shell exits when the probe's SSH stdin closes.
IFS= read -r ignored"#,
    );
    format!("exec sh -c {}", sh_quote(&script))
}

fn build_installed_daemon_probe_command(directory_name: &str, poll_limit: u32) -> String {
    let script = format!(
        "umask 077\nUID_NUMBER=$(id -u) || exit 1\nNAME={}\nPOLL_LIMIT={}\n{}{}",
        sh_quote(directory_name),
        poll_limit,
        DIRECTORY_CHECKS,
        r#"
ROOT=$(absolute_dir /tmp) || exit 1
trusted_directory "$ROOT" || exit 1
# Verify the process ownership check before starting a child.
ps -o ppid= -p "$$" >/dev/null 2>&1 || exit 1
DIR="$ROOT/$NAME"
SOCK="$DIR/pty.sock"
BIN="$HOME/.local/bin/codemux-remote"
DAEMON=
READER=

stop_child() {
    [ -n "$1" ] || return 0
    parent=$(ps -o ppid= -p "$1" 2>/dev/null) || return 0
    # Only signal processes still reported as children of this supervisor.
    if [ "$parent" -eq "$$" ] 2>/dev/null; then
        kill -KILL "$1" 2>/dev/null || :
        wait "$1" 2>/dev/null || :
    fi
}
cleanup() {
    trap '' HUP INT TERM
    stop_child "$READER"
    stop_child "$DAEMON"
    rm -rf "$DIR"
}

# Do not accept an existing path. Defer soft signals while recording
# ownership of the directory and child processes.
trap '' HUP INT TERM
mkdir -m 700 "$DIR" || exit 1
trap 'status=$?; trap - 0; cleanup || status=1; exit "$status"' 0
private_directory "$DIR" || exit 1
mkdir "$DIR/home" "$DIR/tmp" "$DIR/run" || exit 1
(
    cd "$DIR" || exit 1
    exec env -i PATH="$PATH" HOME="$DIR/home" TMPDIR="$DIR/tmp" \
        XDG_DATA_HOME="$DIR/data" XDG_CONFIG_HOME="$DIR/config" \
        XDG_CACHE_HOME="$DIR/cache" XDG_STATE_HOME="$DIR/state" \
        XDG_RUNTIME_DIR="$DIR/run" CODEMUX_PTY_DAEMON_DIR="$DIR/daemon" \
        "$BIN" pty-daemon --socket "$SOCK"
) </dev/null >"$DIR/daemon.log" 2>&1 &
DAEMON=$!

# A separate reader lets the supervisor enforce a deadline even if the SSH
# channel never delivers EOF. Its stdin is deliberately inherited, not the
# implicit /dev/null of a non-interactive background command.
exec 3<&0
(
    IFS= read -r ignored || :
    : > "$DIR/client-closed"
) <&3 &
READER=$!
exec 3<&-
trap 'exit 1' HUP INT TERM

i=0
READY=0
while [ "$i" -lt "$POLL_LIMIT" ]; do
    [ ! -e "$DIR/client-closed" ] || exit 0
    kill -0 "$DAEMON" 2>/dev/null || exit 1
    if [ "$READY" = 0 ] && daemon_alive "$SOCK" && [ "$(cat "$SOCK.pid")" = "$DAEMON" ]; then
        printf '%s\n' 'codemux:pty-tunnel-ready:v1'
        READY=1
    fi
    i=$((i+1))
    sleep 0.1
done
exit 1"#,
    );
    format!("exec sh -c {}", sh_quote(&script))
}

pub(super) fn verify_daemon_protocol(version: &str, protocol: u32) -> Result<(), String> {
    if protocol == PROTOCOL_VERSION {
        return Ok(());
    }
    Err(format!(
        "The device's running terminal daemon ({version}) uses PTY protocol {protocol}, but this desktop requires {PROTOCOL_VERSION}. Close its terminals and set up the device again before retrying."
    ))
}

/// Check the live daemon that normal discovery would reuse. This probe
/// sends only Hello and neither starts daemons nor changes remote paths.
/// Returns false if no live daemon is present.
pub(crate) async fn verify_workspace_daemon(
    ssh_target: &str,
    workspace_id: &str,
) -> Result<bool, String> {
    let Some(remote_socket) = probe_remote_socket(ssh_target, workspace_id).await? else {
        return Ok(false);
    };
    let (local_socket, mut child) = spawn_daemon_probe(
        ssh_target,
        &remote_socket,
        build_existing_daemon_command(&remote_socket),
    )?;
    let result = verify_daemon_forward(&local_socket.path, &mut child).await;
    let _ = child.kill().await;
    result.map(|()| true)
}

/// Older helpers may lack capability fields in their version output even
/// when their daemon supports them. Verify a scratch daemon without using
/// the device's workspace paths, shared manifest, or agent plugin state.
pub(crate) async fn verify_installed_workspace_daemon(ssh_target: &str) -> Result<(), String> {
    validate_ssh_target(ssh_target)?;
    let directory_name = format!("cmx-hello-{}", uuid::Uuid::new_v4().simple());
    let remote_socket = format!("/tmp/{directory_name}/pty.sock");
    let (local_socket, mut child) = spawn_daemon_probe(
        ssh_target,
        &remote_socket,
        // The remote process ends even if the network never delivers EOF.
        build_installed_daemon_probe_command(&directory_name, INSTALLED_PROBE_POLL_LIMIT),
    )?;
    let result = verify_daemon_forward(&local_socket.path, &mut child).await;
    match timeout(PROBE_CLEANUP_TIMEOUT, child.wait()).await {
        Ok(Ok(status)) if status.success() => result,
        _ => {
            let _ = child.kill().await;
            result.and(Err("The installed device daemon probe did not finish cleanup".into()))
        }
    }
}

fn spawn_daemon_probe(
    ssh_target: &str,
    remote_socket: &str,
    remote_command: String,
) -> Result<(LocalSocket, Child), String> {
    let local_socket = LocalSocket::allocate()?;
    let argv = build_forward_argv(ssh_target, remote_socket, &local_socket.path, remote_command);
    let mut command = Command::new("ssh");
    command
        .args(argv)
        .kill_on_drop(true)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    crate::execution::sanitize_appimage_env_tokio(&mut command);
    let child = command
        .spawn()
        .map_err(|error| format!("Could not open the device daemon probe: {error}"))?;
    Ok((local_socket, child))
}

async fn verify_daemon_forward(path: &Path, child: &mut Child) -> Result<(), String> {
    // Child::wait closes its stdin immediately. Keep the probe command's
    // lifetime bound to this handle until we finish checking the listener.
    let _stdin = child.stdin.take();
    timeout(PROBE_TIMEOUT, async {
        wait_for_socket(path, child, PROBE_TIMEOUT).await?;
        let client = PtyDaemonClient::connect(path).await.map_err(|e| e.to_string())?;
        let (_, version, protocol) = client.hello().await.map_err(|e| e.to_string())?;
        verify_daemon_protocol(&version, protocol)?;
        if !client.supports_prepared_spawn() {
            return Err(format!(
                "The device's running terminal daemon ({version}) does not support prepared terminal spawns. Close its terminals and set up the device again before retrying."
            ));
        }
        Ok(())
    })
    .await
    .map_err(|_| "The device's running terminal daemon did not answer the probe in time")?
}

/// Wait for both the SSH forward and the remote daemon's authenticated
/// readiness message. The local forward can appear before the remote shell
/// has checked the directory and daemon.
pub(super) async fn wait_for_socket(
    path: &Path,
    child: &mut Child,
    deadline: Duration,
) -> Result<(), String> {
    let stdout = child.stdout.take();
    let ready = timeout(deadline, async {
        let Some(stdout) = stdout else {
            return std::future::pending().await;
        };
        let mut reader = BufReader::new(stdout.take(16 * 1024));
        let mut line = Vec::new();
        loop {
            line.clear();
            if reader.read_until(b'\n', &mut line).await.map_err(|e| e.to_string())? == 0 {
                // Wait for the process exit so SSH failures include its status.
                return std::future::pending().await;
            }
            if line.strip_suffix(b"\n") == Some(TUNNEL_READY.as_bytes()) {
                break;
            }
        }
        while !local_socket_ready(path) {
            sleep(Duration::from_millis(20)).await;
        }
        Ok(())
    });
    tokio::select! {
        biased;
        status = child.wait() => {
            let status = status.map_err(|error| format!("wait for ssh: {error}"))?;
            Err(format!("ssh exited before tunnel came up (status={status})"))
        }
        result = ready => result.map_err(|_| format!("tunnel was not ready within {deadline:?}"))?,
    }
}

/// Open the tunnel and wait for the SSH process, local forward and daemon to
/// be ready. On failure the child is stopped before returning.
pub async fn spawn_ssh_tunnel(
    opts: TunnelOptions<'_>,
    spawn_timeout: Duration,
) -> Result<TunnelHandle, String> {
    validate_ssh_target(opts.ssh_target)?;
    if !forward_socket_path(opts.local_socket) || !forward_socket_path(Path::new(opts.remote_socket)) {
        return Err("SSH tunnel socket paths must be short absolute paths".into());
    }
    prepare_local_socket(opts.local_socket)?;
    let argv = build_tunnel_argv(&opts);
    let mut cmd = Command::new("ssh");
    cmd.args(&argv)
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("failed to spawn ssh: {e}"))?;

    if let Err(reason) = wait_for_socket(opts.local_socket, &mut child, spawn_timeout).await {
        let _ = child.kill().await;
        let mut diagnostic = Vec::new();
        if let Some(stderr) = child.stderr.take() {
            let _ = timeout(Duration::from_millis(250), stderr.take(16 * 1024).read_to_end(&mut diagnostic))
                .await;
        }
        let stderr = String::from_utf8_lossy(&diagnostic);
        return if stderr.trim().is_empty() {
            Err(reason)
        } else {
            Err(format!("{reason}: {}", stderr.trim()))
        };
    }
    Ok(TunnelHandle {
        ssh_process: child,
        local_socket: opts.local_socket.to_path_buf(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{symlink, PermissionsExt};
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
            "-T",
            "BatchMode=yes",
            "StrictHostKeyChecking=yes",
            "ForwardAgent=no",
            "ControlPath=none",
            "ExitOnForwardFailure=yes",
            "StreamLocalBindUnlink=yes",
            "StreamLocalBindMask=0177",
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
        assert_eq!(argv[target_idx - 1], "--");
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
        assert!(cmd.contains("$1.pid"), "must probe the per-socket pidfile");
        assert!(cmd.contains("kill -0"), "must verify the pid is alive");
        // The socket path + binary still appear (same as the old command);
        // the daemon is invoked via the `$BIN` var so the literal is
        // `"$BIN" pty-daemon --socket "$SOCK"`.
        assert!(cmd.contains("/tmp/codemux-ptyd-abc.sock"));
        assert!(cmd.contains("codemux-remote"));
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
            cmd.contains("BIN=\"$HOME\"/"),
            "only the home directory prefix expands: {cmd}"
        );
        assert!(
            cmd.contains("/tmp/codemux-ptyd-abc.sock"),
            "socket path is preserved"
        );
    }

    #[test]
    fn local_tunnel_directory_is_private_and_rejects_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let parent = dir.path().join("private");
        let socket = parent.join("ssh.sock");
        prepare_local_socket(&socket).unwrap();
        assert!(has_private_socket_parent(&socket));

        let link = dir.path().join("alias");
        symlink(&parent, &link).unwrap();
        assert!(prepare_local_socket(&link.join("ssh.sock")).is_err());

        fs::set_permissions(&parent, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(prepare_local_socket(&socket).is_err());
    }

    #[tokio::test]
    async fn installed_daemon_probe_isolates_state_and_cleans_up_after_disconnect_or_timeout() {
        use tokio::net::UnixListener;

        let fixture = tempfile::tempdir().unwrap();
        let helper = fixture.path().join(".local/bin/codemux-remote");
        let record = fixture.path().join("isolated-helper-environment");
        fs::create_dir_all(helper.parent().unwrap()).unwrap();
        fs::write(
            &helper,
            format!(
                r#"#!/bin/sh
printf '%s\n' "$$" "$HOME" "$XDG_DATA_HOME" "$CODEMUX_PTY_DAEMON_DIR" "${{CLAUDE_CONFIG_DIR-unset}}" "$PWD" "$@" > {}
exec sleep 60
"#,
                sh_quote(record.to_str().unwrap()),
            ),
        )
        .unwrap();
        fs::set_permissions(&helper, fs::Permissions::from_mode(0o700)).unwrap();

        for disconnect in [true, false] {
            let name = format!("cmx-hello-fixture-{}", uuid::Uuid::new_v4().simple());
            let scratch = Path::new("/tmp").canonicalize().unwrap().join(&name);
            let socket = scratch.join("pty.sock");
            let argv = build_installed_daemon_probe_command(&name, if disconnect { 150 } else { 20 });
            let mut child = Command::new("sh")
                .args(["-c", &argv])
                .env("HOME", fixture.path())
                .env("XDG_DATA_HOME", fixture.path())
                .env("CODEMUX_PTY_DAEMON_DIR", fixture.path())
                .env("CLAUDE_CONFIG_DIR", fixture.path())
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .kill_on_drop(true)
                .spawn()
                .unwrap();
            let stdin = child.stdin.take().unwrap();
            let environment = timeout(Duration::from_secs(5), async {
                loop {
                    if let Ok(contents) = fs::read_to_string(&record) {
                        if contents.lines().count() == 9 {
                            break contents;
                        }
                    }
                    sleep(Duration::from_millis(20)).await;
                }
            })
            .await
            .expect("the installed helper should start");
            let mut lines = environment.lines();
            let pid: i32 = lines.next().unwrap().parse().unwrap();
            for (actual, expected) in lines.zip([
                scratch.join("home"),
                scratch.join("data"),
                scratch.join("daemon"),
                PathBuf::from("unset"),
                scratch.clone(),
                PathBuf::from("pty-daemon"),
                PathBuf::from("--socket"),
                socket.clone(),
            ]) {
                assert_eq!(Path::new(actual), expected);
            }
            assert!(is_private_directory(&scratch));

            let status = if disconnect {
                let _listener = UnixListener::bind(&socket).unwrap();
                fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).unwrap();
                let pidfile = socket.with_extension("sock.pid");
                fs::write(&pidfile, pid.to_string()).unwrap();
                fs::set_permissions(&pidfile, fs::Permissions::from_mode(0o600)).unwrap();
                let mut reader = BufReader::new(child.stdout.take().unwrap());
                let mut ready = String::new();
                timeout(Duration::from_secs(5), reader.read_line(&mut ready))
                    .await
                    .expect("the probe should report readiness")
                    .unwrap();
                assert_eq!(ready.trim_end(), TUNNEL_READY);
                drop(stdin);
                timeout(Duration::from_secs(5), child.wait()).await
            } else {
                // Keep stdin open so the remote-side deadline has to clean up.
                let result = timeout(Duration::from_secs(5), child.wait()).await;
                drop(stdin);
                result
            };
            assert_eq!(
                status.expect("the probe should exit").unwrap().success(),
                disconnect
            );
            assert!(!scratch.exists(), "the probe must remove its own state");
            // SAFETY: this checks liveness without signaling the recorded PID.
            assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
            fs::remove_file(&record).unwrap();
        }
    }

    #[tokio::test]
    async fn workspace_daemon_preflight_checks_running_protocol_and_leaves_listener_alive() {
        use crate::pty_daemon::protocol::{ClientRequest, Frame, ServerResponse};
        use tokio::io::AsyncWriteExt;
        use tokio::net::UnixListener;

        let socket = LocalSocket::allocate().unwrap();
        let listener = UnixListener::bind(&socket.path).unwrap();
        fs::set_permissions(&socket.path, fs::Permissions::from_mode(0o600)).unwrap();
        let pid = socket.path.with_extension("sock.pid");
        fs::write(&pid, std::process::id().to_string()).unwrap();
        fs::set_permissions(&pid, fs::Permissions::from_mode(0o600)).unwrap();

        for (protocol, guarded_shells, prepared_spawn, expected_error) in [
            (PROTOCOL_VERSION + 1, true, true, Some("uses PTY protocol")),
            (PROTOCOL_VERSION, false, true, Some("does not support prepared terminal spawns")),
            (PROTOCOL_VERSION, true, false, Some("does not support prepared terminal spawns")),
            (PROTOCOL_VERSION, true, true, None),
        ] {
            let command = build_existing_daemon_command(socket.path.to_str().unwrap());
            let mut child = Command::new("sh")
                .args(["-c", &command])
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .kill_on_drop(true)
                .spawn()
                .unwrap();
            let serve = async {
                let (stream, _) = listener.accept().await.unwrap();
                let mut stream = BufReader::new(stream);
                let mut request = String::new();
                stream.read_line(&mut request).await.unwrap();
                let ClientRequest::Hello { request_id } = serde_json::from_str(&request).unwrap() else {
                    panic!("The preflight should only send Hello");
                };
                let reply = Frame::Response(ServerResponse::Hello {
                    request_id,
                    protocol_version: protocol,
                    daemon_pid: std::process::id(),
                    daemon_version: "0.0.1".into(),
                    host_claude_plugin: false,
                    guarded_shells,
                    prepared_spawn,
                });
                let mut frame = serde_json::to_vec(&reply).unwrap();
                frame.push(b'\n');
                stream.write_all(&frame).await.unwrap();
                request.clear();
                assert_eq!(stream.read_line(&mut request).await.unwrap(), 0);
            };
            let result = timeout(PROBE_TIMEOUT, async {
                tokio::join!(verify_daemon_forward(&socket.path, &mut child), serve)
            })
            .await
            .expect("Probe must connect and close its client");
            let _ = child.kill().await;
            if let Some(expected_error) = expected_error {
                let error = result.0.unwrap_err();
                assert!(error.contains(expected_error), "{error}");
            } else {
                result.0.unwrap();
            }
            assert!(socket.path.exists(), "the live socket should remain intact");
        }
    }
}
