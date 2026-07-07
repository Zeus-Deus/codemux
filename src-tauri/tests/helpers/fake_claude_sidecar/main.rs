//! Fake claude-agent sidecar used by `tests/claude_adapter.rs`.
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
    emit: String,
    method: String,
    #[serde(default)]
    params: Value,
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
            if entry.delay_ms > 0 {
                std::thread::sleep(std::time::Duration::from_millis(entry.delay_ms));
            }
            match entry.emit.as_str() {
                "notification" => {
                    write_line(&json!({
                        "jsonrpc": "2.0",
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
            // Response to a server-initiated request — unused here.
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
                if let Some(id) = id {
                    write_line(&json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": { "turnStarted": true },
                    }));
                }
                fire_script_entries_for(method, &script);
            }
            "interrupt" | "set-model" | "set-permission-mode" => {
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
