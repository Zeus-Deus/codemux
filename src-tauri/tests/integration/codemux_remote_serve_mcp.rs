//! End-to-end integration test for `codemux-remote serve` + `codemux-remote mcp`.
//!
//! This is the headline-feature test for the headless daemon. It exercises the full
//! agent-on-the-remote-controls-Codemux loop, but locally so CI
//! can run it without SSH:
//!
//! 1. Spawn `codemux-remote serve` with an isolated `--state-dir`
//!    in a tempdir, on an ephemeral port.
//! 2. Poll the manifest file until the daemon has written it.
//! 3. Hit the daemon's `/health` endpoint to confirm it's live.
//! 4. Spawn `codemux-remote mcp` pointed at the same state-dir.
//!    Drive it over stdio with JSON-RPC: `initialize` → `tools/list`
//!    → `tools/call workspace_create` → `tools/call workspace_list`.
//! 5. Assert the workspace shows up in the list.
//! 6. Send SIGTERM to the daemon and verify the manifest is gone.
//!
//! Unix-only — `codemux-remote` itself is Unix-only (the PTY daemon
//! and the new serve mode both use Unix-only signals).

#![cfg(unix)]

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use tempfile::TempDir;

fn binary_path() -> PathBuf {
    if let Ok(path) = std::env::var("CARGO_BIN_EXE_codemux-remote") {
        return PathBuf::from(path);
    }
    PathBuf::from("target/debug/codemux-remote")
}

/// Spawn `codemux-remote serve` with an isolated state dir and a
/// random ephemeral port. Returns the running child + the state dir
/// + the manifest contents (once they appear).
struct ServeFixture {
    child: Child,
    state_dir: TempDir,
    /// Isolated `HOME` for the daemon. Two reasons it must never be
    /// the developer's real home: (1) `worktree_create` materialises
    /// worktrees under `$HOME/.codemux/worktrees/...`; (2) on startup
    /// the daemon auto-registers `codemux-remote mcp` into agent
    /// configs (`~/.claude.json`, …) — pointing them at this build's
    /// transient debug binary, which both pollutes the developer's
    /// machine and breaks the `commands::mcp` discovery unit tests
    /// that scan real user-level config paths.
    home: TempDir,
    endpoint: String,
    secret: String,
}

impl ServeFixture {
    fn start() -> Self {
        let bin = binary_path();
        assert!(
            bin.exists(),
            "codemux-remote binary not built at {:?}; \
             run `cargo build --bin codemux-remote` first",
            bin
        );
        let state_dir = TempDir::new().expect("tempdir");
        let home = TempDir::new().expect("home tempdir");
        // Spawn the daemon. Inherit stderr so test failures surface
        // the daemon's own diagnostics. Redirect stdout to /dev/null
        // because we don't care about it (the daemon logs to stderr).
        let mut cmd = Command::new(&bin);
        cmd.arg("serve")
            .arg("--state-dir")
            .arg(state_dir.path())
            .env("HOME", home.path())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit());
        let child = cmd.spawn().expect("spawn codemux-remote serve");

        // Wait for the manifest to appear (the daemon writes it
        // *after* bind succeeds). Up to 10s.
        let manifest_path = state_dir.path().join("manifest.json");
        let deadline = Instant::now() + Duration::from_secs(10);
        let manifest: Value = loop {
            if let Ok(bytes) = std::fs::read(&manifest_path) {
                if let Ok(v) = serde_json::from_slice::<Value>(&bytes) {
                    break v;
                }
            }
            if Instant::now() > deadline {
                panic!("manifest never appeared at {}", manifest_path.display());
            }
            std::thread::sleep(Duration::from_millis(100));
        };
        let endpoint = manifest["endpoint"]
            .as_str()
            .expect("manifest.endpoint")
            .to_string();
        let secret = manifest["secret"]
            .as_str()
            .expect("manifest.secret")
            .to_string();

        // Confirm health probe before declaring readiness.
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(2))
            .build()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Ok(resp) = client.get(format!("{endpoint}/health")).send() {
                if resp.status().is_success() {
                    break;
                }
            }
            if Instant::now() > deadline {
                panic!("daemon at {endpoint} never became healthy");
            }
            std::thread::sleep(Duration::from_millis(100));
        }

        Self { child, state_dir, home, endpoint, secret }
    }

    fn stop(mut self) {
        // Send SIGTERM; serve handles it gracefully and cleans up.
        let pid = self.child.id() as i32;
        unsafe {
            libc::kill(pid, libc::SIGTERM);
        }
        // Wait up to 5s for the child to exit.
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => break,
                _ if Instant::now() > deadline => {
                    eprintln!("[test] daemon didn't exit after SIGTERM; killing");
                    let _ = self.child.kill();
                    break;
                }
                _ => std::thread::sleep(Duration::from_millis(50)),
            }
        }
    }
}

#[test]
fn http_health_and_tools_list() {
    let fx = ServeFixture::start();
    let client = reqwest::blocking::Client::new();

    // Health works without auth.
    let resp = client
        .get(format!("{}/health", fx.endpoint))
        .send()
        .unwrap();
    assert_eq!(resp.status(), 200);

    // Authed: tools/list returns a non-empty catalog.
    let resp = client
        .get(format!("{}/tools/list", fx.endpoint))
        .bearer_auth(&fx.secret)
        .send()
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().unwrap();
    let names: Vec<&str> = body["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    for required in [
        "workspace_create",
        "workspace_list",
        "workspace_info",
        "workspace_close",
        "terminal_spawn",
        "terminal_write",
        "terminal_read",
        "app_status",
    ] {
        assert!(names.contains(&required), "missing tool {required}");
    }

    fx.stop();
}

#[test]
fn http_workspace_create_then_list() {
    let fx = ServeFixture::start();
    let client = reqwest::blocking::Client::new();

    // workspace_create
    let resp = client
        .post(format!("{}/tools/call", fx.endpoint))
        .bearer_auth(&fx.secret)
        .json(&json!({
            "name": "workspace_create",
            "arguments": {
                "path": "/tmp/repo-e2e",
                "name": "demo",
                "branch": "feat/x"
            }
        }))
        .send()
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().unwrap();
    assert_eq!(body["ok"], json!(true));
    let id = body["data"]["workspace"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(body["data"]["workspace"]["name"], json!("demo"));
    assert_eq!(body["data"]["workspace"]["branch"], json!("feat/x"));

    // workspace_list contains it
    let resp = client
        .post(format!("{}/tools/call", fx.endpoint))
        .bearer_auth(&fx.secret)
        .json(&json!({ "name": "workspace_list", "arguments": {} }))
        .send()
        .unwrap();
    let body: Value = resp.json().unwrap();
    let workspaces = body["data"]["workspaces"].as_array().unwrap();
    assert!(workspaces.iter().any(|w| w["id"] == json!(id)));

    fx.stop();
}

/// Run `git -C <dir> <args>`, asserting success; returns trimmed stdout.
fn git(dir: &std::path::Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("spawn git");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// Headless worktree provisioning parity (issue #78), end-to-end
/// through the real daemon over HTTP:
///
/// 1. A bare "origin" + a local clone whose `origin/main` is one
///    commit stale (a publisher clone pushed after the local clone).
/// 2. `worktree_create` against the local clone must land the new
///    branch on the FRESH origin tip (fetch-before-branch), not the
///    stale local ref.
/// 3. The gitignored `.env` must be copied into the worktree before
///    the tool returns.
/// 4. The project's `.codemux/config.json` setup script must run with
///    the CODEMUX_* env + the deterministic port from the response.
#[test]
fn http_worktree_create_provisions_like_desktop() {
    let work = TempDir::new().expect("work tempdir");

    // Seed repo with provisioning config committed.
    let seed = work.path().join("seed");
    std::fs::create_dir_all(&seed).unwrap();
    git(&seed, &["init", "--initial-branch=main"]);
    git(&seed, &["config", "user.email", "t@e.com"]);
    git(&seed, &["config", "user.name", "T"]);
    std::fs::write(seed.join("README.md"), "hi").unwrap();
    std::fs::write(seed.join(".gitignore"), ".env\nsetup-ran.txt\n").unwrap();
    std::fs::create_dir_all(seed.join(".codemux")).unwrap();
    std::fs::write(
        seed.join(".codemux/config.json"),
        r#"{"setup": ["printf '%s|%s|%s|%s' \"$CODEMUX_BRANCH\" \"$CODEMUX_PORT\" \"$CODEMUX_ROOT_PATH\" \"$CODEMUX_WORKSPACE_PATH\" > setup-ran.txt"]}"#,
    )
    .unwrap();
    git(&seed, &["add", "."]);
    git(&seed, &["commit", "-m", "init"]);

    // Bare origin + local clone + publisher that makes local stale.
    let bare = work.path().join("origin.git");
    git(work.path(), &["clone", "--bare", seed.to_str().unwrap(), bare.to_str().unwrap()]);
    let local = work.path().join("local");
    git(work.path(), &["clone", bare.to_str().unwrap(), local.to_str().unwrap()]);
    let publisher = work.path().join("publisher");
    git(work.path(), &["clone", bare.to_str().unwrap(), publisher.to_str().unwrap()]);
    git(
        &publisher,
        &["-c", "user.name=T", "-c", "user.email=t@e.com",
          "commit", "--allow-empty", "-m", "remote-only"],
    );
    git(&publisher, &["push", "origin", "main"]);

    let remote_tip = git(&bare, &["rev-parse", "main"]);
    let stale = git(&local, &["rev-parse", "refs/remotes/origin/main"]);
    assert_ne!(stale, remote_tip, "precondition: local origin/main must be stale");

    // The gitignored .env exists only in the local clone's root —
    // exactly the untracked artifact the includes step must carry over.
    std::fs::write(local.join(".env"), "SECRET=remote-host").unwrap();

    // The fixture isolates HOME, so the worktree lands in its tempdir.
    let fx = ServeFixture::start();
    let client = reqwest::blocking::Client::new();

    let resp = client
        .post(format!("{}/tools/call", fx.endpoint))
        .bearer_auth(&fx.secret)
        .json(&json!({
            "name": "worktree_create",
            "arguments": {
                "repo_path": local.to_string_lossy(),
                "branch": "provisioned",
                "base": "main"
            }
        }))
        .send()
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().unwrap();
    assert_eq!(body["ok"], json!(true), "body: {body}");

    let ws = &body["data"]["workspace"];
    let setup = &body["data"]["setup"];
    let wt = PathBuf::from(ws["path"].as_str().expect("workspace.path"));
    assert!(
        wt.starts_with(fx.home.path()),
        "worktree must live under the isolated HOME: {}",
        wt.display()
    );

    // (2) Fetch-before-branch: new branch starts at the fresh tip.
    assert_eq!(
        git(&wt, &["rev-parse", "HEAD"]),
        remote_tip,
        "daemon-created branch must start at the freshly-fetched origin/main"
    );

    // (3) Includes copied synchronously.
    assert_eq!(
        std::fs::read_to_string(wt.join(".env")).unwrap(),
        "SECRET=remote-host"
    );
    assert!(setup["includes_copied"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v == ".env"));

    // (4) Setup script ran with CODEMUX_* env + the reported port.
    assert_eq!(setup["setup_commands"], json!(1));
    let port = setup["port"].as_u64().expect("setup.port");
    let marker = wt.join("setup-ran.txt");
    let deadline = Instant::now() + Duration::from_secs(15);
    while !marker.exists() {
        assert!(
            Instant::now() < deadline,
            "setup script never produced {}",
            marker.display()
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    let recorded = std::fs::read_to_string(&marker).unwrap();
    let parts: Vec<&str> = recorded.split('|').collect();
    assert_eq!(parts.len(), 4, "marker: {recorded}");
    assert_eq!(parts[0], "provisioned", "CODEMUX_BRANCH");
    assert_eq!(parts[1], port.to_string(), "CODEMUX_PORT");
    assert_eq!(
        std::fs::canonicalize(parts[2]).unwrap(),
        std::fs::canonicalize(&local).unwrap(),
        "CODEMUX_ROOT_PATH"
    );
    assert_eq!(
        std::fs::canonicalize(parts[3]).unwrap(),
        std::fs::canonicalize(&wt).unwrap(),
        "CODEMUX_WORKSPACE_PATH"
    );

    fx.stop();
}

/// Drive `codemux-remote mcp` over stdio: initialize → tools/list →
/// tools/call workspace_create → tools/call workspace_list. This is
/// the actual code path a CLI agent (Claude Code, Codex) takes.
#[test]
fn mcp_stdio_roundtrip() {
    let fx = ServeFixture::start();
    let bin = binary_path();

    // Stash a copy of the secret + state_dir before we move the
    // fixture for stop() later.
    let state_dir_path = fx.state_dir.path().to_path_buf();

    let mut mcp = Command::new(&bin)
        .arg("mcp")
        .arg("--state-dir")
        .arg(&state_dir_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn codemux-remote mcp");

    let mut stdin = mcp.stdin.take().expect("mcp stdin");
    let stdout = mcp.stdout.take().expect("mcp stdout");
    let mut reader = BufReader::new(stdout);

    // 1) initialize
    send(&mut stdin, &json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "test-client", "version": "0.0" }
        }
    }));
    let resp = recv(&mut reader);
    assert_eq!(resp["id"], json!(1));
    assert!(resp["result"]["serverInfo"]["name"]
        .as_str()
        .unwrap()
        .contains("codemux-remote"));

    // initialized notification (no response expected)
    send(&mut stdin, &json!({
        "jsonrpc": "2.0",
        "method": "notifications/initialized"
    }));

    // 2) tools/list
    send(&mut stdin, &json!({
        "jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}
    }));
    let resp = recv(&mut reader);
    assert_eq!(resp["id"], json!(2));
    let names: Vec<&str> = resp["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    assert!(names.contains(&"workspace_create"));
    assert!(names.contains(&"workspace_list"));

    // 3) tools/call workspace_create
    send(&mut stdin, &json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": {
            "name": "workspace_create",
            "arguments": { "path": "/tmp/from-mcp", "name": "via-mcp" }
        }
    }));
    let resp = recv(&mut reader);
    assert_eq!(resp["id"], json!(3));
    assert!(
        resp["result"]["isError"].as_bool() == Some(false),
        "workspace_create reported error: {resp}"
    );
    let text = resp["result"]["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("via-mcp"), "expected name in response text: {text}");

    // 4) tools/call workspace_list — confirm it's there.
    send(&mut stdin, &json!({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "tools/call",
        "params": { "name": "workspace_list", "arguments": {} }
    }));
    let resp = recv(&mut reader);
    assert_eq!(resp["id"], json!(4));
    let list_text = resp["result"]["content"][0]["text"].as_str().unwrap();
    assert!(
        list_text.contains("via-mcp"),
        "workspace_list missing the workspace we just created: {list_text}"
    );

    // 5) Close stdin → MCP server exits cleanly on EOF.
    drop(stdin);
    let _ = mcp.wait();

    fx.stop();
}

#[test]
fn mcp_fails_cleanly_when_daemon_not_running() {
    let bin = binary_path();
    let state_dir = TempDir::new().unwrap();

    let output = Command::new(&bin)
        .arg("mcp")
        .arg("--state-dir")
        .arg(state_dir.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("invoke mcp without daemon");
    assert!(!output.status.success(), "expected non-zero exit");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("no daemon manifest"),
        "expected manifest-not-found message, got: {stderr}"
    );
}

#[test]
fn serve_status_reports_alive() {
    let fx = ServeFixture::start();
    let bin = binary_path();
    let out = Command::new(&bin)
        .arg("serve")
        .arg("status")
        .arg("--state-dir")
        .arg(fx.state_dir.path())
        .output()
        .expect("status");
    assert!(out.status.success(), "status failed");
    let json: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(json["alive"], json!(true));
    assert!(json["endpoint"].as_str().unwrap().starts_with("http://127.0.0.1:"));
    fx.stop();
}

#[test]
fn serve_status_reports_absent_when_no_manifest() {
    let bin = binary_path();
    let state_dir = TempDir::new().unwrap();
    let out = Command::new(&bin)
        .arg("serve")
        .arg("status")
        .arg("--state-dir")
        .arg(state_dir.path())
        .output()
        .expect("status");
    assert!(!out.status.success(), "expected nonzero exit when no manifest");
}

#[test]
fn second_serve_refuses_when_first_is_running() {
    let fx = ServeFixture::start();
    let bin = binary_path();
    // Try to start a second instance against the same state-dir.
    let out = Command::new(&bin)
        .arg("serve")
        .arg("--state-dir")
        .arg(fx.state_dir.path())
        .stderr(Stdio::piped())
        .stdout(Stdio::piped())
        .output()
        .expect("second serve");
    assert!(!out.status.success(), "second serve should refuse to start");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("already running"),
        "expected 'already running' message, got: {stderr}"
    );
    fx.stop();
}

/// `codemux-remote workspace register` should register a workspace
/// in the running daemon's registry. This is what the desktop's push
/// flow runs over SSH right after a successful rsync — the workspace
/// then shows up in `workspace_list` from any MCP-aware agent on the
/// remote, without the agent having to know it exists.
#[test]
fn workspace_register_subcommand_registers_in_daemon() {
    let fx = ServeFixture::start();
    let bin = binary_path();

    // First check the registry is empty.
    let client = reqwest::blocking::Client::new();
    let listed = client
        .post(format!("{}/tools/call", fx.endpoint))
        .bearer_auth(&fx.secret)
        .json(&json!({ "name": "workspace_list", "arguments": {} }))
        .send()
        .unwrap()
        .json::<Value>()
        .unwrap();
    assert_eq!(
        listed["data"]["workspaces"].as_array().unwrap().len(),
        0,
        "fresh daemon should have no workspaces"
    );

    // Run `codemux-remote workspace register …` exactly as the push
    // flow does over SSH.
    let out = Command::new(&bin)
        .arg("workspace")
        .arg("register")
        .arg("--state-dir")
        .arg(fx.state_dir.path())
        .arg("--path")
        .arg("/tmp/from-push")
        .arg("--name")
        .arg("from-push-test")
        .arg("--branch")
        .arg("feat/x")
        .stderr(Stdio::inherit())
        .output()
        .expect("run workspace register");
    assert!(
        out.status.success(),
        "workspace register failed: stderr already printed"
    );
    let printed: Value = serde_json::from_slice(&out.stdout).expect("workspace JSON on stdout");
    assert_eq!(printed["name"], json!("from-push-test"));
    assert_eq!(printed["branch"], json!("feat/x"));
    assert_eq!(printed["path"], json!("/tmp/from-push"));

    // Confirm via HTTP.
    let listed = client
        .post(format!("{}/tools/call", fx.endpoint))
        .bearer_auth(&fx.secret)
        .json(&json!({ "name": "workspace_list", "arguments": {} }))
        .send()
        .unwrap()
        .json::<Value>()
        .unwrap();
    let ws = listed["data"]["workspaces"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["name"] == json!("from-push-test"))
        .expect("registered workspace must appear in workspace_list");
    assert_eq!(ws["branch"], json!("feat/x"));

    fx.stop();
}

/// `workspace register` waits for the daemon to come up if it's
/// briefly absent, then fails with a clear message if it never does.
/// This is the "systemctl --user enable --now codemux-remote just
/// returned, but the daemon hasn't bound its port yet" case the push
/// flow hits in the wild.
#[test]
fn workspace_register_fails_clean_without_daemon() {
    let bin = binary_path();
    let state_dir = TempDir::new().unwrap();

    let out = Command::new(&bin)
        .arg("workspace")
        .arg("register")
        .arg("--state-dir")
        .arg(state_dir.path())
        .arg("--path")
        .arg("/tmp/anywhere")
        .arg("--connect-timeout-secs")
        .arg("1")
        .stderr(Stdio::piped())
        .stdout(Stdio::piped())
        .output()
        .expect("run workspace register");
    assert!(!out.status.success(), "should fail when no daemon");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("did not become healthy"),
        "expected timeout message, got: {stderr}"
    );
}

/// terminal_spawn → terminal_write → terminal_read end-to-end. The
/// MCP agent's killer use case: drive a shell on the remote.
#[test]
fn terminal_spawn_write_read_via_http() {
    let fx = ServeFixture::start();
    let client = reqwest::blocking::Client::new();

    let resp = client
        .post(format!("{}/tools/call", fx.endpoint))
        .bearer_auth(&fx.secret)
        .json(&json!({
            "name": "terminal_spawn",
            "arguments": { "cwd": "/tmp", "command": "/bin/sh" }
        }))
        .send()
        .unwrap();
    let body: Value = resp.json().unwrap();
    assert_eq!(body["ok"], json!(true), "terminal_spawn failed: {body}");
    let tid = body["data"]["terminal"]["id"].as_str().unwrap().to_string();

    // Write a marker the shell will echo back.
    let resp = client
        .post(format!("{}/tools/call", fx.endpoint))
        .bearer_auth(&fx.secret)
        .json(&json!({
            "name": "terminal_write",
            "arguments": {
                "terminal_id": tid,
                "data": "printf 'PTY-MARKER-OK\\n'\n"
            }
        }))
        .send()
        .unwrap();
    assert_eq!(resp.status(), 200);

    // Poll terminal_read until the marker appears (or fail after 2s).
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut found = false;
    while Instant::now() < deadline {
        let resp = client
            .post(format!("{}/tools/call", fx.endpoint))
            .bearer_auth(&fx.secret)
            .json(&json!({
                "name": "terminal_read",
                "arguments": { "terminal_id": tid }
            }))
            .send()
            .unwrap();
        let body: Value = resp.json().unwrap();
        let text = body["data"]["data"].as_str().unwrap_or("");
        if text.contains("PTY-MARKER-OK") {
            found = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(found, "PTY marker never echoed back");

    // Close.
    let _ = client
        .post(format!("{}/tools/call", fx.endpoint))
        .bearer_auth(&fx.secret)
        .json(&json!({
            "name": "terminal_close",
            "arguments": { "terminal_id": tid }
        }))
        .send()
        .unwrap();
    fx.stop();
}

// ─── stdio helpers for the MCP test ──────────────────────────────

fn send<W: Write>(w: &mut W, value: &Value) {
    let line = serde_json::to_string(value).unwrap();
    writeln!(w, "{line}").unwrap();
    w.flush().unwrap();
}

fn recv<R: BufRead>(r: &mut R) -> Value {
    let mut line = String::new();
    let read = r.read_line(&mut line).expect("mcp stdout read");
    assert!(read > 0, "mcp stdout closed before response");
    serde_json::from_str(line.trim())
        .unwrap_or_else(|e| panic!("mcp returned non-JSON: {line:?}: {e}"))
}
