//! The control socket end to end through the built `codemux` binary: external
//! CLI processes can read but cannot claim owner authority from a same-user
//! PID, name, or agent marker.
//!
//! Each test runs its own `codemux serve` child in a throwaway home with its
//! own `XDG_RUNTIME_DIR`, and drives it with the `codemux` CLI. Every child
//! starts from a cleared environment plus an allow-list, so neither a
//! developer's running app nor the agent identity of a chat this suite runs
//! under can leak in.

#![cfg(unix)]

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::os::unix::net::UnixListener;
use std::path::Path;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

const CODEMUX: &str = env!("CARGO_BIN_EXE_codemux");
const SOCKET: &str = if cfg!(debug_assertions) { "codemux-dev.sock" } else { "codemux.sock" };
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
        if key == "XDG_RUNTIME_DIR" {
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
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
                let socket = self.root.path().join("run").join(SOCKET);
                assert!(
                    std::fs::symlink_metadata(&socket).is_ok_and(|meta| meta.file_type().is_socket()),
                    "serve answered outside its private runtime dir: {socket:?}"
                );
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

    /// [`Serve::json`] as the owner, but a reply slower than `limit` fails
    /// the test (the CLI is stopped by its pid) instead of hanging it.
    fn json_within(&self, command: &str, params: &str, limit: Duration) -> Value {
        let out = self.root.path().join(format!("{command}.reply"));
        let mut cmd = Command::new(CODEMUX);
        isolate(&mut cmd, self.root.path());
        let stdout = std::fs::File::create(&out).unwrap();
        let mut child = cmd.args(["json", command, params]).stdout(stdout).spawn().unwrap();
        let deadline = Instant::now() + limit;
        while child.try_wait().unwrap().is_none() {
            if Instant::now() > deadline {
                let _ = child.kill();
                let _ = child.wait();
                panic!("`codemux json {command}` got no reply within {limit:?}:\n{}", self.log());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let reply = std::fs::read_to_string(&out).unwrap();
        serde_json::from_str(&reply).unwrap_or_else(|_| panic!("`codemux json {command}` printed {reply:?}:\n{}", self.log()))
    }

    /// Stops the serve and keeps its home. This test started it; stop it by
    /// its pid, and never signal a pid already reaped, which may be reused.
    fn stop(&mut self) {
        if self.child.try_wait().ok().flatten().is_some() {
            return;
        }
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

impl Drop for Serve {
    fn drop(&mut self) {
        self.stop();
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

    /// [`Mcp::call`] tagged with `workspace`, as a chat's calls are, so the
    /// git tools run in that workspace's folder.
    fn call_in(&mut self, workspace: &str, name: &str, arguments: Value) -> Value {
        let meta = json!({"codemux/workspace_id": workspace});
        self.request("tools/call", json!({"name": name, "arguments": arguments, "_meta": meta}))["result"].clone()
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
fn external_cli_reads_but_cannot_claim_owner_or_agent_access() {
    let serve = Serve::start(&[]);
    let marker = [(AGENT_PROCESS, "1")];

    let refused = serve.json("agent_connector_status", "{}", &marker);
    assert_eq!(refused["ok"], false, "{refused}");
    assert_eq!(error_code(&refused), "admin_refused", "{refused}");
    assert_eq!(refused["protocol_version"], 2);

    let terminal_agent = serve.json("web_remote_pair", r#"{"name":"phone"}"#, &[(AGENT_PROCESS, "terminal")]);
    assert_eq!(error_code(&terminal_agent), "admin_refused", "a terminal agent uses the current marker: {terminal_agent}");

    let legacy = serve.json("agent_connector_status", "{}", &[(PROVIDER_ORIGIN, "1")]);
    assert_eq!(error_code(&legacy), "admin_refused", "the retired marker cannot claim owner access: {legacy}");

    let owner = serve.json("agent_connector_status", "{}", &[]);
    assert_eq!(error_code(&owner), "admin_refused", "a separate CLI has no owner proof: {owner}");

    let status = serve.json("status", "{}", &marker);
    assert_eq!(status["ok"], true, "agents keep read commands: {status}");
    let notify = serve.json("notify", r#"{"message":"from an agent"}"#, &marker);
    assert_eq!(error_code(&notify), "access_denied", "the marker cannot grant writes: {notify}");

    let native = serve.json("thread_list", r#"{"workspace_id":"none"}"#, &marker);
    assert_eq!(error_code(&native), "workspace_not_found", "registry reads reach the app as unverified: {native}");
    let tools = serve.json("tools_list", "{}", &[]);
    assert_eq!(tools["data"]["caller"], json!({"principal": "socket:unverified", "access": "read_only"}), "{tools}");
    assert!(tools["data"]["tools"].as_array().unwrap().iter().all(|tool| tool["annotations"]["readOnlyHint"] == true));
    let read = serve.json("tool_call", r#"{"name":"workspace_list","arguments":{}}"#, &[]);
    assert_eq!(read["data"]["isError"], false, "{read}");
    let write = serve.json("tool_call", r#"{"name":"workspace_create","arguments":{"path":"/tmp"}}"#, &[]);
    assert_eq!(write["data"]["structuredContent"]["error"]["code"], "access_denied", "{write}");
}

#[test]
fn inherited_identity_cannot_authorize_an_external_cli() {
    // The serve process may have been launched from an agent shell, but its
    // separate CLI peers still need independent owner proof.
    let serve = Serve::start(&[
        (AGENT_PROCESS, "1"),
        (PROVIDER_ORIGIN, "1"),
        ("CODEMUX_MCP_GATEWAY_URL", "http://127.0.0.1:1/mcp"),
        ("CODEMUX_MCP_GATEWAY_TOKEN", "inherited"),
    ]);
    let project = serve.root.path().join("project");
    std::fs::create_dir(&project).unwrap();
    assert_eq!(serve.json("status", "{}", &[])["ok"], true);
    let admin = serve.json("agent_connector_status", "{}", &[]);
    assert_eq!(error_code(&admin), "admin_refused", "{admin}");
    let create = serve.json("create_workspace", &json!({ "path": project }).to_string(), &[]);
    assert_eq!(error_code(&create), "access_denied", "{create}");
}

#[test]
fn pty_shell_scrubs_inherited_agent_identity() {
    // The app starts its initial terminal through its trusted startup path.
    // The external socket cannot create a workspace or write to a PTY.
    let fixture = tempfile::tempdir().unwrap();
    let shell = fixture.path().join("capture-shell");
    let env_dump = fixture.path().join("env.txt");
    let env_dump_partial = fixture.path().join("env.txt.partial");
    std::fs::write(
        &shell,
        format!(
            "#!/bin/sh\nenv > '{}' && mv '{}' '{}'\nexec /bin/sh\n",
            env_dump_partial.display(),
            env_dump_partial.display(),
            env_dump.display(),
        ),
    )
    .unwrap();
    std::fs::set_permissions(&shell, std::fs::Permissions::from_mode(0o700)).unwrap();

    let serve = Serve::start(&[
        (AGENT_PROCESS, "1"),
        (PROVIDER_ORIGIN, "1"),
        ("CODEMUX_MCP_GATEWAY_URL", "http://127.0.0.1:1/mcp"),
        ("CODEMUX_MCP_GATEWAY_TOKEN", "inherited"),
        ("SHELL", shell.to_str().unwrap()),
    ]);
    let deadline = Instant::now() + Duration::from_secs(60);
    while !env_dump.exists() {
        assert!(Instant::now() < deadline, "the startup terminal never ran:\n{}", serve.log());
        std::thread::sleep(Duration::from_millis(100));
    }
    let env = std::fs::read_to_string(&env_dump).unwrap();
    for key in IDENTITY_VARS {
        assert!(!env.lines().any(|line| line.starts_with(&format!("{key}="))), "{key} reached the terminal");
    }
    assert!(env.contains("CODEMUX_SESSION_ID="), "the dump is the terminal's env");
}

/// An unverified CLI cannot start a project's setup script, even if it
/// supplies or clears the old agent marker.
#[test]
fn external_cli_cannot_trigger_project_setup() {
    let serve = Serve::start(&[]);
    for (name, env) in [("plain", &[][..]), ("marked", &[(AGENT_PROCESS, "1")][..])] {
        let project = serve.root.path().join(name);
        std::fs::create_dir_all(project.join(".codemux")).unwrap();
        let ran = project.join("setup-ran");
        let script = format!("touch '{}'", ran.display());
        std::fs::write(project.join(".codemux/config.json"), json!({ "setup": [script] }).to_string()).unwrap();
        let created = serve.json("create_workspace", &json!({ "path": project }).to_string(), env);
        assert_eq!(error_code(&created), "access_denied", "{name}: {created}");
        assert!(!ran.exists(), "a refused setup ran for {name}");
    }
}

/// The legacy setup and close commands also gate access before they
/// attempt to resolve a workspace or execute its scripts.
#[test]
fn external_cli_cannot_run_setup_or_teardown_commands() {
    let serve = Serve::start(&[]);
    for command in ["rerun_setup", "close_workspace", "archive_workspace"] {
        let reply = serve.json_within(command, r#"{"workspace_id":"missing"}"#, Duration::from_secs(10));
        assert_eq!(error_code(&reply), "access_denied", "{command}: {reply}");
    }
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
    // The app caps every external stdio relay at read-only, regardless of a
    // client's requested access, before resolving a write's target.
    let send = json!({"workspace_id": "none", "thread_id": "t", "client_request_id": "k", "message": "x"});
    let denied = read_only.call("thread_send", send.clone());
    assert_eq!(denied["isError"], true, "{denied}");
    assert_eq!(denied["structuredContent"]["error"]["code"], "access_denied", "{denied}");
    let reached = supervised.call("thread_send", send);
    assert_eq!(reached["structuredContent"]["error"]["code"], "access_denied", "{reached}");
}

/// The legacy git tools run inside `codemux mcp`, so no gate in the app sees
/// their arguments. A file name must stay a path: as a git option,
/// `--output=<file>` made `git diff` write that file through a read-only
/// connection.
#[test]
fn mcp_git_tools_take_file_names_as_paths() {
    let serve = Serve::start(&[]);
    let root = serve.root.path();
    // Legacy Git tools execute in the MCP process's launch directory when
    // there is no workspace. No socket write is needed to exercise their
    // path argument handling.
    let repo = root.to_path_buf();
    git(&repo, &["init", "-b", "trunk"]);
    git(&repo, &["config", "user.name", "Codemux test"]);
    git(&repo, &["config", "user.email", "test@example.invalid"]);
    std::fs::write(repo.join("tracked"), "one\n").unwrap();
    git(&repo, &["add", "tracked"]);
    git(&repo, &["commit", "-m", "first"]);
    // A change, so a diff written anywhere would not be empty.
    std::fs::write(repo.join("tracked"), "two\n").unwrap();
    let (kept, fresh) = (root.join("kept.txt"), root.join("fresh.txt"));
    std::fs::write(&kept, "keep\n").unwrap();
    let mut read_only = Mcp::start(&serve, &["--client", "integration-bot", "--access", "read-only"]);
    for target in [&kept, &fresh] {
        let file = format!("--output={}", target.display());
        let reply = read_only.call("git_diff", json!({ "file": file }));
        assert_ne!(reply["isError"], true, "{reply}");
        assert_eq!(reply["content"][0]["text"], "", "no file has that name: {reply}");
    }
    assert_eq!(std::fs::read_to_string(&kept).unwrap(), "keep\n", "git_diff overwrote a file");
    assert!(!fresh.exists(), "git_diff created a file");
    let diff = read_only.call("git_diff", json!({ "file": "tracked" }));
    assert!(diff["content"][0]["text"].as_str().unwrap().contains("+two"), "{diff}");

    // `--all` as an option would stage the untracked file too.
    std::fs::write(repo.join("untracked"), "new\n").unwrap();
    let mut supervised = Mcp::start(&serve, &["--client", "integration-bot", "--access", "supervised"]);
    let staged = supervised.call("git_stage", json!({ "file": "--all" }));
    assert_eq!(staged["isError"], true, "no file has that name: {staged}");
    assert_eq!(git(&repo, &["diff", "--cached", "--name-only"]), "", "nothing was staged");
    let staged = supervised.call("git_stage", json!({ "file": "tracked" }));
    assert_ne!(staged["isError"], true, "{staged}");
    assert_eq!(git(&repo, &["diff", "--cached", "--name-only"]), "tracked");
    // `-m` takes the next argument whatever it looks like.
    let committed = supervised.call("git_commit", json!({ "message": "--amend" }));
    assert_ne!(committed["isError"], true, "{committed}");
    assert_eq!(git(&repo, &["log", "--format=%s"]), "--amend\nfirst", "a new commit, not an amended one");
}

#[test]
fn mcp_git_tools_use_launch_directory_only_without_a_workspace() {
    let serve = Serve::start(&[]);
    let cwd = serve.root.path();
    git(cwd, &["init"]);
    let file = cwd.join("local.txt");
    std::fs::write(&file, "original\n").unwrap();
    git(cwd, &["add", "local.txt"]);
    std::fs::write(&file, "edited in launch directory\n").unwrap();
    let mut mcp = Mcp::start(&serve, &["--client", "integration-bot", "--access", "read-only"]);

    let args = json!({ "file": "local.txt" });
    let missing = mcp.call_in("closed-workspace", "git_diff", args.clone());
    assert_eq!(missing["isError"], true, "{missing}");
    assert!(missing["content"][0]["text"].as_str().unwrap().contains("closed-workspace"), "{missing}");

    let unscoped = mcp.call("git_diff", args);
    assert_ne!(unscoped["isError"], true, "{unscoped}");
    assert!(unscoped["content"][0]["text"].as_str().unwrap().contains("+edited in launch directory"), "{unscoped}");
}

/// Even a client requesting Supervised access cannot trigger teardown through
/// the stdio or legacy socket route. The gate runs before workspace lookup.
#[test]
fn external_clients_cannot_run_archive_or_close() {
    let serve = Serve::start(&[]);
    let mut supervised = Mcp::start(&serve, &["--client", "integration-bot", "--access", "supervised"]);
    for name in ["workspace_archive", "workspace_close"] {
        let refused = supervised.call(name, json!({ "workspace_id": "missing" }));
        assert_eq!(refused["structuredContent"]["error"]["code"], "access_denied", "{name}: {refused}");
    }
    let as_bot = [("CODEMUX_CLIENT", "integration-bot"), ("CODEMUX_ACCESS", "supervised")];
    for command in ["archive_workspace", "close_workspace"] {
        for env in [&as_bot[..], &[][..]] {
            let refused = serve.json(command, r#"{"workspace_id":"missing"}"#, env);
            assert_eq!(error_code(&refused), "access_denied", "{command}: {refused}");
        }
    }
    let names = supervised.tool_names();
    for hidden in ["workspace_archive", "workspace_close"] {
        assert!(!names.iter().any(|name| name == hidden), "supervised lists {hidden}: {names:?}");
    }
}

/// A control socket like a Codemux build from before `client.access`
/// (protocol 1), bound where `path`'s app was: it answers every line the way
/// that build would (no `tool_call`, the legacy commands succeed) and records
/// each command.
fn old_app_at(path: &Path) -> Arc<Mutex<Vec<String>>> {
    let _ = std::fs::remove_file(path);
    let listener = UnixListener::bind(path).unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&seen);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { return };
            for line in BufReader::new(&stream).lines() {
                let Ok(line) = line else { break };
                let command = serde_json::from_str::<Value>(&line).unwrap()["command"].as_str().unwrap().to_owned();
                recorded.lock().unwrap().push(command.clone());
                let reply = match command.as_str() {
                    "tool_call" => json!({"ok": false, "protocol_version": 1, "data": null, "error": "Unknown control command: tool_call"}),
                    _ => json!({"ok": true, "protocol_version": 1, "data": {}, "error": null}),
                };
                if writeln!(&stream, "{reply}").is_err() {
                    break;
                }
            }
        }
    });
    seen
}

/// `codemux mcp --access read-only` outlives the app it first reached, and an
/// older build comes up on the same socket. That build ignores `client`, so
/// the client must refuse before the call reaches it, including a registry
/// twin it would answer from its legacy arm on an app without `tool_call`.
#[test]
fn narrowed_mcp_refuses_an_old_app_that_replaced_a_new_one() {
    let mut serve = Serve::start(&[]);
    let status = serve.json("status", "{}", &[]);
    let socket = std::path::PathBuf::from(status["data"]["socket_path"].as_str().unwrap_or_else(|| panic!("{status}")));
    let mut mcp = Mcp::start(&serve, &["--access", "read-only"]);
    let listed = mcp.call("workspace_list", json!({}));
    assert_eq!(listed["isError"], false, "the new app answers: {listed}");

    serve.stop();
    let seen = old_app_at(&socket);
    let project = serve.root.path().join("project");
    std::fs::create_dir(&project).unwrap();
    let refused = mcp.call("workspace_create", json!({"path": project}));
    assert_eq!(refused["isError"], true, "{refused}");
    assert_eq!(refused["structuredContent"]["error"]["code"], "server_too_old", "{refused}");
    assert_eq!(*seen.lock().unwrap(), ["status"], "only the version probe reached the older app");
}

#[test]
fn legacy_workspace_writes_require_owner_proof() {
    let serve = Serve::start(&[]);
    let project = serve.root.path().join("project");
    std::fs::create_dir(&project).unwrap();
    for (command, params) in [
        ("create_workspace", json!({ "path": project })),
        ("close_workspace", json!({ "workspace_id": "missing" })),
    ] {
        let refused = serve.json(command, &params.to_string(), &[]);
        assert_eq!(error_code(&refused), "access_denied", "{command}: {refused}");
    }
    let state = serve.json("get_app_state", "{}", &[]);
    assert_eq!(state["ok"], true, "read-only state remains available: {state}");
    assert!(!state["data"]["workspaces"].as_array().unwrap().iter().any(|workspace|
        workspace["cwd"] == project.display().to_string()), "the refused create opened the project: {state}");
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
fn external_cli_cannot_create_a_worktree_branch() {
    let serve = Serve::start(&[]);
    let repo = serve.root.path().join("trunk-repo");
    std::fs::create_dir_all(&repo).unwrap();
    git(&repo, &["init", "-b", "trunk"]);
    git(&repo, &["commit", "--allow-empty", "-m", "first"]);
    let trunk = git(&repo, &["rev-parse", "HEAD"]);

    let params = json!({ "repo_path": repo, "branch": "feat", "layout": "empty" });
    let refused = serve.json("create_worktree_workspace", &params.to_string(), &[]);
    assert_eq!(error_code(&refused), "access_denied", "{refused}");
    assert_eq!(git(&repo, &["rev-parse", "HEAD"]), trunk);
    assert_eq!(git(&repo, &["branch", "--list", "feat"]), "", "a refused call created a branch");
}
