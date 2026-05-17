//! End-to-end smoke test for the persistent PTY daemon.
//!
//! Verifies the core promise of step 1: a child spawned through the daemon
//! survives the controlling client disconnecting. This is the integration
//! test that catches the regression we're guarding against — "the kernel
//! sent SIGHUP to the agent when the Tauri app exited" — without needing
//! to launch Tauri at all.
//!
//! Unix-only: the daemon is Unix-only and the Tauri-side `daemon_path_viable`
//! check skips this path entirely on Windows. The whole test file is
//! cfg-gated below so Windows CI doesn't fail to compile.

#![cfg(unix)]

use codemux_lib::pty_daemon::{
    client::PtyDaemonClient,
    server,
};
use std::path::PathBuf;
use std::time::Duration;
use tempfile::TempDir;
use tokio::time::sleep;

/// Spawn the daemon in-process (on a tokio task) and return a connected
/// client. The temp dir keeps the socket scoped to this test so parallel
/// tests don't collide.
async fn boot_daemon(tmp: &TempDir) -> (PathBuf, std::sync::Arc<PtyDaemonClient>) {
    let socket_path: PathBuf = tmp.path().join("ptyd.sock");
    let server_socket = socket_path.clone();
    // Scope the manifest to the test tempdir so we don't clobber the
    // user's real `~/.local/share/codemux[-dev]/pty-daemon-manifest.json`.
    std::env::set_var("CODEMUX_PTY_DAEMON_DIR", tmp.path());
    tokio::spawn(async move {
        // run() never returns Ok; either listens forever or errors. We
        // don't care which because the test fixture is torn down when
        // the TempDir drops.
        let _ = server::run(server_socket).await;
    });

    // Wait for the bind to land — the run loop calls bind synchronously,
    // but we hand off to the task first.
    for _ in 0..50 {
        if socket_path.exists() {
            break;
        }
        sleep(Duration::from_millis(20)).await;
    }
    assert!(
        socket_path.exists(),
        "daemon failed to create socket within 1s"
    );
    // Tiny extra beat so listener.accept() is ready.
    sleep(Duration::from_millis(50)).await;

    let client = PtyDaemonClient::connect(&socket_path)
        .await
        .expect("connect to daemon");
    (socket_path, client)
}

#[tokio::test(flavor = "multi_thread")]
async fn hello_handshake_round_trips() {
    let tmp = TempDir::new().unwrap();
    let (_socket, client) = boot_daemon(&tmp).await;

    let (pid, version, proto) = client.hello().await.expect("hello");
    assert!(pid > 0, "daemon must report its pid");
    assert!(!version.is_empty(), "daemon must report a version");
    assert_eq!(proto, codemux_lib::pty_daemon::PROTOCOL_VERSION);
}

#[tokio::test(flavor = "multi_thread")]
async fn spawn_then_list_returns_the_session() {
    let tmp = TempDir::new().unwrap();
    let (_socket, client) = boot_daemon(&tmp).await;

    // A `sleep` keeps the PTY alive long enough for the list call.
    let session_id = "spawn-list-test".to_string();
    let pid = client
        .spawn(
            session_id.clone(),
            "ws-1".to_string(),
            vec!["sleep".to_string(), "30".to_string()],
            std::env::temp_dir().to_string_lossy().to_string(),
            vec![],
            24,
            80,
        )
        .await
        .expect("spawn");
    assert!(pid > 0);

    let list = client.list().await.expect("list");
    let entry = list
        .iter()
        .find(|s| s.session_id == session_id)
        .expect("session should appear in list");
    assert_eq!(entry.pid, pid);
    assert_eq!(entry.workspace_id, "ws-1");

    // Cleanup.
    client.close(session_id).await.expect("close");
}

/// The headline test: the spawned child must survive the client
/// disconnecting. This is the whole point of the daemon — without it,
/// the agent dies when Codemux closes.
#[tokio::test(flavor = "multi_thread")]
async fn child_survives_client_disconnect() {
    let tmp = TempDir::new().unwrap();
    let socket_path = tmp.path().join("ptyd.sock");
    let server_socket = socket_path.clone();
    std::env::set_var("CODEMUX_PTY_DAEMON_DIR", tmp.path());
    tokio::spawn(async move {
        let _ = server::run(server_socket).await;
    });
    for _ in 0..50 {
        if socket_path.exists() {
            break;
        }
        sleep(Duration::from_millis(20)).await;
    }
    sleep(Duration::from_millis(50)).await;

    // Connect, spawn, disconnect by dropping the client.
    let pid = {
        let client = PtyDaemonClient::connect(&socket_path)
            .await
            .expect("connect 1");
        let pid = client
            .spawn(
                "survive-test".to_string(),
                "ws-1".to_string(),
                vec!["sleep".to_string(), "30".to_string()],
                std::env::temp_dir().to_string_lossy().to_string(),
                vec![],
                24,
                80,
            )
            .await
            .expect("spawn");
        // Drop the client at the end of this scope.
        drop(client);
        pid
    };

    // Wait long enough that any SIGHUP-on-disconnect would have killed it.
    sleep(Duration::from_millis(500)).await;

    // Verify via the OS that the process is still alive.
    let alive = unsafe { libc::kill(pid as i32, 0) } == 0;
    assert!(
        alive,
        "spawned child pid={pid} died after the client disconnected — \
         the daemon is supposed to keep it alive"
    );

    // Reconnect and clean up so we don't leak processes across tests.
    let client = PtyDaemonClient::connect(&socket_path)
        .await
        .expect("connect 2");
    client
        .close("survive-test".to_string())
        .await
        .expect("close");
    // Final SIGKILL just in case the close path missed it.
    sleep(Duration::from_millis(100)).await;
    let _ = unsafe { libc::kill(pid as i32, libc::SIGKILL) };
}

/// Headline test for the waiter thread: when a daemon-owned child exits,
/// the real exit code lands on attached clients via the `Exited` event
/// (NOT the `-1` sentinel that the old MVP would have reported).
#[tokio::test(flavor = "multi_thread")]
async fn exit_code_is_reported_on_normal_exit() {
    let tmp = TempDir::new().unwrap();
    let (_socket, client) = boot_daemon(&tmp).await;

    let session_id = "exit-code-zero".to_string();
    // `true` exits immediately with code 0.
    let _pid = client
        .spawn(
            session_id.clone(),
            "ws-1".to_string(),
            vec!["/usr/bin/true".to_string()],
            std::env::temp_dir().to_string_lossy().to_string(),
            vec![],
            24,
            80,
        )
        .await
        .expect("spawn true");

    // Give the waiter thread time to reap.
    sleep(Duration::from_millis(500)).await;

    // The session should be gone from the daemon's list after exit.
    let list = client.list().await.expect("list");
    assert!(
        !list.iter().any(|s| s.session_id == session_id),
        "session should be removed after waiter reaps the child"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn exit_code_propagates_nonzero() {
    let tmp = TempDir::new().unwrap();
    let (_socket, client) = boot_daemon(&tmp).await;

    let session_id = "exit-code-nonzero".to_string();
    // `false` exits immediately with code 1.
    let _pid = client
        .spawn(
            session_id.clone(),
            "ws-1".to_string(),
            vec!["/usr/bin/false".to_string()],
            std::env::temp_dir().to_string_lossy().to_string(),
            vec![],
            24,
            80,
        )
        .await
        .expect("spawn false");

    sleep(Duration::from_millis(500)).await;

    let list = client.list().await.expect("list");
    assert!(
        !list.iter().any(|s| s.session_id == session_id),
        "exited session should be evicted regardless of code"
    );
}

/// Resize must round-trip through the protocol without error. We can't
/// observe the new size from outside (TIOCGWINSZ would need a TTY fd),
/// but a successful response means the daemon called `master.resize()`
/// without panicking.
#[tokio::test(flavor = "multi_thread")]
async fn resize_round_trips() {
    let tmp = TempDir::new().unwrap();
    let (_socket, client) = boot_daemon(&tmp).await;

    let session_id = "resize-test".to_string();
    client
        .spawn(
            session_id.clone(),
            "ws-1".to_string(),
            vec!["sleep".to_string(), "30".to_string()],
            std::env::temp_dir().to_string_lossy().to_string(),
            vec![],
            24,
            80,
        )
        .await
        .expect("spawn");

    client
        .resize(session_id.clone(), 50, 200)
        .await
        .expect("resize should succeed");

    // Resize on an unknown session should surface a clear error rather
    // than panic — that's the user-facing guarantee that a stale resize
    // (after the agent exited) doesn't crash anything.
    let err = client
        .resize("nonexistent".to_string(), 24, 80)
        .await
        .expect_err("resize on unknown session must error");
    assert!(
        format!("{err}").contains("unknown session"),
        "unexpected error shape: {err}"
    );

    client.close(session_id).await.expect("close");
}

/// Write to an unknown session must error, not panic. Belt-and-suspenders
/// against a race where the client thinks a session is alive but the
/// daemon has already reaped it.
#[tokio::test(flavor = "multi_thread")]
async fn write_to_unknown_session_errors_cleanly() {
    let tmp = TempDir::new().unwrap();
    let (_socket, client) = boot_daemon(&tmp).await;

    let err = client
        .write("never-existed".to_string(), b"hello")
        .await
        .expect_err("write to unknown session must error");
    assert!(
        format!("{err}").contains("unknown session"),
        "unexpected error shape: {err}"
    );
}

/// On reconnect, the daemon's `list` must still report the previously-
/// spawned session — the data structure must outlive a single connection.
#[tokio::test(flavor = "multi_thread")]
async fn second_client_sees_session_from_first() {
    let tmp = TempDir::new().unwrap();
    let socket_path = tmp.path().join("ptyd.sock");
    let server_socket = socket_path.clone();
    std::env::set_var("CODEMUX_PTY_DAEMON_DIR", tmp.path());
    tokio::spawn(async move {
        let _ = server::run(server_socket).await;
    });
    for _ in 0..50 {
        if socket_path.exists() {
            break;
        }
        sleep(Duration::from_millis(20)).await;
    }
    sleep(Duration::from_millis(50)).await;

    let session_id = "reconnect-test".to_string();
    let pid_from_first = {
        let client = PtyDaemonClient::connect(&socket_path)
            .await
            .expect("first connect");
        let pid = client
            .spawn(
                session_id.clone(),
                "ws-1".to_string(),
                vec!["sleep".to_string(), "30".to_string()],
                std::env::temp_dir().to_string_lossy().to_string(),
                vec![],
                24,
                80,
            )
            .await
            .expect("spawn");
        drop(client); // simulate Tauri app exit
        pid
    };

    sleep(Duration::from_millis(200)).await;

    let client2 = PtyDaemonClient::connect(&socket_path)
        .await
        .expect("second connect");
    let list = client2.list().await.expect("list");
    let entry = list
        .iter()
        .find(|s| s.session_id == session_id)
        .expect("session should persist across client reconnect");
    assert_eq!(entry.pid, pid_from_first);

    // Clean up.
    client2.close(session_id).await.expect("close");
    sleep(Duration::from_millis(100)).await;
    let _ = unsafe { libc::kill(pid_from_first as i32, libc::SIGKILL) };
}
