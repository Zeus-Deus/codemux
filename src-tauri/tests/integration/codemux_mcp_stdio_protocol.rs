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
