//! `codemux-remote mcp`: the stdio MCP server an agent on this host runs to
//! reach this host's codemux-remote tools.
//!
//! It manages this host's folders, git worktrees and shells through the
//! local `codemux-remote serve` daemon. It does not run Codemux chats: an
//! agent that wants threads connects to the Codemux app instead.
//!
//! - `initialize` negotiates the version with the shared protocol helpers
//!   and returns [`INSTRUCTIONS`].
//! - `tools/list` comes from the compiled catalog, so it answers without a
//!   daemon. The bridge and the daemon are the same binary, so an older
//!   daemon that is still running only answers "unknown tool" for a tool it
//!   predates, until `serve` restarts.
//! - `tools/call` forwards to the daemon's `/tools/call`. The result text is
//!   pretty-printed JSON of the reply's `data`, or of its `{kind, message}`
//!   error: vexis-agent matches `"kind": "not_found"` in it, space included.
//! - With no daemon running, a call returns an `isError` result of kind
//!   `unavailable`, not a JSON-RPC error, so the model can tell the user.
//! - `ping` answers `{}`. Notifications and client replies get no answer.
//!   Any other method is -32601.
//!
//! Requests are handled one at a time. codemux-remote has no long waits,
//! so a slow call cannot hold up anything a client would want to cancel.

use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::time::Duration;

use reqwest::StatusCode;
use serde_json::{json, Value};

use super::manifest::{self, Manifest};
use super::{config, tools};
use crate::mcp::protocol::{
    self, CallToolParams, Incoming, InitializeRequest, InitializeResult, Request, RequestId,
    Response, RpcError, ServerInfo,
};

/// What `initialize` tells the client this server is for.
pub const INSTRUCTIONS: &str =
    "codemux-remote manages folders, git worktrees and shells on this host. \
    Workspaces you register here appear under Devices in the Codemux app. It does not run Codemux \
    chats: to start, message or wait on threads, connect to the Codemux app instead (`codemux mcp` \
    on the machine running it, or its network connector from another machine).";

const SERVER: ServerInfo = ServerInfo {
    name: "codemux-remote",
    version: env!("CARGO_PKG_VERSION"),
};

/// The daemon is on loopback, so a connection that takes longer than this
/// is not coming.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

const NOT_RUNNING: &str = "codemux-remote serve isn't running on this host. Start it with \
    `systemctl --user start codemux-remote` or `codemux-remote serve`.";

pub fn run_stdio(state_dir: PathBuf) -> Result<(), String> {
    let mut daemon = Daemon::new(config::manifest_path(&state_dir))?;
    serve(
        std::io::stdin().lock(),
        std::io::stdout().lock(),
        &mut daemon,
    )
}

/// Answer newline-delimited JSON-RPC from `input` until EOF.
fn serve(
    mut input: impl BufRead,
    mut output: impl Write,
    daemon: &mut Daemon,
) -> Result<(), String> {
    // Bytes, not `String`: one line of invalid UTF-8 is a parse error the
    // client hears about, not the end of the session.
    let mut line = Vec::new();
    loop {
        line.clear();
        match input.read_until(b'\n', &mut line) {
            Ok(0) => return Ok(()),
            Ok(_) => {}
            Err(e) => return Err(format!("stdin read: {e}")),
        }
        if line.iter().all(u8::is_ascii_whitespace) {
            continue;
        }
        if let Some(response) = handle_line(&line, daemon) {
            writeln!(output, "{}", response.to_line()).map_err(|e| format!("stdout write: {e}"))?;
            output.flush().map_err(|e| format!("stdout flush: {e}"))?;
        }
    }
}

/// The reply to one line, if it needs one.
fn handle_line(line: &[u8], daemon: &mut Daemon) -> Option<Response> {
    let message: Value = match serde_json::from_slice(line) {
        Ok(message) => message,
        Err(e) => return Some(Response::error(None, RpcError::parse_error(e))),
    };
    // An envelope error still carries the id when it is readable.
    let id = message.get("id").and_then(RequestId::from_value);
    match protocol::parse(message) {
        Ok(Incoming::Request(request)) => {
            let outcome = handle_request(&request, daemon);
            Some(Response::from_outcome(request.id, outcome))
        }
        Ok(Incoming::Notification(_) | Incoming::Response) => None,
        Err(error) => Some(Response::error(id, error)),
    }
}

fn handle_request(request: &Request, daemon: &mut Daemon) -> Result<Value, RpcError> {
    match request.method.as_str() {
        "initialize" => Ok(InitializeResult {
            protocol_version: InitializeRequest::parse(&request.params).negotiated_version(),
            tools_list_changed: Some(false),
            server_info: SERVER,
            instructions: Some(INSTRUCTIONS),
        }
        .to_value()),
        "ping" => Ok(json!({})),
        "tools/list" => Ok(tools_list()),
        "tools/call" => {
            let call = CallToolParams::parse(&request.params)?;
            tool_result(daemon.call_tool(&call.name, &call.arguments))
        }
        other => Err(RpcError::method_not_found(other)),
    }
}

fn tools_list() -> Value {
    let tools: Vec<Value> = tools::catalog()
        .into_iter()
        .map(|tool| {
            json!({
                "name": tool.name,
                "description": tool.description,
                "inputSchema": tool.input_schema,
            })
        })
        .collect();
    json!({ "tools": tools })
}

/// Turn a daemon call into an MCP tool result.
fn tool_result(outcome: CallOutcome) -> Result<Value, RpcError> {
    let (payload, is_error) = match outcome {
        CallOutcome::Answered { status, body } => {
            let is_error = !status.is_success() || body.get("ok") == Some(&json!(false));
            let payload = if is_error {
                body.get("error")
                    .cloned()
                    .unwrap_or_else(|| json!({ "kind": "internal", "message": "daemon error" }))
            } else {
                body.get("data").cloned().unwrap_or(json!({}))
            };
            (payload, is_error)
        }
        CallOutcome::Unavailable(message) => {
            (json!({ "kind": "unavailable", "message": message }), true)
        }
        CallOutcome::Failed(message) => {
            return Err(RpcError::new(protocol::code::SERVER_ERROR, message))
        }
    };
    let text = serde_json::to_string_pretty(&payload).unwrap_or_else(|_| payload.to_string());
    Ok(protocol::text_with_status(text, is_error))
}

/// What came of one tool call through the daemon.
#[derive(Debug)]
enum CallOutcome {
    /// The daemon answered, with this HTTP status and JSON body.
    Answered { status: StatusCode, body: Value },
    /// There is no daemon to call: no manifest, a dead pid, or a daemon
    /// that refused the connection or the secret even from a fresh
    /// manifest.
    Unavailable(String),
    /// The call reached the daemon but no usable answer came back, such as
    /// a timeout. The tool may have run, so it is not retried.
    Failed(String),
}

/// How one HTTP attempt ended.
enum Sent {
    Answered {
        status: StatusCode,
        body: Value,
    },
    /// The request never reached a tool: the connection was refused, or the
    /// daemon refused the secret.
    NotReached,
    Failed(String),
}

/// The local daemon as last read from its manifest. Every `serve` start
/// picks a new port and secret, and a host upgrade restarts it under
/// running agents, so a refused or unreachable call reads the manifest
/// again and retries once. Only connect errors and 401s retry: the
/// request never reached a tool.
struct Daemon {
    manifest_path: PathBuf,
    manifest: Option<Manifest>,
    http: reqwest::blocking::Client,
}

impl Daemon {
    fn new(manifest_path: PathBuf) -> Result<Self, String> {
        let http = reqwest::blocking::Client::builder()
            .timeout(tools::CALL_TIMEOUT)
            .connect_timeout(CONNECT_TIMEOUT)
            // A new loopback connection per call: a pooled one could outlive
            // a daemon restart and break mid-request, where a new one is
            // refused cleanly and retried.
            .pool_max_idle_per_host(0)
            .build()
            .map_err(|e| format!("build http client: {e}"))?;
        Ok(Self {
            manifest_path,
            manifest: None,
            http,
        })
    }

    fn call_tool(&mut self, name: &str, arguments: &Value) -> CallOutcome {
        // At most two attempts: a stale cached manifest is dropped and read
        // again; a manifest that was just read is not.
        loop {
            let (manifest, fresh) = match self.manifest() {
                Ok(found) => found,
                Err(message) => return CallOutcome::Unavailable(message),
            };
            match self.post(&manifest, name, arguments) {
                Sent::Answered { status, body } => return CallOutcome::Answered { status, body },
                Sent::Failed(message) => return CallOutcome::Failed(message),
                Sent::NotReached => {
                    self.manifest = None;
                    if fresh {
                        return CallOutcome::Unavailable(NOT_RUNNING.to_string());
                    }
                }
            }
        }
    }

    /// The cached manifest while its daemon's pid is alive, otherwise a
    /// fresh read; `true` marks a fresh one.
    fn manifest(&mut self) -> Result<(Manifest, bool), String> {
        if let Some(cached) = self
            .manifest
            .as_ref()
            .filter(|m| manifest::pid_alive(m.pid))
        {
            return Ok((cached.clone(), false));
        }
        self.manifest = None;
        let read = manifest::read(&self.manifest_path).map_err(|e| {
            format!(
                "codemux-remote can't read its daemon manifest at {}: {e}. Restart it with \
                 `systemctl --user restart codemux-remote` or `codemux-remote serve`.",
                self.manifest_path.display()
            )
        })?;
        match read {
            Some(found) if manifest::pid_alive(found.pid) => {
                self.manifest = Some(found.clone());
                Ok((found, true))
            }
            _ => Err(NOT_RUNNING.to_string()),
        }
    }

    fn post(&self, manifest: &Manifest, name: &str, arguments: &Value) -> Sent {
        let response = match self
            .http
            .post(format!("{}/tools/call", manifest.endpoint))
            .bearer_auth(&manifest.secret)
            .json(&json!({ "name": name, "arguments": arguments }))
            .send()
        {
            Ok(response) => response,
            Err(e) if e.is_connect() => return Sent::NotReached,
            Err(e) => return Sent::Failed(format!("daemon /tools/call: {e}")),
        };
        let status = response.status();
        if status == StatusCode::UNAUTHORIZED {
            return Sent::NotReached;
        }
        match response.json() {
            Ok(body) => Sent::Answered { status, body },
            Err(e) => Sent::Failed(format!("decode tools/call: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::pty::PtyManager;
    use crate::remote::server::{self, DaemonState};
    use crate::remote::workspace::WorkspaceStore;
    use std::sync::Arc;
    use tempfile::TempDir;

    /// A real daemon router on its own runtime. Stopping it shuts the
    /// runtime down, which closes the listener and every open connection,
    /// as a `serve` exit would.
    struct TestDaemon {
        runtime: Option<tokio::runtime::Runtime>,
        manifest: Manifest,
        _dir: TempDir,
    }

    impl TestDaemon {
        fn start(host_id: &str, secret: &str) -> Self {
            let dir = TempDir::new().unwrap();
            let workspaces = WorkspaceStore::open(
                &dir.path().join("codemux.db"),
                host_id.into(),
                dir.path().join("workspaces"),
            )
            .unwrap();
            let started_at = chrono::Utc::now().to_rfc3339();
            let state = Arc::new(DaemonState {
                secret: secret.into(),
                started_at: started_at.clone(),
                workspaces,
                ptys: PtyManager::new(),
            });
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .enable_all()
                .build()
                .unwrap();
            let listener = runtime.block_on(server::bind_listener(None)).unwrap();
            let endpoint = format!("http://{}", listener.local_addr().unwrap());
            runtime.spawn(async move { axum::serve(listener, server::router(state)).await });
            let manifest = Manifest {
                endpoint,
                secret: secret.into(),
                pid: std::process::id(),
                started_at,
                host_id: host_id.into(),
                owner_id: None,
            };
            Self {
                runtime: Some(runtime),
                manifest,
                _dir: dir,
            }
        }

        fn stop(mut self) {
            self.runtime
                .take()
                .unwrap()
                .shutdown_timeout(Duration::from_secs(5));
        }
    }

    /// A state dir whose manifest names `daemon`, or none when `None`.
    fn state_dir(daemon: Option<&Manifest>) -> (TempDir, PathBuf) {
        let dir = TempDir::new().unwrap();
        let path = config::manifest_path(dir.path());
        if let Some(manifest) = daemon {
            manifest::write(&path, manifest).unwrap();
        }
        (dir, path)
    }

    fn rpc(daemon: &mut Daemon, message: Value) -> Value {
        handle_line(message.to_string().as_bytes(), daemon)
            .expect("a request gets a reply")
            .to_value()
    }

    fn call(daemon: &mut Daemon, name: &str, arguments: Value) -> Value {
        let reply = rpc(
            daemon,
            json!({ "jsonrpc": "2.0", "id": 7, "method": "tools/call", "params": { "name": name, "arguments": arguments } }),
        );
        assert_eq!(reply["id"], json!(7));
        assert!(
            reply.get("error").is_none(),
            "a tool call is never a JSON-RPC error here: {reply}"
        );
        reply["result"].clone()
    }

    fn text(result: &Value) -> &str {
        result["content"][0]["text"].as_str().expect("text content")
    }

    fn initialize(daemon: &mut Daemon, offer: Option<&str>) -> Value {
        let mut params =
            json!({ "capabilities": {}, "clientInfo": { "name": "test", "version": "0" } });
        if let Some(offer) = offer {
            params["protocolVersion"] = json!(offer);
        }
        rpc(
            daemon,
            json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": params }),
        )["result"]
            .clone()
    }

    #[test]
    fn initialize_explains_what_this_server_controls() {
        let (_dir, path) = state_dir(None);
        let mut daemon = Daemon::new(path).unwrap();
        let result = initialize(&mut daemon, Some("2025-06-18"));
        assert_eq!(result["serverInfo"]["name"], json!("codemux-remote"));
        assert_eq!(
            result["serverInfo"]["version"],
            json!(env!("CARGO_PKG_VERSION"))
        );
        assert_eq!(result["capabilities"]["tools"]["listChanged"], json!(false));
        let instructions = result["instructions"].as_str().unwrap();
        assert!(
            instructions.contains("does not run Codemux chats"),
            "{instructions}"
        );
        assert!(instructions.contains("`codemux mcp`"), "{instructions}");
    }

    #[test]
    fn initialize_negotiates_the_protocol_version() {
        let (_dir, path) = state_dir(None);
        let mut daemon = Daemon::new(path).unwrap();
        // vexis-agent offers 2024-11-05 and still gets it back.
        for offer in protocol::SUPPORTED_PROTOCOL_VERSIONS {
            assert_eq!(
                initialize(&mut daemon, Some(offer))["protocolVersion"],
                json!(offer)
            );
        }
        for offer in [Some("2026-07-28"), Some("1999-01-01"), None] {
            assert_eq!(
                initialize(&mut daemon, offer)["protocolVersion"],
                json!(protocol::LATEST_PROTOCOL_VERSION),
                "{offer:?}"
            );
        }
    }

    #[test]
    fn tools_list_works_without_a_daemon() {
        let (_dir, path) = state_dir(None);
        let mut daemon = Daemon::new(path).unwrap();
        let reply = rpc(
            &mut daemon,
            json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
        );
        let listed = reply["result"]["tools"].as_array().expect("tools");
        assert_eq!(listed.len(), 12);
        let catalog = tools::catalog();
        for (tool, spec) in listed.iter().zip(&catalog) {
            assert_eq!(tool["name"], json!(spec.name));
            assert_eq!(tool["description"], json!(spec.description));
            assert_eq!(tool["inputSchema"], spec.input_schema);
        }
    }

    #[test]
    fn tool_call_without_a_daemon_is_a_tool_error() {
        let gone = Manifest {
            pid: i32::MAX as u32 - 1,
            ..Manifest::new("http://127.0.0.1:9".into(), "gone".into())
        };
        for manifest in [None, Some(&gone)] {
            let (_dir, path) = state_dir(manifest);
            let mut daemon = Daemon::new(path).unwrap();
            let result = call(&mut daemon, "workspace_list", json!({}));
            assert_eq!(result["isError"], json!(true), "{result}");
            assert!(
                text(&result).contains("\"kind\": \"unavailable\""),
                "{}",
                text(&result)
            );
            assert!(text(&result).contains("isn't running"), "{}", text(&result));
        }

        // A manifest that can't be read is reported as such, still as a
        // tool result.
        let (_dir, path) = state_dir(None);
        std::fs::write(&path, "{").unwrap();
        let mut daemon = Daemon::new(path).unwrap();
        let result = call(&mut daemon, "workspace_list", json!({}));
        assert_eq!(result["isError"], json!(true), "{result}");
        assert!(
            text(&result).contains("\"kind\": \"unavailable\""),
            "{}",
            text(&result)
        );
        assert!(
            text(&result).contains("can't read its daemon manifest"),
            "{}",
            text(&result)
        );
    }

    #[test]
    fn tool_call_reads_the_manifest_again_after_a_restart() {
        let first = TestDaemon::start("host-a", "secret-a");
        let (_dir, path) = state_dir(Some(&first.manifest));
        let mut daemon = Daemon::new(path.clone()).unwrap();
        let host = |result: &Value| {
            serde_json::from_str::<Value>(text(result)).unwrap()["host_id"].clone()
        };

        let result = call(&mut daemon, "app_status", json!({}));
        assert_eq!(result["isError"], json!(false), "{result}");
        assert_eq!(host(&result), json!("host-a"));

        // A restart: a new port and secret, then the old daemon goes away.
        let second = TestDaemon::start("host-b", "secret-b");
        assert_ne!(second.manifest.endpoint, first.manifest.endpoint);
        manifest::write(&path, &second.manifest).unwrap();
        first.stop();

        let result = call(&mut daemon, "app_status", json!({}));
        assert_eq!(result["isError"], json!(false), "{result}");
        assert_eq!(host(&result), json!("host-b"));
        second.stop();
    }

    #[test]
    fn a_refused_secret_reads_the_manifest_again_but_only_once() {
        let running = TestDaemon::start("host", "current");
        let (_dir, path) = state_dir(Some(&running.manifest));
        let mut daemon = Daemon::new(path.clone()).unwrap();
        daemon.manifest = Some(Manifest {
            secret: "previous".into(),
            ..running.manifest.clone()
        });
        let result = call(&mut daemon, "app_status", json!({}));
        assert_eq!(result["isError"], json!(false), "{result}");
        assert_eq!(daemon.manifest.as_ref().unwrap().secret, "current");

        // A manifest that is wrong when freshly read is not retried forever.
        manifest::write(
            &path,
            &Manifest {
                secret: "wrong".into(),
                ..running.manifest.clone()
            },
        )
        .unwrap();
        let mut daemon = Daemon::new(path).unwrap();
        let result = call(&mut daemon, "app_status", json!({}));
        assert_eq!(result["isError"], json!(true), "{result}");
        assert!(
            text(&result).contains("\"kind\": \"unavailable\""),
            "{}",
            text(&result)
        );
        running.stop();
    }

    #[test]
    fn tool_errors_keep_pretty_json_text() {
        let running = TestDaemon::start("host", "secret");
        let (_dir, path) = state_dir(Some(&running.manifest));
        let mut daemon = Daemon::new(path).unwrap();
        let result = call(
            &mut daemon,
            "terminal_read",
            json!({ "terminal_id": "no-such-terminal", "max_bytes": 1 }),
        );
        assert_eq!(result["isError"], json!(true), "{result}");
        assert!(
            text(&result).contains("\"kind\": \"not_found\""),
            "{}",
            text(&result)
        );

        // Successes are pretty JSON of `data`, with `isError` stated.
        let result = call(&mut daemon, "workspace_list", json!({}));
        assert_eq!(result["isError"], json!(false));
        assert_eq!(text(&result), "{\n  \"workspaces\": []\n}");
        running.stop();
    }

    /// The stdio loop answers requests in order, skips notifications and
    /// blank lines, reports bad lines and keeps reading.
    #[test]
    fn stdio_answers_requests_and_keeps_reading_after_bad_lines() {
        let (_dir, path) = state_dir(None);
        let mut daemon = Daemon::new(path).unwrap();
        let mut input = Vec::new();
        for line in [
            json!({ "jsonrpc": "2.0", "id": "a", "method": "ping" }).to_string(),
            json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }).to_string(),
            String::new(),
            "not json".to_string(),
            json!({ "jsonrpc": "2.0", "id": 5, "method": "tools/call", "params": [1] }).to_string(),
            json!({ "jsonrpc": "2.0", "id": 6, "method": "tools/call", "params": {} }).to_string(),
            json!({ "jsonrpc": "2.0", "id": 8, "method": "resources/list" }).to_string(),
        ] {
            input.extend_from_slice(line.as_bytes());
            input.push(b'\n');
        }
        input.extend_from_slice(b"\xff\xfe\n");
        input.extend_from_slice(
            json!({ "jsonrpc": "2.0", "id": 9, "method": "ping" })
                .to_string()
                .as_bytes(),
        );

        let mut output = Vec::new();
        serve(input.as_slice(), &mut output, &mut daemon).unwrap();
        let replies: Vec<Value> = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        let summary: Vec<(Value, Value)> = replies
            .iter()
            .map(|reply| (reply["id"].clone(), reply["error"]["code"].clone()))
            .collect();
        assert_eq!(
            summary,
            vec![
                (json!("a"), Value::Null),
                (Value::Null, json!(protocol::code::PARSE_ERROR)),
                (json!(5), json!(protocol::code::INVALID_REQUEST)),
                (json!(6), json!(protocol::code::INVALID_PARAMS)),
                (json!(8), json!(protocol::code::METHOD_NOT_FOUND)),
                (Value::Null, json!(protocol::code::PARSE_ERROR)),
                (json!(9), Value::Null),
            ],
            "{replies:?}"
        );
        assert_eq!(replies[0]["result"], json!({}));
    }
}
