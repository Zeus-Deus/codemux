//! Native GUI launch regression on a private Linux display and session bus.

#![cfg(target_os = "linux")]

use serde_json::{json, Value};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const CODEMUX: &str = env!("CARGO_BIN_EXE_codemux");
const TEST: &str = "duplicate_desktop_launch_focuses_the_layout_owner";
const ISOLATED_DISPLAY: &str = "CODEMUX_TEST_DUPLICATE_DESKTOP_DISPLAY";

struct Process(Child);

impl Process {
    fn spawn(command: &mut Command) -> Self {
        Self(command.stdin(Stdio::null()).spawn().unwrap())
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        if self.0.try_wait().ok().flatten().is_none() {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
}

fn eventually<T>(reason: &str, mut check: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let Some(value) = check() {
            return value;
        }
        assert!(Instant::now() < deadline, "{reason}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn fixture_command(program: &str, root: &Path) -> Command {
    let executable = which::which(program).unwrap();
    let mut command = Command::new(executable);
    command
        .env_clear()
        .current_dir(root)
        .env("HOME", root)
        .env(
            "PATH",
            format!("{}:/usr/bin:/bin", root.join("bin").display()),
        )
        .env("CODEMUX_API_URL", "http://127.0.0.1:1")
        .env("CODEMUX_DISABLE_PTY_DAEMON", "1")
        .env("CODEMUX_GDK_BACKEND", "x11")
        .env("GIO_USE_VFS", "local")
        .env("NO_AT_BRIDGE", "1")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("SHELL", "/bin/true");
    for key in [
        "DISPLAY",
        "XAUTHORITY",
        "DBUS_SESSION_BUS_ADDRESS",
        "LD_LIBRARY_PATH",
    ] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    for (key, name) in [
        ("XDG_CONFIG_HOME", "config"),
        ("XDG_DATA_HOME", "data"),
        ("XDG_STATE_HOME", "state"),
        ("XDG_CACHE_HOME", "cache"),
        ("XDG_RUNTIME_DIR", "run"),
    ] {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        if key == "XDG_RUNTIME_DIR" {
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        command.env(key, dir);
    }
    command
}

fn xdotool(root: &Path, args: &[&str]) -> Option<String> {
    let output = fixture_command("xdotool", root)
        .args(args)
        .output()
        .unwrap();
    output
        .status
        .success()
        .then(|| String::from_utf8(output.stdout).unwrap().trim().to_owned())
}

#[test]
#[ignore = "requires xvfb-run, dbus-run-session, openbox, xdotool, xmessage, and WebKitGTK 2.40+"]
fn duplicate_desktop_launch_focuses_the_layout_owner() {
    if std::env::var_os(ISOLATED_DISPLAY).is_none() {
        let session = tempfile::tempdir().unwrap();
        let bus_config = session.path().join("dbus.conf");
        // Allow the single-instance service without activating installed
        // portals, keyrings, and other desktop services on the test bus.
        std::fs::write(
            &bus_config,
            r#"<busconfig>
  <type>session</type>
  <listen>unix:tmpdir=/tmp</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <allow own="*"/>
    <allow send_destination="*"/>
    <allow receive_sender="*"/>
  </policy>
</busconfig>"#,
        )
        .unwrap();
        let status = Command::new("xvfb-run")
            .args(["-a", "dbus-run-session"])
            .arg(format!("--config-file={}", bus_config.display()))
            .arg("--")
            .arg(std::env::current_exe().unwrap())
            .args(["--exact", TEST, "--ignored", "--nocapture"])
            .env(ISOLATED_DISPLAY, "1")
            .env_remove("WAYLAND_DISPLAY")
            .status()
            .unwrap();
        assert!(
            status.success(),
            "isolated desktop test exited with {status}"
        );
        return;
    }
    assert!(std::env::var_os("DISPLAY").is_some());
    assert!(std::env::var_os("DBUS_SESSION_BUS_ADDRESS").is_some());

    let root = tempfile::tempdir().unwrap();
    let root = root.path();
    let browser_run = root.join(".codemux/run");
    std::fs::create_dir_all(&browser_run).unwrap();
    // Stay on the per-session browser adoption path even if a losing launch
    // unexpectedly builds part of the backend.
    std::fs::write(
        browser_run.join("agent-browser-fixture.pid"),
        std::process::id().to_string(),
    )
    .unwrap();
    let bin = root.join("bin");
    std::fs::create_dir(&bin).unwrap();
    for program in ["pkill", "fuser"] {
        std::os::unix::fs::symlink("/bin/true", bin.join(program)).unwrap();
    }

    let mut window_manager = fixture_command("openbox", root);
    if let Some(data_dirs) = std::env::var_os("XDG_DATA_DIRS") {
        window_manager.env("XDG_DATA_DIRS", data_dirs);
    }
    let _window_manager = Process::spawn(&mut window_manager);
    let primary_log = root.join("desktop.log");
    let log = std::fs::File::create(&primary_log).unwrap();
    let mut primary = Process::spawn(
        fixture_command(CODEMUX, root)
            .env("CODEMUX_WEBKIT_COMPAT", "1")
            .stdout(log.try_clone().unwrap())
            .stderr(log),
    );
    let main = eventually("Codemux window did not open", || {
        assert!(
            primary.0.try_wait().unwrap().is_none(),
            "{}",
            std::fs::read_to_string(&primary_log).unwrap()
        );
        xdotool(root, &["search", "--onlyvisible", "--name", "^Codemux$"])
            .and_then(|ids| ids.lines().next().map(str::to_owned))
    });
    eventually("desktop control endpoint did not become ready", || {
        fixture_command(CODEMUX, root)
            .args(["json", "status"])
            .output()
            .unwrap()
            .status
            .success()
            .then_some(())
    });

    let folder = root.join("winner-workspace");
    std::fs::create_dir(&folder).unwrap();
    let created = fixture_command(CODEMUX, root)
        .args([
            "json",
            "create_workspace",
            &json!({"path": folder}).to_string(),
        ])
        .output()
        .unwrap();
    assert!(
        created.status.success(),
        "{}",
        String::from_utf8_lossy(&created.stderr)
    );
    let created: Value = serde_json::from_slice(&created.stdout).unwrap();
    let workspace = created["data"]["workspace_id"].as_str().unwrap();
    let layout = root
        .join("config")
        .join(codemux_lib::APP_DIR_NAME)
        .join("layout.json");
    let layout_has_workspace = || {
        let bytes = std::fs::read(&layout).ok()?;
        let state: Value = serde_json::from_slice(&bytes).ok()?;
        state["workspaces"]
            .as_array()?
            .iter()
            .any(|item| item["workspace_id"].as_str() == Some(workspace))
            .then_some(())
    };
    eventually(
        "layout owner did not persist the workspace",
        layout_has_workspace,
    );

    let _other_window = Process::spawn(fixture_command("xmessage", root).args([
        "-title",
        "Focus fixture",
        "Other window",
    ]));
    let other = eventually("focus fixture did not open", || {
        xdotool(
            root,
            &["search", "--onlyvisible", "--name", "^Focus fixture$"],
        )
        .and_then(|ids| ids.lines().next().map(str::to_owned))
    });
    assert!(xdotool(root, &["windowactivate", &other]).is_some());
    eventually("the other window did not gain focus", || {
        (xdotool(root, &["getactivewindow"]).as_deref() == Some(other.as_str())).then_some(())
    });

    let sentinel = root
        .join("data")
        .join(codemux_lib::APP_DIR_NAME)
        .join("webview-renderer.sentinel");
    assert!(
        !sentinel.try_exists().unwrap(),
        "the compatibility-forced primary armed the crash sentinel"
    );
    // Only the primary requested compatibility. This launch would use the
    // accelerated renderer if it were creating a window of its own.
    let mut duplicate = Process::spawn(&mut fixture_command(CODEMUX, root));
    let status = eventually("duplicate launched its own desktop", || {
        duplicate.0.try_wait().unwrap()
    });
    assert!(status.success(), "duplicate exited with {status}");
    eventually("duplicate did not focus the existing desktop", || {
        (xdotool(root, &["getactivewindow"]).as_deref() == Some(main.as_str())).then_some(())
    });
    assert!(
        !sentinel.try_exists().unwrap(),
        "the forwarded launch armed the crash sentinel: {:?}",
        std::fs::read_to_string(&sentinel)
    );
    assert!(primary.0.try_wait().unwrap().is_none());
    let contender = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(layout.with_extension("lock"))
        .unwrap();
    assert!(matches!(
        contender.try_lock(),
        Err(std::fs::TryLockError::WouldBlock)
    ));
    assert!(layout_has_workspace().is_some());
}
