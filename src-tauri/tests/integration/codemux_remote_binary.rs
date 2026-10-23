//! Integration tests for the `codemux-remote` slim binary.
//!
//! We spawn the actual built binary (not the in-process server) so the
//! tests catch issues with the CLI dispatch, the version reporting,
//! and the same-as-in-app daemon behavior when invoked through the
//! binary's entry point. This is what the SSH bootstrap will do on
//! the remote host, so the same code path needs to work end-to-end.
//!
//! Unix-only: the daemon path is Unix-only, and the binary's CLI
//! reports that with a non-zero exit on other platforms.

#![cfg(unix)]

use codemux_lib::pty_daemon::client::PtyDaemonClient;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;
use tempfile::TempDir;
use tokio::time::sleep;

/// Locate the just-built `codemux-remote` binary. Skips the test if
/// the binary hasn't been built yet — running `cargo test --bin` will
/// build the target first.
fn binary_path() -> PathBuf {
    // CARGO_BIN_EXE_<name> is set by Cargo when running `cargo test`
    // and points at the freshly-built binary for that test invocation.
    // Falls back to the workspace target/ in case the env var is
    // missing (some IDEs run tests differently).
    if let Ok(path) = std::env::var("CARGO_BIN_EXE_codemux-remote") {
        return PathBuf::from(path);
    }
    PathBuf::from("target/debug/codemux-remote")
}

#[test]
fn version_subcommand_prints_json() {
    let bin = binary_path();
    if !bin.exists() {
        eprintln!(
            "[test] codemux-remote binary at {:?} not built; \
             run `cargo build --bin codemux-remote` first",
            bin
        );
        // Don't fail — the binary may not be built in some test
        // contexts. The integration test for the daemon path covers
        // the runtime behavior; this test is about the CLI shape.
        return;
    }
    let output = Command::new(&bin)
        .arg("version")
        .output()
        .expect("invoke binary");
    assert!(
        output.status.success(),
        "version subcommand failed: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("utf-8 stdout");
    // Shape contract: clients parse this with serde_json, so the
    // exact field names matter. If you rename one you break SSH
    // bootstrap.
    let parsed: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("valid JSON");
    assert_eq!(parsed["name"], "codemux-remote");
    assert!(parsed["version"].is_string());
    assert!(parsed["protocol_version"].is_number());
}

#[test]
fn no_subcommand_defaults_to_version() {
    let bin = binary_path();
    if !bin.exists() {
        return;
    }
    let output = Command::new(&bin).output().expect("invoke binary");
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("utf-8 stdout");
    let parsed: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("valid JSON");
    assert_eq!(parsed["name"], "codemux-remote");
}

/// The headline test: spawning the binary in `pty-daemon` mode and
/// then dialing it from the in-app `PtyDaemonClient` must work end-
/// to-end. This is exactly the call shape the SSH bootstrap will use
/// once the tunnel is wired.
#[tokio::test(flavor = "multi_thread")]
async fn daemon_subcommand_accepts_client_connections() {
    let bin = binary_path();
    if !bin.exists() {
        return;
    }
    let tmp = TempDir::new().unwrap();
    let socket = tmp.path().join("ptyd.sock");

    // Manifest dir override so the binary doesn't try to write into
    // the user's real `~/.local/share/codemux/`.
    let mut child = Command::new(&bin)
        .arg("pty-daemon")
        .arg("--socket")
        .arg(&socket)
        .env("CODEMUX_PTY_DAEMON_DIR", tmp.path())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn codemux-remote pty-daemon");

    // Wait for the socket to appear (binary races against us). 5
    // seconds is generous — the daemon binds in ms in practice.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while std::time::Instant::now() < deadline {
        if Path::new(&socket).exists() {
            break;
        }
        sleep(Duration::from_millis(50)).await;
    }
    assert!(
        Path::new(&socket).exists(),
        "binary did not create socket within 5s"
    );
    // Tiny extra beat so listener.accept() is ready.
    sleep(Duration::from_millis(100)).await;

    let client = PtyDaemonClient::connect(&socket)
        .await
        .expect("connect to binary's socket");

    // Hello round-trips with a matching protocol version.
    let (pid, version, proto) = client.hello().await.expect("hello");
    assert!(pid > 0);
    assert!(!version.is_empty());
    assert_eq!(proto, codemux_lib::pty_daemon::PROTOCOL_VERSION);

    // Spawn a child + reap it — exercises the full path the SSH
    // bootstrap-and-push flow will use in 2d.
    let session_id = "remote-binary-test".to_string();
    let spawn_pid = client
        .spawn(
            session_id.clone(),
            "ws-test".to_string(),
            vec!["/usr/bin/true".to_string()],
            std::env::temp_dir().to_string_lossy().to_string(),
            vec![],
            24,
            80,
        )
        .await
        .expect("spawn /usr/bin/true via the binary's daemon");
    assert!(spawn_pid > 0);

    // Give the waiter thread a moment to reap.
    sleep(Duration::from_millis(500)).await;
    let list = client.list().await.expect("list");
    assert!(
        !list.iter().any(|s| s.session_id == session_id),
        "child should be reaped and the session evicted from the daemon's list"
    );

    // Clean shutdown so we don't leak the child process.
    let _ = child.kill();
    let _ = child.wait();
}

/// `codemux-remote workspace list` reads the daemon's SQLite registry
/// directly and prints a stable JSON envelope on stdout. The desktop's
/// host-inventory poller invokes this over SSH on a recurring cadence,
/// so the shape is a wire contract: keep this test in lockstep with
/// any change to the JSON envelope.
///
/// We exercise three properties:
/// 1. An empty registry produces `{"host_id":"…","workspaces":[]}`
///    (no panic, no error exit, no extra noise on stdout).
/// 2. A non-empty registry round-trips every documented field of
///    `remote::workspace::Workspace` (id, name, path, branch,
///    project_root, origin_host_id, owner_id null, notes null,
///    created_at, updated_at) — these are exactly the fields the
///    desktop reconcile pass consumes, and a silent omission would
///    surface as missing data in the overview.
/// 3. The implementation works against an arbitrary `--state-dir` so
///    tests + SSH calls into per-user state dirs don't collide.
#[test]
fn workspace_list_subcommand_prints_inventory_json() {
    let bin = binary_path();
    if !bin.exists() {
        return;
    }
    let tmp = TempDir::new().unwrap();
    let state_dir = tmp.path().join("state");

    // 1. Empty state-dir → empty workspaces array but valid envelope.
    let empty = Command::new(&bin)
        .args([
            "workspace",
            "list",
            "--state-dir",
        ])
        .arg(&state_dir)
        .output()
        .expect("invoke binary");
    assert!(
        empty.status.success(),
        "workspace list (empty) failed: stderr={}",
        String::from_utf8_lossy(&empty.stderr)
    );
    let stdout = String::from_utf8(empty.stdout).expect("utf-8 stdout");
    let parsed: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("valid JSON envelope");
    assert!(parsed["host_id"].is_string(), "host_id must be a string");
    assert!(parsed["workspaces"].is_array(), "workspaces must be an array");
    assert_eq!(
        parsed["workspaces"].as_array().unwrap().len(),
        0,
        "fresh state dir must return an empty workspaces array"
    );

    // 2. Seed the registry by opening WorkspaceStore directly and
    // creating one workspace, then re-run the CLI and assert the
    // round-trip.
    {
        use codemux_lib::remote::{config, workspace::WorkspaceStore};
        let store = WorkspaceStore::open(
            &config::database_path(&state_dir),
            "fixture-host".into(),
            config::workspaces_root(&state_dir),
        )
        .expect("open store");
        let ws = store
            .create(
                Some("inventory-test".into()),
                "/srv/inventory-test".into(),
                Some("feature/inventory".into()),
                Some("/srv/origin".into()),
            )
            .expect("create workspace");
        assert!(!ws.id.is_empty());
    }

    let populated = Command::new(&bin)
        .args([
            "workspace",
            "list",
            "--state-dir",
        ])
        .arg(&state_dir)
        .output()
        .expect("invoke binary");
    assert!(
        populated.status.success(),
        "workspace list (populated) failed: stderr={}",
        String::from_utf8_lossy(&populated.stderr)
    );
    let stdout = String::from_utf8(populated.stdout).expect("utf-8 stdout");
    let parsed: serde_json::Value =
        serde_json::from_str(stdout.trim()).expect("valid JSON envelope");
    let workspaces = parsed["workspaces"]
        .as_array()
        .expect("workspaces is an array");
    assert_eq!(workspaces.len(), 1, "exactly the workspace we just created");
    let w = &workspaces[0];
    assert!(w["id"].is_string(), "id must be a string (UUID)");
    assert_eq!(w["name"], "inventory-test");
    assert_eq!(w["path"], "/srv/inventory-test");
    assert_eq!(w["branch"], "feature/inventory");
    assert_eq!(w["project_root"], "/srv/origin");
    assert_eq!(
        w["origin_host_id"], "fixture-host",
        "origin_host_id round-trips through the registry"
    );
    assert!(
        w["owner_id"].is_null(),
        "owner_id is null in v1 (reserved for cloud relay)"
    );
    assert!(w["notes"].is_null(), "notes is null until the desktop attaches some");
    assert!(w["created_at"].is_string());
    assert!(w["updated_at"].is_string());
}
