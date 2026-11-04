//! The `codemux` control verbs end to end: the built CLI against a real
//! `codemux serve`, and against a scripted control socket that pins the wire
//! contract (requests, identity, exit codes) whatever the app's version.
//!
//! Every child starts from a cleared environment plus an allow-list and its
//! own `XDG_RUNTIME_DIR`, so neither a developer's running app nor the agent
//! identity of a chat this suite runs under can leak in.

#![cfg(unix)]

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

const CODEMUX: &str = env!("CARGO_BIN_EXE_codemux");
const FAKE_CURSOR: &str = env!("CARGO_BIN_EXE_fake_cursor_acp");
/// The control socket's file name in `XDG_RUNTIME_DIR` (debug builds use a
/// name of their own).
const SOCKET: &str = if cfg!(debug_assertions) { "codemux-dev.sock" } else { "codemux.sock" };

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
        ("XDG_RUNTIME_DIR", "run"),
    ] {
        let dir = root.join(sub);
        std::fs::create_dir_all(&dir).unwrap();
        cmd.env(key, dir);
    }
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

/// A `codemux serve` child, stopped (SIGTERM, then kill) when dropped.
struct Serve {
    child: Child,
    root: tempfile::TempDir,
}

impl Serve {
    fn start() -> Self {
        let root = tempfile::tempdir().unwrap();
        let bin = root.path().join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        // Codemux finds Cursor as `cursor-agent` on PATH. A symlink, not a
        // copy: copy-then-exec races ETXTBSY.
        std::os::unix::fs::symlink(FAKE_CURSOR, bin.join("cursor-agent")).unwrap();
        let log = std::fs::File::create(root.path().join("serve.log")).unwrap();
        let mut cmd = Command::new(CODEMUX);
        isolate(&mut cmd, root.path());
        // In-process terminals (no detached daemon outlives the test), no
        // reachable account API, and a shell with nothing to say.
        cmd.env("CODEMUX_DISABLE_PTY_DAEMON", "1")
            .env("CODEMUX_API_URL", "http://127.0.0.1:1")
            .env("SHELL", "/bin/true")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .args(["serve", "--scope", "loopback", "--port", &free_loopback_port().to_string()])
            .stdin(Stdio::null())
            .stdout(log.try_clone().unwrap())
            .stderr(log);
        let mut serve = Self { child: cmd.spawn().unwrap(), root };
        serve.wait_ready();
        serve
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
            let output = self.cli(&["json", "status"]);
            if output.status.success() {
                return;
            }
            assert!(Instant::now() < deadline, "codemux serve never answered on its socket:\n{}", self.log());
            std::thread::sleep(Duration::from_millis(200));
        }
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

    /// A folder opened as a workspace through the legacy socket command.
    fn workspace(&self, name: &str) -> (String, PathBuf) {
        let folder = self.root.path().join(name);
        std::fs::create_dir_all(&folder).unwrap();
        let created = self.ok(&["json", "create_workspace", &json!({"path": folder}).to_string()]);
        (created["data"]["workspace_id"].as_str().expect("a workspace id").to_owned(), folder)
    }
}

impl Drop for Serve {
    fn drop(&mut self) {
        // This test started the serve; stop it by its pid.
        unsafe { libc::kill(self.child.id() as i32, libc::SIGTERM) };
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if self.child.try_wait().ok().flatten().is_some() {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
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
        let run = root.path().join("run");
        std::fs::create_dir_all(&run).unwrap();
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
            tool_result(match request["params"]["arguments"]["thread_id"].as_str() {
                Some("blocked") => thread_status("needs_input", json!({"phase": "waiting_approval", "pending": blocked.clone()})),
                Some("settled") => thread_status("done", json!({"phase": "completed", "settled": true})),
                _ => thread_status("running", json!({})),
            })
        }
        Some("tool_call") if request["params"]["name"] == "thread_wait" => {
            let arguments = &request["params"]["arguments"];
            let approval = json!({"approvals": [{"request_id": "r1", "turn_id": "t1", "request_kind": "command"}], "questions": [], "secrets": []});
            tool_result(match (arguments["thread_id"].as_str(), arguments["since"].as_str()) {
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

    // A run already waiting on input is reported at once, without parking a wait.
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
    assert!(!requests.iter().any(|request| tool_name(request) == Some("thread_wait") && request["params"]["arguments"]["thread_id"] == "blocked"));
    let waits: Vec<&Value> = requests.iter().filter(|request| tool_name(request) == Some("thread_wait")).collect();
    assert_eq!(waits[0]["client"], json!({"name": "itest", "access": "supervised"}));
    assert_eq!(waits[0]["params"]["arguments"], json!({"workspace_id": "ws", "thread_id": "done", "timeout_ms": 60_000}));
    assert_eq!(waits[1]["params"]["arguments"]["since"], "c1");
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
        (_, Some("thread_status")) => tool_result(thread_status("needs_input", json!({"pending": pending.clone()}))),
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
        (_, Some("thread_status")) => tool_result(thread_status("running", json!({"queued_ids": ["q1"]}))),
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
    assert_eq!(listed["caller"]["principal"], "local:itest", "{listed}");
    let names: Vec<&str> = listed["tools"].as_array().unwrap().iter().filter_map(|tool| tool["name"].as_str()).collect();
    for name in ["thread_launch", "thread_wait", "workspace_list"] {
        assert!(names.contains(&name), "{name} in {names:?}");
    }
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
    let (workspace, _) = serve.workspace("project");
    // The CLI refuses to guess one.
    let usage = serve.cli(&["thread", "start", "--workspace", &workspace, "--provider", "codex", "-m", "hi"]);
    assert_eq!(exit_code(&usage), 64);
    // And the server refuses a launch that names none.
    let launch = json!({"workspace_id": workspace, "client_request_id": "itest-no-mode", "provider": "codex", "message": "hi"});
    let refused = serve.cli(&["call", "thread_launch", &launch.to_string()]);
    assert_eq!(exit_code(&refused), 1);
    let error = &stdout_json(&refused)["error"];
    assert_eq!(error["code"], "invalid_arguments", "{error}");
    assert!(error["message"].as_str().unwrap().contains("permission_mode"), "{error}");
}

#[test]
fn workspace_create_list_info_close_round_trip() {
    let serve = Serve::start();
    let folder = serve.root.path().join("opened");
    std::fs::create_dir_all(&folder).unwrap();
    let folder = folder.display().to_string();

    let created = serve.ok(&["workspace", "create", &folder, "--request-id", "itest-open", "--json"]);
    assert_eq!(created["ok"], true, "{created}");
    let workspace = created["result"]["workspace_id"].as_str().expect("a workspace id").to_owned();
    // The same key replays the same outcome.
    let replayed = serve.ok(&["workspace", "create", &folder, "--request-id", "itest-open", "--json"]);
    assert_eq!(replayed["result"]["workspace_id"], workspace.as_str(), "{replayed}");

    let text = serve.cli(&["workspace", "list"]);
    assert!(String::from_utf8_lossy(&text.stdout).contains(&workspace), "{}", String::from_utf8_lossy(&text.stdout));
    let info = serve.ok(&["workspace", "info", &workspace, "--json"]);
    assert_eq!(info["result"]["workspace_id"], workspace.as_str());

    let closed = serve.ok(&["workspace", "close", &workspace, "--json"]);
    assert_eq!(closed["ok"], true, "{closed}");
    let listed = serve.ok(&["workspace", "list", "--json"]);
    let ids: Vec<&str> = listed["result"]["workspaces"].as_array().unwrap().iter().filter_map(|w| w["workspace_id"].as_str()).collect();
    assert!(!ids.contains(&workspace.as_str()), "{ids:?}");
}

#[test]
fn thread_start_wait_completes_with_a_fixture_provider() {
    let serve = Serve::start();
    let (workspace, _) = serve.workspace("project");
    // Providers register in the background after boot. A configured Cursor
    // is enough: the launch checks the permission mode against its catalog.
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let probe = serve.cli(&["call", "agent_capabilities", &json!({"workspace_id": workspace, "provider": "cursor"}).to_string()]);
        let caps = stdout_json(&probe);
        if caps["result"]["providers"][0]["configured"] == true {
            break;
        }
        // Only a provider still starting is worth waiting for.
        let code = caps["error"]["code"].as_str().unwrap_or("provider_not_ready");
        assert!(matches!(code, "provider_not_configured" | "provider_not_ready"), "{caps}");
        assert!(Instant::now() < deadline, "Cursor never registered: {caps}\n{}", serve.log());
        std::thread::sleep(Duration::from_millis(500));
    }

    let start = |extra: &[&str]| {
        let mut args = vec!["--client", "itest", "thread", "start", "--workspace", &workspace, "--provider", "cursor",
            "--permission-mode", "agent", "-m", "echo-prompt PONG", "--request-id", "itest-launch", "--json"];
        args.extend_from_slice(extra);
        serve.cli(&args)
    };
    let waited = start(&["--wait", "--timeout", "2m"]);
    assert_eq!(exit_code(&waited), 0, "{}\n{}\n{}", String::from_utf8_lossy(&waited.stdout), String::from_utf8_lossy(&waited.stderr), serve.log());
    let report = stdout_json(&waited);
    assert_eq!((report["ok"].clone(), report["state"].clone()), (json!(true), json!("done")), "{report}");
    assert!(report["final_text"].as_str().unwrap_or_default().contains("PONG"), "{report}");
    let thread = report["thread_id"].as_str().expect("a thread id").to_owned();
    assert_eq!(report["client_request_id"], "itest-launch");

    // Retrying the same key returns the same thread, not a second one.
    let retried = start(&[]);
    assert_eq!(exit_code(&retried), 0);
    assert_eq!(stdout_json(&retried)["result"]["thread_id"], thread.as_str());

    let page = serve.ok(&["--client", "itest", "thread", "read", &thread, "--workspace", &workspace, "--all", "--json"]);
    assert!(page["result"]["messages"].as_array().is_some_and(|messages| !messages.is_empty()), "{page}");
    let status = serve.ok(&["--client", "itest", "thread", "status", &thread, "--workspace", &workspace, "--json"]);
    assert_eq!(status["result"]["state"], "done", "{status}");
    // A finished run's wait returns at once.
    let again = serve.ok(&["--client", "itest", "thread", "wait", &thread, "--workspace", &workspace, "--json"]);
    assert_eq!(again["state"], "done", "{again}");
    let stopped = serve.ok(&["--client", "itest", "thread", "stop", &thread, "--workspace", &workspace, "--json"]);
    assert_eq!(stopped["ok"], true, "{stopped}");
}

#[test]
fn connector_status_enable_and_disable_round_trip() {
    let serve = Serve::start();
    let status = serve.ok(&["connector", "status", "--json"]);
    assert_eq!((status["tool"].clone(), status["result"]["enabled"].clone()), (json!("agent_connector_status"), json!(false)));
    // Integration builds default to the real 4378; never take it here.
    let port = free_loopback_port();
    let enabled = serve.ok(&["connector", "enable", "--port", &port.to_string(), "--json"]);
    assert_eq!(enabled["result"]["enabled"], true, "{enabled}");
    assert_eq!(enabled["result"]["port"], port, "{enabled}");
    assert_eq!(enabled["result"]["localUrl"], format!("http://127.0.0.1:{port}/mcp"), "{enabled}");
    let listed = serve.ok(&["connector", "list", "--json"]);
    assert!(listed["result"]["clients"].is_array(), "{listed}");
    let pending = serve.ok(&["connector", "pending", "--json"]);
    assert!(pending["result"]["pending"].is_array(), "{pending}");
    let disabled = serve.ok(&["connector", "disable", "--json"]);
    assert_eq!(disabled["result"]["enabled"], false, "{disabled}");

    // Revoking a name that matches nothing is refused, not a silent success.
    let unknown = serve.cli(&["connector", "revoke", "nobody", "--json"]);
    assert_eq!(exit_code(&unknown), 1);
    assert_eq!(stdout_json(&unknown)["error"]["code"], "invalid_arguments");
}

#[test]
fn connector_tokens_issue_list_and_revoke() {
    let serve = Serve::start();
    // Its own port, so the door never touches the default 4378.
    serve.ok(&["connector", "enable", "--port", &free_loopback_port().to_string(), "--json"]);
    let issued = serve.ok(&["connector", "issue", "--label", "itest-bot", "--expires", "1d", "--json"]);
    let token = issued["result"]["token"].as_str().expect("a token");
    assert!(token.starts_with("cmcp_"), "{issued}");
    assert_eq!(issued["result"]["access"], "supervised", "tokens default to supervised");

    let text = serve.cli(&["connector", "list"]);
    assert!(String::from_utf8_lossy(&text.stdout).contains("itest-bot"), "{}", String::from_utf8_lossy(&text.stdout));
    serve.ok(&["connector", "revoke", "ITEST-BOT", "--json"]);
    let listed = serve.ok(&["connector", "list", "--json"]);
    assert!(!listed.to_string().contains("itest-bot"), "{listed}");

    serve.ok(&["connector", "issue", "--label", "second-bot", "--access", "read-only", "--json"]);
    serve.ok(&["connector", "issue", "--label", "third-bot", "--json"]);
    assert_eq!(serve.ok(&["connector", "list", "--json"])["result"]["clients"].as_array().map(Vec::len), Some(2));
    serve.ok(&["connector", "revoke", "--all", "--json"]);
    let status = serve.ok(&["connector", "status", "--json"]);
    assert_eq!(status["result"]["clients"], json!([]), "{status}");
    assert_eq!(status["result"]["enabled"], true, "revoke --all leaves the door on: {status}");
}

/// Settings and the guide print setup commands from these two fields.
#[test]
fn connector_status_names_the_executable_outside_assistants_launch() {
    let serve = Serve::start();
    let status = serve.ok(&["connector", "status", "--json"])["result"].clone();
    let executable = status["localExecutable"].as_str().expect("localExecutable");
    // PATH reaches no codemux, or another install, never this test build, so
    // it names the binary itself.
    assert!(Path::new(executable).is_absolute(), "{status}");
    assert_eq!(std::fs::canonicalize(executable).unwrap(), std::fs::canonicalize(CODEMUX).unwrap());
    let command = status["localCommand"].as_str().expect("localCommand");
    assert!(command.ends_with(" mcp") && command.contains(executable), "{status}");
}
