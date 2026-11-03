//! The control socket end to end through the built `codemux` binary: it tells
//! the owner from processes Codemux started for agents, `codemux mcp` carries
//! a client's name and access, and the legacy workspace commands keep their
//! shapes.
//!
//! Each test runs its own `codemux serve` child in a throwaway home with its
//! own `XDG_RUNTIME_DIR`, and drives it with the `codemux` CLI. Every child
//! starts from a cleared environment plus an allow-list, so neither a
//! developer's running app nor the agent identity of a chat this suite runs
//! under can leak in.

#![cfg(unix)]

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const CODEMUX: &str = env!("CARGO_BIN_EXE_codemux");
const AGENT_PROCESS: &str = "CODEMUX_AGENT_PROCESS";
const PROVIDER_ORIGIN: &str = "CODEMUX_MCP_PROVIDER_ORIGIN";
const IDENTITY_VARS: [&str; 4] =
    [AGENT_PROCESS, "CODEMUX_MCP_GATEWAY_URL", "CODEMUX_MCP_GATEWAY_TOKEN", PROVIDER_ORIGIN];

/// A `codemux serve` child, stopped (SIGTERM, then kill) when dropped.
struct Serve {
    child: Child,
    root: tempfile::TempDir,
}

/// Clears the env and points every state dir (and the control socket) into `root`.
fn isolate(cmd: &mut Command, root: &Path) {
    cmd.env_clear().current_dir(root);
    cmd.env("HOME", root).env("PATH", "/usr/bin:/bin").env("LANG", "C.UTF-8");
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

fn free_loopback_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

impl Serve {
    fn start(env: &[(&str, &str)]) -> Self {
        let root = tempfile::tempdir().unwrap();
        let log = std::fs::File::create(root.path().join("serve.log")).unwrap();
        let mut cmd = Command::new(CODEMUX);
        isolate(&mut cmd, root.path());
        // In-process terminals (no detached daemon outlives the test), no
        // reachable account API, and a predictable shell.
        cmd.env("CODEMUX_DISABLE_PTY_DAEMON", "1")
            .env("CODEMUX_API_URL", "http://127.0.0.1:1")
            .env("SHELL", "/bin/sh")
            .envs(env.iter().copied())
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
            if self.try_json("status", "{}", &[]).is_some_and(|reply| reply["ok"] == true) {
                return;
            }
            assert!(Instant::now() < deadline, "codemux serve never answered on its socket:\n{}", self.log());
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    /// `codemux json <command> <params>` with an isolated env plus `env`.
    /// `None` when the CLI printed no reply (the server is not up yet).
    fn try_json(&self, command: &str, params: &str, env: &[(&str, &str)]) -> Option<Value> {
        let mut cmd = Command::new(CODEMUX);
        isolate(&mut cmd, self.root.path());
        let output = cmd.envs(env.iter().copied()).args(["json", command, params]).output().unwrap();
        serde_json::from_slice(&output.stdout).ok()
    }

    fn json(&self, command: &str, params: &str, env: &[(&str, &str)]) -> Value {
        self.try_json(command, params, env)
            .unwrap_or_else(|| panic!("`codemux json {command}` printed no reply:\n{}", self.log()))
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

/// A `codemux mcp` child speaking to a [`Serve`], stopped when dropped.
struct Mcp {
    child: Child,
    stdin: ChildStdin,
    lines: mpsc::Receiver<String>,
    next_id: u64,
    log: std::path::PathBuf,
}

impl Mcp {
    fn start(serve: &Serve, args: &[&str]) -> Self {
        let log = serve.root.path().join(format!("mcp-{}.log", args.join("_")));
        let mut cmd = Command::new(CODEMUX);
        isolate(&mut cmd, serve.root.path());
        let mut child = cmd
            .arg("mcp")
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(std::fs::File::create(&log).unwrap())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        let stdin = child.stdin.take().unwrap();
        let mut mcp = Self { child, stdin, lines, next_id: 0, log };
        let init = mcp.request(
            "initialize",
            json!({"protocolVersion": "2025-11-25", "capabilities": {}, "clientInfo": {"name": "ignored", "version": "1"}}),
        );
        assert!(init["result"]["protocolVersion"].is_string(), "{init}");
        mcp
    }

    fn request(&mut self, method: &str, params: Value) -> Value {
        self.next_id += 1;
        let message = json!({"jsonrpc": "2.0", "id": self.next_id, "method": method, "params": params});
        // A child that already exited (an unknown flag) fails the write or
        // the read; its stderr says why.
        let sent = writeln!(self.stdin, "{message}").and_then(|()| self.stdin.flush());
        let line = sent.ok().and_then(|()| self.lines.recv_timeout(Duration::from_secs(30)).ok()).unwrap_or_else(|| {
            panic!("codemux mcp gave no reply to {method}:\n{}", std::fs::read_to_string(&self.log).unwrap_or_default())
        });
        let reply: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(reply["id"], self.next_id, "{reply}");
        reply
    }

    fn tool_names(&mut self) -> Vec<String> {
        let listed = self.request("tools/list", json!({}));
        listed["result"]["tools"].as_array().unwrap().iter().map(|tool| tool["name"].as_str().unwrap().to_owned()).collect()
    }

    fn call(&mut self, name: &str, arguments: Value) -> Value {
        self.request("tools/call", json!({"name": name, "arguments": arguments}))["result"].clone()
    }
}

impl Drop for Mcp {
    fn drop(&mut self) {
        // This test started it; stop it by its own handle.
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn error_code(reply: &Value) -> &str {
    reply["error"].as_str().and_then(|error| error.split(':').next()).unwrap_or("")
}

#[test]
fn agent_marked_cli_is_refused_admin_but_owner_is_not() {
    let serve = Serve::start(&[]);
    let marker = [(AGENT_PROCESS, "1")];

    let refused = serve.json("agent_connector_status", "{}", &marker);
    assert_eq!(refused["ok"], false, "{refused}");
    assert_eq!(error_code(&refused), "admin_refused", "{refused}");
    assert_eq!(refused["protocol_version"], 2);

    let codemux_self = serve.json("web_remote_pair", r#"{"name":"phone"}"#, &[(PROVIDER_ORIGIN, "1")]);
    assert_eq!(error_code(&codemux_self), "admin_refused", "a provider-origin `codemux mcp` is an agent: {codemux_self}");

    let owner = serve.json("agent_connector_status", "{}", &[]);
    assert_eq!(owner["ok"], true, "{owner}");

    let status = serve.json("status", "{}", &marker);
    assert_eq!(status["ok"], true, "agents keep read commands: {status}");
    let notify = serve.json("notify", r#"{"message":"from an agent"}"#, &marker);
    assert_eq!(notify["ok"], true, "agents keep write commands: {notify}");

    let native = serve.json("thread_list", r#"{"workspace_id":"none"}"#, &marker);
    assert_eq!(error_code(&native), "agent_identity_required", "{native}");
}

#[test]
fn pty_shell_is_the_owner() {
    // An instance started from an agent's shell: every identity var is set.
    let serve = Serve::start(&[
        (AGENT_PROCESS, "1"),
        (PROVIDER_ORIGIN, "1"),
        ("CODEMUX_MCP_GATEWAY_URL", "http://127.0.0.1:1/mcp"),
        ("CODEMUX_MCP_GATEWAY_TOKEN", "inherited"),
    ]);
    let root = serve.root.path();
    let project = root.join("project");
    std::fs::create_dir_all(&project).unwrap();
    let created = serve.json("create_workspace", &json!({ "path": project }).to_string(), &[]);
    assert_eq!(created["ok"], true, "{created}");

    // Type into the new workspace's terminal, as the owner.
    let (env_dump, out) = (root.join("env.txt"), root.join("out.json"));
    let line = format!(
        "env > '{env}'; '{CODEMUX}' json agent_connector_status '{{}}' > '{out}.part' && mv '{out}.part' '{out}'\n",
        env = env_dump.display(),
        out = out.display(),
    );
    let params = json!({ "data": line }).to_string();
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        // The terminal registers shortly after the workspace opens.
        let written = serve.json("write_terminal", &params, &[]);
        if written["ok"] == true {
            break;
        }
        assert!(Instant::now() < deadline, "the terminal never accepted input: {written}\n{}", serve.log());
        std::thread::sleep(Duration::from_millis(200));
    }
    while !out.exists() {
        assert!(Instant::now() < deadline, "the terminal never ran the command:\n{}", serve.log());
        std::thread::sleep(Duration::from_millis(100));
    }

    let reply: Value = serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
    assert_eq!(reply["ok"], true, "a user's terminal is the owner: {reply}");
    let env = std::fs::read_to_string(&env_dump).unwrap();
    for key in IDENTITY_VARS {
        assert!(!env.lines().any(|line| line.starts_with(&format!("{key}="))), "{key} reached the terminal");
    }
    assert!(env.contains("CODEMUX_SESSION_ID="), "the dump is the terminal's env");
}

#[test]
fn mcp_supervised_client_sees_no_admin_tools_and_is_refused_them() {
    let serve = Serve::start(&[]);
    let mut supervised = Mcp::start(&serve, &["--client", "integration-bot", "--access", "supervised"]);
    let names = supervised.tool_names();
    for hidden in ["automation_create", "terminal_write", "preset_apply", "workspace_create", "worktree_create", "workspace_unarchive"] {
        assert!(!names.iter().any(|name| name == hidden), "supervised lists {hidden}: {names:?}");
    }
    for shown in ["thread_launch", "thread_wait", "notify", "git_commit", "workspace_list"] {
        assert!(names.iter().any(|name| name == shown), "supervised lacks {shown}: {names:?}");
    }
    let refused = supervised.call("automation_create", json!({"name": "n", "prompt": "p", "schedule": "0 * * * *"}));
    assert_eq!(refused["isError"], true, "{refused}");
    assert!(refused["content"][0]["text"].as_str().unwrap().starts_with("access_denied: "), "{refused}");
    // Registry tools reach the app through the socket as this client.
    let listed = supervised.call("thread_list", json!({"workspace_id": "none"}));
    assert_eq!(listed["isError"], true, "{listed}");
    assert_eq!(listed["structuredContent"]["error"]["code"], "workspace_not_found", "{listed}");
    let automations = serve.json("automation_list", "{}", &[]);
    assert_eq!(automations["data"], json!([]), "nothing was created: {automations}");

    let mut read_only = Mcp::start(&serve, &["--client", "integration-bot", "--access", "read-only"]);
    let listed = read_only.request("tools/list", json!({}));
    for tool in listed["result"]["tools"].as_array().unwrap() {
        let name = tool["name"].as_str().unwrap();
        let reads = match tool.get("annotations") {
            Some(annotations) => annotations["readOnlyHint"] == true,
            None => codemux_lib::control_identity::mcp_tool_access(name, &json!({}))
                == Some(codemux_lib::agent_control::Access::Read),
        };
        assert!(reads, "read-only lists {name}");
    }
    let refused = read_only.call("notify", json!({"message": "x"}));
    assert!(refused["content"][0]["text"].as_str().unwrap().starts_with("access_denied: "), "{refused}");
    // Outside stdio relays registry tools without a local access check, so
    // the app's gate on the access this client carries is what refuses a
    // write. It runs before the target checks: a client the app took for full
    // access would get `workspace_not_found` here, as the supervised one does.
    let send = json!({"workspace_id": "none", "thread_id": "t", "client_request_id": "k", "message": "x"});
    let denied = read_only.call("thread_send", send.clone());
    assert_eq!(denied["isError"], true, "{denied}");
    assert_eq!(denied["structuredContent"]["error"]["code"], "access_denied", "{denied}");
    let reached = supervised.call("thread_send", send);
    assert_eq!(reached["structuredContent"]["error"]["code"], "workspace_not_found", "{reached}");
}

/// `get_app_state`'s focused workspace id.
fn active_workspace(serve: &Serve) -> String {
    let state = serve.json("get_app_state", "{}", &[]);
    let active = &state["data"]["active_workspace_id"];
    active.as_str().or_else(|| active["0"].as_str()).unwrap_or_else(|| panic!("no active workspace: {state}")).to_owned()
}

#[test]
fn socket_legacy_create_arms_keep_shapes_and_focus() {
    let serve = Serve::start(&[]);
    let root = serve.root.path();
    let (first, second) = (root.join("first"), root.join("second"));
    for dir in [&first, &second] {
        std::fs::create_dir_all(dir).unwrap();
    }

    let created = serve.json("create_workspace", &json!({ "path": first }).to_string(), &[]);
    assert_eq!(created["ok"], true, "{created}");
    let id = created["data"]["workspace_id"].as_str().unwrap().to_owned();
    assert_eq!(created["data"]["cwd"], first.display().to_string());
    assert_eq!(created["data"]["created"], true);
    assert_eq!(active_workspace(&serve), id, "the legacy command still takes focus");

    let background = serve.json("create_workspace", &json!({ "path": second, "select": false }).to_string(), &[]);
    assert_eq!(background["ok"], true, "{background}");
    let other = background["data"]["workspace_id"].as_str().unwrap().to_owned();
    assert_ne!(other, id);
    assert_eq!(active_workspace(&serve), id, "select:false keeps focus where it was");

    let closed = serve.json("close_workspace", &json!({ "workspace_id": other }).to_string(), &[]);
    assert_eq!(closed["data"], json!({"workspace_id": other, "closed": true, "worktree_removed": false}), "{closed}");
    let missing = serve.json("close_workspace", "{}", &[]);
    assert_eq!(missing["error"], "Missing required parameter: workspace_id", "{missing}");
}

/// `git` in `dir` with a fixed identity; its trimmed stdout.
fn git(dir: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(dir)
        .args(args)
        .env("GIT_AUTHOR_NAME", "Codemux test")
        .env("GIT_AUTHOR_EMAIL", "test@example.invalid")
        .env("GIT_COMMITTER_NAME", "Codemux test")
        .env("GIT_COMMITTER_EMAIL", "test@example.invalid")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", dir.join(".no-global-gitconfig"))
        .output()
        .unwrap();
    assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

#[test]
fn socket_worktree_default_base_is_repo_default_branch() {
    let serve = Serve::start(&[]);
    // A repository whose only branch is `trunk`: the old default base `main`
    // does not exist there.
    let repo = serve.root.path().join("trunk-repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-b", "trunk"]);
    git(&repo, &["commit", "--allow-empty", "-m", "first"]);
    let trunk = git(&repo, &["rev-parse", "HEAD"]);

    let params = json!({ "repo_path": repo, "branch": "feat", "layout": "empty" });
    let created = serve.json("create_worktree_workspace", &params.to_string(), &[]);
    assert_eq!(created["ok"], true, "{created}");
    let data = &created["data"];
    assert_eq!(data["base_branch"], "trunk", "{created}");
    assert_eq!(data["adopted"], false);
    let checkout = Path::new(data["cwd"].as_str().unwrap());
    assert_eq!(git(checkout, &["rev-parse", "HEAD"]), trunk);
    assert_eq!(git(checkout, &["branch", "--show-current"]), "feat");
    let id = data["workspace_id"].as_str().unwrap().to_owned();
    assert_eq!(active_workspace(&serve), id, "the legacy command still takes focus");

    let params = json!({ "repo_path": repo, "branch": "feat-2", "layout": "empty", "select": false });
    let background = serve.json("create_worktree_workspace", &params.to_string(), &[]);
    assert_eq!(background["data"]["base_branch"], "trunk", "{background}");
    assert_eq!(active_workspace(&serve), id, "select:false keeps focus where it was");

    let missing = serve.json("create_worktree_workspace", &json!({ "branch": "x" }).to_string(), &[]);
    assert_eq!(missing["error"], "Missing required parameter: repo_path", "{missing}");
}
