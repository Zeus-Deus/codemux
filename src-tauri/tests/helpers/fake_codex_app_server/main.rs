//! Fake `codex app-server` fixture used by `tests/codex_adapter.rs`.
//!
//! Reads newline-delimited JSON-RPC from stdin and reacts with a canned
//! script that exercises the adapter's event wiring. The script is loaded
//! from the path in `FAKE_CODEX_SCRIPT`, or defaults to "no script"
//! (basic request/response only).
//!
//! # Script format
//!
//! ```json
//! [
//!   { "after": "turn/start", "emit": "notification", "method": "turn/started",
//!     "params": { "threadId":"c1","turnId":"t1" } },
//!   { "after": "turn/start", "delay_ms": 10, "emit": "notification",
//!     "method": "item/agentMessage/delta",
//!     "params": { "threadId":"c1","turnId":"t1","itemId":"i1","delta": "Hi" } },
//!   { "after": "turn/start", "delay_ms": 20, "emit": "server_request",
//!     "method": "item/commandExecution/requestApproval",
//!     "params": { "cmd":"ls","turnId":"t1" } },
//!   { "after": "turn/start", "delay_ms": 50, "emit": "notification",
//!     "method": "turn/completed",
//!     "params": { "threadId":"c1","turnId":"t1","status":"succeeded" } }
//! ]
//! ```
//!
//! # Environment toggles
//!
//! * `FAKE_CODEX_SCRIPT` — path to a script JSON file.
//! * `FAKE_CODEX_FAIL_RESUME=1` — make `thread/resume` fail with a
//!   recoverable real-world "no rollout found" error.
//! * `FAKE_CODEX_THREAD_ID` — override the id returned from
//!   `thread/start` (default `"c-1"`).
//! * `FAKE_CODEX_EXIT_AFTER=<method>` — exit the fixture 0 after
//!   responding to the named method (used to simulate crashes).
//! * `FAKE_CODEX_UNAUTHENTICATED=1` — return no account from `account/read`
//!   while reporting that the active provider requires OpenAI auth.
//! * `FAKE_CODEX_CAPTURE_TURN` — write the latest `turn/start` params JSON
//!   to this path so adapter tests can assert the exact wire contract.
//! * `FAKE_CODEX_CAPTURE_ROLLBACK` — write the latest `thread/rollback`
//!   params JSON to this path.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::Deserialize;
use serde_json::{json, Value};

static TURN_COUNTER: AtomicU64 = AtomicU64::new(1);
static SERVER_REQ_COUNTER: AtomicU64 = AtomicU64::new(1000);

#[derive(Debug, Deserialize)]
#[allow(dead_code)] // serde decodes these; handler only dispatches on id/method.
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

fn write_line(value: &Value) {
    let mut bytes = serde_json::to_vec(value).expect("serialize");
    bytes.push(b'\n');
    let stdout = std::io::stdout();
    let mut handle = stdout.lock();
    handle.write_all(&bytes).expect("write");
    handle.flush().expect("flush");
}

fn load_script() -> Vec<ScriptEntry> {
    match std::env::var("FAKE_CODEX_SCRIPT") {
        Ok(path) => {
            let raw = std::fs::read_to_string(&path).expect("FAKE_CODEX_SCRIPT read");
            serde_json::from_str(&raw).expect("FAKE_CODEX_SCRIPT parse")
        }
        Err(_) => Vec::new(),
    }
}

fn env_truthy(key: &str) -> bool {
    matches!(
        std::env::var(key).as_deref(),
        Ok("1") | Ok("true") | Ok("yes")
    )
}

fn fire_script_entries_for(
    method: &str,
    script: &[ScriptEntry],
    pending_server: &Arc<Mutex<HashMap<String, Value>>>,
) {
    let matches: Vec<_> = script
        .iter()
        .filter(|e| e.after == method)
        .cloned()
        .collect();
    if matches.is_empty() {
        return;
    }
    let pending = Arc::clone(pending_server);
    std::thread::spawn(move || {
        for entry in matches {
            if entry.delay_ms > 0 {
                std::thread::sleep(std::time::Duration::from_millis(entry.delay_ms));
            }
            match entry.emit.as_str() {
                "notification" => {
                    write_line(&json!({
                        "jsonrpc":"2.0",
                        "method": entry.method,
                        "params": entry.params,
                    }));
                }
                "server_request" => {
                    let id = SERVER_REQ_COUNTER.fetch_add(1, Ordering::Relaxed);
                    let id_val = json!(id);
                    // Pending server req tracking (no one actually
                    // consumes the response here — the adapter's call to
                    // `respond` will reach us as a response message we
                    // ignore silently).
                    {
                        let mut p = pending.lock().unwrap();
                        p.insert(
                            serde_json::to_string(&id_val).unwrap_or_default(),
                            Value::Null,
                        );
                    }
                    write_line(&json!({
                        "jsonrpc":"2.0",
                        "id": id_val,
                        "method": entry.method,
                        "params": entry.params,
                    }));
                }
                other => {
                    eprintln!("fake_codex_app_server: unknown emit kind {other}");
                }
            }
        }
    });
}

fn main() {
    let script = load_script();
    let fail_resume = env_truthy("FAKE_CODEX_FAIL_RESUME");
    let thread_id = std::env::var("FAKE_CODEX_THREAD_ID").unwrap_or_else(|_| "c-1".to_string());
    let exit_after = std::env::var("FAKE_CODEX_EXIT_AFTER").ok();
    let unauthenticated = env_truthy("FAKE_CODEX_UNAUTHENTICATED");
    let capture_turn = std::env::var("FAKE_CODEX_CAPTURE_TURN").ok();
    let capture_rollback = std::env::var("FAKE_CODEX_CAPTURE_ROLLBACK").ok();

    let pending_server_requests: Arc<Mutex<HashMap<String, Value>>> =
        Arc::new(Mutex::new(HashMap::new()));

    let stdin = std::io::stdin();
    let mut stdin = BufReader::new(stdin.lock());

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
                eprintln!("fake_codex_app_server: malformed line: {e}");
                continue;
            }
        };

        // Response to a server-initiated request: quietly accept.
        if msg.method.is_none() {
            if let Some(id) = msg.id.as_ref() {
                let key = serde_json::to_string(id).unwrap_or_default();
                pending_server_requests.lock().unwrap().remove(&key);
            }
            continue;
        }

        let method = msg.method.as_deref().unwrap_or("");
        let id = msg.id.clone();

        match method {
            "initialize" => {
                if let Some(id) = id {
                    write_line(&json!({"jsonrpc":"2.0","id":id,"result":{"ok":true}}));
                }
            }
            "initialized" => {
                // no-op notification
            }
            "model/list" => {
                if let Some(id) = id {
                    write_line(&json!({
                        "jsonrpc":"2.0",
                        "id": id,
                        "result": {
                            "data": [{
                                "id": "gpt-test",
                                "model": "gpt-test",
                                "displayName": "GPT Test",
                                "description": "Fixture model",
                                "hidden": false,
                                "isDefault": true,
                                "defaultReasoningEffort": "medium",
                                "supportedReasoningEfforts": [{
                                    "reasoningEffort": "medium",
                                    "description": "Fixture effort"
                                }],
                                "inputModalities": ["text"],
                                "additionalSpeedTiers": []
                            }],
                            "nextCursor": null
                        },
                    }));
                }
            }
            "account/read" => {
                if let Some(id) = id {
                    let account = if unauthenticated {
                        Value::Null
                    } else {
                        json!({"type":"chatgpt","planType":"pro"})
                    };
                    write_line(&json!({
                        "jsonrpc":"2.0",
                        "id": id,
                        "result": {
                            "account": account,
                            "requiresOpenaiAuth": true
                        },
                    }));
                }
            }
            "thread/start" => {
                if let Some(id) = id {
                    write_line(&json!({
                        "jsonrpc":"2.0",
                        "id": id,
                        "result": {"thread": {"id": thread_id }},
                    }));
                }
                fire_script_entries_for(method, &script, &pending_server_requests);
            }
            "thread/resume" => {
                if fail_resume {
                    if let Some(id) = id {
                        write_line(&json!({
                            "jsonrpc":"2.0",
                            "id": id,
                            "error": {"code":-32600,"message":"no rollout found for thread id old-thread"},
                        }));
                    }
                } else if let Some(id) = id {
                    write_line(&json!({
                        "jsonrpc":"2.0",
                        "id": id,
                        "result": {"thread": {"id": thread_id}},
                    }));
                }
                fire_script_entries_for(method, &script, &pending_server_requests);
            }
            "turn/start" => {
                if let Some(path) = capture_turn.as_deref() {
                    std::fs::write(
                        path,
                        serde_json::to_vec(&msg.params).expect("capture params"),
                    )
                    .expect("write captured turn/start params");
                }
                let t = TURN_COUNTER.fetch_add(1, Ordering::Relaxed);
                let tid = format!("t-{t}");
                if let Some(id) = id {
                    write_line(&json!({
                        "jsonrpc":"2.0",
                        "id": id,
                        "result": {"turn": {"id": tid }},
                    }));
                }
                fire_script_entries_for(method, &script, &pending_server_requests);
            }
            "turn/interrupt" => {
                if let Some(id) = id {
                    write_line(&json!({
                        "jsonrpc":"2.0",
                        "id": id,
                        "result": {"ok": true},
                    }));
                }
                fire_script_entries_for(method, &script, &pending_server_requests);
            }
            "thread/read" => {
                if let Some(id) = id {
                    write_line(&json!({
                        "jsonrpc":"2.0",
                        "id": id,
                        "result": {"threadId": thread_id,"turns":[]},
                    }));
                }
            }
            "thread/rollback" => {
                if let Some(path) = capture_rollback.as_deref() {
                    std::fs::write(
                        path,
                        serde_json::to_vec(&msg.params).expect("capture rollback params"),
                    )
                    .expect("write captured thread/rollback params");
                }
                if let Some(id) = id {
                    write_line(&json!({
                        "jsonrpc":"2.0",
                        "id": id,
                        "result": {"threadId": thread_id,"turns":[]},
                    }));
                }
            }
            other => {
                if let Some(id) = id {
                    write_line(&json!({
                        "jsonrpc":"2.0",
                        "id": id,
                        "error": {"code":-32601,"message": format!("method not found: {other}")},
                    }));
                }
            }
        }

        if let Some(target) = exit_after.as_deref() {
            if target == method {
                // Flush then exit so the adapter's watchdog sees the
                // child vanish.
                std::process::exit(0);
            }
        }
    }
}
