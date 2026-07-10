//! Test helper binary: a tiny JSON-RPC peer used by the `json_rpc_child`
//! integration tests. Reads newline-delimited JSON from stdin and reacts to
//! a small fixed menu of "methods" that exercise the helper's routing,
//! timeout, and server-initiated-request paths.
//!
//! Methods understood:
//!   - `echo`             → result = params
//!   - `sleep`            → params.ms millis (on a background thread so the
//!                          main loop keeps processing new messages);
//!                          result = {"slept": ms}
//!   - `notify`           → send back a notification with method "tick" and
//!                          params = original params
//!   - `server_request`   → send a server-initiated request with method
//!                          "need_input", id from params.server_id, and
//!                          params = params.payload. Awaits the response,
//!                          replies with the echoed response payload.
//!   - `emit_notification` → emit a notification but DO NOT respond. Used to
//!                          test notification passthrough.
//!   - `malformed`        → emit a bogus non-JSON line then a real success.
//!   - `exit`             → exit cleanly. If params.code is a number, use it
//!                          as the exit code; if params.stderr is a string,
//!                          write it to stderr first.
//!   - `echo_then_exit`   → respond with result = params, then exit 0
//!                          immediately. Exercises the "child writes its
//!                          final response and dies" race — the response
//!                          bytes may still be in the kernel pipe buffer
//!                          when the watchdog observes the exit.
//!   - `echo_err`         → respond with an RPC error with
//!                          code/message/data from params.
//!
//! Anything else produces a `MethodNotFound`-style error response.
//!
//! Shutdown: closing stdin triggers EOF; the program exits 0.

use std::io::{BufRead, BufReader, Write};

use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Debug, Deserialize)]
struct IncomingMessage {
    id: Option<Value>,
    method: Option<String>,
    #[serde(default)]
    params: Value,
    // Response shape (when the harness answers a server-initiated request)
    #[serde(default)]
    result: Value,
    #[serde(default)]
    error: Value,
}

/// Serialize `value` as one line of JSON on stdout. Uses the interpreter's
/// built-in stdout lock so writes from background threads interleave
/// safely.
fn write_line(value: &Value) {
    let mut bytes = serde_json::to_vec(value).expect("serialize");
    bytes.push(b'\n');
    let stdout = std::io::stdout();
    let mut handle = stdout.lock();
    handle.write_all(&bytes).expect("write");
    handle.flush().expect("flush");
}

/// Write raw bytes (used for the `malformed` fixture which emits a
/// deliberately invalid line).
fn write_raw(raw: &[u8]) {
    let stdout = std::io::stdout();
    let mut handle = stdout.lock();
    handle.write_all(raw).expect("write");
    handle.flush().expect("flush");
}

fn main() {
    let stdin = std::io::stdin();
    let mut stdin = BufReader::new(stdin.lock());

    // Track pending server-initiated requests awaiting a response from the
    // harness. key = id (serialized as JSON), value = original source
    // request id so we can reply back after receiving the response.
    let mut pending_server_replies: std::collections::HashMap<String, Value> =
        std::collections::HashMap::new();

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
                eprintln!("fake_rpc_child: malformed line: {e}");
                continue;
            }
        };

        // If this is a response to a server-initiated request we previously
        // sent, match it up and reply to the originating caller with the
        // response payload.
        if msg.method.is_none() {
            if let Some(id) = msg.id.as_ref() {
                let key = serde_json::to_string(id).unwrap_or_default();
                if let Some(original_id) = pending_server_replies.remove(&key) {
                    let body = if !msg.error.is_null() {
                        json!({
                            "jsonrpc": "2.0",
                            "id": original_id,
                            "error": msg.error,
                        })
                    } else {
                        json!({
                            "jsonrpc": "2.0",
                            "id": original_id,
                            "result": {"server_reply": msg.result},
                        })
                    };
                    write_line(&body);
                    continue;
                }
            }
            // Otherwise it's a stray response — ignore.
            continue;
        }

        let method = msg.method.as_deref().unwrap_or("");
        let id = msg.id.clone();

        match method {
            "echo" => {
                if let Some(id) = id {
                    write_line(&json!({"jsonrpc":"2.0","id":id,"result":msg.params}));
                }
            }
            "sleep" => {
                let ms = msg
                    .params
                    .get("ms")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0);
                let id_clone = id.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(ms));
                    if let Some(id) = id_clone {
                        write_line(&json!({"jsonrpc":"2.0","id":id,"result":{"slept":ms}}));
                    }
                });
            }
            "notify" => {
                write_line(&json!({"jsonrpc":"2.0","method":"tick","params":msg.params}));
                if let Some(id) = id {
                    write_line(&json!({"jsonrpc":"2.0","id":id,"result":{"notified":true}}));
                }
            }
            "emit_notification" => {
                write_line(
                    &json!({"jsonrpc":"2.0","method":"heartbeat","params":msg.params}),
                );
            }
            "server_request" => {
                let server_id = msg
                    .params
                    .get("server_id")
                    .cloned()
                    .unwrap_or(json!("srv-1"));
                let payload = msg.params.get("payload").cloned().unwrap_or(Value::Null);
                if let Some(id) = id {
                    let key = serde_json::to_string(&server_id).unwrap_or_default();
                    pending_server_replies.insert(key, id);
                }
                write_line(&json!({
                    "jsonrpc":"2.0",
                    "id": server_id,
                    "method": "need_input",
                    "params": payload,
                }));
            }
            "malformed" => {
                write_raw(b"this-is-not-json\n");
                if let Some(id) = id {
                    write_line(&json!({"jsonrpc":"2.0","id":id,"result":"after-malformed"}));
                }
            }
            "echo_err" => {
                let code = msg
                    .params
                    .get("code")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(-1);
                let message = msg
                    .params
                    .get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("fake")
                    .to_string();
                let data = msg.params.get("data").cloned();
                if let Some(id) = id {
                    let mut err = json!({"code":code,"message":message});
                    if let Some(d) = data {
                        err["data"] = d;
                    }
                    write_line(&json!({"jsonrpc":"2.0","id":id,"error":err}));
                }
            }
            "echo_then_exit" => {
                if let Some(id) = id {
                    write_line(&json!({"jsonrpc":"2.0","id":id,"result":msg.params}));
                }
                std::process::exit(0);
            }
            "exit" => {
                let code = msg
                    .params
                    .get("code")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0) as i32;
                let stderr_msg = msg.params.get("stderr").and_then(|v| v.as_str());
                if let Some(m) = stderr_msg {
                    eprintln!("{m}");
                }
                std::process::exit(code);
            }
            other => {
                if let Some(id) = id {
                    write_line(&json!({
                        "jsonrpc":"2.0",
                        "id":id,
                        "error":{"code":-32601,"message":format!("unknown method: {other}")}
                    }));
                }
            }
        }
    }
}
