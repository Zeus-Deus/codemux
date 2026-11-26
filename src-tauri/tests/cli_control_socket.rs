//! The `codemux` control verbs end to end: the built CLI against a real
//! `codemux serve`, and against a scripted control socket that pins the wire
//! contract (requests, identity, exit codes) whatever the app's version.
//!
//! Every child starts from a cleared environment plus an allow-list and its
//! own `XDG_RUNTIME_DIR`, so neither a developer's running app nor the agent
//! identity of a chat this suite runs under can leak in.

#![cfg(unix)]

use serde_json::{json, Value};
use futures_util::{SinkExt, StreamExt};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::{DirBuilderExt, FileTypeExt};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio_tungstenite::tungstenite::Message;

const CODEMUX: &str = env!("CARGO_BIN_EXE_codemux");
const FAKE_CURSOR: &str = env!("CARGO_BIN_EXE_fake_cursor_acp");
/// The control socket's file name in `XDG_RUNTIME_DIR` (debug builds use a
/// name of their own).
const SOCKET: &str = if cfg!(debug_assertions) { "codemux-dev.sock" } else { "codemux.sock" };

fn fixture_runtime_dir(root: &Path) -> PathBuf {
    let run = root.join("run");
    // Match XDG_RUNTIME_DIR's private permissions so serve can recover
    // a stale socket at the same path on restart, regardless of umask.
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&run)
        .unwrap();
    run
}

/// Clears the env and points every state dir (and the control socket) into
/// `root`. `<root>/bin` comes first on PATH: it holds the fake providers.
fn isolate(cmd: &mut Command, root: &Path) {
    cmd.env_clear().current_dir(root);
    let path = format!("{}:/usr/bin:/bin", root.join("bin").display());
    cmd.env("HOME", root).env("PATH", path).env("LANG", "C.UTF-8");
    for (key, sub) in [
        ("XDG_CONFIG_HOME", "config"),
        ("XDG_DATA_HOME", "data"),
        ("XDG_STATE_HOME", "state"),
        ("XDG_CACHE_HOME", "cache"),
    ] {
        let dir = root.join(sub);
        std::fs::create_dir_all(&dir).unwrap();
        cmd.env(key, dir);
    }
    cmd.env("XDG_RUNTIME_DIR", fixture_runtime_dir(root));
}

/// Runs the CLI in `root`'s isolated env.
fn cli_in(root: &Path, args: &[&str]) -> Output {
    cli_with_env(root, &[], args)
}

/// Runs the CLI in `root`'s isolated env plus `env`.
fn cli_with_env(root: &Path, env: &[(&str, &str)], args: &[&str]) -> Output {
    let mut cmd = Command::new(CODEMUX);
    isolate(&mut cmd, root);
    cmd.envs(env.iter().copied()).args(args).stdin(Stdio::null()).output().unwrap()
}

fn stdout_json(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "stdout is not one JSON value ({error}):\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

fn exit_code(output: &Output) -> i32 {
    output.status.code().expect("the CLI exited on its own")
}

fn free_loopback_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn layout_path(root: &Path) -> PathBuf {
    // Match the config directory dirs::config_dir resolves in the child.
    #[cfg(target_os = "macos")]
    let config = root.join("Library/Application Support");
    #[cfg(not(target_os = "macos"))]
    let config = root.join("config");
    config.join(codemux_lib::APP_DIR_NAME).join("layout.json")
}

fn serve_command(root: &Path, port: u16) -> Command {
    let mut cmd = Command::new(CODEMUX);
    isolate(&mut cmd, root);
    // In-process terminals, no reachable account API, and a shell with nothing to say.
    cmd.env("CODEMUX_DISABLE_PTY_DAEMON", "1")
        .env("CODEMUX_API_URL", "http://127.0.0.1:1")
        .env("SHELL", "/bin/true")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .args(["serve", "--scope", "loopback", "--port", &port.to_string()])
        .stdin(Stdio::null());
    cmd
}

/// A `codemux serve` child, stopped (SIGTERM, then kill) when dropped.
struct Serve {
    child: Child,
    root: tempfile::TempDir,
    port: u16,
    session_token: String,
}

impl Serve {
    fn start() -> Self {
        let mut serve = Self::spawn_in(tempfile::tempdir().unwrap());
        serve.wait_ready();
        serve
    }

    fn spawn_in(root: tempfile::TempDir) -> Self {
        let bin = root.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        // Codemux finds Cursor as `cursor-agent` on PATH. A symlink, not a
        // copy: copy-then-exec races ETXTBSY.
        std::os::unix::fs::symlink(FAKE_CURSOR, bin.join("cursor-agent")).unwrap();
        // Keep startup and shutdown browser cleanup inside this fixture.
        std::os::unix::fs::symlink("/bin/true", bin.join("pkill")).unwrap();
        std::os::unix::fs::symlink("/bin/true", bin.join("fuser")).unwrap();
        let port = free_loopback_port();
        let cmd = serve_command(root.path(), port);
        Self::spawn(root, cmd, port)
    }

    fn spawn(root: tempfile::TempDir, mut cmd: Command, port: u16) -> Self {
        let log = std::fs::File::create(root.path().join("serve.log")).unwrap();
        cmd.stdout(log.try_clone().unwrap()).stderr(log);
        Self { child: cmd.spawn().unwrap(), root, port, session_token: String::new() }
    }

    fn restart(&mut self) {
        assert!(self.child.try_wait().unwrap().is_some());
        let log = std::fs::File::create(self.root.path().join("serve.log")).unwrap();
        self.port = free_loopback_port();
        self.session_token.clear();
        self.child = serve_command(self.root.path(), self.port)
            .stdout(log.try_clone().unwrap()).stderr(log).spawn().unwrap();
        self.wait_ready();
    }

    fn log(&self) -> String {
        std::fs::read_to_string(self.root.path().join("serve.log")).unwrap_or_default()
    }

    fn wait_ready(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                panic!("codemux serve exited early ({status}):\n{}", self.log());
            }
            // The footer follows registration of the shutdown signal handlers.
            if self.log().contains("Press Ctrl-C (or send SIGTERM) to stop the server.")
                && self.cli(&["json", "status"]).status.success()
            {
                let socket = self.root.path().join("run").join(SOCKET);
                assert!(
                    std::fs::symlink_metadata(&socket).is_ok_and(|meta| meta.file_type().is_socket()),
                    "serve answered outside its private runtime dir: {socket:?}"
                );
                self.pair_remote();
                return;
            }
            assert!(Instant::now() < deadline, "codemux serve never became ready:\n{}", self.log());
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    /// Pair through the one-time token printed by this isolated server.
    /// Authenticated web-remote invokes set up mutations that a standalone
    /// socket CLI is deliberately no longer allowed to perform.
    fn pair_remote(&mut self) {
        let token = self.log().lines()
            .find_map(|line| line.trim().strip_prefix("Token:").map(str::trim))
            .expect("serve printed its pairing token")
            .to_owned();
        let paired: Value = reqwest::blocking::Client::builder().timeout(Duration::from_secs(10)).build().unwrap()
            .post(format!("http://127.0.0.1:{}/api/pair", self.port))
            .json(&json!({"token": token})).send().unwrap().error_for_status().unwrap().json().unwrap();
        assert_eq!(paired["approved"], true, "{paired}");
        self.session_token = paired["session_token"].as_str().expect("paired session token").to_owned();
    }

    fn remote_invoke(&self, command: &str, args: Value) -> Value {
        let ticket: Value = reqwest::blocking::Client::builder().timeout(Duration::from_secs(10)).build().unwrap()
            .post(format!("http://127.0.0.1:{}/api/ws-ticket", self.port))
            .header("Authorization", format!("Bearer {}", self.session_token))
            .send().unwrap().error_for_status().unwrap().json().unwrap();
        let ticket = ticket["ticket"].as_str().expect("websocket ticket");
        let url = format!("ws://127.0.0.1:{}/ws?ticket={ticket}", self.port);
        tauri::async_runtime::block_on(async {
            let (mut socket, _) = tokio_tungstenite::connect_async(url).await.expect("remote websocket");
            socket.send(Message::Text(json!({"t": "invoke", "id": 1, "cmd": command, "args": args}).to_string().into()))
                .await.expect("remote invoke");
            let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
            loop {
                let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                let frame = tokio::time::timeout(remaining, socket.next()).await.expect("remote invoke timed out")
                    .expect("remote websocket closed").expect("remote websocket frame");
                if let Message::Text(text) = frame {
                    let reply: Value = serde_json::from_str(&text).expect("remote JSON frame");
                    if reply["id"] == 1 {
                        assert_eq!(reply["t"], "ok", "{command}: {reply}");
                        return reply["data"].clone();
                    }
                }
            }
        })
    }

    fn cli(&self, args: &[&str]) -> Output {
        cli_in(self.root.path(), args)
    }

    /// Runs a command that must exit 0 and print JSON.
    fn ok(&self, args: &[&str]) -> Value {
        let output = self.cli(args);
        assert_eq!(exit_code(&output), 0, "`codemux {}`:\n{}\nstderr:\n{}\nserve log:\n{}", args.join(" "),
            String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr), self.log());
        stdout_json(&output)
    }

    /// A folder opened through the authenticated remote session.
    fn workspace(&self, name: &str) -> (String, PathBuf) {
        let folder = self.root.path().join(name);
        std::fs::create_dir_all(&folder).unwrap();
        let created = self.remote_invoke("create_workspace", json!({"cwd": folder}));
        (created.as_str().expect("a workspace id").to_owned(), folder)
    }

    fn shutdown(&mut self) -> Option<ExitStatus> {
        if let Ok(Some(status)) = self.child.try_wait() {
            return Some(status);
        }
        unsafe { libc::kill(self.child.id() as i32, libc::SIGTERM) };
        self.wait_for_exit()
    }

    fn wait_for_exit(&mut self) -> Option<ExitStatus> {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if let Ok(Some(status)) = self.child.try_wait() {
                return Some(status);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        None
    }
}

impl Drop for Serve {
    fn drop(&mut self) {
        if self.shutdown().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

/// A scripted control socket: answers each request line with `answer` and
/// keeps every request it saw.
struct FakeApp {
    root: tempfile::TempDir,
    requests: Arc<Mutex<Vec<Value>>>,
}

impl FakeApp {
    fn start(answer: impl Fn(&Value) -> Value + Send + Sync + 'static) -> Self {
        let root = tempfile::tempdir().unwrap();
        let run = fixture_runtime_dir(root.path());
        let listener = UnixListener::bind(run.join(SOCKET)).unwrap();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let (seen, answer) = (Arc::clone(&requests), Arc::new(answer));
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { return };
                let (seen, answer) = (Arc::clone(&seen), Arc::clone(&answer));
                std::thread::spawn(move || {
                    let mut writer = stream.try_clone().unwrap();
                    for line in BufReader::new(stream).lines() {
                        let Ok(line) = line else { return };
                        let request: Value = serde_json::from_str(&line).unwrap();
                        let reply = answer(&request);
                        seen.lock().unwrap().push(request);
                        if writeln!(writer, "{reply}").is_err() {
                            return;
                        }
                    }
                });
            }
        });
        Self { root, requests }
    }

    fn cli(&self, args: &[&str]) -> Output {
        cli_in(self.root.path(), args)
    }

    fn requests(&self) -> Vec<Value> {
        self.requests.lock().unwrap().clone()
    }
}

fn ok_reply(data: Value) -> Value {
    json!({"ok": true, "protocol_version": 2, "data": data, "error": null})
}

/// What the socket's `tool_call` answers for a registry tool's value.
fn tool_result(value: Value) -> Value {
    ok_reply(json!({"isError": false, "content": [{"type": "text", "text": value.to_string()}], "structuredContent": value}))
}

fn waited(state: &str, cursor: &str, extra: Value) -> Value {
    let mut value = json!({"timed_out": false, "settled": false, "state": state, "turn_id": "t1", "failure": null,
        "final_text": null, "pending": {"approvals": [], "questions": [], "secrets": []},
        "status": {"queued_ids": [], "uncertain_queued_ids": []}, "wait_cursor": cursor});
    for (key, field) in extra.as_object().cloned().unwrap_or_default() {
        value[key] = field;
    }
    value
}

/// A `thread_status` value: the legacy status object plus the run's state.
fn thread_status(state: &str, extra: Value) -> Value {
    let mut value = json!({"workspace_id": "ws", "thread_id": "th", "phase": "running", "settled": false, "turn_id": "t1",
        "queued_ids": [], "uncertain_queued_ids": [], "state": state, "failure": null,
        "pending": {"approvals": [], "questions": [], "secrets": []}, "paused_until_ms": null, "wait_cursor": "s1"});
    for (key, field) in extra.as_object().cloned().unwrap_or_default() {
        value[key] = field;
    }
    value
}

/// The registry tool a scripted request called, if it is a `tool_call`.
fn tool_name(request: &Value) -> Option<&str> {
    (request["command"] == "tool_call").then(|| request["params"]["name"].as_str()).flatten()
}

// ── Without an app ────────────────────────────────────────────────────────

#[test]
fn usage_errors_exit_64_and_help_exits_0() {
    let root = tempfile::tempdir().unwrap();
    let help = cli_in(root.path(), &["thread", "--help"]);
    assert_eq!(exit_code(&help), 0);
    assert!(String::from_utf8_lossy(&help.stdout).contains("Exit codes: 0 done"), "the help names the exit codes");
    for args in [
        &["thread", "start", "--workspace", "ws", "--provider", "codex", "-m", "hi"][..],
        &["--access", "root", "tools"],
        &["call"],
    ] {
        let output = cli_in(root.path(), args);
        assert_eq!(exit_code(&output), 64, "{args:?}: {}", String::from_utf8_lossy(&output.stderr));
    }
}

#[test]
fn with_no_app_running_verbs_report_codemux_unavailable() {
    let root = tempfile::tempdir().unwrap();
    let called = cli_in(root.path(), &["call", "workspace_list"]);
    assert_eq!(exit_code(&called), 1);
    let envelope = stdout_json(&called);
    assert_eq!((envelope["ok"].clone(), envelope["tool"].clone()), (json!(false), json!("workspace_list")));
    assert_eq!(envelope["error"]["code"], "codemux_unavailable", "{envelope}");

    let tools = cli_in(root.path(), &["tools", "--json"]);
    assert_eq!(exit_code(&tools), 1);
    let listed = stdout_json(&tools);
    assert_eq!((listed["door"].clone(), listed["error"]["code"].clone()), (json!("local"), json!("codemux_unavailable")));
    assert!(listed["error"]["message"].as_str().unwrap().contains("codemux capabilities"), "{listed}");

    let waited = cli_in(root.path(), &["thread", "wait", "th", "--workspace", "ws", "--json"]);
    assert_eq!(exit_code(&waited), 1);
    let report = stdout_json(&waited);
    assert_eq!((report["state"].clone(), report["thread_id"].clone()), (json!("failed"), json!("th")));

    // capabilities needs no app, and leads with the version install.sh reads.
    let capabilities = cli_in(root.path(), &["capabilities"]);
    assert_eq!(exit_code(&capabilities), 0);
    let text = String::from_utf8_lossy(&capabilities.stdout);
    assert_eq!(text.lines().nth(1), Some(format!("  \"version\": \"{}\",", env!("CARGO_PKG_VERSION")).as_str()));
    let parsed: Value = serde_json::from_str(&text).unwrap();
    assert!(parsed["commands"]["thread"]["subcommands"]["start"].is_object());
}

#[test]
fn caller_env_is_read_only_by_commands_that_call_the_app() {
    let root = tempfile::tempdir().unwrap();
    // A blank or misspelled variable never stops a command that does not call the app.
    for env in [&[("CODEMUX_CLIENT", "")][..], &[("CODEMUX_CLIENT", "---")], &[("CODEMUX_ACCESS", "readonly")]] {
        let output = cli_with_env(root.path(), env, &["capabilities"]);
        assert_eq!(exit_code(&output), 0, "{env:?}: {}", String::from_utf8_lossy(&output.stderr));
    }
    // A blank one is unset: the call goes ahead and finds no app.
    let blank = cli_with_env(root.path(), &[("CODEMUX_CLIENT", ""), ("CODEMUX_ACCESS", " ")], &["tools", "--json"]);
    assert_eq!(stdout_json(&blank)["error"]["code"], "codemux_unavailable");
    // A misspelled one is a usage error for a command that calls the app, before any call.
    let misspelled = cli_with_env(root.path(), &[("CODEMUX_ACCESS", "readonly")], &["tools", "--json"]);
    assert_eq!(exit_code(&misspelled), 64);
    assert!(String::from_utf8_lossy(&misspelled.stderr).contains("CODEMUX_ACCESS=\"readonly\""), "{}", String::from_utf8_lossy(&misspelled.stderr));
    // A flag overrides the variable.
    let flagged = cli_with_env(root.path(), &[("CODEMUX_ACCESS", "readonly")], &["--access", "read-only", "tools", "--json"]);
    assert_eq!(exit_code(&flagged), 1);
}

// ── Against a scripted socket ─────────────────────────────────────────────

#[test]
fn an_older_app_is_reported_as_codemux_outdated() {
    let app = FakeApp::start(|request| {
        let command = request["command"].as_str().unwrap_or_default();
        if command == "status" {
            return json!({"ok": true, "protocol_version": 1, "data": {}, "error": null});
        }
        json!({"ok": false, "protocol_version": 1, "data": null, "error": format!("Unknown control command: {command}")})
    });
    for args in [&["call", "workspace_list"][..], &["tools", "--json"], &["workspace", "list", "--json"]] {
        let output = app.cli(args);
        assert_eq!(exit_code(&output), 1, "{args:?}");
        let printed = stdout_json(&output);
        assert_eq!(printed["error"]["code"], "codemux_outdated", "{args:?}: {printed}");
        let message = printed["error"]["message"].as_str().unwrap();
        assert!(message.contains("`tool_call`") || message.contains("`tools_list`"), "{message}");
        assert!(message.contains("Restart Codemux"), "{message}");
    }
    let text = app.cli(&["thread", "status", "th", "--workspace", "ws"]);
    assert_eq!(exit_code(&text), 1);
    assert!(String::from_utf8_lossy(&text.stderr).contains("codemux_outdated: "), "{}", String::from_utf8_lossy(&text.stderr));
    // Registry tools go through tool_call, never as bare socket commands.
    assert!(app.requests().iter().all(|request| request["command"] != "workspace_list" && request["command"] != "thread_status"));
}

#[test]
fn waits_carry_the_client_and_map_run_states_to_exit_codes() {
    let approval = json!({"approvals": [{"request_id": "r1", "turn_id": "t1", "request_kind": "command"}], "questions": [], "secrets": []});
    let blocked = approval.clone();
    let app = FakeApp::start(move |request| match request["command"].as_str() {
        Some("status") => ok_reply(json!({})),
        Some("tool_call") if request["params"]["name"] == "thread_status" => {
            tool_result(thread_status("done", json!({"phase": "completed", "settled": true})))
        }
        Some("tool_call") if request["params"]["name"] == "thread_wait" => {
            let arguments = &request["params"]["arguments"];
            let approval = json!({"approvals": [{"request_id": "r1", "turn_id": "t1", "request_kind": "command"}], "questions": [], "secrets": []});
            tool_result(match (arguments["thread_id"].as_str(), arguments["since"].as_str()) {
                (Some("blocked"), None) => waited("needs_input", "c1", json!({"pending": blocked.clone()})),
                (_, None) => waited("running", "c1", json!({})),
                (Some("asks"), Some(_)) => waited("needs_input", "c2", json!({"pending": approval})),
                (Some("fails"), Some(_)) => waited("failed", "c2", json!({"failure": "interrupted"})),
                (_, Some(_)) => waited("done", "c2", json!({"final_text": "PONG"})),
            })
        }
        _ => json!({"ok": false, "protocol_version": 2, "data": null, "error": "unknown_tool: Unknown tool"}),
    });

    let done = app.cli(&["--client", "itest", "--access", "supervised", "thread", "wait", "done", "--workspace", "ws"]);
    assert_eq!(exit_code(&done), 0, "{}", String::from_utf8_lossy(&done.stderr));
    assert_eq!(String::from_utf8_lossy(&done.stdout), "PONG\n");

    let asks = app.cli(&["thread", "wait", "asks", "--workspace", "ws", "--json"]);
    assert_eq!(exit_code(&asks), 2);
    let report = stdout_json(&asks);
    assert_eq!((report["ok"].clone(), report["state"].clone()), (json!(false), json!("needs_input")));
    assert_eq!(report["pending"]["approvals"][0]["request_id"], "r1");

    let fails = app.cli(&["thread", "wait", "fails", "--workspace", "ws", "--json"]);
    assert_eq!(exit_code(&fails), 1);
    assert_eq!(stdout_json(&fails)["failure"], "interrupted");

    // A run already waiting on input is reported by the first read, which does not park.
    let blocked = app.cli(&["thread", "wait", "blocked", "--workspace", "ws", "--json"]);
    assert_eq!(exit_code(&blocked), 2);
    assert_eq!(stdout_json(&blocked)["pending"], approval);

    // A run's state is not a receipt's: status exits 0 whatever it is.
    let status = app.cli(&["thread", "status", "settled", "--workspace", "ws", "--json"]);
    assert_eq!(exit_code(&status), 0, "{}", String::from_utf8_lossy(&status.stdout));
    let printed = stdout_json(&status);
    assert_eq!((printed["ok"].clone(), printed["result"]["state"].clone()), (json!(true), json!("done")));
    let text = app.cli(&["thread", "status", "settled", "--workspace", "ws"]);
    assert_eq!(exit_code(&text), 0);
    assert!(String::from_utf8_lossy(&text.stdout).starts_with("done  phase=completed"), "{}", String::from_utf8_lossy(&text.stdout));

    let requests = app.requests();
    // A narrowed client asks the app whether it enforces access first.
    assert_eq!(requests[0]["command"], "status");
    let blocked: Vec<&Value> = requests.iter()
        .filter(|request| tool_name(request) == Some("thread_wait") && request["params"]["arguments"]["thread_id"] == "blocked")
        .collect();
    assert_eq!(blocked.len(), 1, "{blocked:?}");
    assert_eq!(blocked[0]["params"]["arguments"]["timeout_ms"], 1);
    let waits: Vec<&Value> = requests.iter().filter(|request| tool_name(request) == Some("thread_wait")).collect();
    assert_eq!(waits[0]["client"], json!({"name": "itest", "access": "supervised"}));
    assert_eq!(waits[0]["params"]["arguments"], json!({"workspace_id": "ws", "thread_id": "done", "timeout_ms": 1}));
    assert_eq!(waits[1]["params"]["arguments"], json!({"workspace_id": "ws", "thread_id": "done", "timeout_ms": 60_000, "since": "c1"}));
    // Unnamed calls run as the owner: no client at all.
    let owner = requests.iter().find(|request| request["params"]["arguments"]["thread_id"] == "asks").unwrap();
    assert!(owner.get("client").is_none(), "{owner}");
}

#[test]
fn the_needs_input_hint_answers_a_question_as_printed() {
    let pending = json!({"approvals": [], "secrets": [],
        "questions": [{"question_id": "q1", "turn_id": "t1", "text": "Which database?", "questions": [{"title": "Database", "options": []}]}]});
    let app = FakeApp::start(move |request| match (request["command"].as_str(), tool_name(request)) {
        (Some("status"), _) => ok_reply(json!({})),
        (_, Some("thread_wait")) => tool_result(waited("needs_input", "c1", json!({"pending": pending.clone()}))),
        (_, Some("thread_answer")) => tool_result(json!({"workspace_id": "ws", "thread_id": "th", "question_id": "q1",
            "turn_id": "t2", "operation_id": "op", "state": "succeeded"})),
        _ => json!({"ok": false, "protocol_version": 2, "data": null, "error": "unknown_tool: Unknown tool"}),
    });
    let asks = app.cli(&["thread", "wait", "th", "--workspace", "ws"]);
    assert_eq!(exit_code(&asks), 2);
    let stderr = String::from_utf8_lossy(&asks.stderr);
    let hint = stderr.lines().find_map(|line| line.strip_prefix("Answer: codemux ")).unwrap_or_else(|| panic!("no answer hint:\n{stderr}"));
    assert_eq!(hint, "thread answer th --question q1 --answer <text> --workspace ws", "the thread comes first, as in the other verbs");

    // The hint runs as printed once its placeholder is filled in.
    let argv: Vec<&str> = hint.split_whitespace().map(|word| if word == "<text>" { "staging" } else { word }).collect();
    let answered = app.cli(&argv);
    assert_eq!(exit_code(&answered), 0, "{}", String::from_utf8_lossy(&answered.stderr));
    assert_eq!(String::from_utf8_lossy(&answered.stdout), "t2\n");
    let request = app.requests().into_iter().find(|request| tool_name(request) == Some("thread_answer")).unwrap();
    let arguments = &request["params"]["arguments"];
    assert_eq!(
        [&arguments["workspace_id"], &arguments["thread_id"], &arguments["question_id"], &arguments["answers"]],
        [&json!("ws"), &json!("th"), &json!("q1"), &json!(["staging"])]
    );
}

#[test]
fn a_queued_send_waits_for_the_run_its_message_starts() {
    // The server's shapes: the receipt names only the queue entry, so the
    // CLI waits on the operation. That reads running until the message's own
    // run (t2) starts, then follows it; another run's answer never counts.
    let receipt = |accepted_as: &str, turn: Value| json!({"operation_id": "op", "tool": "thread_send", "state": "succeeded",
        "workspace_id": "ws", "thread_id": "th", "error": null,
        "result": {"workspace_id": "ws", "thread_id": "th", "accepted_as": accepted_as, "turn": turn}});
    let app = FakeApp::start(move |request| match (request["command"].as_str(), tool_name(request)) {
        (Some("status"), _) => ok_reply(json!({})),
        (_, Some("thread_send")) => tool_result(receipt("queued", json!({"queued_id": "q1"}))),
        (_, Some("thread_wait")) => {
            let arguments = &request["params"]["arguments"];
            let operation = json!({"operation_id": "op", "state": "succeeded", "error": null});
            tool_result(match (arguments["operation_id"].as_str(), arguments["since"].as_str()) {
                (Some("op"), None) => waited("running", "c1", json!({"turn_id": null, "operation": operation})),
                (Some("op"), Some("c1")) => waited("done", "c2", json!({"turn_id": "t2", "final_text": "answer of t2", "operation": operation})),
                _ => waited("done", "c9", json!({"final_text": "ANSWER OF ANOTHER RUN"})),
            })
        }
        _ => json!({"ok": false, "protocol_version": 2, "data": null, "error": "unknown_tool: Unknown tool"}),
    });
    let sent = app.cli(&["thread", "send", "th", "--workspace", "ws", "-m", "follow-up", "--delivery", "queue", "--wait", "--json"]);
    assert_eq!(exit_code(&sent), 0, "{}", String::from_utf8_lossy(&sent.stderr));
    let report = stdout_json(&sent);
    assert_eq!((report["turn_id"].clone(), report["final_text"].clone()), (json!("t2"), json!("answer of t2")), "{report}");
    let requests = app.requests();
    assert!(!requests.iter().any(|request| tool_name(request) == Some("operation_status")), "the server resolves the operation");
}

#[test]
fn timeout_bounds_calls_to_an_app_that_stops_answering() {
    // The app takes every request and answers only long after the deadline.
    let app = FakeApp::start(|request| {
        if request["command"] == "status" {
            return ok_reply(json!({}));
        }
        std::thread::sleep(Duration::from_secs(8));
        json!({"ok": false, "protocol_version": 2, "data": null, "error": "operation_failed: too late"})
    });
    for args in [
        &["thread", "wait", "th", "--workspace", "ws", "--timeout", "1s", "--json"][..],
        &["thread", "start", "--workspace", "ws", "--provider", "cursor", "--permission-mode", "agent", "-m", "hi",
            "--request-id", "k", "--timeout", "1s", "--json"],
    ] {
        let started = Instant::now();
        let output = app.cli(args);
        let elapsed = started.elapsed();
        assert_eq!(exit_code(&output), 3, "{args:?}: {}", String::from_utf8_lossy(&output.stdout));
        assert_eq!(stdout_json(&output)["error"]["code"], "timed_out", "{args:?}");
        assert!(elapsed < Duration::from_secs(5), "{args:?} took {elapsed:?}");
    }
}

#[test]
fn a_retried_launch_reports_its_own_run_not_a_newer_runs_approval() {
    // A keyed retry returns the launch's settled receipt: its first message
    // was queued, so it names only the queue entry. That message has run
    // and finished as t1; a newer run, t2, is now the thread's current one
    // and waits on an approval. The server resolves the operation to t1.
    let app = FakeApp::start(|request| match (request["command"].as_str(), tool_name(request)) {
        (Some("status"), _) => ok_reply(json!({})),
        (_, Some("thread_launch")) => tool_result(json!({"operation_id": "op", "tool": "thread_launch", "state": "succeeded",
            "workspace_id": "ws", "thread_id": "th", "error": null,
            "result": {"workspace_id": "ws", "thread_id": "th", "accepted_as": "queued", "turn": {"turn_id": "", "queued_id": "q1", "steered": false}}})),
        (_, Some("thread_status")) => tool_result(thread_status("needs_input", json!({"turn_id": "t2", "phase": "waiting_approval",
            "pending": {"approvals": [{"request_id": "r2", "turn_id": "t2", "request_kind": "command"}], "questions": [], "secrets": []}}))),
        (_, Some("thread_wait")) if request["params"]["arguments"]["operation_id"] == "op" => tool_result(waited("done", "c1",
            json!({"turn_id": "t1", "final_text": "answer of t1", "operation": {"operation_id": "op", "state": "succeeded", "error": null}}))),
        _ => json!({"ok": false, "protocol_version": 2, "data": null, "error": "unknown_tool: Unknown tool"}),
    });
    let retried = app.cli(&["thread", "start", "--workspace", "ws", "--provider", "cursor", "--permission-mode", "agent", "-m", "hi",
        "--request-id", "launch-key", "--wait", "--json"]);
    assert_eq!(exit_code(&retried), 0, "{}", String::from_utf8_lossy(&retried.stdout));
    let report = stdout_json(&retried);
    assert_eq!((report["turn_id"].clone(), report["final_text"].clone()), (json!("t1"), json!("answer of t1")), "{report}");
    assert_eq!(report["pending"]["approvals"], json!([]), "{report}");
}

#[test]
fn input_that_opens_after_the_first_read_is_reported_at_once() {
    // An approval opens right after the follow's first read of the run. A
    // wait that started without that read's cursor takes the approval as its
    // baseline, as the server's waits do, and reports it only at its timeout.
    let approval = json!({"approvals": [{"request_id": "r1", "turn_id": "t1", "request_kind": "command"}], "questions": [], "secrets": []});
    let read = Arc::new(Mutex::new(false));
    let app = FakeApp::start(move |request| {
        let first = !std::mem::replace(&mut *read.lock().unwrap(), true);
        let arguments = &request["params"]["arguments"];
        match tool_name(request) {
            Some("thread_status") if first => tool_result(thread_status("running", json!({"wait_cursor": "before"}))),
            Some("thread_wait") if first => tool_result(waited("running", "before", json!({"timed_out": true}))),
            Some("thread_wait") if arguments["since"] == "before" => tool_result(waited("needs_input", "after", json!({"pending": approval}))),
            Some("thread_wait") => {
                std::thread::sleep(Duration::from_millis(arguments["timeout_ms"].as_u64().unwrap_or(0)));
                tool_result(waited("needs_input", "after", json!({"pending": approval, "timed_out": true})))
            }
            _ => json!({"ok": false, "protocol_version": 2, "data": null, "error": "unknown_tool: Unknown tool"}),
        }
    });
    let started = Instant::now();
    let asks = app.cli(&["thread", "wait", "th", "--workspace", "ws", "--timeout", "8s", "--json"]);
    let elapsed = started.elapsed();
    assert_eq!(exit_code(&asks), 2, "{}", String::from_utf8_lossy(&asks.stdout));
    assert_eq!(stdout_json(&asks)["pending"]["approvals"][0]["request_id"], "r1");
    assert!(elapsed < Duration::from_secs(4), "the approval waited for a timeout: {elapsed:?}");
}

// ── Against a real app ────────────────────────────────────────────────────

#[test]
fn json_exits_1_on_ok_false_and_0_on_ok() {
    let serve = Serve::start();
    let refused = serve.cli(&["json", "no_such_command"]);
    assert_eq!(exit_code(&refused), 1);
    assert_eq!(stdout_json(&refused)["ok"], false);
    assert_eq!(serve.ok(&["json", "status"])["ok"], true);
}

#[test]
fn tools_lists_registry_tools_for_the_named_client() {
    let serve = Serve::start();
    let listed = serve.ok(&["--client", "itest", "tools", "--json"]);
    assert_eq!((listed["ok"].clone(), listed["door"].clone()), (json!(true), json!("local")));
    assert_eq!(listed["caller"]["principal"], "socket:unverified", "{listed}");
    let names: Vec<&str> = listed["tools"].as_array().unwrap().iter().filter_map(|tool| tool["name"].as_str()).collect();
    for name in ["thread_wait", "workspace_list"] {
        assert!(names.contains(&name), "{name} in {names:?}");
    }
    assert!(!names.contains(&"thread_launch"), "{names:?}");
    // A read-only client is offered only what it can run.
    let narrowed = serve.ok(&["--client", "itest", "--access", "read-only", "tools", "--json"]);
    let names: Vec<&str> = narrowed["tools"].as_array().unwrap().iter().filter_map(|tool| tool["name"].as_str()).collect();
    assert!(names.contains(&"thread_wait") && !names.contains(&"thread_launch"), "{names:?}");

    let table = serve.cli(&["tools"]);
    assert_eq!(exit_code(&table), 0);
    assert!(String::from_utf8_lossy(&table.stdout).starts_with("NAME"), "{}", String::from_utf8_lossy(&table.stdout));
}

#[test]
fn call_round_trips_and_reports_coded_errors() {
    let serve = Serve::start();
    let listed = serve.ok(&["call", "workspace_list"]);
    assert_eq!((listed["ok"].clone(), listed["tool"].clone()), (json!(true), json!("workspace_list")));
    assert!(listed["result"]["workspaces"].is_array(), "{listed}");

    let missing = serve.cli(&["call", "thread_read", r#"{"workspace_id":"nope","thread_id":"x"}"#]);
    assert_eq!(exit_code(&missing), 1);
    assert_eq!(stdout_json(&missing)["error"]["code"], "workspace_not_found");

    // The door reaches registry tools only, never legacy socket commands.
    let legacy = serve.cli(&["call", "get_app_state"]);
    assert_eq!(exit_code(&legacy), 1);
    assert_eq!(stdout_json(&legacy)["error"]["code"], "unknown_tool");

    let not_json = serve.cli(&["call", "workspace_list", "[1]"]);
    assert_eq!(exit_code(&not_json), 1);
    assert_eq!(stdout_json(&not_json)["error"]["code"], "invalid_arguments");
}

#[test]
fn thread_start_needs_a_permission_mode_and_the_cli_adds_none() {
    let serve = Serve::start();
    let workspace = serve.ok(&["json", "status"])["data"]["active_workspace_id"].as_str().unwrap().to_owned();
    // The CLI refuses to guess one.
    let usage = serve.cli(&["thread", "start", "--workspace", &workspace, "--provider", "codex", "-m", "hi"]);
    assert_eq!(exit_code(&usage), 64);
    // Socket policy refuses the write before considering launch arguments.
    let launch = json!({"workspace_id": workspace, "client_request_id": "itest-no-mode", "provider": "codex", "message": "hi"});
    let refused = serve.cli(&["call", "thread_launch", &launch.to_string()]);
    assert_eq!(exit_code(&refused), 1);
    let error = &stdout_json(&refused)["error"];
    assert_eq!(error["code"], "access_denied", "{error}");
}

#[test]
fn workspace_create_and_close_refused_while_reads_work() {
    let serve = Serve::start();
    let folder = serve.root.path().join("opened");
    std::fs::create_dir_all(&folder).unwrap();
    let folder = folder.display().to_string();
    let create = serve.cli(&["workspace", "create", &folder, "--request-id", "itest-open", "--json"]);
    assert_eq!(exit_code(&create), 1);
    assert_eq!(stdout_json(&create)["error"]["code"], "access_denied");
    let workspace = serve.ok(&["json", "status"])["data"]["active_workspace_id"].as_str().unwrap().to_owned();
    let text = serve.cli(&["workspace", "list"]);
    assert!(String::from_utf8_lossy(&text.stdout).contains(&workspace), "{}", String::from_utf8_lossy(&text.stdout));
    let info = serve.ok(&["workspace", "info", &workspace, "--json"]);
    assert_eq!(info["result"]["workspace_id"], workspace.as_str());
    let closed = serve.cli(&["workspace", "close", &workspace, "--json"]);
    assert_eq!(exit_code(&closed), 1);
    assert_eq!(stdout_json(&closed)["error"]["code"], "access_denied");
    let listed = serve.ok(&["workspace", "list", "--json"]);
    let ids: Vec<&str> = listed["result"]["workspaces"].as_array().unwrap().iter().filter_map(|w| w["workspace_id"].as_str()).collect();
    assert!(ids.contains(&workspace.as_str()), "{ids:?}");
}

#[test]
fn serve_sigterm_saves_workspace_layout_and_exits_successfully() {
    let mut serve = Serve::start();
    let (workspace_id, _) = serve.workspace("persist-on-shutdown");
    let status = serve.shutdown().expect("serve should stop after SIGTERM");
    assert!(status.success(), "serve exited with {status}:\n{}", serve.log());

    let layout: Value = serde_json::from_slice(&std::fs::read(layout_path(serve.root.path())).unwrap()).unwrap();
    assert!(layout["workspaces"].as_array().unwrap().iter().any(|workspace| {
        workspace["workspace_id"].as_str() == Some(workspace_id.as_str())
    }));
}

#[test]
fn serve_saves_its_layout_without_overwriting_the_recovery_copy() {
    let root = tempfile::tempdir().unwrap();
    let layout = layout_path(root.path());
    let corrupt = b"{\"workspaces\":[";
    std::fs::create_dir_all(layout.parent().unwrap()).unwrap();
    std::fs::write(&layout, corrupt).unwrap();

    #[cfg(target_os = "macos")]
    let data_dir = root.path().join("Library/Application Support");
    #[cfg(not(target_os = "macos"))]
    let data_dir = root.path().join("data");
    let orphan = data_dir.join(codemux_lib::APP_DIR_NAME).join("scrollback/missing-workspace");
    std::fs::create_dir_all(&orphan).unwrap();
    let scrollback = orphan.join("pane.dat");
    std::fs::write(&scrollback, b"terminal history").unwrap();

    let mut serve = Serve::spawn_in(root);
    serve.wait_ready();
    let (workspace_id, _) = serve.workspace("during-recovery");
    let status = serve.shutdown().expect("serve should stop after SIGTERM");
    assert!(status.success(), "serve exited with {status}:\n{}", serve.log());

    let saved: Value = serde_json::from_slice(&std::fs::read(layout.with_extension("json.recovery-session")).unwrap()).unwrap();
    assert!(saved["workspaces"].as_array().unwrap().iter().any(|workspace| {
        workspace["workspace_id"].as_str() == Some(workspace_id.as_str())
    }));
    let backup = std::fs::read_to_string(layout.with_extension("json.recovery-pending")).unwrap();
    assert_eq!(std::fs::read(layout.with_file_name(backup.trim())).unwrap(), corrupt);
    assert_eq!(std::fs::read(scrollback).unwrap(), b"terminal history");
}

#[test]
fn serve_keeps_a_repair_installed_during_recovery_through_shutdown() {
    let mut serve = Serve::start();
    let (original, _) = serve.workspace("original");
    assert!(serve.shutdown().unwrap().success());
    let layout = layout_path(serve.root.path());
    let repaired = std::fs::read(&layout).unwrap();
    std::fs::write(&layout, b"broken").unwrap();
    serve.restart();
    let backup = recovery_backup_path(&layout);
    std::fs::write(&backup, &repaired).unwrap();
    std::fs::rename(backup, &layout).unwrap();
    let (id, _) = serve.workspace("flush-after-repair");
    // Closing a workspace forces a journaled layout flush. Normal shutdown
    // then drains the queued writer and does its guarded final save.
    serve.remote_invoke("close_workspace", json!({"workspaceId": id}));
    assert!(serve.shutdown().unwrap().success(), "{}", serve.log());
    assert_eq!(std::fs::read(&layout).unwrap(), repaired);
    serve.restart();
    serve.ok(&["workspace", "info", &original, "--json"]);
    assert!(!layout.with_extension("json.recovery-pending").exists());
}

#[test]
fn serve_replays_journal_only_sync_closes_during_recovery() {
    use codemux_lib::state::{build_archive_entry, AppStateSnapshot};
    let mut serve = Serve::start();
    let (closed, _) = serve.workspace("close-before-crash");
    let (archived, _) = serve.workspace("archive-before-crash");
    assert!(serve.shutdown().unwrap().success());
    let layout = layout_path(serve.root.path());
    let original = std::fs::read(&layout).unwrap();
    let snapshot: AppStateSnapshot = serde_json::from_slice(&original).unwrap();
    let entry = build_archive_entry(snapshot.workspaces.iter().find(|ws| ws.workspace_id.0 == archived).unwrap());
    let db = rusqlite::Connection::open(layout.with_file_name("codemux.db")).unwrap();
    for id in [closed.as_str(), archived.as_str(), "missing-only"] {
        db.execute("INSERT INTO workspaces_sync (workspace_id, title, server_id, dirty)
                    VALUES (?1, ?1, 'cloud-' || ?1, 0) ON CONFLICT(user_id, workspace_id)
                    DO UPDATE SET server_id = excluded.server_id, dirty = 0", [id]).unwrap();
    }
    // Exact durable state after the old journal commit, before record_closed.
    db.execute("INSERT INTO pending_layout_closes VALUES ('workspace', ?1, '', NULL)", [&closed]).unwrap();
    db.execute("INSERT INTO pending_layout_closes VALUES ('archive', ?1, ?2, ?3)",
        rusqlite::params![archived, entry.archive_id, serde_json::to_string(&entry).unwrap()]).unwrap();
    std::fs::write(&layout, b"broken").unwrap();
    for _ in 0..2 {
        serve.restart();
        assert!(serve.shutdown().unwrap().success());
        for id in [closed.as_str(), archived.as_str(), "missing-only"] {
            let (deleted, dirty, server): (bool, bool, String) = db.query_row(
                "SELECT deleted_at IS NOT NULL, dirty, server_id FROM workspaces_sync WHERE workspace_id = ?1",
                [id], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?))).unwrap();
            assert_eq!((deleted, dirty), (id != "missing-only", id != "missing-only"), "{id}");
            assert_eq!(server, format!("cloud-{id}"));
        }
        assert!(recovery_backup_path(&layout).exists());
    }
}

// Boot in a fresh process each time so only the saved layout, scrollback,
// and database survive the restart.
#[test]
fn corrupt_layout_restart_preserves_pane_and_sync_row() {
    exercise_recovery_exit("discarded-before-boot");
}

#[test]
fn corrupt_layout_discard_while_running_preserves_pane_and_sync_row() {
    exercise_recovery_exit("discarded-while-running");
}

#[test]
fn corrupt_layout_repair_preserves_legacy_pane_and_sync_row() {
    exercise_recovery_exit("repaired");
}

fn recovery_backup_path(layout: &Path) -> PathBuf {
    let backup = std::fs::read_to_string(layout.with_extension("json.recovery-pending")).unwrap();
    layout.with_file_name(backup.trim())
}

fn exercise_recovery_exit(exit: &str) {
    let root = tempfile::tempdir().unwrap();
    let bin = root.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    for program in ["pkill", "fuser"] {
        std::os::unix::fs::symlink("/bin/true", bin.join(program)).unwrap();
    }
    let boot = |phase: &str| -> Value {
        let mut cmd = Command::new(std::env::current_exe().unwrap());
        isolate(&mut cmd, root.path());
        let output = cmd
            .env("CODEMUX_TEST_LAYOUT_BOOT", phase)
            .env("CODEMUX_DISABLE_PTY_DAEMON", "1")
            .env("CODEMUX_API_URL", "http://127.0.0.1:1")
            .env("SHELL", "/bin/true")
            .args(["--ignored", "--exact", "write_panes_and_reconcile_fixture", "--nocapture"])
            .output().unwrap();
        assert!(output.status.success(), "{phase} boot:\n{}", String::from_utf8_lossy(&output.stderr));
        serde_json::from_slice(&std::fs::read(root.path().join(format!("{phase}.json"))).unwrap()).unwrap()
    };
    let original = boot("original");
    let layout = layout_path(root.path());
    assert_eq!(original["on_boot"], json!(["workspace-1", "pane-3", "session-4", "surface-2", "tab-5"]));
    let legacy_layout = std::fs::read(&layout).unwrap();
    std::fs::write(&layout, b"{\"workspaces\":[").unwrap();
    let recovery = boot("recovery");
    let pending_restart = boot("pending-restart");
    assert_eq!(pending_restart["on_boot"], recovery["after_create"], "saved recovery identities must restore");
    for report in [&recovery, &pending_restart] {
        assert_eq!(report["original"], original["original"], "scrollback bytes, metadata and linked row");
        // Both the fallback and later allocations must be distinct, including
        // sessions: scrollback metadata can also be looked up by session alone.
        for id in report["after_create"].as_array().unwrap() {
            assert!(!original["after_create"].as_array().unwrap().contains(id), "reused {id}");
        }
    }
    if exit == "repaired" {
        // The user repairs the set-aside layout and restores it over the fallback.
        let backup = recovery_backup_path(&layout);
        std::fs::write(&backup, legacy_layout).unwrap();
        std::fs::rename(backup, &layout).unwrap();
    } else if exit == "discarded-before-boot" {
        std::fs::remove_file(recovery_backup_path(&layout)).unwrap();
    }

    let exited = boot(exit);
    assert_eq!(layout.with_extension("json.recovery-pending").exists(), exit == "discarded-while-running");
    let expected = if exit == "repaired" { &original } else { &pending_restart };
    assert_eq!(exited["on_boot"], expected["after_create"]);
    if matches!(exit, "repaired" | "discarded-while-running") {
        assert_eq!(exited["original"], original["original"]);
    } else {
        // Discard makes the absent workspace eligible for normal cleanup.
        // Its tombstone still belongs to the original cloud workspace.
        for field in ["id", "workspace_id", "server_id", "title"] {
            assert_eq!(exited["original"]["row"][field], original["original"]["row"][field]);
        }
        assert!(exited["original"]["row"]["deleted_at"].is_string());
        assert!(exited["original"]["scrollback"].is_null(), "startup cleans orphan scrollback");
    }
    let later = boot("later");
    assert!(!layout.with_extension("json.recovery-pending").exists());
    if exit == "discarded-while-running" {
        assert!(later["original"]["row"]["deleted_at"].is_string());
        assert!(later["original"]["scrollback"].is_null());
    }
    assert_eq!(later["on_boot"], exited["after_create"]);
    if exit == "repaired" {
        assert_eq!(later["original"], original["original"]);
    }
    for report in [&exited, &later] {
        for id in report["created"].as_array().unwrap() {
            for previous in [&original, &recovery, &pending_restart] {
                assert!(!previous["after_create"].as_array().unwrap().contains(id), "reused {id}");
            }
            assert!(!report["on_boot"].as_array().unwrap().contains(id), "reused live {id}");
        }
    }
}

fn write_legacy_layout(layout: &Path) {
    use codemux_lib::state;

    // Start from a saved pre-UUID workspace, independent of how this
    // binary allocates new IDs. All references match its numeric IDs.
    let mut snapshot = state::AppStateStore::default().snapshot();
    let ws = &mut snapshot.workspaces[0];
    ws.workspace_id.0 = "workspace-1".into();
    snapshot.active_workspace_id = ws.workspace_id.clone();
    let surface = &mut ws.surfaces[0];
    surface.surface_id.0 = "surface-2".into();
    ws.active_surface_id = surface.surface_id.clone();
    ws.tabs[0].surface_id = Some(surface.surface_id.clone());
    ws.tabs[0].tab_id = "tab-5".into();
    ws.active_tab_id = ws.tabs[0].tab_id.clone();
    let state::PaneNodeSnapshot::Terminal { pane_id, session_id, .. } = &mut surface.root else {
        panic!("expected a terminal");
    };
    pane_id.0 = "pane-3".into();
    session_id.0 = "session-4".into();
    surface.active_pane_id = pane_id.clone();
    snapshot.terminal_sessions[0].session_id = session_id.clone();
    std::fs::create_dir_all(layout.parent().unwrap()).unwrap();
    std::fs::write(layout, serde_json::to_vec(&snapshot).unwrap()).unwrap();
}

#[test]
#[ignore = "run by the recovery tests in a private home"]
fn write_panes_and_reconcile_fixture() {
    use codemux_lib::{database::DatabaseStore, scrollback, state, workspaces_sync};
    use tauri::Manager;

    let phase = std::env::var("CODEMUX_TEST_LAYOUT_BOOT").expect("requires an isolated fixture");
    let layout = layout_path(&std::env::current_dir().unwrap());
    if phase == "original" {
        write_legacy_layout(&layout);
    }
    let app = codemux_lib::build_headless_app().unwrap();
    let store = app.state::<state::AppStateStore>();
    let db = app.state::<DatabaseStore>();
    if phase == "discarded-while-running" {
        assert!(state::layout_recovery_pending());
        std::fs::remove_file(recovery_backup_path(&layout)).unwrap();
        assert!(state::load_persisted_state().is_some());
    }
    assert_eq!(state::layout_recovery_pending(), matches!(phase.as_str(), "recovery" | "pending-restart" | "discarded-while-running"));

    let layout_ids = |ws: &state::WorkspaceSnapshot| {
        let state::PaneNodeSnapshot::Terminal { pane_id, session_id, .. } = &ws.surfaces[0].root else {
            panic!("expected a terminal");
        };
        [
            ws.workspace_id.0.clone(), pane_id.0.clone(), session_id.0.clone(),
            ws.surfaces[0].surface_id.0.clone(), ws.tabs[0].tab_id.clone(),
        ]
    };
    let on_boot: Vec<_> = store.snapshot().workspaces.iter().flat_map(layout_ids).collect();
    let created = store.create_workspace_at_path(std::env::current_dir().unwrap().join(&phase));
    let workspaces = store.snapshot().workspaces;
    let after_create: Vec<_> = workspaces.iter().flat_map(layout_ids).collect();
    let transcript = |title: &str| format!("$ echo {title}\r\n\x1b[32m{title}\x1b[0m\r\n");
    for ws in &workspaces {
        if ws.workspace_id != created && !matches!(phase.as_str(), "original" | "recovery") {
            continue;
        }
        let [workspace_id, pane_id, session_id, ..] = layout_ids(ws);
        let title = format!("{phase} {workspace_id}");
        store.rename_workspace(&workspace_id, title.clone());
        scrollback::save_scrollback(&scrollback::ScrollbackPayload {
            workspace_id, pane_id, session_id,
            working_directory: ws.cwd.clone(), original_command: None, cols: 80, rows: 24,
            data: transcript(&title), adapter_captures: Default::default(), adapter_id: None,
            alternate_buffer: false,
        }).unwrap();
    }
    workspaces_sync::reconcile_from_state(&db, &store).unwrap();
    let rows = db.list_workspaces_sync();
    for ws in &store.snapshot().workspaces {
        let [workspace_id, pane_id, session_id, ..] = layout_ids(ws);
        let saved = scrollback::load_scrollback(&workspace_id, &pane_id).unwrap();
        assert_eq!(saved.data, transcript(&ws.title));
        assert_eq!(saved.meta.session_id, session_id);
        assert!(rows.iter().any(|r| r.workspace_id.as_ref() == Some(&ws.workspace_id.0) && r.title == ws.title));
    }
    let new_row = rows.iter().find(|r| r.workspace_id.as_ref() == Some(&created.0)).unwrap();
    assert_eq!(new_row.server_id, None, "{phase}: {} inherited an old cloud row", created.0);
    state::flush_persisted_state(&store).unwrap();

    let original_row = || db.list_workspaces_sync_for_sync().into_iter()
        .find(|row| row.workspace_id.as_deref() == Some("workspace-1")).unwrap();
    if phase == "original" {
        db.mark_workspace_sync_synced(original_row().id, Some("cloud-original")).unwrap();
    }
    let meta = scrollback::meta_path_for("workspace-1", "pane-3");
    assert_eq!(meta.exists(), meta.with_extension("dat").exists());
    let report = json!({
        "on_boot": on_boot,
        "after_create": after_create,
        "created": layout_ids(&store.find_workspace(&created.0).unwrap()),
        "original": {
            "scrollback": meta.exists().then(|| json!({
                "data": std::fs::read_to_string(meta.with_extension("dat")).unwrap(),
                "meta": std::fs::read_to_string(meta).unwrap(),
            })),
            "row": original_row(),
        }
    });
    std::fs::write(format!("{phase}.json"), serde_json::to_vec(&report).unwrap()).unwrap();
}

#[test]
fn serve_refuses_to_share_a_layout_with_another_instance() {
    use std::os::unix::fs::MetadataExt;

    let owner = Serve::start();
    let port = owner.ok(&["json", "web_remote_status"])["data"]["port"]
        .as_u64().unwrap().try_into().unwrap();
    let (workspace_id, _) = owner.workspace("retained-by-layout-owner");
    let layout = layout_path(owner.root.path());
    let deadline = Instant::now() + Duration::from_secs(10);
    let saved = loop {
        if let Ok(bytes) = std::fs::read(&layout) {
            let snapshot: Value = serde_json::from_slice(&bytes).unwrap();
            let workspace_saved = snapshot["workspaces"].as_array().unwrap().iter().any(|workspace| {
                workspace["workspace_id"].as_str() == Some(workspace_id.as_str())
            });
            // Wait for both fixture shells to finish updating their layout metadata.
            let shells_settled = snapshot["terminal_sessions"].as_array().unwrap().iter().all(|session| {
                matches!(session["state"].as_str(), Some("exited" | "failed"))
            });
            if workspace_saved && shells_settled {
                break bytes;
            }
        }
        assert!(Instant::now() < deadline, "owner did not persist its workspace:\n{}", owner.log());
        std::thread::sleep(Duration::from_millis(50));
    };
    let layout_inode = std::fs::metadata(&layout).unwrap().ino();

    // A different runtime directory bypasses the control-socket liveness probe,
    // but both children still use the same config, data, and browser stubs.
    let contender_root = tempfile::tempdir().unwrap();
    let mut cmd = serve_command(owner.root.path(), port);
    cmd.env("XDG_RUNTIME_DIR", contender_root.path());
    let mut contender = Serve::spawn(contender_root, cmd, port);
    let status = contender.wait_for_exit().unwrap_or_else(|| {
        panic!("second serve did not exit:\n{}", contender.log());
    });
    let log = contender.log();
    assert_eq!(status.code(), Some(1), "second serve exited with {status}:\n{log}");
    assert!(log.contains("Codemux is already running with the workspace layout"), "second serve did not report layout ownership:\n{log}");
    assert_eq!(std::fs::read(&layout).unwrap(), saved);
    assert_eq!(std::fs::metadata(&layout).unwrap().ino(), layout_inode, "second serve replaced the layout");

    let live = owner.ok(&["workspace", "info", &workspace_id, "--json"]);
    assert_eq!(live["result"]["workspace_id"], workspace_id);
}

#[test]
fn serve_preserves_an_unreadable_layout_link() {
    let root = tempfile::tempdir().unwrap();
    let layout = layout_path(root.path());
    let target = root.path().join("missing-layout.json");
    std::fs::create_dir_all(layout.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&target, &layout).unwrap();

    let mut serve = Serve::spawn_in(root);
    serve.wait_ready();
    let status = serve.shutdown().expect("serve should stop after SIGTERM");
    assert!(status.success(), "serve exited with {status}:\n{}", serve.log());
    let backup = std::fs::read_to_string(layout.with_extension("json.recovery-pending")).unwrap();
    assert_eq!(std::fs::read_link(layout.with_file_name(backup.trim())).unwrap(), target);
}

#[cfg(target_os = "linux")]
#[test]
fn standalone_workspace_creation_is_refused_before_project_git_runs() {
    use std::os::unix::fs::PermissionsExt;

    let mut serve = Serve::start();
    let root = serve.root.path();
    let folder = root.join("refused-workspace");
    std::fs::create_dir_all(&folder).unwrap();
    let git = root.join("bin/git");
    std::fs::write(&git, "#!/bin/sh\n: > \"$HOME/workspace-git-entered\"\nexec /usr/bin/git \"$@\"\n").unwrap();
    std::fs::set_permissions(git, std::fs::Permissions::from_mode(0o755)).unwrap();

    let refused = serve.cli(&["json", "create_workspace", &json!({"path": folder}).to_string()]);
    assert_eq!(exit_code(&refused), 1);
    assert!(stdout_json(&refused)["error"].as_str().unwrap_or_default().contains("access_denied"),
        "{}", String::from_utf8_lossy(&refused.stdout));
    assert!(!root.join("workspace-git-entered").exists(), "refused write reached Git");
    assert!(serve.shutdown().expect("serve stops after the refusal").success());
}

#[test]
fn serve_sigterm_reports_layout_save_failure() {
    let mut serve = Serve::start();
    let layout = layout_path(serve.root.path());
    if layout.is_file() {
        std::fs::rename(&layout, layout.with_extension("startup.json")).unwrap();
    }
    // A directory at the destination blocks the final atomic file replacement
    // even when the test process has permission to write to the parent.
    std::fs::create_dir_all(layout).unwrap();
    serve.workspace("fail-layout-save");

    let status = serve.shutdown().expect("serve should stop after SIGTERM");
    let log = serve.log();
    assert_eq!(status.code(), Some(1), "serve exited with {status}:\n{log}");
    assert!(log.lines().any(|line| {
        line.starts_with("[codemux serve]") && line.contains("Failed to replace layout state")
    }), "serve did not report the shutdown layout error:\n{log}");
}

#[test]
fn thread_start_is_refused_even_with_a_configured_fixture_provider() {
    let serve = Serve::start();
    let workspace = serve.ok(&["json", "status"])["data"]["active_workspace_id"].as_str().unwrap().to_owned();
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let probe = serve.cli(&["call", "agent_capabilities", &json!({"workspace_id": workspace, "provider": "cursor"}).to_string()]);
        let caps = stdout_json(&probe);
        if caps["result"]["providers"][0]["configured"] == true {
            break;
        }
        let code = caps["error"]["code"].as_str().unwrap_or("provider_not_ready");
        assert!(matches!(code, "provider_not_configured" | "provider_not_ready"), "{caps}");
        assert!(Instant::now() < deadline, "Cursor never registered: {caps}\n{}", serve.log());
        std::thread::sleep(Duration::from_millis(500));
    }
    let started = serve.cli(&["--client", "itest", "thread", "start", "--workspace", &workspace,
        "--provider", "cursor", "--permission-mode", "agent", "-m", "echo-prompt PONG", "--json"]);
    assert_eq!(exit_code(&started), 1, "{}", String::from_utf8_lossy(&started.stdout));
    assert_eq!(stdout_json(&started)["error"]["code"], "access_denied");
}

#[test]
fn connector_admin_commands_are_refused_to_standalone_cli() {
    let serve = Serve::start();
    let port = free_loopback_port().to_string();
    for args in [
        vec!["connector", "status", "--json"],
        vec!["connector", "enable", "--port", &port, "--json"],
        vec!["connector", "disable", "--json"],
        vec!["connector", "list", "--json"],
        vec!["connector", "pending", "--json"],
        vec!["connector", "issue", "--label", "itest-bot", "--json"],
        vec!["connector", "revoke", "itest-bot", "--json"],
    ] {
        let refused = serve.cli(&args);
        assert_eq!(exit_code(&refused), 1, "{}: {}", args.join(" "), String::from_utf8_lossy(&refused.stdout));
        assert_eq!(stdout_json(&refused)["error"]["code"], "admin_refused", "{}", args.join(" "));
    }
}

#[test]
fn serve_banner_names_desktop_settings_for_connector_administration() {
    let serve = Serve::start();
    let banner = serve.log();
    assert!(banner.contains("agent connector is off; enable it in desktop Settings"), "{banner}");
    assert!(banner.contains("Foreground serve prints a fresh pairing link when restarted"), "{banner}");
    assert!(!banner.contains("Run `codemux connector enable`"), "{banner}");
    assert!(!banner.contains("Run `codemux remote pair`"), "{banner}");
}

#[test]
fn standalone_remote_cli_cannot_mint_pairing_or_change_listener() {
    let serve = Serve::start();
    let pair = serve.cli(&["remote", "pair"]);
    assert_eq!(exit_code(&pair), 1, "{}", String::from_utf8_lossy(&pair.stderr));
    assert!(String::from_utf8_lossy(&pair.stderr).contains("needs verified owner access"));

    let other_port = free_loopback_port().to_string();
    let enable = serve.cli(&["remote", "enable", "--port", &other_port]);
    assert_eq!(exit_code(&enable), 1, "{}", String::from_utf8_lossy(&enable.stderr));
    assert!(String::from_utf8_lossy(&enable.stderr).contains("needs verified owner access"));
    assert_eq!(serve.ok(&["json", "web_remote_status"])["data"]["port"].as_u64(), Some(serve.port as u64));
}
