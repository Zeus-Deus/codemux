use crate::database::DatabaseStore;
use crate::pty_daemon::protocol::{ClientRequest, Frame, ServerResponse, PROTOCOL_VERSION};
use crate::state::AppStateStore;
use std::collections::hash_map::DefaultHasher;
use std::fs;
use std::hash::{Hash, Hasher};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tauri::Manager;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;

fn executable(path: &Path, script: &str) {
    fs::write(path, script).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}

fn write_helper_version(path: &Path, version: serde_json::Value) {
    executable(
        path,
        &format!("#!/bin/sh\n[ \"$1\" = version ] || exit 1\nprintf '%s\\n' '{version}'\n"),
    );
}

#[tokio::test]
async fn push_requires_prepared_terminal_spawns_before_copy() {
    const TEST: &str =
        "commands::hosts::hosts_push_tests::push_requires_prepared_terminal_spawns_before_copy";
    if crate::test_isolation::isolated(TEST) {
        return;
    }
    let root = crate::test_isolation::isolated_root().unwrap();
    let checkout = root.join("checkout");
    fs::create_dir(&checkout).unwrap();
    let app = tauri::test::mock_app();
    let state = AppStateStore::default();
    let workspace_id = state.snapshot().active_workspace_id.0;
    state.set_workspace_worktree(&workspace_id, checkout.display().to_string(), "Test".into());
    app.manage(state);
    app.manage(DatabaseStore::new_in_memory());
    let db = app.state::<DatabaseStore>();
    let host = db.insert_host("Test device", "test-device").unwrap();

    // The registered listener needs a short socket path even when the
    // test runner sets a deeply nested TMPDIR.
    let remote = tempfile::Builder::new()
        .prefix("cmux-push-test.")
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir_in("/tmp")
        .unwrap();
    let home = remote.path().canonicalize().unwrap();
    let bin = home.join(".local/bin");
    fs::create_dir_all(&bin).unwrap();
    let helper = bin.join("codemux-remote");
    write_helper_version(
        &helper,
        serde_json::json!({
            "version": env!("CARGO_PKG_VERSION"),
            "protocol_version": PROTOCOL_VERSION,
            "guarded_shells": true,
            "prepared_spawn": true,
        }),
    );

    let mut hash = DefaultHasher::new();
    workspace_id.hash(&mut hash);
    let key = format!("{:x}", hash.finish());
    let state_dir = home.join(".cmux-tunnels");
    fs::DirBuilder::new()
        .mode(0o700)
        .create(&state_dir)
        .unwrap();
    let remote_socket = state_dir.join(format!("{key}.sock"));
    let remote_listener = UnixListener::bind(&remote_socket).unwrap();
    fs::set_permissions(&remote_socket, fs::Permissions::from_mode(0o600)).unwrap();
    fs::write(
        state_dir.join(format!("{key}.root")),
        state_dir.to_str().unwrap(),
    )
    .unwrap();
    let pid_file = remote_socket.with_extension("sock.pid");
    fs::write(&pid_file, std::process::id().to_string()).unwrap();
    fs::set_permissions(pid_file, fs::Permissions::from_mode(0o600)).unwrap();

    executable(
        &root.join("ssh"),
        r#"#!/bin/sh
strict=no
multiplex=no
forward=
while [ "$#" -gt 0 ]; do
    case "$1" in
        StrictHostKeyChecking=yes) strict=yes;;
        ControlPath=none) multiplex=disabled;;
        -L) shift; forward=$1;;
        --) shift; break;;
    esac
    shift
done
[ "$strict" = yes ] && [ "$multiplex" = disabled ] || exit 255
[ "$#" -eq 2 ] && [ "$1" = test-device ] || exit 255
case "$2" in
    *CMX_PUSH_HELPER*) step=helper;;
    *PROBE_ONLY=1*) step=discovery;;
    *codemux:pty-tunnel-ready:v1*)
        [ "${forward#*:}" = "$CODEMUX_TEST_PUSH_REMOTE_SOCKET" ] || exit 1
        printf '%s\n' "${forward%%:*}" > "$CODEMUX_TEST_PUSH_FORWARD"
        step=hello;;
    *) printf '%s\n' unexpected-ssh >> "$CODEMUX_TEST_PUSH_LOG"; exit 1;;
esac
printf '%s\n' "$step" >> "$CODEMUX_TEST_PUSH_LOG"
HOME="$CODEMUX_TEST_PUSH_HOME" exec /bin/sh -c "$2"
"#,
    );
    executable(
        &root.join("rsync"),
        "#!/bin/sh\nprintf '%s\\n' rsync >> \"$CODEMUX_TEST_PUSH_LOG\"\nexit 1\n",
    );

    let path = std::env::var_os("PATH").unwrap_or_default();
    let path =
        std::env::join_paths(std::iter::once(root.clone()).chain(std::env::split_paths(&path)))
            .unwrap();
    let log = root.join("push-log");
    let forward = root.join("push-forward");
    std::env::set_var("PATH", path);
    std::env::set_var("CODEMUX_TEST_PUSH_LOG", &log);
    std::env::set_var("CODEMUX_TEST_PUSH_FORWARD", &forward);
    std::env::set_var("CODEMUX_TEST_PUSH_HOME", &home);
    std::env::set_var("CODEMUX_TEST_PUSH_REMOTE_SOCKET", &remote_socket);

    let wire = tokio::spawn(serve_forwarded_hello(forward));
    let error =
        super::workspace_push_to_host(app.handle().clone(), db, workspace_id.clone(), host.id)
            .await
            .unwrap_err();
    assert!(
        error.contains("does not support prepared terminal spawns"),
        "{error}"
    );
    tokio::time::timeout(Duration::from_secs(5), wire)
        .await
        .expect("the Hello connection should close")
        .unwrap();
    assert_eq!(
        fs::read_to_string(&log).unwrap(),
        "helper\ndiscovery\nhello\n",
    );
    assert_eq!(
        app.state::<AppStateStore>()
            .find_workspace(&workspace_id)
            .unwrap()
            .host_id,
        None,
    );
    assert!(
        remote_socket.exists(),
        "the live daemon socket must be preserved"
    );

    // Without a listener, the installed helper would start at placement.
    // An older helper with the same protocol also needs to fail preflight.
    drop(remote_listener);
    fs::remove_file(remote_socket).unwrap();
    fs::write(&log, "").unwrap();
    write_helper_version(
        &helper,
        serde_json::json!({
            "version": "0.26.0",
            "protocol_version": PROTOCOL_VERSION,
        }),
    );
    let error = super::workspace_push_to_host(
        app.handle().clone(),
        app.state(),
        workspace_id.clone(),
        host.id,
    )
    .await
    .unwrap_err();
    assert!(
        error.contains("does not advertise prepared terminal spawns"),
        "{error}"
    );
    assert_eq!(fs::read_to_string(log).unwrap(), "helper\ndiscovery\n");
    assert_eq!(
        app.state::<AppStateStore>()
            .find_workspace(&workspace_id)
            .unwrap()
            .host_id,
        None,
    );
}

async fn serve_forwarded_hello(path_file: PathBuf) {
    let socket = loop {
        match fs::read_to_string(&path_file) {
            Ok(path) if !path.trim().is_empty() => break PathBuf::from(path.trim()),
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => panic!("read the forwarded socket path: {error}"),
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    };
    // Simulate OpenSSH's local bind. The remote listener advertises the
    // current protocol but lacks a capability that placement requires.
    let listener = UnixListener::bind(&socket).unwrap();
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600)).unwrap();
    let (stream, _) = listener.accept().await.unwrap();
    let mut stream = BufReader::new(stream);
    let mut request = String::new();
    stream.read_line(&mut request).await.unwrap();
    let ClientRequest::Hello { request_id } = serde_json::from_str(&request).unwrap() else {
        panic!("preflight must send Hello");
    };
    let mut reply = serde_json::to_vec(&Frame::Response(ServerResponse::Hello {
        request_id,
        protocol_version: PROTOCOL_VERSION,
        daemon_pid: std::process::id(),
        daemon_version: "0.26.0".into(),
        host_claude_plugin: true,
        guarded_shells: true,
        prepared_spawn: false,
    }))
    .unwrap();
    reply.push(b'\n');
    stream.write_all(&reply).await.unwrap();
    request.clear();
    assert_eq!(stream.read_line(&mut request).await.unwrap(), 0);
}
