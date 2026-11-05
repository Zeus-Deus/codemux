//! `codemux mcp` as a real child process, driven the way MCP clients drive it.
//!
//! The child gets a cleared environment and private home and runtime dirs, so
//! it can never reach a developer's running app, and an agent shell running
//! these tests cannot change what it lists. The test process's own
//! environment is never touched.

use std::io::{BufRead, BufReader, Write};
use std::path::Path;
#[cfg(unix)]
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

const REPLY_TIMEOUT: Duration = Duration::from_secs(20);

struct Server {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: mpsc::Receiver<String>,
}

impl Server {
    fn spawn(root: &Path) -> Self {
        let mut command = Command::new(env!("CARGO_BIN_EXE_codemux"));
        command.arg("mcp").env_clear();
        for key in ["PATH", "SystemRoot", "windir", "TEMP", "TMP"] {
            if let Some(value) = std::env::var_os(key) {
                command.env(key, value);
            }
        }
        for (key, dir) in [
            ("HOME", "home"),
            ("XDG_RUNTIME_DIR", "run"),
            ("XDG_CONFIG_HOME", "config"),
            ("XDG_DATA_HOME", "data"),
            ("XDG_STATE_HOME", "state"),
            ("XDG_CACHE_HOME", "cache"),
        ] {
            let path = root.join(dir);
            std::fs::create_dir_all(&path).unwrap();
            command.env(key, path);
        }
        command
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = command.spawn().expect("spawn codemux mcp");
        let stdout = child.stdout.take().unwrap();
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        Self { stdin: child.stdin.take(), child, lines }
    }

    fn send(&mut self, message: Value) {
        let stdin = self.stdin.as_mut().expect("stdin still open");
        writeln!(stdin, "{message}").unwrap();
        stdin.flush().unwrap();
    }

    /// The next output line, or `None` when stdout closed or `wait` passed.
    fn next(&self, wait: Duration) -> Option<Value> {
        let line = self.lines.recv_timeout(wait).ok()?;
        Some(serde_json::from_str(&line).unwrap_or_else(|error| panic!("not JSON ({error}): {line}")))
    }

    fn reply(&self) -> Value {
        self.next(REPLY_TIMEOUT).expect("a reply")
    }

    fn close_stdin(&mut self) {
        self.stdin.take();
    }

    /// Every line written until stdout closes.
    fn rest(&self) -> Vec<Value> {
        let mut lines = Vec::new();
        while let Some(line) = self.next(REPLY_TIMEOUT) {
            lines.push(line);
        }
        lines
    }

    fn exits_within(&mut self, limit: Duration) -> Option<std::process::ExitStatus> {
        let deadline = Instant::now() + limit;
        while Instant::now() < deadline {
            if let Some(status) = self.child.try_wait().unwrap() {
                return Some(status);
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        None
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        // Only this test's own child.
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

fn initialize(id: u64, version: &str) -> Value {
    json!({
        "jsonrpc": "2.0", "id": id, "method": "initialize",
        "params": { "protocolVersion": version, "capabilities": {}, "clientInfo": { "name": "stdio-test", "version": "1" } }
    })
}

fn html_render(id: u64) -> Value {
    json!({
        "jsonrpc": "2.0", "id": id, "method": "tools/call",
        "params": { "name": "html_render", "arguments": { "title": "Chart", "html": "<!doctype html><p>hi</p>" } }
    })
}

#[test]
fn discover_probe_gets_method_not_found_then_initialize_echoes_2025_11_25() {
    let root = tempfile::tempdir().unwrap();
    let mut server = Server::spawn(root.path());
    server.send(json!({ "jsonrpc": "2.0", "id": "probe-1", "method": "server/discover", "params": {} }));
    let probe = server.reply();
    assert_eq!(probe["id"], "probe-1", "the fallback signal must carry the probe's id: {probe}");
    assert_eq!(probe["error"]["code"], -32601);

    server.send(initialize(1, "2025-11-25"));
    let init = server.reply();
    assert_eq!(init["id"], 1);
    assert_eq!(init["result"]["protocolVersion"], "2025-11-25");
    assert_eq!(init["result"]["serverInfo"]["name"], "codemux");

    server.send(initialize(2, "2024-11-05"));
    assert_eq!(server.reply()["result"]["protocolVersion"], "2024-11-05", "older offers are echoed");
    server.close_stdin();
    assert!(server.rest().is_empty());
    assert!(server.exits_within(REPLY_TIMEOUT).expect("exits at EOF").success());
}

/// A stateless-era (2026-07-28) request, `_meta` envelope included.
fn modern(id: u64, method: &str, version: &str, mut params: Value) -> Value {
    params["_meta"] = json!({
        "io.modelcontextprotocol/protocolVersion": version,
        "io.modelcontextprotocol/clientInfo": { "name": "modern-test", "version": "1" },
        "io.modelcontextprotocol/clientCapabilities": {}
    });
    json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
}

#[test]
fn stateless_and_initialize_era_clients_share_one_codemux_mcp() {
    if !codemux_lib::mcp::protocol::MODERN_ERA_ENABLED {
        // Switched off, every request is initialize-era, which the other tests cover.
        eprintln!("skipped: the stateless era is switched off (mcp::protocol::MODERN_ERA_ENABLED)");
        return;
    }
    let root = tempfile::tempdir().unwrap();
    let mut server = Server::spawn(root.path());

    // A stateless client probes, is answered, and works without `initialize`.
    server.send(modern(1, "server/discover", "2026-07-28", json!({})));
    let discover = server.reply();
    assert_eq!(discover["id"], 1);
    let result = &discover["result"];
    assert_eq!(result["resultType"], "complete", "{discover}");
    assert_eq!(result["supportedVersions"][0], "2026-07-28");
    assert!(result["supportedVersions"].as_array().unwrap().contains(&json!("2024-11-05")));
    assert_eq!(result["_meta"]["io.modelcontextprotocol/serverInfo"]["name"], "codemux");
    assert!(result["ttlMs"].is_u64() && result["cacheScope"].is_string(), "{discover}");

    server.send(modern(2, "tools/list", "2026-07-28", json!({})));
    let modern_list = server.reply();
    assert_eq!(modern_list["result"]["resultType"], "complete");
    assert_eq!(modern_list["result"]["cacheScope"], "private");

    // An initialize-era client on the same process sees the same catalog, unmarked.
    server.send(initialize(3, "2024-11-05"));
    assert_eq!(server.reply()["result"]["protocolVersion"], "2024-11-05");
    server.send(json!({ "jsonrpc": "2.0", "id": 4, "method": "tools/list" }));
    let legacy_list = server.reply();
    assert_eq!(legacy_list["result"]["tools"], modern_list["result"]["tools"]);
    assert!(legacy_list["result"].get("resultType").is_none(), "{legacy_list}");

    // Both call the same tool; only the stateless result is marked.
    let mut call = html_render(5);
    call["params"]["_meta"] = modern(0, "", "2026-07-28", json!({}))["params"]["_meta"].clone();
    server.send(call);
    let modern_call = server.reply();
    assert_eq!(modern_call["result"]["resultType"], "complete", "{modern_call}");
    server.send(html_render(6));
    let legacy_call = server.reply();
    assert!(legacy_call["result"].get("resultType").is_none(), "{legacy_call}");
    assert_eq!(modern_call["result"]["content"], legacy_call["result"]["content"]);

    // A revision nobody speaks lists the supported ones; no envelope is the fallback signal.
    server.send(modern(7, "server/discover", "2099-01-01", json!({})));
    let refused = server.reply();
    assert_eq!((refused["id"].clone(), refused["error"]["code"].clone()), (json!(7), json!(-32022)), "{refused}");
    assert_eq!(refused["error"]["data"]["requested"], "2099-01-01");
    server.send(json!({ "jsonrpc": "2.0", "id": 8, "method": "server/discover" }));
    assert_eq!(server.reply()["error"]["code"], -32601);

    server.close_stdin();
    assert!(server.rest().is_empty());
    assert!(server.exits_within(REPLY_TIMEOUT).expect("exits at EOF").success());
}

#[test]
fn piped_session_still_gets_its_replies_after_stdin_closes() {
    let root = tempfile::tempdir().unwrap();
    let mut server = Server::spawn(root.path());
    // Everything at once, then EOF: `printf '...' | codemux mcp`.
    server.send(initialize(1, "2025-06-18"));
    server.send(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
    server.send(html_render(2));
    server.close_stdin();
    let lines = server.rest();
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert_eq!(lines[0]["id"], 1);
    assert_eq!(lines[0]["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(lines[1]["id"], 2);
    assert!(lines[1]["result"].get("isError").is_none(), "{}", lines[1]);
    assert!(lines[1]["result"]["content"][0]["text"].as_str().unwrap().contains("Shown to the reader"));
    assert!(server.exits_within(REPLY_TIMEOUT).expect("exits at EOF").success());
}

#[test]
fn notifications_never_get_an_output_line() {
    let root = tempfile::tempdir().unwrap();
    let mut server = Server::spawn(root.path());
    server.send(json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
    server.send(json!({ "jsonrpc": "2.0", "method": "notifications/cancelled", "params": { "requestId": 99 } }));
    server.send(json!({ "jsonrpc": "2.0", "method": "notifications/roots/list_changed" }));
    server.send(json!({ "jsonrpc": "2.0", "id": 9, "method": "ping" }));
    // The first line out is the ping's reply: nothing was written before it.
    let first = server.reply();
    assert_eq!(first["id"], 9, "{first}");
    assert_eq!(first["result"], json!({}));
    server.close_stdin();
    assert!(server.rest().is_empty());
}

/// A control socket that reads each request and never answers, so a native
/// tool call stays in flight.
#[cfg(unix)]
struct SilentSocket {
    path: PathBuf,
    requests: mpsc::Receiver<String>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

#[cfg(unix)]
impl SilentSocket {
    fn bind(runtime_dir: &Path) -> Self {
        use std::sync::atomic::{AtomicBool, Ordering};
        // The basename `codemux` uses for the build under test.
        let name = if cfg!(debug_assertions) { "codemux-dev.sock" } else { "codemux.sock" };
        let path = runtime_dir.join(name);
        std::fs::create_dir_all(runtime_dir).unwrap();
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        let stop = std::sync::Arc::new(AtomicBool::new(false));
        let (tx, requests) = mpsc::channel();
        let stopped = std::sync::Arc::clone(&stop);
        std::thread::spawn(move || {
            let mut held = Vec::new();
            for stream in listener.incoming() {
                if stopped.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(stream) = stream else { break };
                let mut line = String::new();
                let _ = BufReader::new(stream.try_clone().unwrap()).read_line(&mut line);
                let _ = tx.send(line);
                held.push(stream);
            }
        });
        Self { path, requests, stop }
    }

    fn next_request(&self) -> String {
        self.requests.recv_timeout(REPLY_TIMEOUT).expect("the call reached the socket")
    }
}

#[cfg(unix)]
impl Drop for SilentSocket {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
        // Wake the accept loop so its thread ends.
        let _ = std::os::unix::net::UnixStream::connect(&self.path);
    }
}

#[cfg(unix)]
#[test]
fn ping_and_cancel_work_while_a_native_call_waits_on_the_socket() {
    let root = tempfile::tempdir().unwrap();
    let socket = SilentSocket::bind(&root.path().join("run"));
    let mut server = Server::spawn(root.path());
    server.send(initialize(1, "2025-11-25"));
    assert_eq!(server.reply()["id"], 1);

    let wait = |id: u64| {
        json!({
            "jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": { "name": "thread_wait", "arguments": { "workspace_id": "ws", "thread_id": "t", "timeout_ms": 1000 } }
        })
    };
    server.send(wait(2));
    assert!(socket.next_request().contains("thread_wait"));

    // The call is parked at the socket; cheap requests and legacy tools still answer.
    server.send(json!({ "jsonrpc": "2.0", "id": 3, "method": "ping" }));
    assert_eq!(server.reply()["id"], 3);
    server.send(html_render(4));
    assert_eq!(server.reply()["id"], 4);

    // A cancelled call gets no reply, and its id is free again afterwards.
    server.send(json!({ "jsonrpc": "2.0", "method": "notifications/cancelled", "params": { "requestId": 2 } }));
    server.send(json!({ "jsonrpc": "2.0", "id": 5, "method": "ping" }));
    assert_eq!(server.reply()["id"], 5);
    server.send(wait(2));
    assert!(socket.next_request().contains("thread_wait"));

    // Stdin closing ends a parked wait instead of waiting on it.
    server.close_stdin();
    let rest = server.rest();
    assert!(rest.is_empty(), "a cancelled or abandoned wait gets no reply: {rest:?}");
    let status = server.exits_within(Duration::from_secs(10)).expect("a parked wait must not hold the process open");
    assert!(status.success());
}

/// A real dual-era client: the MCP Python SDK (2.x) that Hermes Agent ships.
/// `CODEMUX_TEST_MCP_PYTHON` names its interpreter; otherwise Hermes's own
/// virtualenv is used when it is installed. `None` (the test is skipped)
/// when neither has an SDK that knows the stateless era.
#[cfg(unix)]
fn dual_era_python() -> Option<PathBuf> {
    let python = std::env::var_os("CODEMUX_TEST_MCP_PYTHON").map(PathBuf::from).or_else(|| {
        std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".hermes/hermes-agent/.venv/bin/python"))
    })?;
    let probe = Command::new(&python)
        .args(["-I", "-c", "from mcp.client import Client; from mcp_types.version import MODERN_PROTOCOL_VERSIONS"])
        .env_clear()
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    probe.is_ok_and(|status| status.success()).then_some(python)
}

/// Connects to `codemux mcp` once per connect mode given after the env JSON
/// and reports what each session negotiated, listed and got back.
#[cfg(unix)]
const DUAL_ERA_CLIENT: &str = r#"
import asyncio, json, sys
import mcp_types
from mcp.client import Client
from mcp.client.stdio import StdioServerParameters, stdio_client

async def session(codemux, env, mode):
    server = StdioServerParameters(command=codemux, args=["mcp"], env=env, cwd=env["HOME"])
    info = mcp_types.Implementation(name="sdk-" + mode, version="2.0.0")
    async with Client(stdio_client(server), mode=mode, cache=None, client_info=info) as client:
        listed = await client.list_tools()
        render = await client.call_tool("html_render", {"title": "Chart", "html": "<p>hi</p>"})
        workspaces = await client.call_tool("workspace_list", {})
        discover = client.session.discover_result
        return {
            "version": client.protocol_version,
            "supported": list(discover.supported_versions) if discover else None,
            "server": client.server_info.name if client.server_info else None,
            "tools": sorted(tool.name for tool in listed.tools),
            "render_error": bool(render.is_error),
            "workspaces_error": bool(workspaces.is_error),
            "workspaces": workspaces.structured_content,
        }

async def main():
    codemux, env = sys.argv[1], json.loads(sys.argv[2])
    print(json.dumps({mode: await session(codemux, env, mode) for mode in sys.argv[3:]}))

asyncio.run(main())
"#;

/// An isolated `codemux serve` the SDK's `codemux mcp` children reach through
/// their private `XDG_RUNTIME_DIR`. Stopped by its own pid when dropped.
#[cfg(unix)]
struct Serve {
    child: Child,
    root: tempfile::TempDir,
}

#[cfg(unix)]
impl Serve {
    const DIRS: [(&'static str, &'static str); 5] = [
        ("XDG_CONFIG_HOME", "config"),
        ("XDG_DATA_HOME", "data"),
        ("XDG_STATE_HOME", "state"),
        ("XDG_CACHE_HOME", "cache"),
        ("XDG_RUNTIME_DIR", "run"),
    ];

    /// The isolated env every process of this test runs with.
    fn env(root: &Path) -> Vec<(String, String)> {
        let mut env = vec![("HOME".to_owned(), root.display().to_string())];
        for (key, dir) in Self::DIRS {
            let path = root.join(dir);
            std::fs::create_dir_all(&path).unwrap();
            env.push((key.to_owned(), path.display().to_string()));
        }
        env
    }

    fn start() -> Self {
        let root = tempfile::tempdir().unwrap();
        let log = std::fs::File::create(root.path().join("serve.log")).unwrap();
        let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
        let child = Command::new(env!("CARGO_BIN_EXE_codemux"))
            .env_clear()
            .envs(Self::env(root.path()))
            .env("PATH", "/usr/bin:/bin")
            .env("SHELL", "/bin/sh")
            // In-process terminals and no reachable account API.
            .env("CODEMUX_DISABLE_PTY_DAEMON", "1")
            .env("CODEMUX_API_URL", "http://127.0.0.1:1")
            .current_dir(root.path())
            .args(["serve", "--scope", "loopback", "--port", &port.to_string()])
            .stdin(Stdio::null())
            .stdout(log.try_clone().unwrap())
            .stderr(log)
            .spawn()
            .unwrap();
        let mut serve = Self { child, root };
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            if let Some(status) = serve.child.try_wait().unwrap() {
                panic!("codemux serve exited early ({status}):\n{}", serve.log());
            }
            let status = Command::new(env!("CARGO_BIN_EXE_codemux"))
                .env_clear()
                .envs(Self::env(serve.root.path()))
                .args(["json", "status", "{}"])
                .output()
                .unwrap();
            if serde_json::from_slice::<Value>(&status.stdout).is_ok_and(|reply| reply["ok"] == true) {
                return serve;
            }
            assert!(Instant::now() < deadline, "codemux serve never answered:\n{}", serve.log());
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    fn log(&self) -> String {
        std::fs::read_to_string(self.root.path().join("serve.log")).unwrap_or_default()
    }
}

#[cfg(unix)]
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

/// The MCP Python SDK connects to the same `codemux mcp` build three ways:
/// probing (`auto`, which goes stateless), pinned to 2026-07-28, and with
/// `initialize`. All three list one catalog and run a local tool and a
/// registry tool that reaches the app. With the stateless era switched off,
/// the probing client falls back to `initialize` and nothing is pinned.
#[cfg(unix)]
#[test]
fn a_real_dual_era_sdk_drives_codemux_mcp_in_both_eras() {
    let Some(python) = dual_era_python() else {
        eprintln!("skipped: no MCP Python SDK with the stateless era (set CODEMUX_TEST_MCP_PYTHON)");
        return;
    };
    let stateless = codemux_lib::mcp::protocol::MODERN_ERA_ENABLED;
    let modes: &[&str] = if stateless { &["auto", "2026-07-28", "legacy"] } else { &["auto", "legacy"] };
    let serve = Serve::start();
    let env: serde_json::Map<String, Value> =
        Serve::env(serve.root.path()).into_iter().map(|(key, value)| (key, Value::String(value))).collect();
    let output = Command::new(&python)
        .arg("-I")
        .arg("-c")
        .arg(DUAL_ERA_CLIENT)
        .arg(env!("CARGO_BIN_EXE_codemux"))
        .arg(Value::Object(env).to_string())
        .args(modes)
        .env_clear()
        .envs(Serve::env(serve.root.path()))
        .env("PATH", "/usr/bin:/bin")
        .current_dir(serve.root.path())
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "the SDK client failed:\n{stderr}\nserve:\n{}", serve.log());
    let report: Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("{error}: {}\n{stderr}", String::from_utf8_lossy(&output.stdout)));

    let auto = &report["auto"];
    if stateless {
        assert_eq!(auto["version"], "2026-07-28", "a dual-era client goes stateless: {report}");
        assert_eq!(auto["supported"][0], "2026-07-28");
        assert!(auto["supported"].as_array().unwrap().contains(&json!("2025-11-25")), "{report}");
        assert_eq!(report["2026-07-28"]["version"], "2026-07-28");
    } else {
        assert_eq!(auto["version"], "2025-11-25", "a dual-era client falls back to initialize: {report}");
    }
    assert_eq!(auto["server"], "codemux");
    assert_eq!(report["legacy"]["version"], "2025-11-25", "{report}");
    assert_eq!(report["legacy"]["server"], "codemux");
    for &mode in modes {
        let session = &report[mode];
        assert_eq!(session["tools"], auto["tools"], "{mode}: one catalog");
        assert!(session["tools"].as_array().unwrap().contains(&json!("thread_launch")), "{mode}: {session}");
        assert_eq!(session["render_error"], false, "{mode}: {session}");
        assert_eq!(session["workspaces_error"], false, "{mode}: {session}\nserve:\n{}", serve.log());
        assert!(session["workspaces"]["workspaces"].is_array(), "{mode}: {session}");
    }
}
