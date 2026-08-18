//! Test helper binary: a minimal stand-in for `cursor-agent acp`.
//!
//! Speaks just enough of the Agent Client Protocol (plus Cursor's
//! extensions) for `tests/cursor_adapter.rs` to drive the adapter's turn
//! lifecycle without an installed, authenticated Cursor CLI.
//!
//! Methods understood:
//!   - `initialize` / `authenticate` → empty result.
//!   - `session/new`  → `{sessionId, configOptions}`.
//!   - `session/load` → empty result (ACP omits the id on purpose).
//!   - `session/set_config_option` → echoes the config options back.
//!   - `session/prompt` → behavior is chosen by the prompt text:
//!       * `chunks:<n>` streams `<n>` `agent_message_chunk` updates
//!         immediately before the response, all on the same line-oriented
//!         stdout. Exercises the notification/response ordering barrier:
//!         an adapter that completes the turn on the response alone drops
//!         the tail of the message.
//!       * `permission` sends a `session/request_permission` request and
//!         then finishes the turn WITHOUT waiting for the answer, leaving
//!         an outstanding request the adapter has to cancel.
//!       * `await-cancel` holds the turn open — it responds only once a
//!         `session/cancel` notification arrives, and then with
//!         `stopReason: cancelled`. A client that never delivers the
//!         cancel hangs, which is exactly what makes it a test of Stop.
//!       * anything else answers with a single chunk and `end_turn`.
//!   - `session/cancel` (notification) → completes a held turn, else
//!     ignored.
//!
//! Shutdown: closing stdin triggers EOF; the program exits 0.

use std::io::{BufRead, BufReader, Write};

use serde_json::{json, Value};

fn write_line(value: &Value) {
    let mut bytes = serde_json::to_vec(value).expect("serialize");
    bytes.push(b'\n');
    let stdout = std::io::stdout();
    let mut handle = stdout.lock();
    handle.write_all(&bytes).expect("write");
    handle.flush().expect("flush");
}

fn config_options() -> Value {
    json!([
        {
            "id": "model",
            "name": "Model",
            "category": "model",
            "type": "select",
            "currentValue": "fake-model",
            "options": [{"value": "fake-model", "name": "Fake Model"}]
        },
        {
            "id": "mode",
            "name": "Mode",
            "category": "mode",
            "type": "select",
            "currentValue": "agent",
            "options": [
                {"value": "agent", "name": "Agent"},
                {"value": "ask", "name": "Ask"}
            ]
        }
    ])
}

fn chunk(session_id: &str, text: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": "session/update",
        "params": {
            "sessionId": session_id,
            "update": {
                "sessionUpdate": "agent_message_chunk",
                "content": {"type": "text", "text": text}
            }
        }
    })
}

/// The prompt's plain text, concatenated across content blocks.
fn prompt_text(params: &Value) -> String {
    params
        .get("prompt")
        .and_then(Value::as_array)
        .map(|blocks| {
            blocks
                .iter()
                .filter_map(|block| block.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default()
}

fn requested_chunk_count(text: &str) -> Option<usize> {
    let rest = text.split_once("chunks:")?.1;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

fn main() {
    let stdin = std::io::stdin();
    let mut stdin = BufReader::new(stdin.lock());
    let mut line = String::new();
    let mut next_request_id = 1_000;
    // Id of a `session/prompt` deliberately left unanswered until a
    // `session/cancel` arrives.
    let mut held_prompt: Option<Value> = None;

    loop {
        line.clear();
        if stdin.read_line(&mut line).expect("read_line") == 0 {
            break;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let message: Value = match serde_json::from_str(trimmed) {
            Ok(value) => value,
            Err(error) => {
                eprintln!("fake_cursor_acp: malformed line: {error}");
                continue;
            }
        };
        let Some(method) = message.get("method").and_then(Value::as_str) else {
            // A response to one of our server-initiated requests. The
            // fixtures never need to correlate them.
            continue;
        };
        let id = message.get("id").cloned();
        let params = message.get("params").cloned().unwrap_or(Value::Null);

        match method {
            "initialize" | "authenticate" | "session/load" => {
                if let Some(id) = id {
                    write_line(&json!({"jsonrpc": "2.0", "id": id, "result": {}}));
                }
            }
            "session/new" => {
                if let Some(id) = id {
                    write_line(&json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "sessionId": "fake-cursor-session",
                            "configOptions": config_options(),
                        }
                    }));
                }
            }
            "session/set_config_option" => {
                if let Some(id) = id {
                    write_line(&json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {"configOptions": config_options()}
                    }));
                }
            }
            "session/prompt" => {
                let session_id = params
                    .get("sessionId")
                    .and_then(Value::as_str)
                    .unwrap_or("fake-cursor-session")
                    .to_string();
                let text = prompt_text(&params);
                if text.contains("await-cancel") {
                    // Hold the turn open. Only a `session/cancel` ends it,
                    // so the adapter has to actually deliver the Stop —
                    // and deliver it AFTER this prompt, or we would never
                    // have gotten here to hold anything.
                    held_prompt = id;
                    continue;
                }
                if text.contains("permission") {
                    next_request_id += 1;
                    write_line(&json!({
                        "jsonrpc": "2.0",
                        "id": next_request_id,
                        "method": "session/request_permission",
                        "params": {
                            "sessionId": session_id,
                            "toolCall": {"toolCallId": "tool-1", "title": "Write file"},
                            "options": [
                                {"optionId": "yes", "name": "Allow", "kind": "allow_once"},
                                {"optionId": "no", "name": "Reject", "kind": "reject_once"}
                            ]
                        }
                    }));
                } else {
                    let count = requested_chunk_count(&text).unwrap_or(1);
                    for index in 0..count {
                        write_line(&chunk(&session_id, &format!("[{index}]")));
                    }
                }
                if let Some(id) = id {
                    write_line(&json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {"stopReason": "end_turn"}
                    }));
                }
            }
            "session/cancel" => {
                if let Some(prompt_id) = held_prompt.take() {
                    write_line(&json!({
                        "jsonrpc": "2.0",
                        "id": prompt_id,
                        "result": {"stopReason": "cancelled"}
                    }));
                }
            }
            other => {
                if let Some(id) = id {
                    write_line(&json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "error": {"code": -32601, "message": format!("unknown method: {other}")}
                    }));
                }
            }
        }
    }
}
