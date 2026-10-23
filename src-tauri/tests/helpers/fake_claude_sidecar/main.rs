//! Fake claude-agent sidecar used by `tests/integration/claude_adapter.rs`.
//!
//! Reads newline-delimited JSON-RPC from stdin. Responds to the same
//! method names the real sidecar exposes, but with canned responses
//! and no actual SDK involvement.
//!
//! A JSON "script" loaded from `FAKE_CLAUDE_SIDECAR_SCRIPT=<path>`
//! describes notifications the fake should emit after specific
//! incoming RPCs. Script entries:
//!
//! ```json
//! [
//!   { "after": "send-turn", "delay_ms": 5, "emit": "notification",
//!     "method": "sdk-message",
//!     "params": { "threadId": "t", "message": {"type": "assistant"} } },
//!   { "after": "send-turn", "delay_ms": 50, "emit": "notification",
//!     "method": "session-ended",
//!     "params": { "threadId": "t", "reason": "iteration-complete" } }
//! ]
//! ```
//!
//! Env toggles:
//!   * `FAKE_CLAUDE_SIDECAR_SCRIPT` — path to the JSON script.
//!   * `FAKE_CLAUDE_SIDECAR_EXIT_AFTER=<method>` — exit 0 after
//!     responding to this method. Used to simulate crashes.
//!   * `FAKE_CLAUDE_STEER_ERROR=<code>:<message>` — reject `steer-turn`,
//!     after `FAKE_CLAUDE_STEER_DELAY_MS` when set.
//!   * `FAKE_CLAUDE_INTERRUPT_ERROR=<message>` — reject `interrupt`, after
//!     `FAKE_CLAUDE_INTERRUPT_DELAY_MS` when set.
//!
//! A scripted `"user_message_uuids": "$steered"` is replaced with the
//! uuids of every `steer-turn` received so far, the way the CLI echoes
//! the prompts a turn consumed.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::sync::{Arc, Mutex};

use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Debug, Deserialize)]
#[allow(dead_code)] // serde decodes all fields; dispatch only uses id/method.
struct IncomingMessage {
    id: Option<Value>,
    method: Option<String>,
    #[serde(default)]
    params: Value,
    #[serde(default)]
    result: Value,
    #[serde(default)]
    error: Value,
}

#[derive(Debug, Clone, Deserialize)]
struct ScriptEntry {
    after: String,
    #[serde(default)]
    delay_ms: u64,
    #[serde(default)]
    wait_for: Option<String>,
    emit: String,
    method: String,
    #[serde(default)]
    params: Value,
}

/// Prompt uuids received via `steer-turn`, for `$steered` substitution.
static STEERED: Mutex<Vec<String>> = Mutex::new(Vec::new());

fn substitute_steered(mut params: Value) -> Value {
    if let Some(uuids) = params.pointer_mut("/message/user_message_uuids") {
        if uuids == "$steered" {
            *uuids = json!(*STEERED.lock().unwrap());
        }
    }
    params
}

/// Append the raw params of a received RPC to the capture file named by
/// `FAKE_CLAUDE_SIDECAR_CAPTURE`, one JSON object per line. Lets a test
/// assert on exactly what the adapter sent (e.g. that an auto-resume
/// `start-session` carried the persisted `resume` cursor and model).
/// A no-op when the env var is unset.
fn capture_params(method: &str, params: &Value) {
    let Ok(path) = std::env::var("FAKE_CLAUDE_SIDECAR_CAPTURE") else {
        return;
    };
    let record = json!({ "method": method, "params": params });
    let mut line = serde_json::to_string(&record).expect("serialize capture");
    line.push('\n');
    // Best-effort append; a capture failure must not perturb the RPC path.
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = f.write_all(line.as_bytes());
    }
}

/// Serialize and write one framed JSON line. Uses the stdout lock so
/// writes from script threads interleave safely with the main loop.
fn write_line(value: &Value) {
    let mut bytes = serde_json::to_vec(value).expect("serialize");
    bytes.push(b'\n');
    let stdout = std::io::stdout();
    let mut handle = stdout.lock();
    handle.write_all(&bytes).expect("write");
    handle.flush().expect("flush");
}

fn load_script() -> Vec<ScriptEntry> {
    match std::env::var("FAKE_CLAUDE_SIDECAR_SCRIPT") {
        Ok(path) => {
            let raw = std::fs::read_to_string(&path).expect("read script");
            serde_json::from_str(&raw).expect("parse script")
        }
        Err(_) => Vec::new(),
    }
}

/// Spawn script entries matching `method` on a background thread so
/// the main loop stays responsive to further RPCs / shutdown.
fn fire_script_entries_for(method: &str, script: &[ScriptEntry]) {
    let matches: Vec<ScriptEntry> = script
        .iter()
        .filter(|e| e.after == method)
        .cloned()
        .collect();
    if matches.is_empty() {
        return;
    }
    std::thread::spawn(move || {
        for entry in matches {
            if let Some(path) = &entry.wait_for {
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
                while !std::path::Path::new(path).exists() {
                    if std::time::Instant::now() >= deadline { return; }
                    std::thread::sleep(std::time::Duration::from_millis(2));
                }
            }
            if entry.delay_ms > 0 {
                std::thread::sleep(std::time::Duration::from_millis(entry.delay_ms));
            }
            match entry.emit.as_str() {
                "notification" => {
                    let params = substitute_steered(entry.params);
                    write_line(&json!({
                        "jsonrpc": "2.0",
                        "method": entry.method,
                        "params": params,
                    }));
                    // Lets a test prove a late line really was written.
                    if let Ok(path) = std::env::var("FAKE_CLAUDE_EMITTED_MARKER") {
                        let mut f = std::fs::OpenOptions::new()
                            .create(true)
                            .append(true)
                            .open(&path)
                            .expect("open emitted marker");
                        let _ = writeln!(f, "{} {}", entry.method, params);
                    }
                }
                "server_request" => {
                    write_line(&json!({
                        "jsonrpc": "2.0",
                        "id": "fixture-mcp-call",
                        "method": entry.method,
                        "params": entry.params,
                    }));
                }
                other => {
                    eprintln!("fake_claude_sidecar: unknown emit kind {other}");
                }
            }
        }
    });
}

fn main() {
    // Also serves as a native executable fixture for Windows install probes.
    // The sidecar's normal no-argument JSON-RPC behavior is unchanged.
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args == ["--version"] {
        println!("9.9.9 (Claude Code)");
        return;
    }
    if args == ["auth", "status"] {
        println!("{{\"loggedIn\":false}}");
        return;
    }
    let script = load_script();
    let exit_after = std::env::var("FAKE_CLAUDE_SIDECAR_EXIT_AFTER").ok();

    let stdin = std::io::stdin();
    let mut stdin = BufReader::new(stdin.lock());

    // Per-thread map so `respond-to-request` / `stop-session` can
    // check whether a given thread id is known and fabricate
    // plausible responses.
    let sessions: Arc<Mutex<HashMap<String, ()>>> = Arc::new(Mutex::new(HashMap::new()));

    let mut line = String::new();
    loop {
        line.clear();
        let n = stdin.read_line(&mut line).expect("read_line");
        if n == 0 {
            // Regression fixture: keep emitting scripted output for a while
            // after the host closed stdin, like an SDK still mid-run.
            if let Some(ms) = std::env::var("FAKE_CLAUDE_LINGER_AFTER_EOF_MS")
                .ok()
                .and_then(|s| s.parse::<u64>().ok())
            {
                std::thread::sleep(std::time::Duration::from_millis(ms));
            }
            break;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let msg: IncomingMessage = match serde_json::from_str(trimmed) {
            Ok(m) => m,
            Err(e) => {
                eprintln!("fake_claude_sidecar: malformed line: {e}");
                continue;
            }
        };
        if msg.method.is_none() {
            // Capture the actual response frame so tests can prove callback
            // cancellation answers the sidecar instead of abandoning its RPC.
            if let Ok(response) = serde_json::from_str::<Value>(trimmed) {
                capture_params("server-response", &response);
            }
            continue;
        }
        let method = msg.method.as_deref().unwrap_or("");
        let id = msg.id.clone();

        match method {
            "ping" => {
                if let Some(id) = id {
                    write_line(&json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": { "pong": true, "echo": msg.params, "server_time": "1970-01-01T00:00:00Z" },
                    }));
                }
            }
            "start-session" => {
                capture_params(method, &msg.params);
                let thread_id = msg
                    .params
                    .get("threadId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("t-fake")
                    .to_string();
                let path = msg
                    .params
                    .get("pathToClaudeCodeExecutable")
                    .cloned()
                    .unwrap_or_else(|| json!("/usr/bin/claude"));
                sessions.lock().unwrap().insert(thread_id.clone(), ());
                if let Some(id) = id {
                    write_line(&json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "threadId": thread_id,
                            "pathToClaudeCodeExecutable": path,
                        },
                    }));
                }
                fire_script_entries_for(method, &script);
            }
            "send-turn" => {
                capture_params(method, &msg.params);
                // Regression fixture: SDK callbacks/results may precede the
                // send-turn acknowledgement, just as in the real sidecar.
                let early_delay = std::env::var("FAKE_CLAUDE_SEND_ACK_DELAY_MS")
                    .ok()
                    .and_then(|s| s.parse::<u64>().ok());
                let ack_gate = std::env::var("FAKE_CLAUDE_FIRST_ACK_GATE")
                    .ok()
                    .filter(|gate| !std::path::Path::new(gate).exists());
                if let Some(gate) = &ack_gate {
                    fire_script_entries_for(method, &script);
                    while !std::path::Path::new(gate).exists() {
                        std::thread::sleep(std::time::Duration::from_millis(2));
                    }
                } else if let Some(delay) = early_delay {
                    fire_script_entries_for(method, &script);
                    std::thread::sleep(std::time::Duration::from_millis(delay));
                }
                if let Some(id) = id {
                    write_line(&json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": { "turnStarted": true },
                    }));
                }
                if early_delay.is_none() && ack_gate.is_none() {
                    fire_script_entries_for(method, &script);
                }
            }
            "compact-idle" => {
                capture_params(method, &msg.params);
                // Regression fixture: the whole compaction can finish before
                // its acknowledgement reaches the host.
                if let Some(delay) = std::env::var("FAKE_CLAUDE_COMPACT_ACK_DELAY_MS")
                    .ok()
                    .and_then(|s| s.parse::<u64>().ok())
                {
                    fire_script_entries_for(method, &script);
                    std::thread::sleep(std::time::Duration::from_millis(delay));
                    if let Some(id) = id {
                        write_line(&json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "result": { "started": true },
                        }));
                    }
                    continue;
                }
                if let Some(id) = id {
                    write_line(&json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": { "started": true },
                    }));
                }
                fire_script_entries_for(method, &script);
            }
            "steer-turn" => {
                capture_params(method, &msg.params);
                let rejection = std::env::var("FAKE_CLAUDE_STEER_ERROR").ok();
                if rejection.is_none() {
                    if let Some(uuid) = msg.params.get("uuid").and_then(Value::as_str) {
                        STEERED.lock().unwrap().push(uuid.to_owned());
                    }
                }
                fire_script_entries_for(method, &script);
                if let Some(delay) = std::env::var("FAKE_CLAUDE_STEER_DELAY_MS")
                    .ok()
                    .and_then(|s| s.parse::<u64>().ok())
                {
                    std::thread::sleep(std::time::Duration::from_millis(delay));
                }
                if let Some(id) = id {
                    match rejection.as_deref().and_then(|r| r.split_once(':')) {
                        Some((code, message)) => write_line(&json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "error": { "code": code.parse::<i64>().unwrap(), "message": message },
                        })),
                        None => write_line(&json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "result": { "steered": true },
                        })),
                    }
                }
            }
            "interrupt" if std::env::var("FAKE_CLAUDE_INTERRUPT_ERROR").is_ok() => {
                // The interrupt fails with the variable's text. Scripted
                // entries still fire: the sidecar may act on it anyway.
                capture_params(method, &msg.params);
                fire_script_entries_for(method, &script);
                if let Some(delay) = std::env::var("FAKE_CLAUDE_INTERRUPT_DELAY_MS")
                    .ok()
                    .and_then(|s| s.parse::<u64>().ok())
                {
                    std::thread::sleep(std::time::Duration::from_millis(delay));
                }
                if let Some(id) = id {
                    write_line(&json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": {
                            "code": -32603,
                            "message": std::env::var("FAKE_CLAUDE_INTERRUPT_ERROR").unwrap(),
                        },
                    }));
                }
            }
            "interrupt" | "set-model" | "set-permission-mode" => {
                if method == "interrupt" {
                    capture_params(method, &msg.params);
                }
                if let Some(id) = id {
                    write_line(&json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {},
                    }));
                }
                fire_script_entries_for(method, &script);
            }
            "respond-to-request" => {
                capture_params(method, &msg.params);
                let request_id = msg
                    .params
                    .get("requestId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                // Reject known-bad ids; pass the rest through as OK.
                if request_id == "unknown-request" {
                    if let Some(id) = id {
                        write_line(&json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "error": {
                                "code": -32602,
                                "message": format!("request {request_id} not found or already resolved"),
                            },
                        }));
                    }
                } else if let Some(id) = id {
                    write_line(&json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {},
                    }));
                }
                fire_script_entries_for(method, &script);
            }
            "respond-to-user-input" => {
                if let Some(id) = id {
                    write_line(&json!({"jsonrpc": "2.0", "id": id, "result": {}}));
                }
            }
            "initialization-result" => {
                if let Some(id) = id {
                    write_line(&json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "commands": [],
                            "agents": [],
                            "models": [],
                            "account": {},
                            "output_style": "default",
                            "available_output_styles": ["default"]
                        },
                    }));
                }
            }
            "stop-session" => {
                let thread_id = msg
                    .params
                    .get("threadId")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let existed = sessions.lock().unwrap().remove(thread_id).is_some();
                if let Some(id) = id {
                    let result = if existed {
                        json!({})
                    } else {
                        json!({ "alreadyClosed": true })
                    };
                    write_line(&json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": result,
                    }));
                }
                fire_script_entries_for(method, &script);
            }
            "probe-installed" => {
                // Used by the auth.rs integration tests. The claude
                // binary path is passed in params; we just echo
                // plausible values.
                let binary = msg
                    .params
                    .get("binaryPath")
                    .and_then(|v| v.as_str())
                    .unwrap_or("claude");
                let response = if binary.contains("nonexistent") {
                    json!({ "installed": false })
                } else {
                    json!({ "installed": true, "version": "fake-1.0.0" })
                };
                if let Some(id) = id {
                    write_line(&json!({"jsonrpc": "2.0", "id": id, "result": response}));
                }
            }
            "probe-authenticated" => {
                if let Some(id) = id {
                    write_line(&json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "status": "unauthenticated",
                            "message": "fake sidecar: claude CLI is not authenticated"
                        },
                    }));
                }
            }
            other => {
                if let Some(id) = id {
                    write_line(&json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": {
                            "code": -32601,
                            "message": format!("unknown method: {other}")
                        },
                    }));
                }
            }
        }

        if let Some(target) = exit_after.as_deref() {
            if target == method {
                std::process::exit(0);
            }
        }
    }
}
