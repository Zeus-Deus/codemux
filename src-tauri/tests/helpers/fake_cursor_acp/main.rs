//! Test helper binary: a minimal stand-in for `cursor-agent acp`.
//!
//! Speaks just enough of the Agent Client Protocol (plus Cursor's
//! extensions) for `tests/cursor_adapter.rs` to drive the adapter's turn
//! lifecycle without an installed, authenticated Cursor CLI.
//!
//! Methods understood:
//!   - `initialize` → advertises HTTP MCP servers, like cursor-agent
//!     2026.10.14. `authenticate` → empty result.
//!   - Capability discovery rejects `authenticate` (it would start browser
//!     login in the real CLI) and serves `cursor/list_available_models` from
//!     startup credentials. A copy named `signed-out` reports auth required.
//!   - `session/new`  → `{sessionId, configOptions}`, followed by the
//!     `available_commands_update` the real CLI pushes once a session
//!     exists. Entries carry only a name and description — this path has
//!     no argument hints.
//!   - `session/load` → empty result (ACP omits the id on purpose).
//!   - `session/new` and `session/load` remember their `mcpServers`.
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
//!       * `mcp-servers` answers with the remembered `mcpServers` as JSON.
//!       * `echo-prompt` answers with the exact prompt text it received.
//!       * `mcp-env` answers with `{"token", "loads"}`: the gateway token in
//!         this process's environment and how many `session/load`s arrived.
//!       * anything else answers with a single chunk and `end_turn`.
//!   - `session/cancel` (notification) → completes a held turn, else
//!     ignored.
//!
//! Shutdown: closing stdin triggers EOF; the program exits 0.

use std::io::{BufRead, BufReader, Write};

use serde_json::{json, Value};

mod fixture_peer;

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

fn available_commands(session_id: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": "session/update",
        "params": {
            "sessionId": session_id,
            "update": {
                "sessionUpdate": "available_commands_update",
                "availableCommands": [
                    {"name": "copy-request-id", "description": "Copy the request id"},
                    {"name": "plan", "description": "Draft a plan (project)"}
                ]
            }
        }
    })
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
    // Explicit per-child opt-in; without it the legacy Cursor fixture is unchanged.
    if let Some(root) = std::env::var_os("CODEMUX_AGENT_CONTROL_FIXTURE_ROOT") {
        fixture_peer::run(root.into());
        return;
    }
    let stdin = std::io::stdin();
    let mut stdin = BufReader::new(stdin.lock());
    let mut line = String::new();
    let mut next_request_id = 1_000;
    // Id of a `session/prompt` deliberately left unanswered until a
    // `session/cancel` arrives.
    let mut held_prompt: Option<Value> = None;
    let mut mcp_servers = Value::Null;
    let mut capability_harvest = false;
    let signed_out = std::env::current_exe()
        .expect("fixture executable")
        .file_stem()
        .is_some_and(|name| name == "signed-out");
    let mut loads = 0;

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

        if matches!(method, "session/new" | "session/load") {
            mcp_servers = params.get("mcpServers").cloned().unwrap_or(Value::Null);
        }
        if method == "session/load" {
            loads += 1;
        }
        match method {
            "initialize" => {
                capability_harvest = params["clientInfo"]["name"] == "codemux-capability-harvest";
                if let Some(id) = id {
                    write_line(&json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {"agentCapabilities": {"mcpCapabilities": {"http": true, "sse": true}}}
                    }));
                }
            }
            "authenticate" if capability_harvest => {
                if let Some(id) = id {
                    write_line(&json!({"jsonrpc": "2.0", "id": id, "error": {
                        "code": -32602, "message": "Capability discovery must not initiate browser login"
                    }}));
                }
            }
            "cursor/list_available_models" => {
                if let Some(id) = id {
                    if signed_out {
                        write_line(&json!({"jsonrpc": "2.0", "id": id, "error": {
                            "code": -32000, "message": "Authentication required. Run cursor-agent login first."
                        }}));
                    } else {
                        write_line(&json!({"jsonrpc": "2.0", "id": id, "result": {
                            "models": [{"value": "fake-model", "name": "Fake Model"}]
                        }}));
                    }
                }
            }
            "authenticate" | "session/load" => {
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
                // The catalogue lands after the session exists, never in
                // the initialize result.
                write_line(&available_commands("fake-cursor-session"));
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
                } else if text.contains("mcp-servers") {
                    write_line(&chunk(&session_id, &mcp_servers.to_string()));
                } else if text.contains("echo-prompt") {
                    write_line(&chunk(&session_id, &text));
                } else if text.contains("mcp-env") {
                    let token = std::env::var("CODEMUX_MCP_GATEWAY_TOKEN").ok();
                    write_line(&chunk(&session_id, &json!({"token": token, "loads": loads}).to_string()));
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
