//! The outside-agent contract end to end: a real Codemux (the headless
//! `codemux serve` runtime, booted in this process) driven through every
//! outside door the guide documents. Those are `codemux mcp` over stdio, the
//! agent connector over HTTP and the CLI verbs. The fake Cursor and Codex
//! providers stand in for real ones, so the suite needs no credentials and
//! runs in CI.
//!
//! One app per process: booting twice would rebind the control socket and
//! share global registries. Each test takes its own workspaces, client names,
//! labels and request keys from [`tag`], so the tests run in parallel.
//!
//! Isolation: the process environment is repointed before the app boots, and
//! every child starts from a cleared environment plus [`ChildEnv`]. Neither
//! the developer's running Codemux nor the agent identity of a chat this
//! suite runs under can leak in. The control socket lives in its own short
//! `XDG_RUNTIME_DIR`, so no child ever falls back to `/run/user/<uid>`.
//!
//! Unix-only: `tauri::test` needs WebView2Loader.dll at process startup on
//! Windows (same gate as the other headless-serve tests).

#![cfg(unix)]

use codemux_lib::agent_control::ControlAccess;
use codemux_lib::mcp_connector;
use serde_json::{json, Value};
use std::collections::{BTreeSet, HashMap};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitStatus, Output, Stdio};
use std::sync::mpsc;
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use tauri::test::MockRuntime;
use tauri::AppHandle;

const CODEMUX: &str = env!("CARGO_BIN_EXE_codemux");
const FAKE_CURSOR: &str = env!("CARGO_BIN_EXE_fake_cursor_acp");
const FAKE_CODEX: &str = env!("CARGO_BIN_EXE_fake_codex_app_server");
/// The connector speaks this revision after initialize (any supported one works).
const HTTP_PROTOCOL: &str = "2025-06-18";
/// How long a fake turn or a receipt may take to settle on a loaded CI runner.
const SETTLE: Duration = Duration::from_secs(30);
/// D13: parked HTTP waits per connector client.
const WAITS_PER_CLIENT: usize = 8;

/// The 62 legacy stdio tools in-app agents see as `mcp__codemux__*`
/// (`mcp_server::LEGACY_TOOL_NAMES`). Provider-origin and agent-marked
/// `codemux mcp` must keep exactly these.
const LEGACY_TOOL_NAMES: [&str; 62] = [
    "delegation_targets", "delegate_remote_task", "task_status", "task_cancel",
    "browser_navigate", "browser_snapshot", "browser_accessibility_snapshot", "browser_click", "browser_fill",
    "browser_screenshot", "browser_console_logs", "browser_click_at", "browser_type_at", "browser_scroll_at",
    "browser_key_press", "browser_drag", "browser_click_os", "browser_type_os", "browser_get_styles", "browser_wait",
    "browser_evaluate", "browser_viewport", "browser_viewport_presets",
    "workspace_list", "workspace_info", "workspace_create",
    "automation_list", "automation_get", "automation_create", "automation_update", "automation_delete",
    "automation_pause", "automation_resume", "automation_runs",
    "pane_list", "pane_split_right", "pane_split_down", "conversation_read", "conversation_search", "notify",
    "git_status", "git_diff", "git_stage", "git_commit", "git_push", "terminal_write", "terminal_read",
    "workspace_open", "app_status", "port_list", "worktree_create", "preset_apply", "preset_list",
    "workspace_close", "workspace_archive", "workspace_unarchive", "workspace_archive_list", "pane_close",
    "issue_list", "issue_get", "issue_link_workspace", "html_render",
];

// ── The world ─────────────────────────────────────────────────────────────

/// The allow-listed environment every child process gets, after `env_clear`.
struct ChildEnv {
    vars: Vec<(String, String)>,
    logs: PathBuf,
}

impl ChildEnv {
    fn command(&self, program: &str, extra: &[(&str, &str)]) -> Command {
        let mut command = Command::new(program);
        command.env_clear().envs(self.vars.iter().map(|(k, v)| (k, v))).envs(extra.iter().copied());
        command
    }

    /// `codemux <args>` with this env plus `extra`, stdin closed.
    fn cli(&self, args: &[&str], extra: &[(&str, &str)]) -> Output {
        self.command(CODEMUX, extra).args(args).stdin(Stdio::null()).output().expect("run the codemux CLI")
    }
}

struct World {
    handle: AppHandle<MockRuntime>,
    root: PathBuf,
    env: ChildEnv,
    /// A git repository whose default branch is `trunk`, not `main`.
    repo: PathBuf,
    /// The repository opened as a workspace by the fixture client, which
    /// makes it a project connector clients may branch.
    repo_workspace: String,
    /// The connector's `/mcp` URL on its own loopback port.
    mcp_url: String,
}

static WORLD: OnceLock<World> = OnceLock::new();

fn world() -> &'static World {
    WORLD.get_or_init(boot)
}

/// Process environment the app must not inherit: identities and endpoints of
/// whatever runs this suite, provider homes and keys, and displays.
fn inherited(key: &str) -> bool {
    const PREFIXES: [&str; 12] =
        ["CODEMUX", "CLAUDE", "HERMES", "CODEX", "CURSOR", "OPENAI", "ANTHROPIC", "XDG_", "GIT_", "MCP_", "GH_", "GITHUB_"];
    PREFIXES.iter().any(|prefix| key.starts_with(prefix))
        || matches!(key, "DISPLAY" | "WAYLAND_DISPLAY" | "DBUS_SESSION_BUS_ADDRESS")
}

/// The fake Codex's script: its first turn asks one question, and the answer
/// (a `turn/steer`) finishes that turn with a reply.
fn codex_script() -> Value {
    json!([
        {"after": "turn/start", "on_turn": 1, "emit": "notification", "method": "turn/started",
         "params": {"threadId": "c-1", "turnId": "t-1"}},
        {"after": "turn/start", "on_turn": 1, "delay_ms": 10, "emit": "notification", "method": "item/completed",
         "params": {"threadId": "c-1", "turnId": "t-1", "item": {"type": "agentMessage", "id": "q-1", "delivery": "async", "text": "",
             "questions": [{"title": "Which storage?", "options": ["SQLite", "PostgreSQL"]}]}}},
        {"after": "turn/steer", "emit": "notification", "method": "item/completed",
         "params": {"threadId": "c-1", "turnId": "t-1", "item": {"type": "agentMessage", "id": "after", "text": "Using SQLite."}}},
        {"after": "turn/steer", "delay_ms": 20, "emit": "notification", "method": "turn/completed",
         "params": {"threadId": "c-1", "turnId": "t-1", "status": "completed"}}
    ])
}

fn free_loopback_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn boot() -> World {
    // Under target/tmp (cargo clean removes it), kept for post-mortem.
    let root = tempfile::Builder::new()
        .prefix("outside-agent-control-")
        .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
        .unwrap()
        .keep()
        .canonicalize()
        .unwrap();
    // The socket path must fit `sun_path`, so it gets a short dir of its own.
    let runtime = tempfile::Builder::new().prefix("oac-").tempdir().unwrap().keep();
    let bin = root.join("bin");
    let home = root.join("home");
    let logs = root.join("logs");
    for dir in [&bin, &home, &logs] {
        std::fs::create_dir_all(dir).unwrap();
    }
    // Codemux finds the providers on PATH. Symlinks, not copies: copy-then-exec
    // races ETXTBSY.
    std::os::unix::fs::symlink(FAKE_CURSOR, bin.join("cursor-agent")).unwrap();
    std::os::unix::fs::symlink(FAKE_CODEX, bin.join("codex")).unwrap();
    let script = root.join("codex-script.json");
    std::fs::write(&script, codex_script().to_string()).unwrap();
    std::fs::write(
        home.join(".gitconfig"),
        "[user]\n\tname = Outside Agent Control\n\temail = outside@example.invalid\n[init]\n\tdefaultBranch = trunk\n",
    )
    .unwrap();

    let path = |sub: &str| {
        let dir = root.join(sub);
        std::fs::create_dir_all(&dir).unwrap();
        dir.display().to_string()
    };
    let vars: Vec<(String, String)> = [
        ("HOME", home.display().to_string()),
        ("XDG_CONFIG_HOME", path("config")),
        ("XDG_DATA_HOME", path("data")),
        ("XDG_STATE_HOME", path("state")),
        ("XDG_CACHE_HOME", path("cache")),
        ("XDG_RUNTIME_DIR", runtime.display().to_string()),
        ("PATH", format!("{}:/usr/bin:/bin", bin.display())),
        ("LANG", "C.UTF-8".into()),
        // Terminals start in process and say nothing; nothing reaches the account API.
        ("SHELL", "/bin/true".into()),
        ("CODEMUX_DISABLE_PTY_DAEMON", "1".into()),
        ("CODEMUX_API_URL", "http://127.0.0.1:1".into()),
        ("GIT_CONFIG_NOSYSTEM", "1".into()),
        // The fake Cursor's `exec-codemux` runs this binary.
        ("CODEMUX_TEST_BIN", CODEMUX.into()),
        ("FAKE_CODEX_SCRIPT", script.display().to_string()),
        ("FAKE_CODEX_ASYNC_MODE", "running".into()),
    ]
    .into_iter()
    .map(|(key, value)| (key.to_string(), value))
    .collect();
    for (key, _) in std::env::vars_os() {
        if key.to_str().is_some_and(inherited) {
            std::env::remove_var(&key);
        }
    }
    for (key, value) in &vars {
        std::env::set_var(key, value);
    }
    // The app opens a default workspace where it starts: keep that out of
    // the source checkout (no indexing, no PR polling of this repository).
    std::env::set_current_dir(&home).unwrap();
    let env = ChildEnv { vars, logs };

    let repo = root.join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-q", "-b", "trunk"]);
    std::fs::write(repo.join("README.md"), "# outside agent control\n").unwrap();
    git(&repo, &["add", "README.md"]);
    git(&repo, &["commit", "-q", "-m", "seed"]);

    // The connector, on its own free loopback port, is bound by the app's
    // setup from the saved config.
    let port = free_loopback_port();
    {
        let db = codemux_lib::database::init_database().expect("open the isolated store");
        db.mcp_connector_set_config(&format!(r#"{{"enabled":true,"port":{port}}}"#)).expect("enable the connector");
    }
    let app = codemux_lib::build_headless_app().expect("boot the headless app");
    let handle = app.handle().clone();
    // The app lives for the whole process; only the handle is kept.
    Box::leak(Box::new(app));

    eventually("the control socket answers", Duration::from_secs(30), || env.cli(&["json", "status", "{}"], &[]).status.success());
    // Setup bound the saved port. Something may have taken it since it was
    // probed; then move the connector to another free port (the bind is
    // synchronous, so the status says at once whether it listens).
    let mut status = mcp_connector::agent_connector_status(handle.clone()).expect("the connector's status");
    for _ in 0..8 {
        if status.listener_running {
            break;
        }
        status = mcp_connector::agent_connector_set_config(handle.clone(), true, None, Some(free_loopback_port()))
            .expect("move the connector to a free port");
    }
    assert!(status.listener_running, "the connector never bound a free loopback port: {:?}", status.bind_error);
    let mcp_url = status.local_url.expect("the connector's local URL");

    let mut fixture = StdioDoor::named(&env, "fixture", None);
    let opened = fixture
        .call("workspace_create", json!({"path": repo, "client_request_id": "fixture-repo"}))
        .expect("open the repository as a workspace");
    let repo_workspace = opened["workspace_id"].as_str().expect("a workspace id").to_owned();
    // Providers register in the background after boot.
    eventually("Cursor is configured", Duration::from_secs(60), || {
        let probe = fixture.call("agent_capabilities", json!({"workspace_id": repo_workspace, "provider": "cursor"}));
        match probe {
            Ok(caps) => caps["providers"][0]["configured"] == true,
            Err(error) => {
                assert!(matches!(error.code.as_str(), "provider_not_configured" | "provider_not_ready"), "{error:?}");
                false
            }
        }
    });
    assert!(fixture.close().success());
    World { handle, root, env, repo, repo_workspace, mcp_url }
}

impl World {
    fn cli(&self, args: &[&str], extra: &[(&str, &str)]) -> Output {
        self.env.cli(args, extra)
    }

    /// A CLI command that must exit 0 and print one JSON value.
    fn cli_ok(&self, args: &[&str]) -> Value {
        let output = self.cli(args, &[]);
        assert_eq!(exit_code(&output), 0, "`codemux {}`: {}", args.join(" "), describe(&output));
        stdout_json(&output)
    }

    /// `get_app_state`'s focused workspace.
    fn active_workspace(&self) -> Option<String> {
        let state = self.cli_ok(&["json", "get_app_state", "{}"]);
        id_text(&state["data"]["active_workspace_id"])
    }

    fn open_workspace_ids(&self) -> Vec<String> {
        let state = self.cli_ok(&["json", "get_app_state", "{}"]);
        state["data"]["workspaces"].as_array().into_iter().flatten().filter_map(|w| id_text(&w["workspace_id"])).collect()
    }

    /// A fresh plain folder opened as a workspace by the fixture client.
    fn fresh_workspace(&self, name: &str, cleanup: &mut Cleanup) -> String {
        let folder = self.root.join("folders").join(name);
        std::fs::create_dir_all(&folder).unwrap();
        let mut fixture = StdioDoor::named(&self.env, "fixture", None);
        let opened = fixture
            .call("workspace_create", json!({"path": folder, "client_request_id": format!("folder-{name}")}))
            .expect("open a folder as a workspace");
        let workspace = opened["workspace_id"].as_str().unwrap().to_owned();
        cleanup.workspaces.push(workspace.clone());
        workspace
    }

    /// The trunk commit every new worktree starts from.
    fn trunk(&self) -> String {
        git(&self.repo, &["rev-parse", "trunk"])
    }

    /// A personal token, issued the way Settings does.
    fn issue(&self, label: &str, access: ControlAccess) -> Issued {
        let issued = mcp_connector::agent_connector_issue(self.handle.clone(), label.into(), access, Some(24))
            .expect("issue a personal token");
        Issued { token: issued.token, client_id: issued.client_id }
    }

    fn http(&self, issued: &Issued) -> HttpDoor {
        HttpDoor::new(&self.mcp_url, &issued.token, format!("mcp-client:{}", issued.client_id))
    }

    /// Rows of a read-only query on the app's store.
    fn sql(&self, query: &str, params: &[&dyn rusqlite::ToSql]) -> Vec<serde_json::Map<String, Value>> {
        let path = self.root.join("config").join(codemux_lib::APP_DIR_NAME).join("codemux.db");
        let db = rusqlite::Connection::open_with_flags(&path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).expect("open the store read-only");
        let mut statement = db.prepare(query).expect("prepare");
        let names: Vec<String> = statement.column_names().into_iter().map(str::to_owned).collect();
        let rows = statement
            .query_map(params, |row| {
                let mut map = serde_json::Map::new();
                for (index, name) in names.iter().enumerate() {
                    let value = match row.get_ref(index)? {
                        rusqlite::types::ValueRef::Null => Value::Null,
                        rusqlite::types::ValueRef::Integer(n) => json!(n),
                        rusqlite::types::ValueRef::Real(n) => json!(n),
                        rusqlite::types::ValueRef::Text(text) => json!(String::from_utf8_lossy(text)),
                        rusqlite::types::ValueRef::Blob(_) => Value::Null,
                    };
                    map.insert(name.clone(), value);
                }
                Ok(map)
            })
            .expect("query");
        rows.map(Result::unwrap).collect()
    }

    fn sessions_in(&self, workspace: &str) -> i64 {
        self.sql("SELECT COUNT(*) AS n FROM agent_chat_sessions WHERE workspace_id = ?1", &[&workspace])[0]["n"].as_i64().unwrap()
    }
}

struct Issued {
    token: String,
    client_id: String,
}

// ── Small helpers ─────────────────────────────────────────────────────────

/// 8 hex characters, unique per call: names, labels and keys of one test.
fn tag() -> String {
    uuid::Uuid::new_v4().simple().to_string()[..8].to_owned()
}

fn eventually(what: &str, within: Duration, mut done: impl FnMut() -> bool) {
    let deadline = Instant::now() + within;
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting until {what}");
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git").arg("-C").arg(dir).args(args).output().expect("run git");
    assert!(output.status.success(), "git {args:?} in {}: {}", dir.display(), String::from_utf8_lossy(&output.stderr));
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn exit_code(output: &Output) -> i32 {
    output.status.code().expect("the CLI exited on its own")
}

fn describe(output: &Output) -> String {
    format!("exit {:?}\nstdout: {}\nstderr: {}", output.status.code(), String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr))
}

fn stdout_json(output: &Output) -> Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| panic!("stdout is not one JSON value ({error}): {}", describe(output)))
}

/// A workspace id, serialized bare or as a newtype object.
fn id_text(value: &Value) -> Option<String> {
    value.as_str().or_else(|| value["0"].as_str()).map(str::to_owned)
}

fn text(value: &Value) -> &str {
    value.as_str().unwrap_or_default()
}

fn names(tools: &[Value]) -> Vec<String> {
    tools.iter().filter_map(|tool| tool["name"].as_str().map(str::to_owned)).collect()
}

fn unique(names: &[String]) -> bool {
    names.iter().collect::<BTreeSet<_>>().len() == names.len()
}

/// Whether a git repository has a worktree checked out on `branch`.
fn worktrees_on(repo: &Path, branch: &str) -> usize {
    git(repo, &["worktree", "list", "--porcelain"]).lines().filter(|line| *line == format!("branch refs/heads/{branch}")).count()
}

#[derive(Debug, Clone, PartialEq)]
struct ToolError {
    code: String,
    message: String,
}

/// One door's `tools/call` result, the same shape on every door: `isError`,
/// and `structuredContent` (`error {code, message}` on failure). Legacy stdio
/// tools answer text only, `code: message` on failure.
fn tool_result(result: &Value) -> Result<Value, ToolError> {
    let text = result["content"][0]["text"].as_str().unwrap_or_default();
    if result["isError"] == true {
        let error = &result["structuredContent"]["error"];
        if let Some(code) = error["code"].as_str() {
            return Err(ToolError { code: code.into(), message: error["message"].as_str().unwrap_or_default().into() });
        }
        let bare = text.trim_start_matches("Error: ");
        let (code, message) = bare.split_once(": ").unwrap_or(("", bare));
        return Err(ToolError { code: code.into(), message: message.into() });
    }
    Ok(match &result["structuredContent"] {
        Value::Null => serde_json::from_str(text).unwrap_or_else(|_| json!(text)),
        structured => structured.clone(),
    })
}

/// A JSON-RPC error a door answered instead of a tool result.
fn rpc_error(reply: &Value) -> ToolError {
    ToolError { code: format!("rpc:{}", reply["error"]["code"]), message: reply["error"]["message"].as_str().unwrap_or_default().into() }
}

// ── Doors ─────────────────────────────────────────────────────────────────

/// One MCP door. Both doors return the same result shape.
trait Door {
    /// The whole JSON-RPC reply to `tools/call`.
    fn call_raw(&mut self, tool: &str, arguments: Value) -> Value;
    fn list(&mut self) -> Vec<Value>;
    fn principal(&self) -> &str;
    /// How often `settle` polls: HTTP shares a per-minute budget.
    fn poll(&self) -> Duration;

    fn call(&mut self, tool: &str, arguments: Value) -> Result<Value, ToolError> {
        let reply = self.call_raw(tool, arguments);
        if reply.get("error").is_some_and(|error| !error.is_null()) {
            return Err(rpc_error(&reply));
        }
        tool_result(&reply["result"])
    }

    /// A call that must succeed.
    fn ok(&mut self, tool: &str, arguments: Value) -> Value {
        let principal = self.principal().to_owned();
        self.call(tool, arguments).unwrap_or_else(|error| panic!("{principal} {tool}: {error:?}"))
    }

    /// A call that must fail; returns its code.
    fn refused(&mut self, tool: &str, arguments: Value) -> ToolError {
        let principal = self.principal().to_owned();
        match self.call(tool, arguments) {
            Ok(value) => panic!("{principal} {tool} succeeded: {value}"),
            Err(error) => error,
        }
    }

    /// Polls `operation_status` until the receipt is no longer accepted or running.
    fn settle(&mut self, receipt: &Value) -> Value {
        let id = receipt["operation_id"].as_str().unwrap_or_else(|| panic!("not a receipt: {receipt}")).to_owned();
        let deadline = Instant::now() + SETTLE;
        let mut current = receipt.clone();
        while matches!(text(&current["state"]), "accepted" | "running") {
            assert!(Instant::now() < deadline, "operation {id} never settled: {current}");
            std::thread::sleep(self.poll());
            current = self.ok("operation_status", json!({"operation_id": id}));
        }
        current
    }

    /// Runs a keyed thread mutation and settles it; it must succeed.
    fn run(&mut self, tool: &str, arguments: Value) -> Value {
        let receipt = self.ok(tool, arguments);
        let settled = self.settle(&receipt);
        assert_eq!(settled["state"], "succeeded", "{tool}: {settled}");
        settled
    }

    /// One `thread_wait`.
    fn wait(&mut self, workspace: &str, thread: &str, turn: Option<&str>, timeout_ms: u64) -> Value {
        let mut arguments = json!({"workspace_id": workspace, "thread_id": thread, "timeout_ms": timeout_ms});
        if let Some(turn) = turn {
            arguments["turn_id"] = json!(turn);
        }
        self.ok("thread_wait", arguments)
    }

    /// Waits until the run is no longer running.
    fn finish(&mut self, workspace: &str, thread: &str, turn: Option<&str>) -> Value {
        let deadline = Instant::now() + SETTLE;
        loop {
            let waited = self.wait(workspace, thread, turn, 20_000);
            if waited["state"] != "running" {
                return waited;
            }
            assert!(Instant::now() < deadline, "the run never finished: {waited}");
        }
    }
}

/// The run a settled launch or send receipt started, if it names one.
fn turn_of(receipt: &Value) -> Option<String> {
    receipt["result"]["turn"]["turn_id"].as_str().filter(|turn| !turn.is_empty()).map(str::to_owned)
}

/// `codemux mcp` as an outside client spawns it.
struct StdioDoor {
    child: Child,
    stdin: Option<ChildStdin>,
    rx: mpsc::Receiver<Value>,
    early: HashMap<i64, Value>,
    next_id: i64,
    principal: String,
    log: PathBuf,
    initialized: Value,
}

impl StdioDoor {
    /// `codemux mcp --client <client> [--access <access>]`.
    fn named(env: &ChildEnv, client: &str, access: Option<&str>) -> Self {
        let mut args = vec!["mcp", "--client", client];
        if let Some(access) = access {
            args.extend(["--access", access]);
        }
        Self::spawn(env, &args, &[], &format!("local:{client}"))
    }

    /// Runs `codemux <args>`, then `initialize` and `notifications/initialized`.
    fn spawn(env: &ChildEnv, args: &[&str], extra: &[(&str, &str)], principal: &str) -> Self {
        let log = env.logs.join(format!("stdio-{}-{}.log", principal.replace(':', "_"), tag()));
        let mut child = env
            .command(CODEMUX, extra)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(std::fs::File::create(&log).unwrap())
            .spawn()
            .expect("spawn codemux mcp");
        let stdout = child.stdout.take().unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { return };
                if let Ok(value) = serde_json::from_str::<Value>(&line) {
                    if tx.send(value).is_err() {
                        return;
                    }
                }
            }
        });
        let stdin = child.stdin.take();
        let mut door = Self { child, stdin, rx, early: HashMap::new(), next_id: 0, principal: principal.into(), log, initialized: Value::Null };
        let id = door.send("initialize", json!({"protocolVersion": "2025-06-18", "capabilities": {},
            "clientInfo": {"name": "outside-agent-control", "version": "1"}}));
        door.initialized = door.response(id, Duration::from_secs(20));
        door.notify("notifications/initialized", json!({}));
        door
    }

    fn write(&mut self, message: &Value) {
        let stdin = self.stdin.as_mut().expect("stdin is open");
        writeln!(stdin, "{message}").expect("write to codemux mcp");
        stdin.flush().unwrap();
    }

    /// Sends a request without waiting for its answer.
    fn send(&mut self, method: &str, params: Value) -> i64 {
        self.next_id += 1;
        let id = self.next_id;
        self.write(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        id
    }

    fn notify(&mut self, method: &str, params: Value) {
        self.write(&json!({"jsonrpc": "2.0", "method": method, "params": params}));
    }

    /// The answer to `id`, whatever order answers arrive in.
    fn response(&mut self, id: i64, within: Duration) -> Value {
        if let Some(reply) = self.early.remove(&id) {
            return reply;
        }
        let deadline = Instant::now() + within;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.rx.recv_timeout(left) {
                Ok(reply) => match reply["id"].as_i64() {
                    Some(seen) if seen == id => return reply,
                    Some(seen) => {
                        self.early.insert(seen, reply);
                    }
                    None => {}
                },
                Err(_) => panic!("{} gave no answer to request {id} within {within:?}; stderr:\n{}", self.principal, self.stderr()),
            }
        }
    }

    /// Whether an answer to `id` has arrived, without waiting.
    fn answered(&mut self, id: i64) -> bool {
        while let Ok(reply) = self.rx.try_recv() {
            if let Some(seen) = reply["id"].as_i64() {
                self.early.insert(seen, reply);
            }
        }
        self.early.contains_key(&id)
    }

    fn stderr(&self) -> String {
        let log = std::fs::read_to_string(&self.log).unwrap_or_default();
        log.lines().rev().take(40).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("\n")
    }

    /// Ends stdin (the client hung up) and waits for the server to exit.
    fn close(mut self) -> ExitStatus {
        self.stdin.take();
        self.wait_exit(Duration::from_secs(5)).unwrap_or_else(|| panic!("{} did not exit after EOF; stderr:\n{}", self.principal, self.stderr()))
    }

    fn wait_exit(&mut self, within: Duration) -> Option<ExitStatus> {
        let deadline = Instant::now() + within;
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return Some(status);
            }
            if Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for StdioDoor {
    fn drop(&mut self) {
        // This test started the child; a failing test must not leave it running.
        self.stdin.take();
        if self.wait_exit(Duration::from_secs(5)).is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

impl Door for StdioDoor {
    fn call_raw(&mut self, tool: &str, arguments: Value) -> Value {
        let id = self.send("tools/call", json!({"name": tool, "arguments": arguments}));
        self.response(id, Duration::from_secs(90))
    }

    fn list(&mut self) -> Vec<Value> {
        let id = self.send("tools/list", json!({}));
        let reply = self.response(id, Duration::from_secs(20));
        reply["result"]["tools"].as_array().cloned().unwrap_or_else(|| panic!("tools/list: {reply}"))
    }

    fn principal(&self) -> &str {
        &self.principal
    }

    fn poll(&self) -> Duration {
        Duration::from_millis(50)
    }
}

/// The connector's `/mcp` with a bearer token. Cheap to clone, so parallel
/// waits can share one.
#[derive(Clone)]
struct HttpDoor {
    client: reqwest::blocking::Client,
    url: String,
    token: String,
    principal: String,
}

impl HttpDoor {
    fn new(url: &str, token: &str, principal: String) -> Self {
        let client = reqwest::blocking::Client::builder().timeout(Duration::from_secs(120)).build().unwrap();
        let door = Self { client, url: url.into(), token: token.into(), principal };
        let (status, init) = door.rpc("initialize", json!({"protocolVersion": HTTP_PROTOCOL, "capabilities": {},
            "clientInfo": {"name": "outside-agent-control", "version": "1"}}));
        assert_eq!(status, 200, "initialize: {init}");
        door
    }

    fn post(&self, body: &Value) -> reqwest::blocking::Response {
        self.client
            .post(&self.url)
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .header("mcp-protocol-version", HTTP_PROTOCOL)
            .bearer_auth(&self.token)
            .json(body)
            .send()
            .expect("connector request")
    }

    /// One JSON-RPC request: the HTTP status and the reply (null when not JSON).
    fn rpc(&self, method: &str, params: Value) -> (u16, Value) {
        let response = self.post(&json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}));
        let status = response.status().as_u16();
        (status, response.json().unwrap_or(Value::Null))
    }

    fn ping(&self) -> u16 {
        self.rpc("ping", json!({})).0
    }
}

impl Door for HttpDoor {
    fn call_raw(&mut self, tool: &str, arguments: Value) -> Value {
        let (status, reply) = self.rpc("tools/call", json!({"name": tool, "arguments": arguments}));
        assert_eq!(status, 200, "{} {tool}: HTTP {status} {reply}", self.principal);
        reply
    }

    fn list(&mut self) -> Vec<Value> {
        let (status, reply) = self.rpc("tools/list", json!({}));
        assert_eq!(status, 200, "tools/list: {reply}");
        reply["result"]["tools"].as_array().cloned().unwrap_or_else(|| panic!("tools/list: {reply}"))
    }

    fn principal(&self) -> &str {
        &self.principal
    }

    fn poll(&self) -> Duration {
        Duration::from_millis(200)
    }
}

/// Best-effort owner cleanup when a test ends, passed or not: stop its
/// threads, then close its workspaces (removing disposable worktrees), so a
/// failed test leaks nothing into the others.
#[derive(Default)]
struct Cleanup {
    threads: Vec<(String, String)>,
    /// Plain folders: closed, files kept.
    workspaces: Vec<String>,
    /// Disposable worktrees: closed and removed.
    worktrees: Vec<String>,
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        let Some(world) = WORLD.get() else { return };
        let call = |tool: &str, mut args: Value| {
            args["client_request_id"] = json!(format!("cleanup-{}", tag()));
            let _ = world.cli(&["call", tool, &args.to_string()], &[]);
        };
        for (workspace, thread) in &self.threads {
            call("thread_stop", json!({"workspace_id": workspace, "thread_id": thread}));
        }
        for workspace in &self.worktrees {
            call("workspace_close", json!({"workspace_id": workspace, "delete_worktree": true, "force_delete": true}));
        }
        for workspace in &self.workspaces {
            call("workspace_close", json!({"workspace_id": workspace}));
        }
    }
}

/// Launches a Cursor thread and settles its receipt: the thread and its run.
fn launch(door: &mut dyn Door, workspace: &str, mode: &str, message: &str, key: &str) -> (String, Option<String>) {
    let receipt = door.ok("thread_launch", json!({"workspace_id": workspace, "provider": "cursor", "permission_mode": mode,
        "title": "Outside agent control", "message": message, "client_request_id": key}));
    let settled = door.settle(&receipt);
    assert_eq!(settled["state"], "succeeded", "{settled}");
    (text(&settled["thread_id"]).to_owned(), turn_of(&settled))
}

/// Sends a follow-up and settles it; returns the run it started.
fn send(door: &mut dyn Door, workspace: &str, thread: &str, message: &str, key: &str) -> Option<String> {
    let settled = door.run("thread_send", json!({"workspace_id": workspace, "thread_id": thread, "message": message, "client_request_id": key}));
    turn_of(&settled)
}

/// The thread's status once `turn` (else any run) has begun at the provider.
/// A launch or send receipt can succeed before a queued adapter dispatches
/// the run. The run's start changes the wait cursor, so a wait parked before
/// it would answer then instead of staying parked.
fn until_started(door: &mut dyn Door, workspace: &str, thread: &str, turn: Option<&str>) -> Value {
    let deadline = Instant::now() + SETTLE;
    loop {
        let status = door.ok("thread_status", json!({"workspace_id": workspace, "thread_id": thread}));
        let current = status["turn_id"].as_str();
        if current.is_some() && (turn.is_none() || current == turn) {
            return status;
        }
        assert!(Instant::now() < deadline, "the run never started: {status}");
        std::thread::sleep(door.poll());
    }
}

/// A `thread_wait` meant to park. A probe of the same client can hold the
/// last wait slot for a moment, so a refusal for the waiter budget is
/// retried, for a bounded time.
fn parked_wait(door: &mut dyn Door, wait: &Value) -> Result<Value, ToolError> {
    let deadline = Instant::now() + SETTLE;
    loop {
        match door.call("thread_wait", wait.clone()) {
            Err(error) if error.code == "wait_budget_exhausted" && Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            result => return result,
        }
    }
}

/// The thread's status once its run needs input. A wait without `since`
/// does not return for input already pending when it starts, so this reads
/// the status first and then waits for the next change, as the guide says.
fn until_input(door: &mut dyn Door, workspace: &str, thread: &str) -> Value {
    let deadline = Instant::now() + SETTLE;
    loop {
        let status = door.ok("thread_status", json!({"workspace_id": workspace, "thread_id": thread}));
        match text(&status["state"]) {
            "needs_input" => return status,
            "running" => {}
            other => panic!("the run ended ({other}) instead of asking: {status}"),
        }
        assert!(Instant::now() < deadline, "the run never asked for input: {status}");
        door.ok("thread_wait", json!({"workspace_id": workspace, "thread_id": thread, "since": status["wait_cursor"], "timeout_ms": 10_000}));
    }
}

/// The principals of the receipts whose request key ends in `-<tag>`.
fn receipts(world: &World, tag: &str) -> Vec<(String, String)> {
    world
        .sql("SELECT principal, tool FROM agent_control_operations WHERE request_key LIKE ?1 ORDER BY created_at", &[&format!("%-{tag}")])
        .into_iter()
        .map(|row| (text(&row["principal"]).to_owned(), text(&row["tool"]).to_owned()))
        .collect()
}

fn modes(capabilities: &Value) -> Vec<String> {
    capabilities["providers"][0]["allowed_permission_modes"].as_array().into_iter().flatten().filter_map(|m| m.as_str().map(str::to_owned)).collect()
}

/// A worktree created through a door: its workspace and checkout, checked to
/// start on `branch` from trunk.
fn created_worktree(world: &World, created: &Value, branch: &str) -> (String, PathBuf) {
    assert_eq!(created["state"], "succeeded", "{created}");
    assert_eq!((created["adopted"].clone(), created["base_branch"].clone()), (json!(false), json!("trunk")), "{created}");
    let cwd = PathBuf::from(text(&created["cwd"]));
    assert_eq!(git(&cwd, &["rev-parse", "--abbrev-ref", "HEAD"]), branch);
    // Base defaults to the repository's default branch, which is not `main` here.
    assert_eq!(git(&cwd, &["rev-parse", "HEAD"]), world.trunk());
    (text(&created["workspace_id"]).to_owned(), cwd)
}

// ── The flows ─────────────────────────────────────────────────────────────

#[test]
fn stdio_door_drives_the_full_outside_flow() {
    let w = world();
    let t = tag();
    let mut cleanup = Cleanup::default();
    let focus = w.active_workspace();
    let mut door = StdioDoor::named(&w.env, "hermes", None);

    let init = door.initialized["result"].clone();
    assert_eq!(init["serverInfo"]["name"], "codemux", "{init}");
    assert!(!text(&init["instructions"]).is_empty(), "{init}");
    assert!(["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"].contains(&text(&init["protocolVersion"])), "{init}");

    let listed = door.ok("workspace_list", json!({}));
    assert!(listed["workspaces"].as_array().unwrap().iter().any(|row| row["workspace_id"] == w.repo_workspace.as_str()), "{listed}");

    let branch = format!("oa-stdio-{t}");
    let created = door.ok("worktree_create", json!({"repo_path": w.repo, "branch": branch, "client_request_id": format!("wt-{t}")}));
    let (workspace, cwd) = created_worktree(w, &created, &branch);
    cleanup.worktrees.push(workspace.clone());
    // The per-workspace .mcp.json still marks in-app agents as provider-origin.
    let mcp_json: Value = serde_json::from_str(&std::fs::read_to_string(cwd.join(".mcp.json")).unwrap()).unwrap();
    assert!(mcp_json["mcpServers"]["codemux"]["env"].get("CODEMUX_MCP_PROVIDER_ORIGIN").is_some(), "{mcp_json}");

    let caps = door.ok("agent_capabilities", json!({"workspace_id": workspace, "provider": "cursor"}));
    let allowed = modes(&caps);
    assert!(allowed.contains(&"ask".into()) && allowed.contains(&"agent".into()), "{caps}");

    let receipt = door.ok("thread_launch", json!({"workspace_id": workspace, "provider": "cursor", "model": "fake-model",
        "permission_mode": "agent", "title": format!("Outside {t}"), "message": format!("echo-prompt launch-{t}"),
        "client_request_id": format!("launch-{t}")}));
    let thread = text(&receipt["thread_id"]).to_owned();
    assert!(!thread.is_empty(), "the receipt names the thread at once: {receipt}");
    cleanup.threads.push((workspace.clone(), thread.clone()));
    let launched = door.settle(&receipt);
    assert_eq!(launched["state"], "succeeded", "{launched}");
    let waited = door.finish(&workspace, &thread, turn_of(&launched).as_deref());
    assert_eq!((waited["state"].clone(), waited["timed_out"].clone()), (json!("done"), json!(false)), "{waited}");
    assert!(text(&waited["final_text"]).contains(&format!("launch-{t}")), "{waited}");
    assert_eq!(waited["pending"], json!({"approvals": [], "questions": [], "secrets": []}));
    assert_eq!(waited["failure"], Value::Null);
    // The fields waits returned before stay.
    assert!(waited["status"].is_object() && waited["settled"] == true && waited.get("selected_turn").is_some(), "{waited}");

    let page = door.ok("thread_read", json!({"workspace_id": workspace, "thread_id": thread}));
    let messages = page["messages"].as_array().unwrap();
    assert!(messages.iter().any(|m| m["role"] == "user" && text(&m["content"]).contains(&format!("echo-prompt launch-{t}"))), "{page}");
    assert!(messages.iter().any(|m| m["role"] == "assistant" && text(&m["content"]).contains(&format!("launch-{t}"))), "{page}");

    let turn = send(&mut door, &workspace, &thread, &format!("echo-prompt send-{t}"), &format!("send-{t}"));
    assert!(turn.is_some(), "an idle thread starts the message as a run");
    let waited = door.finish(&workspace, &thread, turn.as_deref());
    assert_eq!(waited["state"], "done", "{waited}");
    assert!(text(&waited["final_text"]).contains(&format!("send-{t}")), "{waited}");

    // A held run: a short wait times out without ending it; an interrupt does.
    let held = send(&mut door, &workspace, &thread, "await-cancel", &format!("hold-{t}")).expect("the held run");
    until_started(&mut door, &workspace, &thread, Some(&held));
    let early = door.wait(&workspace, &thread, Some(&held), 500);
    assert_eq!((early["timed_out"].clone(), early["state"].clone()), (json!(true), json!("running")), "{early}");
    let interrupted = door.run("thread_interrupt", json!({"workspace_id": workspace, "thread_id": thread, "turn_id": held,
        "client_request_id": format!("int-{t}")}));
    assert_eq!(interrupted["result"]["reached_provider"], true, "{interrupted}");
    let ended = door.finish(&workspace, &thread, Some(&held));
    assert_eq!((ended["state"].clone(), ended["failure"].clone()), (json!("failed"), json!("interrupted")), "{ended}");

    // Messages from an outside assistant say so, under its name.
    let rows = w.sql(
        "SELECT json_extract(payload, '$.created_by') AS created_by, json_extract(payload, '$.creation_source') AS source, \
         json_extract(payload, '$.sender_title') AS sender FROM agent_chat_messages \
         WHERE thread_id = ?1 AND json_extract(payload, '$.type') = 'user_message'",
        &[&thread],
    );
    assert_eq!(rows.len(), 3, "{rows:?}");
    for row in &rows {
        assert_eq!((row["created_by"].clone(), row["source"].clone(), row["sender"].clone()), (json!("agent"), json!("mcp"), json!("hermes")), "{row:?}");
    }

    let closed = door.ok("workspace_close", json!({"workspace_id": workspace, "client_request_id": format!("close-{t}"), "delete_worktree": true}));
    assert_eq!((closed["closed"].clone(), closed["worktree_removed"].clone()), (json!(true), json!(true)), "{closed}");
    assert!(!cwd.exists(), "the worktree is removed");
    assert!(!w.open_workspace_ids().contains(&workspace));
    match door.call("thread_status", json!({"workspace_id": workspace, "thread_id": thread})) {
        Ok(status) => assert_eq!(status["runtime_live"], false, "{status}"),
        Err(error) => assert!(matches!(error.code.as_str(), "thread_not_found" | "workspace_not_found"), "{error:?}"),
    }

    let receipts = receipts(w, &t);
    assert!(receipts.iter().all(|(principal, _)| principal == "local:hermes"), "{receipts:?}");
    let tools: BTreeSet<&str> = receipts.iter().map(|(_, tool)| tool.as_str()).collect();
    for tool in ["worktree_create", "thread_launch", "thread_send", "thread_interrupt", "workspace_close"] {
        assert!(tools.contains(tool), "{tool} in {tools:?}");
    }
    // Outside callers never move the desktop's focus.
    assert_eq!(w.active_workspace(), focus);
    assert!(door.close().success());
}

#[test]
fn http_door_drives_the_full_outside_flow() {
    let w = world();
    let t = tag();
    let mut cleanup = Cleanup::default();
    let issued = w.issue(&format!("oa-http-{t}"), ControlAccess::FullAccess);
    let mut door = w.http(&issued);

    let branch = format!("oa-http-{t}");
    let created = door.ok("worktree_create", json!({"repo_path": w.repo, "branch": branch, "client_request_id": format!("wt-{t}")}));
    let (workspace, cwd) = created_worktree(w, &created, &branch);
    cleanup.worktrees.push(workspace.clone());

    let (thread, turn) = launch(&mut door, &workspace, "agent", &format!("echo-prompt http-{t}"), &format!("launch-{t}"));
    cleanup.threads.push((workspace.clone(), thread.clone()));
    let waited = door.finish(&workspace, &thread, turn.as_deref());
    assert_eq!(waited["state"], "done", "{waited}");
    assert!(text(&waited["final_text"]).contains(&format!("http-{t}")), "{waited}");
    let page = door.ok("thread_read", json!({"workspace_id": workspace, "thread_id": thread}));
    assert!(page["messages"].as_array().unwrap().iter().any(|m| m["role"] == "assistant"), "{page}");

    // Connector clients may remove a clean worktree, never force it.
    let forced = door.refused("workspace_close", json!({"workspace_id": workspace, "client_request_id": format!("force-{t}"),
        "delete_worktree": true, "force_delete": true}));
    assert_eq!(forced.code, "permission_denied", "{forced:?}");
    assert!(cwd.exists());
    let closed = door.ok("workspace_close", json!({"workspace_id": workspace, "client_request_id": format!("close-{t}"), "delete_worktree": true}));
    assert_eq!(closed["worktree_removed"], true, "{closed}");
    assert!(!cwd.exists());

    // Only projects already opened in Codemux can be branched from the network.
    let stranger = w.root.join(format!("stranger-{t}"));
    std::fs::create_dir_all(&stranger).unwrap();
    git(&stranger, &["init", "-q", "-b", "trunk"]);
    git(&stranger, &["commit", "-q", "--allow-empty", "-m", "seed"]);
    let refused = door.refused("worktree_create", json!({"repo_path": stranger, "branch": format!("oa-stranger-{t}"),
        "client_request_id": format!("stranger-{t}")}));
    assert_eq!(refused.code, "unknown_project_root", "{refused:?}");
    assert_eq!(worktrees_on(&stranger, &format!("oa-stranger-{t}")), 0);

    let principal = format!("mcp-client:{}", issued.client_id);
    let receipts = receipts(w, &t);
    assert!(!receipts.is_empty() && receipts.iter().all(|(owner, _)| *owner == principal), "{receipts:?}");
}

#[test]
fn doors_list_the_same_outside_tools_with_the_same_result_shape() {
    let w = world();
    let t = tag();
    let mut http = w.http(&w.issue(&format!("shape-{t}"), ControlAccess::FullAccess));
    let mut stdio = StdioDoor::named(&w.env, &format!("shape-{t}"), None);
    let (http_tools, stdio_tools) = (http.list(), stdio.list());
    let (http_names, stdio_names) = (names(&http_tools), names(&stdio_tools));
    assert!(unique(&http_names) && unique(&stdio_names), "{http_names:?} {stdio_names:?}");
    for tool in &http_tools {
        let twin = stdio_tools.iter().find(|other| other["name"] == tool["name"]).unwrap_or_else(|| panic!("{} missing on stdio", tool["name"]));
        assert_eq!(twin["inputSchema"], tool["inputSchema"], "{}", tool["name"]);
        assert_eq!(twin["annotations"], tool["annotations"], "{}", tool["name"]);
    }
    // The CLI's generated index is the same set (the guide's tool table).
    let capabilities = stdout_json(&w.cli(&["capabilities"], &[]));
    let indexed: BTreeSet<String> = names(capabilities["tools"].as_array().unwrap()).into_iter().collect();
    assert_eq!(http_names.iter().cloned().collect::<BTreeSet<_>>(), indexed);

    let missing = json!({"workspace_id": format!("missing-{t}"), "thread_id": "x"});
    for door in [&mut http as &mut dyn Door, &mut stdio] {
        let reply = door.call_raw("thread_read", missing.clone());
        let result = &reply["result"];
        assert_eq!(result["isError"], true, "{reply}");
        assert_eq!(result["structuredContent"]["error"]["code"], "workspace_not_found", "{reply}");
        assert_eq!(result["structuredContent"]["error"]["retryable"], false, "{reply}");
        assert!(text(&result["content"][0]["text"]).starts_with("workspace_not_found: "), "{reply}");
        let listed = door.call_raw("workspace_list", json!({}));
        let result = &listed["result"];
        assert_eq!(result["isError"], false, "{listed}");
        // The text is the compact JSON of the structured value.
        let parsed: Value = serde_json::from_str(text(&result["content"][0]["text"])).unwrap();
        assert_eq!(parsed, result["structuredContent"], "{listed}");
        assert!(result["structuredContent"]["workspaces"].is_array(), "{listed}");
    }
}

#[test]
fn read_only_credentials_cannot_mutate_on_either_door() {
    let w = world();
    let t = tag();
    let mut http = w.http(&w.issue(&format!("ro-{t}"), ControlAccess::ReadOnly));
    let mut stdio = StdioDoor::named(&w.env, &format!("ro-{t}"), Some("read-only"));
    assert_eq!(stdio.principal(), format!("local:ro-{t}"));
    let workspace = w.repo_workspace.as_str();
    for door in [&mut http as &mut dyn Door, &mut stdio] {
        door.ok("workspace_list", json!({}));
        door.ok("thread_list", json!({"workspace_id": workspace}));
        door.ok("agent_capabilities", json!({"workspace_id": workspace, "provider": "cursor"}));
        let launch = json!({"workspace_id": workspace, "provider": "cursor", "permission_mode": "ask", "message": "echo-prompt ro",
            "client_request_id": format!("ro-launch-{t}")});
        let worktree = json!({"repo_path": w.repo, "branch": format!("ro-{t}"), "client_request_id": format!("ro-wt-{t}")});
        let message = json!({"workspace_id": workspace, "thread_id": "x", "message": "echo-prompt ro", "client_request_id": format!("ro-send-{t}")});
        for (tool, arguments) in [("thread_launch", launch), ("worktree_create", worktree), ("thread_send", message)] {
            assert_eq!(door.refused(tool, arguments).code, "access_denied", "{} {tool}", door.principal());
        }
        // Tools it cannot run are not offered.
        let offered = names(&door.list());
        assert!(!offered.contains(&"thread_launch".into()) && offered.contains(&"thread_wait".into()), "{offered:?}");
        let principal = door.principal().to_owned();
        let rows = w.sql("SELECT id FROM agent_control_operations WHERE principal = ?1", &[&principal]);
        assert!(rows.is_empty(), "{principal}: {rows:?}");
    }
    assert_eq!(worktrees_on(&w.repo, &format!("ro-{t}")), 0);

    // `codemux mcp` also serves the legacy write tools, which run locally:
    // a read-only connection must not commit, even with a change staged.
    let mut cleanup = Cleanup::default();
    let name = format!("ro-git-{t}");
    let checkout = w.root.join("folders").join(&name);
    std::fs::create_dir_all(&checkout).unwrap();
    git(&checkout, &["init", "-q"]);
    git(&checkout, &["commit", "-q", "--allow-empty", "-m", "seed"]);
    std::fs::write(checkout.join("staged.txt"), "staged\n").unwrap();
    git(&checkout, &["add", "staged.txt"]);
    let head = git(&checkout, &["rev-parse", "HEAD"]);
    let target = w.fresh_workspace(&name, &mut cleanup);
    let mut call = json!({"name": "git_commit", "arguments": {"message": "x"}, "_meta": {}});
    call["_meta"][codemux_lib::mcp::WORKSPACE_META_KEY] = json!(target);
    let id = stdio.send("tools/call", call);
    let refused = tool_result(&stdio.response(id, Duration::from_secs(20))["result"]).expect_err("a read-only git_commit");
    assert_eq!(refused.code, "access_denied", "{refused:?}");
    assert_eq!(git(&checkout, &["rev-parse", "HEAD"]), head, "the staged change was committed");
}

#[test]
fn supervised_token_cannot_create_worktrees_escalate_modes_or_approve() {
    let w = world();
    let t = tag();
    let mut cleanup = Cleanup::default();
    let mut door = w.http(&w.issue(&format!("sup-{t}"), ControlAccess::Supervised));
    // Setup scripts are command execution: worktrees need full access.
    let refused = door.refused("worktree_create", json!({"repo_path": w.repo, "branch": format!("sup-{t}"), "client_request_id": format!("wt-{t}")}));
    assert_eq!(refused.code, "access_denied", "{refused:?}");
    assert_eq!(worktrees_on(&w.repo, &format!("sup-{t}")), 0);

    let workspace = w.repo_workspace.as_str();
    let caps = door.ok("agent_capabilities", json!({"workspace_id": workspace, "provider": "cursor"}));
    assert_eq!(modes(&caps), ["ask"], "{caps}");
    let args = |mode: &str, key: String| json!({"workspace_id": workspace, "provider": "cursor", "permission_mode": mode,
        "message": format!("echo-prompt sup-{t}"), "client_request_id": key});
    assert_eq!(door.refused("thread_launch", args("agent", format!("agent-{t}"))).code, "permission_ceiling");
    let (thread, turn) = {
        let receipt = door.ok("thread_launch", args("ask", format!("ask-{t}")));
        let settled = door.settle(&receipt);
        assert_eq!(settled["state"], "succeeded", "{settled}");
        (text(&settled["thread_id"]).to_owned(), turn_of(&settled))
    };
    cleanup.threads.push((workspace.into(), thread.clone()));
    let waited = door.finish(workspace, &thread, turn.as_deref());
    assert_eq!(waited["state"], "done", "{waited}");
    assert!(text(&waited["final_text"]).contains(&format!("sup-{t}")), "{waited}");
}

#[test]
fn token_revoked_mid_wait_ends_the_wait_with_grant_revoked() {
    let w = world();
    let t = tag();
    let mut cleanup = Cleanup::default();
    let label = format!("rev-{t}");
    let mut door = w.http(&w.issue(&label, ControlAccess::FullAccess));
    let workspace = w.repo_workspace.clone();
    let (thread, turn) = launch(&mut door, &workspace, "agent", "await-cancel", &format!("launch-{t}"));
    cleanup.threads.push((workspace.clone(), thread.clone()));
    until_started(&mut door, &workspace, &thread, turn.as_deref());

    // Every wait this client may hold, so a refused extra one proves they all
    // parked: a request that reached the connector after the revoke would get
    // a 401 instead of a wait to end.
    let wait = json!({"workspace_id": workspace, "thread_id": thread, "timeout_ms": 60_000});
    let waiters: Vec<_> = (0..WAITS_PER_CLIENT)
        .map(|_| {
            let (mut door, wait) = (door.clone(), wait.clone());
            std::thread::spawn(move || (parked_wait(&mut door, &wait), Instant::now()))
        })
        .collect();
    let mut probe = wait.clone();
    probe["timeout_ms"] = json!(1);
    eventually("every wait parked", Duration::from_secs(10), || {
        door.call("thread_wait", probe.clone()).err().is_some_and(|error| error.code == "wait_budget_exhausted")
    });
    let revoked_at = Instant::now();
    let revoked = w.cli(&["connector", "revoke", &label, "--json"], &[]);
    assert_eq!(exit_code(&revoked), 0, "{}", describe(&revoked));
    for waiter in waiters {
        let (result, returned_at) = waiter.join().unwrap();
        assert_eq!(result.expect_err("the wait ends with the grant").code, "grant_revoked");
        let after = returned_at.duration_since(revoked_at);
        assert!(after < Duration::from_secs(5), "a wait ended {after:?} after the revoke");
    }
    assert_eq!(door.ping(), 401);

    // The owner can still stop what the revoked client started.
    let mut owner = StdioDoor::named(&w.env, &format!("owner-{t}"), None);
    let stopped = owner.run("thread_stop", json!({"workspace_id": workspace, "thread_id": thread, "client_request_id": format!("stop-{t}")}));
    assert_eq!(stopped["result"]["stopped"], true, "{stopped}");
}

#[test]
fn parked_http_waits_release_permits_and_respect_the_waiter_budget() {
    let w = world();
    let t = tag();
    let mut cleanup = Cleanup::default();
    let mut door = w.http(&w.issue(&format!("budget-{t}"), ControlAccess::FullAccess));
    // A second client: two full budgets park as many waits as the connector
    // has request permits (16), so a wait that kept its permit would leave
    // none for the calls below.
    let mut other = w.http(&w.issue(&format!("budget-b-{t}"), ControlAccess::FullAccess));
    let workspace = w.repo_workspace.clone();
    let (thread, turn) = launch(&mut door, &workspace, "agent", "await-cancel", &format!("launch-{t}"));
    cleanup.threads.push((workspace.clone(), thread.clone()));
    until_started(&mut door, &workspace, &thread, turn.as_deref());
    let wait = json!({"workspace_id": workspace, "thread_id": thread, "timeout_ms": 60_000});
    let parked: Vec<_> = [&door, &other]
        .into_iter()
        .flat_map(|client| std::iter::repeat_n(client, WAITS_PER_CLIENT))
        .map(|client| {
            let (mut client, wait) = (client.clone(), wait.clone());
            std::thread::spawn(move || parked_wait(&mut client, &wait))
        })
        .collect();
    let mut probe = wait.clone();
    probe["timeout_ms"] = json!(1);
    for client in [&mut door, &mut other] {
        eventually("one more wait is refused", Duration::from_secs(10), || {
            client.call("thread_wait", probe.clone()).err().is_some_and(|error| error.code == "wait_budget_exhausted")
        });
    }
    // Parked waits hold no request permits: both clients are still served.
    assert_eq!(door.rpc("tools/list", json!({})).0, 200);
    door.ok("thread_status", json!({"workspace_id": workspace, "thread_id": thread}));
    assert_eq!(other.rpc("tools/list", json!({})).0, 200);

    let interrupted_at = Instant::now();
    let mut interrupt = json!({"workspace_id": workspace, "thread_id": thread, "client_request_id": format!("int-{t}")});
    if let Some(turn) = &turn {
        interrupt["turn_id"] = json!(turn);
    }
    door.run("thread_interrupt", interrupt);
    for wait in parked {
        let ended = wait.join().unwrap().expect("a parked wait returns its run");
        assert_eq!((ended["state"].clone(), ended["failure"].clone()), (json!("failed"), json!("interrupted")), "{ended}");
    }
    assert!(interrupted_at.elapsed() < Duration::from_secs(10), "{:?}", interrupted_at.elapsed());
    // Every slot came back.
    door.ok("thread_wait", probe);
}

#[test]
fn retry_with_the_same_client_request_id_is_deduplicated_across_processes() {
    let w = world();
    let t = tag();
    let mut cleanup = Cleanup::default();
    let workspace = w.fresh_workspace(&format!("dedupe-{t}"), &mut cleanup);
    let client = format!("dedupe-{t}");
    let launch_args = json!({"workspace_id": workspace, "provider": "cursor", "permission_mode": "agent",
        "message": format!("echo-prompt dedupe-{t}"), "client_request_id": format!("k-{t}")});
    let first = {
        let mut door = StdioDoor::named(&w.env, &client, None);
        let receipt = door.ok("thread_launch", launch_args.clone());
        let settled = door.settle(&receipt);
        assert!(door.close().success());
        settled
    };
    cleanup.threads.push((workspace.clone(), text(&first["thread_id"]).to_owned()));
    let sessions = w.sessions_in(&workspace);
    assert_eq!(sessions, 1);
    // A new process of the same client is the same principal.
    let mut again = StdioDoor::named(&w.env, &client, None);
    let second = again.ok("thread_launch", launch_args);
    assert_eq!((second["operation_id"].clone(), second["thread_id"].clone()), (first["operation_id"].clone(), first["thread_id"].clone()), "{second}");
    assert_eq!(w.sessions_in(&workspace), sessions);

    let branch = format!("dd-{t}");
    let worktree = json!({"repo_path": w.repo, "branch": branch, "client_request_id": format!("wt-{t}")});
    let created = again.ok("worktree_create", worktree.clone());
    cleanup.worktrees.push(text(&created["workspace_id"]).to_owned());
    assert!(again.close().success());
    let mut third = StdioDoor::named(&w.env, &client, None);
    let replayed = third.ok("worktree_create", worktree);
    assert_eq!(replayed["workspace_id"], created["workspace_id"], "{replayed}");
    assert_eq!(replayed["replayed"], true, "{replayed}");
    assert_eq!(worktrees_on(&w.repo, &branch), 1);

    // Over HTTP, and across a token reissued for the same label.
    let label = format!("dd-http-{t}");
    let issued = w.issue(&label, ControlAccess::FullAccess);
    let mut http = w.http(&issued);
    let keyed = json!({"workspace_id": workspace, "provider": "cursor", "permission_mode": "agent",
        "message": format!("echo-prompt dd-http-{t}"), "client_request_id": format!("kh-{t}")});
    let receipt = http.ok("thread_launch", keyed.clone());
    cleanup.threads.push((workspace.clone(), text(&receipt["thread_id"]).to_owned()));
    assert_eq!(http.ok("thread_launch", keyed.clone())["operation_id"], receipt["operation_id"]);
    let reissued = w.issue(&label, ControlAccess::FullAccess);
    assert_eq!(reissued.client_id, issued.client_id, "a label keeps its client");
    let mut fresh = w.http(&reissued);
    assert_eq!(fresh.ok("thread_launch", keyed)["operation_id"], receipt["operation_id"]);
    assert_eq!(http.ping(), 401, "the old token stops working");
}

#[test]
fn reused_client_request_id_with_a_different_payload_conflicts() {
    let w = world();
    let t = tag();
    let mut cleanup = Cleanup::default();
    let workspace = w.fresh_workspace(&format!("conflict-{t}"), &mut cleanup);
    let mut stdio = StdioDoor::named(&w.env, &format!("conflict-{t}"), None);
    let mut http = w.http(&w.issue(&format!("conflict-{t}"), ControlAccess::FullAccess));
    for (index, door) in [&mut stdio as &mut dyn Door, &mut http].into_iter().enumerate() {
        let key = format!("k{index}-{t}");
        let (thread, _) = launch(door, &workspace, "agent", &format!("echo-prompt first-{t}"), &key);
        cleanup.threads.push((workspace.clone(), thread));
        let sessions = w.sessions_in(&workspace);
        let changed = json!({"workspace_id": workspace, "provider": "cursor", "permission_mode": "agent",
            "title": "Outside agent control", "message": format!("echo-prompt second-{t}"), "client_request_id": key});
        assert_eq!(door.refused("thread_launch", changed).code, "request_key_conflict", "{}", door.principal());
        assert_eq!(w.sessions_in(&workspace), sessions);
    }
}

#[test]
fn same_request_key_from_two_clients_is_two_operations() {
    let w = world();
    let t = tag();
    let mut cleanup = Cleanup::default();
    let mut alpha = StdioDoor::named(&w.env, &format!("alpha-{t}"), None);
    let mut beta = StdioDoor::named(&w.env, &format!("beta-{t}"), None);
    let args = json!({"workspace_id": w.repo_workspace, "provider": "cursor", "permission_mode": "agent",
        "message": format!("echo-prompt pair-{t}"), "client_request_id": format!("same-{t}")});
    let a = alpha.ok("thread_launch", args.clone());
    let b = beta.ok("thread_launch", args);
    for receipt in [&a, &b] {
        cleanup.threads.push((w.repo_workspace.clone(), text(&receipt["thread_id"]).to_owned()));
    }
    assert_ne!(a["operation_id"], b["operation_id"]);
    assert_ne!(a["thread_id"], b["thread_id"]);
    // Labels attribute; receipts are still each client's own.
    assert_eq!(alpha.refused("operation_status", json!({"operation_id": b["operation_id"]})).code, "operation_not_found");
    assert_eq!(beta.refused("operation_status", json!({"operation_id": a["operation_id"]})).code, "operation_not_found");
}

#[test]
fn interrupt_during_a_wait_on_one_stdio_connection() {
    let w = world();
    let t = tag();
    let mut cleanup = Cleanup::default();
    let mut door = StdioDoor::named(&w.env, &format!("concurrent-{t}"), None);
    let workspace = w.repo_workspace.clone();
    let (thread, turn) = launch(&mut door, &workspace, "agent", "await-cancel", &format!("launch-{t}"));
    cleanup.threads.push((workspace.clone(), thread.clone()));
    until_started(&mut door, &workspace, &thread, turn.as_deref());
    let mut wait = json!({"workspace_id": workspace, "thread_id": thread, "timeout_ms": 60_000});
    let mut interrupt = json!({"workspace_id": workspace, "thread_id": thread, "client_request_id": format!("int-{t}")});
    if let Some(turn) = &turn {
        wait["turn_id"] = json!(turn);
        interrupt["turn_id"] = json!(turn);
    }
    let waiting = door.send("tools/call", json!({"name": "thread_wait", "arguments": wait}));
    // No head-of-line blocking: a read answers while the wait is parked.
    let status = door.send("tools/call", json!({"name": "thread_status", "arguments": {"workspace_id": workspace, "thread_id": thread}}));
    let status = door.response(status, Duration::from_secs(3));
    assert_eq!(status["result"]["isError"], false, "{status}");
    assert!(!door.answered(waiting), "the wait is still parked");
    let interrupted = door.send("tools/call", json!({"name": "thread_interrupt", "arguments": interrupt}));
    let receipt = door.response(interrupted, Duration::from_secs(5));
    // Answers are read in arrival order: the wait's would be held as early.
    assert!(!door.early.contains_key(&waiting), "the interrupt's receipt came before the wait's answer");
    tool_result(&receipt["result"]).expect("an interrupt receipt");
    let ended = tool_result(&door.response(waiting, Duration::from_secs(5))["result"]).expect("the wait's result");
    assert_eq!((ended["state"].clone(), ended["failure"].clone()), (json!("failed"), json!("interrupted")), "{ended}");
}

#[test]
fn stdio_eof_during_a_wait_exits_promptly() {
    let w = world();
    let t = tag();
    let mut cleanup = Cleanup::default();
    let mut door = StdioDoor::named(&w.env, &format!("eof-{t}"), None);
    let workspace = w.repo_workspace.clone();
    let (thread, _) = launch(&mut door, &workspace, "agent", "await-cancel", &format!("launch-{t}"));
    cleanup.threads.push((workspace.clone(), thread.clone()));
    door.send("tools/call", json!({"name": "thread_wait", "arguments": {"workspace_id": workspace, "thread_id": thread, "timeout_ms": 60_000}}));
    std::thread::sleep(Duration::from_millis(300));
    door.stdin.take();
    let status = door.wait_exit(Duration::from_secs(3)).expect("codemux mcp exits within 3 s of EOF during a wait");
    assert!(status.success(), "{status}");
    // Ending the wait ended nothing else.
    let mut owner = StdioDoor::named(&w.env, &format!("owner-{t}"), None);
    let status = owner.ok("thread_status", json!({"workspace_id": workspace, "thread_id": thread}));
    assert_eq!(status["state"], "running", "{status}");
}

#[test]
fn refresh_token_rotation_keeps_receipts_and_rejects_replay() {
    let w = world();
    let t = tag();
    let mut cleanup = Cleanup::default();
    let issuer = w.mcp_url.trim_end_matches("/mcp").to_owned();
    let callback = "https://client.example/cb";
    // RFC 7636 appendix B.
    let (verifier, challenge) = ("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk", "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    let http = reqwest::blocking::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(30))
        .build()
        .unwrap();

    let metadata: Value = http.get(format!("{issuer}/.well-known/oauth-authorization-server")).send().unwrap().json().unwrap();
    assert!(metadata["grant_types_supported"].as_array().is_some_and(|types| types.contains(&json!("refresh_token"))), "{metadata}");
    let registered = http
        .post(format!("{issuer}/oauth/mcp/register"))
        .json(&json!({"client_name": format!("oauth-{t}"), "redirect_uris": [callback], "grant_types": ["authorization_code", "refresh_token"]}))
        .send()
        .unwrap();
    assert!(registered.status().is_success(), "register: {}", registered.status());
    let registered: Value = registered.json().unwrap();
    let client_id = text(&registered["client_id"]).to_owned();

    let mut authorize = url::Url::parse(&format!("{issuer}/oauth/mcp/authorize")).unwrap();
    authorize.query_pairs_mut().extend_pairs([
        ("client_id", client_id.as_str()),
        ("redirect_uri", callback),
        ("response_type", "code"),
        ("resource", w.mcp_url.as_str()),
        ("code_challenge_method", "S256"),
        ("code_challenge", challenge),
    ]);
    let waiting = http.get(authorize).send().unwrap();
    assert_eq!(waiting.status(), 200);
    let cookie = waiting.headers()["set-cookie"].to_str().unwrap().split(';').next().unwrap().to_owned();
    let request_id = cookie.split_once('=').unwrap().0.strip_prefix("cmcp_wait_").expect("the waiting cookie").to_owned();
    let page = waiting.text().unwrap();
    let nonce = page.split_once("name=\"nonce\" value=\"").unwrap().1.split('"').next().unwrap().to_owned();
    let complete = || {
        http.post(format!("{issuer}/oauth/mcp/complete"))
            .header("origin", &issuer)
            .header("cookie", &cookie)
            .form(&[("request_id", request_id.as_str()), ("nonce", nonce.as_str())])
            .send()
            .unwrap()
    };
    assert_eq!(complete().status(), 202, "the sign-in waits for the owner");
    w.cli_ok(&["json", "agent_connector_approve", &json!({"request_id": request_id, "access": "supervised"}).to_string()]);
    let redirect = complete();
    assert_eq!(redirect.status(), 303);
    let location = url::Url::parse(redirect.headers()["location"].to_str().unwrap()).unwrap();
    let code = location.query_pairs().find(|(key, _)| key == "code").map(|(_, value)| value.into_owned()).expect("an authorization code");

    let token_url = format!("{issuer}/oauth/mcp/token");
    let exchanged = http
        .post(&token_url)
        .form(&[("grant_type", "authorization_code"), ("client_id", client_id.as_str()), ("code", code.as_str()), ("redirect_uri", callback),
            ("resource", w.mcp_url.as_str()), ("code_verifier", verifier)])
        .send()
        .unwrap();
    assert_eq!(exchanged.status(), 200);
    let first: Value = exchanged.json().unwrap();
    let (a1, r1) = (text(&first["access_token"]).to_owned(), text(&first["refresh_token"]).to_owned());
    assert!(r1.starts_with("cmcr_"), "{first}");
    assert_eq!(first["token_type"], "Bearer");
    assert!(first["expires_in"].as_u64().is_some_and(|seconds| seconds <= 3600), "{first}");

    let principal = format!("mcp-client:{client_id}");
    let mut before = HttpDoor::new(&w.mcp_url, &a1, principal.clone());
    let workspace = w.repo_workspace.clone();
    let receipt = before.ok("thread_launch", json!({"workspace_id": workspace, "provider": "cursor", "permission_mode": "ask",
        "message": format!("echo-prompt oauth-{t}"), "client_request_id": format!("oauth-{t}")}));
    cleanup.threads.push((workspace.clone(), text(&receipt["thread_id"]).to_owned()));
    let settled = before.settle(&receipt);

    let refresh = |token: &str| {
        http.post(&token_url).form(&[("grant_type", "refresh_token"), ("client_id", client_id.as_str()), ("refresh_token", token)]).send().unwrap()
    };
    let rotated = refresh(&r1);
    assert_eq!(rotated.status(), 200);
    let second: Value = rotated.json().unwrap();
    let (a2, r2) = (text(&second["access_token"]).to_owned(), text(&second["refresh_token"]).to_owned());
    assert!(a2 != a1 && r2 != r1, "{second}");
    assert_eq!(before.ping(), 401, "the old access token stops working");
    let mut after = HttpDoor::new(&w.mcp_url, &a2, principal.clone());
    let same = after.ok("operation_status", json!({"operation_id": receipt["operation_id"]}));
    assert_eq!((same["operation_id"].clone(), same["thread_id"].clone()), (settled["operation_id"].clone(), settled["thread_id"].clone()));

    // A spent refresh token is refused; inside the reuse grace it revokes nothing.
    let replayed = refresh(&r1);
    assert_eq!(replayed.status(), 400);
    assert_eq!(replayed.json::<Value>().unwrap(), json!({"error": "invalid_grant"}));
    assert_eq!(after.ping(), 200);
    let receipts = receipts(w, &t);
    assert_eq!(receipts, [(principal, "thread_launch".to_owned())], "the receipt belongs to the client, not the grant");
}

#[test]
fn agent_processes_are_refused_admin_and_native_tools() {
    let w = world();
    let t = tag();
    let mut cleanup = Cleanup::default();
    let mut owner = StdioDoor::named(&w.env, &format!("agents-{t}"), None);
    let workspace = w.repo_workspace.clone();
    let list = json!({"workspace_id": workspace}).to_string();
    let approve = json!({"request_id": "x", "access": "full_access"}).to_string();

    // The chat's own credential keeps `mcp-call` (ACP prompts) working.
    let (thread, turn) = launch(&mut owner, &workspace, "agent", "mcp-servers", &format!("servers-{t}"));
    cleanup.threads.push((workspace.clone(), thread.clone()));
    let servers = owner.finish(&workspace, &thread, turn.as_deref());
    let servers: Value = serde_json::from_str(text(&servers["final_text"])).expect("the injected MCP servers");
    let (url, token) = servers
        .as_array()
        .into_iter()
        .flatten()
        .find_map(|server| {
            let bearer = server["headers"].as_array()?.iter().find_map(|header| text(&header["value"]).strip_prefix("Bearer "))?;
            Some((server["url"].as_str()?.to_owned(), bearer.to_owned()))
        })
        .unwrap_or_else(|| panic!("no gateway server in {servers}"));
    let listed = w.cli(&["mcp-call", "--list"], &[("CODEMUX_MCP_GATEWAY_URL", &url), ("CODEMUX_MCP_GATEWAY_TOKEN", &token)]);
    assert_eq!(exit_code(&listed), 0, "{}", describe(&listed));
    assert!(String::from_utf8_lossy(&listed.stdout).contains("thread_send"), "{}", describe(&listed));

    // The env marker alone refuses (the only signal off Linux).
    let marker = [("CODEMUX_AGENT_PROCESS", "1")];
    let marked = w.cli(&["json", "agent_connector_approve", &approve], &marker);
    assert_eq!(exit_code(&marked), 1, "{}", describe(&marked));
    assert!(String::from_utf8_lossy(&marked.stdout).contains("admin_refused"), "{}", describe(&marked));
    let marked = w.cli(&["call", "thread_list", &list], &marker);
    assert_eq!(stdout_json(&marked)["error"]["code"], "agent_identity_required", "{}", describe(&marked));

    #[cfg(target_os = "linux")]
    refused_by_ancestry(w, &mut owner, &workspace, &thread, &t);
}

/// The fake agent runs each command with no agent marker in its env: only
/// its process ancestry tells the app it is an agent. Linux only.
#[cfg(target_os = "linux")]
fn refused_by_ancestry(w: &World, owner: &mut StdioDoor, workspace: &str, thread: &str, t: &str) {
    let label = format!("agent-{t}");
    let list = json!({"workspace_id": workspace}).to_string();
    let approve = json!({"request_id": "x", "access": "full_access"}).to_string();
    let runs = json!([
        {"args": ["json", "agent_connector_approve", approve]},
        {"args": ["json", "web_remote_pair", "{}"]},
        {"args": ["connector", "issue", "--label", label, "--access", "full-access", "--json"]},
        {"args": ["call", "thread_list", list]},
        {"args": ["json", "status", "{}"]},
    ]);
    let web_before = w.cli_ok(&["json", "web_remote_status", "{}"])["data"]["running"].clone();
    let turn = send(owner, workspace, thread, &format!("exec-codemux {runs}"), &format!("exec-{t}"));
    let waited = owner.finish(workspace, thread, turn.as_deref());
    assert_eq!((waited["state"].clone(), waited["final_text_truncated"].clone()), (json!("done"), json!(false)), "{waited}");
    let results: Vec<Value> = serde_json::from_str(text(&waited["final_text"])).expect("the fake's run results");
    for run in &results[..3] {
        assert_eq!(run["code"], 1, "{run}");
        assert!(text(&run["stdout"]).contains("admin_refused"), "{run}");
    }
    let called: Value = serde_json::from_str(text(&results[3]["stdout"])).unwrap();
    assert_eq!((results[3]["code"].clone(), called["error"]["code"].clone()), (json!(1), json!("agent_identity_required")), "{called}");
    assert_eq!(results[4]["code"], 0, "{}", results[4]);
    // Nothing happened.
    let status = mcp_connector::agent_connector_status(w.handle.clone()).unwrap();
    assert!(!status.clients.iter().any(|client| client.label.as_deref() == Some(label.as_str())));
    assert_eq!(w.cli_ok(&["json", "web_remote_status", "{}"])["data"]["running"], web_before);
}

#[test]
fn approvals_from_outside_follow_the_access_level() {
    let w = world();
    let t = tag();
    let mut cleanup = Cleanup::default();
    let mut owner = StdioDoor::named(&w.env, &format!("approver-{t}"), None);
    let workspace = w.repo_workspace.clone();
    // Ask mode: the fake's approval reaches the person (here, outside clients).
    // It asks 2 s into the run, so the wait below parks first.
    let (thread, turn) =
        launch(&mut owner, &workspace, "ask", &format!("hold-permission-after:2000 ap-{t}"), &format!("ap-{t}"));
    cleanup.threads.push((workspace.clone(), thread.clone()));
    let before = until_started(&mut owner, &workspace, &thread, turn.as_deref());
    assert_eq!(before["state"], "running", "the approval opened before the wait could park: {before}");
    // Event-driven: a parked wait answers once the approval opens, not at its timeout.
    let started = Instant::now();
    let asking = owner.wait(&workspace, &thread, turn.as_deref(), 20_000);
    assert_eq!((asking["state"].clone(), asking["timed_out"] == true), (json!("needs_input"), false), "{asking}");
    assert!(started.elapsed() < Duration::from_secs(5), "{:?}", started.elapsed());
    let first = text(&asking["pending"]["approvals"][0]["request_id"]).to_owned();
    assert!(!first.is_empty(), "{asking}");

    let respond = |request: &str, decision: &str, key: String| {
        json!({"workspace_id": workspace, "thread_id": thread, "request_id": request, "decision": decision, "client_request_id": key})
    };
    let mut supervised = w.http(&w.issue(&format!("ap-sup-{t}"), ControlAccess::Supervised));
    assert_eq!(supervised.refused("thread_respond", respond(&first, "allow_once", format!("allow-{t}"))).code, "permission_ceiling");
    supervised.run("thread_respond", respond(&first, "deny", format!("deny-{t}")));
    let denied = owner.finish(&workspace, &thread, turn.as_deref());
    assert_eq!(denied["state"], "done", "{denied}");
    assert!(text(&denied["final_text"]).contains("permission:no"), "{denied}");

    let turn = send(&mut owner, &workspace, &thread, &format!("hold-permission ap2-{t}"), &format!("ap2-{t}"));
    let asking = until_input(&mut owner, &workspace, &thread);
    let second = text(&asking["pending"]["approvals"][0]["request_id"]).to_owned();
    assert_ne!(second, first);
    // Connector clients never allow always, even at full access.
    let mut full = w.http(&w.issue(&format!("ap-full-{t}"), ControlAccess::FullAccess));
    assert_eq!(full.refused("thread_respond", respond(&second, "allow_always", format!("always-{t}"))).code, "permission_denied");
    owner.run("thread_respond", respond(&second, "allow_once", format!("once-{t}")));
    let allowed = owner.finish(&workspace, &thread, turn.as_deref());
    assert!(text(&allowed["final_text"]).contains("permission:yes"), "{allowed}");

    // A request from an earlier run is stale, not a successful approval.
    let stale = owner.ok("thread_respond", respond(&first, "deny", format!("stale-{t}")));
    let stale = owner.settle(&stale);
    assert_eq!((stale["state"].clone(), stale["error"]["code"].clone()), (json!("failed"), json!("stale_provider_callback")), "{stale}");
}

#[test]
fn cli_verbs_drive_a_thread_with_exit_codes() {
    let w = world();
    let t = tag();
    let mut cleanup = Cleanup::default();
    let mut http = w.http(&w.issue(&format!("cli-{t}"), ControlAccess::FullAccess));
    let listed = w.cli_ok(&["tools", "--json"]);
    assert_eq!(
        names(listed["tools"].as_array().unwrap()).into_iter().collect::<BTreeSet<_>>(),
        names(&http.list()).into_iter().collect::<BTreeSet<_>>(),
    );
    let workspace = w.repo_workspace.as_str();
    let client = format!("cli-{t}");
    let start = |mode: &str, message: String| {
        w.cli(&["--client", &client, "thread", "start", "--workspace", workspace, "--provider", "cursor", "--permission-mode", mode,
            "-m", &message, "--wait", "--json"], &[])
    };
    let done = start("agent", format!("echo-prompt cli-{t}"));
    assert_eq!(exit_code(&done), 0, "{}", describe(&done));
    let report = stdout_json(&done);
    assert_eq!((report["ok"].clone(), report["state"].clone()), (json!(true), json!("done")), "{report}");
    assert!(text(&report["final_text"]).contains(&format!("cli-{t}")), "{report}");
    cleanup.threads.push((workspace.into(), text(&report["thread_id"]).to_owned()));

    let asks = start("ask", format!("hold-permission cli2-{t}"));
    assert_eq!(exit_code(&asks), 2, "needs input: {}", describe(&asks));
    let report = stdout_json(&asks);
    let thread = text(&report["thread_id"]).to_owned();
    cleanup.threads.push((workspace.into(), thread.clone()));
    let request = text(&report["pending"]["approvals"][0]["request_id"]).to_owned();
    assert!(!request.is_empty(), "{report}");
    let denied = w.cli(&["thread", "respond", &thread, &request, "deny", "--workspace", workspace, "--json"], &[]);
    assert_eq!(exit_code(&denied), 0, "{}", describe(&denied));

    let missing = json!({"workspace_id": format!("missing-{t}"), "thread_id": "x"}).to_string();
    let called = w.cli(&["call", "thread_read", &missing], &[]);
    assert_eq!(exit_code(&called), 1);
    assert_eq!(stdout_json(&called)["error"]["code"], "workspace_not_found", "{}", describe(&called));
    let raw = w.cli(&["json", "thread_read", &missing], &[]);
    assert_eq!(exit_code(&raw), 1);
    assert_eq!(stdout_json(&raw)["ok"], false, "{}", describe(&raw));
}

#[test]
fn cli_worktree_verbs_replay_refuse_supervised_and_clean_up() {
    let w = world();
    let t = tag();
    let mut cleanup = Cleanup::default();
    let repo = w.repo.display().to_string();
    let branch = format!("cw-{t}");
    let key = format!("cw-{t}");
    let create = ["worktree", "create", "--repo", &repo, "--branch", &branch, "--request-id", &key, "--json"];
    let created = w.cli_ok(&create);
    let workspace = text(&created["result"]["workspace_id"]).to_owned();
    let checkout = PathBuf::from(text(&created["result"]["cwd"]));
    cleanup.worktrees.push(workspace.clone());
    assert_eq!(created["result"]["base_branch"], "trunk", "{created}");
    assert!(checkout.exists(), "{created}");
    let replayed = w.cli_ok(&create);
    assert_eq!((replayed["result"]["workspace_id"].clone(), replayed["result"]["replayed"].clone()), (json!(workspace), json!(true)), "{replayed}");
    assert_eq!(worktrees_on(&w.repo, &branch), 1);

    let narrowed = format!("cw-sup-{t}");
    let refused = w.cli(&["--client", &narrowed, "--access", "supervised", "worktree", "create", "--repo", &repo, "--branch", &narrowed, "--json"], &[]);
    assert_eq!(exit_code(&refused), 1, "{}", describe(&refused));
    assert_eq!(stdout_json(&refused)["error"]["code"], "access_denied", "{}", describe(&refused));
    assert_eq!(worktrees_on(&w.repo, &narrowed), 0);

    // One command launches a thread in a new worktree and waits for it.
    let launch_branch = format!("cwt-{t}");
    let launch_key = format!("cwt-{t}");
    let message = format!("echo-prompt cwt-{t}");
    let start = ["thread", "start", "--workspace", w.repo_workspace.as_str(), "--provider", "cursor", "--permission-mode", "agent",
        "-m", &message, "--worktree-branch", &launch_branch, "--request-id", &launch_key, "--json"];
    let waited = w.cli(&[&start[..], &["--wait"][..]].concat(), &[]);
    assert_eq!(exit_code(&waited), 0, "{}", describe(&waited));
    let report = stdout_json(&waited);
    assert_eq!(report["state"], "done", "{report}");
    assert!(text(&report["final_text"]).contains(&format!("cwt-{t}")), "{report}");
    let launched = text(&report["workspace_id"]).to_owned();
    assert!(!launched.is_empty() && launched != w.repo_workspace, "the run is in the new workspace: {report}");
    cleanup.worktrees.push(launched.clone());
    cleanup.threads.push((launched.clone(), text(&report["thread_id"]).to_owned()));
    let retried = w.cli_ok(&start);
    let receipt = &retried["result"];
    assert_eq!(receipt["operation_id"], report["operation_id"], "{retried}");
    let worktree = &receipt["result"]["worktree"];
    assert_eq!((worktree["branch"].clone(), worktree["base"].clone(), worktree["base_commit"].clone()),
        (json!(launch_branch), json!("trunk"), json!(w.trunk())), "{retried}");
    let path = PathBuf::from(text(&worktree["path"]));
    assert!(path.exists());

    for (id, checkout) in [(launched.as_str(), path), (workspace.as_str(), checkout)] {
        let closed = w.cli_ok(&["workspace", "close", id, "--remove-worktree", "--json"]);
        assert_eq!(closed["result"]["worktree_removed"], true, "{closed}");
        assert!(!checkout.exists());
    }
    assert_eq!(worktrees_on(&w.repo, &branch) + worktrees_on(&w.repo, &launch_branch), 0);
}

#[test]
fn provider_origin_stdio_keeps_in_app_tools_and_refuses_native_control() {
    let w = world();
    let active = w.active_workspace().expect("a focused workspace");
    let mut in_app = StdioDoor::spawn(&w.env, &["mcp"], &[("CODEMUX_MCP_PROVIDER_ORIGIN", "1")], "local");
    let listed = names(&in_app.list());
    assert!(unique(&listed), "{listed:?}");
    assert_eq!(listed.into_iter().collect::<BTreeSet<_>>(), LEGACY_TOOL_NAMES.iter().map(|name| name.to_string()).collect());
    let refused = in_app.call_raw("thread_launch", json!({"workspace_id": active, "provider": "cursor", "permission_mode": "agent",
        "message": "echo-prompt in-app", "client_request_id": "in-app"}));
    assert!(text(&refused["error"]["message"]).contains("provider_control_denied"), "{refused}");
    // vexis-agent reads the focused workspace with no arguments.
    let info = in_app.ok("workspace_info", json!({}));
    assert_eq!(id_text(&info["workspace_id"]), Some(active.clone()), "{info}");
    assert!(in_app.close().success());

    // The outside door serves the same name from the registry, and keeps its shape.
    let mut outside = StdioDoor::spawn(&w.env, &["mcp"], &[], "local");
    let info = outside.ok("workspace_info", json!({}));
    assert_eq!(id_text(&info["workspace_id"]), Some(active), "{info}");
    assert!(info["surfaces"].is_array(), "{info}");
    let panes = outside.ok("pane_list", json!({}));
    assert!(panes["surfaces"].is_array(), "{panes}");
    assert!(outside.close().success());
}

#[test]
fn thread_launch_into_a_new_worktree_is_one_idempotent_operation() {
    let w = world();
    let t = tag();
    let mut cleanup = Cleanup::default();
    let mut door = StdioDoor::named(&w.env, &format!("wl-{t}"), None);
    let branch = format!("wl-{t}");
    let args = json!({"workspace_id": w.repo_workspace, "worktree": {"branch": branch}, "provider": "cursor", "permission_mode": "agent",
        "message": format!("echo-prompt wl-{t}"), "client_request_id": format!("wl-{t}")});
    let receipt = door.ok("thread_launch", args.clone());
    let reserved = text(&receipt["workspace_id"]).to_owned();
    assert!(!reserved.is_empty() && reserved != w.repo_workspace, "the receipt names the new workspace at once: {receipt}");
    cleanup.worktrees.push(reserved.clone());
    let settled = door.settle(&receipt);
    assert_eq!(settled["state"], "succeeded", "{settled}");
    let thread = text(&settled["thread_id"]).to_owned();
    cleanup.threads.push((reserved.clone(), thread.clone()));
    let worktree = &settled["result"]["worktree"];
    assert_eq!((worktree["branch"].clone(), worktree["base"].clone(), worktree["base_commit"].clone()),
        (json!(branch), json!("trunk"), json!(w.trunk())), "{settled}");
    assert_eq!(git(Path::new(text(&worktree["path"])), &["rev-parse", "--abbrev-ref", "HEAD"]), branch);
    let waited = door.finish(&reserved, &thread, turn_of(&settled).as_deref());
    assert_eq!(waited["state"], "done", "{waited}");
    let retried = door.ok("thread_launch", args);
    assert_eq!(retried["operation_id"], receipt["operation_id"]);
    assert_eq!(worktrees_on(&w.repo, &branch), 1);

    // Worktree launch runs setup scripts, so a supervised client is refused
    // before anything is created.
    let mut supervised = w.http(&w.issue(&format!("wl-sup-{t}"), ControlAccess::Supervised));
    let narrowed = format!("wl-sup-{t}");
    let refused = supervised.refused("thread_launch", json!({"workspace_id": w.repo_workspace, "worktree": {"branch": narrowed},
        "provider": "cursor", "permission_mode": "ask", "message": "echo-prompt wl-sup", "client_request_id": format!("sup-{t}")}));
    assert_eq!(refused.code, "access_denied", "{refused:?}");
    assert_eq!(git(&w.repo, &["branch", "--list", &narrowed]), "");
}

/// Ancestry is the only signal here, and it is Linux-only.
#[cfg(target_os = "linux")]
#[test]
fn user_scope_codemux_mcp_inside_an_agent_is_refused_admin() {
    let w = world();
    let t = tag();
    let mut cleanup = Cleanup::default();
    let mut owner = StdioDoor::named(&w.env, &format!("user-scope-{t}"), None);
    let workspace = w.repo_workspace.clone();
    // What an in-app Codex or Hermes chat would start from a user-level entry:
    // `codemux mcp --client claude-code` with an env allow-list, so no marker.
    let lines = [
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18", "capabilities": {},
            "clientInfo": {"name": "claude-code", "version": "1"}}}),
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": "automation_create", "arguments": {
            "name": "nightly", "prompt": "say hi", "agent": "claude", "schedule": "DTSTART:20260101T090000Z\nRRULE:FREQ=DAILY"}}}),
        json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": {"name": "thread_list", "arguments": {"workspace_id": workspace}}}),
    ];
    let stdin: String = lines.iter().map(|line| format!("{line}\n")).collect();
    let runs = json!([{"args": ["mcp", "--client", "claude-code"], "stdin": stdin}]);
    let (thread, turn) = launch(&mut owner, &workspace, "agent", &format!("exec-codemux {runs}"), &format!("exec-{t}"));
    cleanup.threads.push((workspace.clone(), thread.clone()));
    let waited = owner.finish(&workspace, &thread, turn.as_deref());
    assert_eq!((waited["state"].clone(), waited["final_text_truncated"].clone()), (json!("done"), json!(false)), "{waited}");
    let results: Vec<Value> = serde_json::from_str(text(&waited["final_text"])).expect("the fake's run results");
    assert_eq!(results[0]["code"], 0, "{}", results[0]);
    let replies: HashMap<i64, Value> = text(&results[0]["stdout"])
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter_map(|reply| Some((reply["id"].as_i64()?, reply)))
        .collect();
    assert!(replies[&1]["result"]["serverInfo"].is_object(), "{replies:?}");
    // The server knows it is an agent only from the process ancestry.
    let admin = &replies[&2]["result"];
    assert_eq!(admin["isError"], true, "{admin}");
    assert!(admin.to_string().contains("admin_refused"), "{admin}");
    let native = &replies[&3]["result"];
    assert_eq!(native["structuredContent"]["error"]["code"], "agent_identity_required", "{native}");
}

#[test]
fn outside_answer_resolves_a_codex_question_and_the_wait_returns_done() {
    let w = world();
    let t = tag();
    let mut cleanup = Cleanup::default();
    let mut door = StdioDoor::named(&w.env, &format!("answer-{t}"), None);
    let workspace = w.repo_workspace.clone();
    // Listing reads cached catalogs only; the launch discovers the fake's.
    eventually("Codex is configured", Duration::from_secs(60), || {
        door.call("agent_capabilities", json!({"workspace_id": workspace, "provider": "codex"}))
            .is_ok_and(|caps| caps["providers"][0]["configured"] == true)
    });
    let receipt = door.ok("thread_launch", json!({"workspace_id": workspace, "provider": "codex", "model": "gpt-test",
        "permission_mode": "read-only", "message": "Pick a storage engine.", "client_request_id": format!("launch-{t}")}));
    let launched = door.settle(&receipt);
    assert_eq!(launched["state"], "succeeded", "{launched}");
    let thread = text(&launched["thread_id"]).to_owned();
    cleanup.threads.push((workspace.clone(), thread.clone()));

    let asking = until_input(&mut door, &workspace, &thread);
    let question = &asking["pending"]["questions"][0];
    assert_eq!(question["questions"][0]["title"], "Which storage?", "{asking}");
    assert_eq!(asking["pending"]["approvals"], json!([]), "{asking}");
    let answer = json!({"workspace_id": workspace, "thread_id": thread, "question_id": question["question_id"], "answers": ["SQLite"],
        "client_request_id": format!("answer-{t}")});
    let answered = door.ok("thread_answer", answer.clone());
    let turn = text(&answered["turn_id"]).to_owned();
    assert!(!turn.is_empty(), "the answer names the run that continues: {answered}");
    let ended = door.finish(&workspace, &thread, Some(&turn));
    assert_eq!(ended["state"], "done", "{ended}");
    assert!(text(&ended["final_text"]).contains("Using SQLite."), "{ended}");
    assert_eq!(ended["pending"]["questions"], json!([]), "{ended}");
    // A retry replays the answer instead of sending it again.
    let replayed = door.ok("thread_answer", answer);
    assert_eq!((replayed["turn_id"].clone(), replayed["replayed"].clone()), (json!(turn), json!(true)), "{replayed}");
}
