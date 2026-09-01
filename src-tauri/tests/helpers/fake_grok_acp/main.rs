//! Process-boundary Grok ACP fixture for `tests/grok_adapter.rs`.
//!
//! This deliberately validates the wire contract instead of merely returning
//! canned JSON. A wrong CLI invocation, auth choice, session/new shape, or
//! unexpected resume attempt closes the child and therefore fails the adapter
//! test at the same boundary as the real `grok` executable.

use std::io::{BufRead, BufReader, Write};
use std::thread;
use std::time::Duration;

use serde_json::{json, Value};

const SESSION_ID: &str = "fake-grok-session";

fn write_line(value: &Value) {
    let mut bytes = serde_json::to_vec(value).expect("serialize fixture frame");
    bytes.push(b'\n');
    let stdout = std::io::stdout();
    let mut handle = stdout.lock();
    handle.write_all(&bytes).expect("write fixture frame");
    handle.flush().expect("flush fixture frame");
}

fn response(id: Value, result: Value) {
    write_line(&json!({"jsonrpc": "2.0", "id": id, "result": result}));
}

fn rpc_error(id: Value, code: i64, message: &str, data: Value) {
    write_line(&json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {"code": code, "message": message, "data": data}
    }));
}

fn protocol_failure(id: Option<Value>, message: impl Into<String>) -> ! {
    let message = message.into();
    if let Some(id) = id {
        rpc_error(
            id,
            -32602,
            "fake Grok protocol assertion failed",
            json!({
                "detail": message
            }),
        );
    }
    eprintln!("fake_grok_acp: {message}");
    std::process::exit(70);
}

fn initialize_result() -> Value {
    json!({
        "protocolVersion": 1,
        "authMethods": [
            {"id": "cached_token", "name": "Cached Grok login"},
            {"id": "grok.com", "name": "Browser login"}
        ],
        "agentCapabilities": {
            "loadSession": true,
            "promptCapabilities": {"image": true}
        },
        "_meta": {
            "defaultAuthMethodId": "cached_token",
            "availableCommands": [{
                "name": "initialize-command",
                "description": "Advertised at initialize",
                "input": {"hint": "<value>"}
            }],
            "modelState": {
                "currentModelId": "grok-4.6",
                "reasoningEffort": "high",
                "availableModels": [{
                    "modelId": "grok-4.6",
                    "name": "Grok 4.6",
                    "_meta": {
                        "supportsReasoningEffort": true,
                        "reasoningEffort": "high",
                        "reasoningEfforts": ["low", "high", "adaptive"]
                    }
                }, {
                    "modelId": "grok-future",
                    "name": "Grok Future",
                    "_meta": {
                        "supportsReasoningEffort": true,
                        "reasoningEfforts": ["adaptive"]
                    }
                }]
            }
        }
    })
}

fn model_state(model_id: &str, reasoning_effort: Option<&str>) -> Value {
    let mut state = json!({"currentModelId": model_id});
    if let Some(effort) = reasoning_effort {
        state["reasoningEffort"] = Value::String(effort.to_string());
    }
    json!({"_meta": {"modelState": state}})
}

fn prompt_text(params: &Value) -> String {
    params
        .get("prompt")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|block| block.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join(" ")
}

fn notification(method: &str, params: Value) {
    write_line(&json!({
        "jsonrpc": "2.0",
        "method": method,
        "params": params
    }));
}

fn handle_prompt(id: Value, params: &Value) {
    if params.get("sessionId").and_then(Value::as_str) != Some(SESSION_ID) {
        protocol_failure(Some(id), "session/prompt used the wrong session id");
    }
    if !prompt_text(params).contains("process boundary") {
        protocol_failure(Some(id), "session/prompt lost the user text");
    }
    let Some(prompt_id) = params.pointer("/_meta/promptId").and_then(Value::as_str) else {
        protocol_failure(Some(id), "session/prompt omitted _meta.promptId");
    };
    if params.pointer("/_meta/requestId").and_then(Value::as_str) != Some(prompt_id) {
        protocol_failure(Some(id), "promptId and requestId must match");
    }

    // Grok announces prompt completion before it has durably finalized the
    // session. The adapter must not finish here: a real content chunk follows.
    notification(
        "_x.ai/session/prompt_complete",
        json!({
            "sessionId": SESSION_ID,
            "promptId": prompt_id,
            "stopReason": "error",
            "agentResult": "early completion must not win",
            "usage": {"inputTokens": 900, "outputTokens": 90}
        }),
    );
    thread::sleep(Duration::from_millis(75));

    notification(
        "session/update",
        json!({
            "sessionId": SESSION_ID,
            "update": {
                "sessionUpdate": "agent_message_chunk",
                "content": {"type": "text", "text": "after-early"}
            }
        }),
    );

    // xAI extension envelopes have shipped camelCase update kinds alongside
    // snake_case command fields. This is a full replacement, not a delta.
    notification(
        "_x.ai/session/update",
        json!({
            "session_id": SESSION_ID,
            "update": {
                "type": "availableCommandsUpdate",
                "available_commands": [{
                    "name": "live-command",
                    "description": "Latest runtime snapshot",
                    "argument_hint": "<query>"
                }]
            }
        }),
    );

    // Exercise the live lifecycle spelling and make the acknowledged model
    // externally observable on the ensuing UsageRecorded event.
    notification(
        "_x.ai/session_notification",
        json!({
            "sessionId": SESSION_ID,
            "update": {
                "sessionUpdate": "model_changed",
                "modelId": "grok-provider-effective",
                "reasoningEffort": "adaptive"
            }
        }),
    );

    // This durable terminal releases the compatibility fallback, but the
    // standard RPC response below still owns the turn during the grace period.
    notification(
        "_x.ai/session_notification",
        json!({
            "sessionId": SESSION_ID,
            "update": {
                "sessionUpdate": "turn_completed",
                "promptId": prompt_id,
                "stopReason": "error",
                "agentResult": "durable fallback must lose to standard response",
                "usage": {
                    "inputTokens": 800,
                    "outputTokens": 80,
                    "costUsdTicks": 9_000_000_000_i64
                }
            },
            "_meta": {"totalTokens": 88}
        }),
    );
    thread::sleep(Duration::from_millis(25));

    response(
        id,
        json!({
            "stopReason": "end_turn",
            "_meta": {
                "agentResult": "standard response won",
                "totalTokens": 42,
                "usage": {
                    "inputTokens": 100,
                    "cachedReadTokens": 20,
                    "cacheCreationTokens": 5,
                    "outputTokens": 10,
                    "reasoningTokens": 3,
                    "numTurns": 2,
                    "apiDurationMs": 123,
                    "costUsdTicks": 2_500_000_000_i64
                }
            }
        }),
    );
}

fn main() {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let expected = ["--no-auto-update", "agent", "--no-leader", "stdio"];
    if args.iter().map(String::as_str).collect::<Vec<_>>() != expected {
        protocol_failure(
            None,
            format!("wrong Grok spawn args: got {args:?}, expected {expected:?}"),
        );
    }

    let stdin = std::io::stdin();
    let mut stdin = BufReader::new(stdin.lock());
    let mut line = String::new();
    while stdin.read_line(&mut line).expect("read fixture stdin") != 0 {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            line.clear();
            continue;
        }
        let message: Value = serde_json::from_str(trimmed)
            .unwrap_or_else(|error| protocol_failure(None, format!("malformed JSON: {error}")));
        let Some(method) = message.get("method").and_then(Value::as_str) else {
            line.clear();
            continue;
        };
        let id = message.get("id").cloned();
        let params = message.get("params").cloned().unwrap_or(Value::Null);

        match method {
            "initialize" => {
                if params.get("protocolVersion").and_then(Value::as_u64) != Some(1)
                    || params.pointer("/clientInfo/name").and_then(Value::as_str) != Some("codemux")
                {
                    protocol_failure(id, "unexpected initialize payload");
                }
                response(id.expect("initialize request id"), initialize_result());
            }
            "authenticate" => {
                if params.get("methodId").and_then(Value::as_str) != Some("cached_token")
                    || params.pointer("/_meta/headless").and_then(Value::as_bool) != Some(true)
                {
                    protocol_failure(id, format!("unexpected authenticate payload: {params}"));
                }
                response(id.expect("authenticate request id"), json!({}));
            }
            "session/load" => {
                // The integration test deliberately supplies both a stale
                // cursor and `fresh_session: true`. Reaching this method means
                // the provider violated that boundary contract.
                protocol_failure(id, "fresh_session attempted session/load");
            }
            "session/new" => {
                if params.get("sessionId").is_some()
                    || params.get("mcpServers").and_then(Value::as_array).is_none()
                    || params.pointer("/_meta/yoloMode").and_then(Value::as_bool) != Some(true)
                    || params.pointer("/_meta/modelId").and_then(Value::as_str) != Some("grok-4.6")
                {
                    protocol_failure(id, format!("unexpected session/new payload: {params}"));
                }
                let mut result = model_state("grok-4.6", Some("high"));
                result["sessionId"] = Value::String(SESSION_ID.into());
                response(id.expect("session/new request id"), result);
                notification(
                    "session/update",
                    json!({
                        "sessionId": SESSION_ID,
                        "update": {
                            "sessionUpdate": "available_commands_update",
                            "availableCommands": [{"name": "session-command"}]
                        }
                    }),
                );
            }
            "session/set_model" => {
                if params.get("sessionId").and_then(Value::as_str) != Some(SESSION_ID) {
                    protocol_failure(id, "session/set_model used the wrong session id");
                }
                let model = params
                    .get("modelId")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if model == "grok-incompatible" {
                    rpc_error(
                        id.expect("set_model request id"),
                        -32603,
                        "Internal error",
                        json!({
                            "cause": {"code": "MODEL_SWITCH_INCOMPATIBLE_AGENT"}
                        }),
                    );
                } else {
                    let effort = params
                        .pointer("/_meta/reasoningEffort")
                        .and_then(Value::as_str);
                    response(
                        id.expect("set_model request id"),
                        model_state(model, effort),
                    );
                }
            }
            "session/prompt" => handle_prompt(id.expect("prompt request id"), &params),
            "session/cancel" => {}
            other => protocol_failure(id, format!("unexpected ACP method: {other}")),
        }
        line.clear();
    }
}
