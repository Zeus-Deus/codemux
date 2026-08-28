//! Test helper binary: a stand-in for `hermes -p <profile> acp`.
//!
//! A sibling of `fake_cursor_acp` rather than a mode of it. The two
//! agents overlap only in the JSON-RPC framing: Cursor's fixture is built
//! around `session/set_config_option` and a `configOptions` array, which
//! are Cursor extensions with no counterpart here, while everything this
//! one serves — the `authMethods` list, the `session/new` catalogue,
//! `session/set_mode`, `session/list`, the `session/load` replay, the
//! diff shapes — is either absent there or a different shape. Branching
//! one binary on a flag would leave every handler carrying two unrelated
//! bodies and both fixtures harder to read against their own capture.
//!
//! Every payload below is transcribed from the live capture in
//! `spike/frames*.jsonl` (Hermes Agent 0.20.6), trimmed only where a list
//! repeats itself — 29 models became 3, 9 slash commands became 3.
//!
//! Methods understood:
//!   - `initialize` → `agentCapabilities` + a DYNAMIC `authMethods`
//!     list. The first entry's id is the launched profile's configured
//!     runtime (`FAKE_HERMES_RUNTIME`, default `openai-codex`), the
//!     second is the interactive `hermes-setup` terminal method. With
//!     `FAKE_HERMES_AUTH=terminal-only` the runtime entry is withheld, so
//!     the only method on offer is one no client can complete.
//!   - `authenticate` → succeeds only for a method that was actually
//!     offered. A hard-coded login id therefore fails here, which is the
//!     point.
//!   - `session/new` → `{sessionId, models, modes, _meta.hermes
//!     .sessionProvenance}`, then pushes `available_commands_update` and
//!     `usage_update` unprompted, as the real server does. The session id
//!     carries the launch profile so a test can tell which child answered.
//!   - `session/set_mode` / `session/set_model` → `{}`.
//!   - `session/list` → one row with the agent's own auto-title.
//!   - `session/load` → a known id REPLAYS a two-turn transcript as
//!     `session/update` notifications in one burst ahead of its own
//!     response, then answers with the same catalogue `session/new`
//!     returns. An unknown id answers `{}` with no replay — verified
//!     live: ACP invents a fresh empty session rather than failing.
//!   - `session/prompt` → behaviour chosen by the prompt text:
//!       * `permission` requests approval with the captured two-option
//!         edit set (`rawInput.tool = "write_file"`) and then WAITS for
//!         the answer: an allow emits the tool call and its result, a
//!         reject emits a rejected `tool_call_update`. Either way the
//!         turn ends only after the round trip.
//!       * `diffs` emits both captured diff shapes — a creation with no
//!         `oldText`, a replace carrying one — plus the shell call and
//!         result, which are plain text rather than a diff.
//!       * `unknown-event` emits an unrecognised `sessionUpdate` kind and
//!         an unrecognised notification method before answering normally,
//!         so a test can prove neither takes the session down.
//!       * `await-cancel` holds the turn open until a `session/cancel`
//!         notification arrives, then answers `stopReason: cancelled`.
//!       * anything else answers with one `agent_message_chunk`, a
//!         `usage_update` and `end_turn` + the captured per-turn `usage`.
//!   - `session/cancel` (notification) → completes a held turn.
//!   - `session/close` → `{}`, a no-op, exactly as upstream.
//!
//! Every inbound method is appended to the file named by
//! `FAKE_HERMES_JOURNAL`, one per line, before it is handled. That is how
//! a test tells detach (no `session/cancel`) from shutdown (one), and how
//! it proves a profile change started a second child instead of poking
//! the first.
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

/// Append one line to the journal, if the launcher asked for one.
fn journal(line: &str) {
    let Ok(path) = std::env::var("FAKE_HERMES_JOURNAL") else {
        return;
    };
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "{line}");
        let _ = file.flush();
    }
}

/// The profile from the launch line. `-p <profile>` is a GLOBAL flag and
/// precedes the `acp` subcommand, so it is parsed the same way here.
fn launch_profile() -> Option<String> {
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        if arg == "-p" || arg == "--profile" {
            return args.next();
        }
    }
    None
}

/// `authMethods`, derived from the launched profile's runtime.
///
/// The first entry is what a real server reports for whichever runtime
/// the profile is configured for — its id is NOT a constant, which is the
/// whole reason this is a function of the environment.
fn auth_methods() -> Value {
    let runtime =
        std::env::var("FAKE_HERMES_RUNTIME").unwrap_or_else(|_| "openai-codex".to_string());
    let setup = json!({
        "id": "hermes-setup",
        "name": "Configure Hermes provider",
        "type": "terminal",
        "args": ["--setup"],
        "description": "Open Hermes' interactive model/provider setup in a terminal. \
                        Use this when Hermes has not been configured on this machine yet."
    });
    if std::env::var("FAKE_HERMES_AUTH").as_deref() == Ok("terminal-only") {
        return json!([setup]);
    }
    json!([
        {
            "id": runtime,
            "name": format!("{runtime} runtime credentials"),
            "description": format!(
                "Authenticate Hermes using the currently configured {runtime} runtime credentials."
            )
        },
        setup
    ])
}

fn initialize_result() -> Value {
    json!({
        "protocolVersion": 1,
        "agentInfo": {"name": "hermes-agent", "version": "0.20.6"},
        "agentCapabilities": {
            "loadSession": true,
            "promptCapabilities": {"image": true},
            "sessionCapabilities": {"fork": {}, "list": {}, "resume": {}}
        },
        "authMethods": auth_methods(),
    })
}

/// The catalogue block both `session/new` and `session/load` return.
///
/// Model ids are `provider:model` and the `·` in each name is U+00B7,
/// both as captured. Exactly the three modes the agent offers.
fn session_payload(session_id: &str) -> Value {
    json!({
        "sessionId": session_id,
        "modes": {
            "currentModeId": "default",
            "availableModes": [
                {"id": "default", "name": "Default", "description": "Ask before edits."},
                {"id": "accept_edits", "name": "Accept Edits",
                 "description": "Auto-allow workspace and /tmp edits; still asks for sensitive paths."},
                {"id": "dont_ask", "name": "Don't Ask",
                 "description": "Auto-allow file edits for this session except sensitive paths."}
            ]
        },
        "models": {
            "currentModelId": "openai-codex:gpt-5.6-sol",
            "availableModels": [
                {"modelId": "anthropic:claude-opus-5", "name": "Anthropic · claude-opus-5",
                 "description": "Provider: Anthropic"},
                {"modelId": "anthropic:claude-sonnet-5", "name": "Anthropic · claude-sonnet-5",
                 "description": "Provider: Anthropic"},
                {"modelId": "openai-codex:gpt-5.6-sol",
                 "name": "ChatGPT or Codex Subscription · gpt-5.6-sol",
                 "description": "Provider: ChatGPT or Codex Subscription • current"}
            ]
        },
        "_meta": {"hermes": {"sessionProvenance": {
            "acpSessionId": session_id,
            "currentHermesSessionId": session_id,
            "rootHermesSessionId": session_id,
            "parentHermesSessionId": null,
            "sessionKind": "root",
            "compressionDepth": 0
        }}}
    })
}

fn notification(method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "method": method, "params": params})
}

fn update(session_id: &str, update: Value) -> Value {
    notification(
        "session/update",
        json!({"sessionId": session_id, "update": update}),
    )
}

/// The pair of frames the server pushes unprompted the moment a session
/// exists. Neither is inside a turn, so a client that only accepts
/// updates during one never sees its slash commands or its context meter.
fn push_session_startup_frames(session_id: &str) {
    write_line(&update(
        session_id,
        json!({
            "sessionUpdate": "available_commands_update",
            "availableCommands": [
                {"name": "help", "description": "List available commands"},
                {"name": "model", "description": "Show current model and provider, or switch models",
                 "input": {"hint": "model name to switch to"}},
                {"name": "compress", "description": "Compress conversation context"}
            ]
        }),
    ));
    write_line(&update(
        session_id,
        json!({"sessionUpdate": "usage_update", "size": 272_000, "used": 8398}),
    ));
}

/// Diff blocks in both captured shapes, plus the shell call that carries
/// plain text instead of a diff.
fn emit_edit_transcript(session_id: &str) {
    // Creation: no `oldText`. Its ABSENCE is the only signal that this is
    // a write rather than a replace.
    write_line(&update(
        session_id,
        json!({
            "sessionUpdate": "tool_call",
            "toolCallId": "tc-e12862af6277",
            "kind": "edit",
            "title": "write: /tmp/spike_demo.txt",
            "locations": [{"path": "/tmp/spike_demo.txt"}],
            "content": [{"type": "diff", "path": "/tmp/spike_demo.txt", "newText": "hello"}]
        }),
    ));
    // Replace: carries `oldText`.
    write_line(&update(
        session_id,
        json!({
            "sessionUpdate": "tool_call",
            "toolCallId": "tc-9f111677d567",
            "kind": "edit",
            "title": "patch (replace): /tmp/spike_demo.txt",
            "locations": [{"path": "/tmp/spike_demo.txt"}],
            "content": [{
                "type": "diff", "path": "/tmp/spike_demo.txt",
                "oldText": "hello", "newText": "hello world"
            }]
        }),
    ));
    write_line(&update(
        session_id,
        json!({
            "sessionUpdate": "tool_call",
            "toolCallId": "tc-b849655efdd9",
            "kind": "execute",
            "title": "terminal: echo spike-ok",
            "locations": [],
            "content": [{"type": "content", "content": {"type": "text", "text": "$ echo spike-ok"}}]
        }),
    ));
    write_line(&update(
        session_id,
        json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": "tc-b849655efdd9",
            "kind": "execute",
            "status": "completed",
            "content": [{"type": "content", "content": {
                "type": "text",
                "text": "terminal result\n- **output:** spike-ok\n- **exit_code:** 0"
            }}]
        }),
    ));
}

/// The transcript a `session/load` replays: two user turns with
/// reasoning, tool calls and their results. Emitted in one burst, ahead
/// of the load response, exactly as captured.
fn replay_transcript(session_id: &str) {
    let frames = [
        json!({
            "sessionUpdate": "user_message_chunk",
            "content": {"type": "text", "text": "first question"}
        }),
        json!({
            "sessionUpdate": "agent_thought_chunk",
            "content": {"type": "text", "text": "**Planning sequential file and shell actions**"}
        }),
        json!({
            "sessionUpdate": "tool_call",
            "toolCallId": "call_yf51QttEMinSqbkNyRqCnK98",
            "kind": "edit",
            "title": "write: /tmp/spike_demo.txt",
            "locations": [{"path": "/tmp/spike_demo.txt"}],
            "content": [{"type": "content", "content": {
                "type": "text",
                "text": "Preparing write to /tmp/spike_demo.txt. Approval prompt shows the diff."
            }}]
        }),
        json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": "call_yf51QttEMinSqbkNyRqCnK98",
            "kind": "edit",
            "status": "completed",
            "content": [{"type": "content", "content": {
                "type": "text",
                "text": "write_file completed for `/tmp/spike_demo.txt`"
            }}]
        }),
        json!({
            "sessionUpdate": "agent_message_chunk",
            "content": {"type": "text", "text": "DONE"}
        }),
        json!({
            "sessionUpdate": "user_message_chunk",
            "content": {"type": "text", "text": "second question"}
        }),
        json!({
            "sessionUpdate": "agent_message_chunk",
            "content": {"type": "text", "text": "second answer"}
        }),
        json!({"sessionUpdate": "usage_update", "size": 272_000, "used": 15547}),
    ];
    for frame in frames {
        write_line(&update(session_id, frame));
    }
}

/// The `session/request_permission` an edit raises, verbatim from the
/// capture: two options mapped by `kind`, and the stable tool name at
/// `rawInput.tool`.
fn permission_request(id: i64, session_id: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "session/request_permission",
        "params": {
            "sessionId": session_id,
            "options": [
                {"optionId": "allow_once", "kind": "allow_once", "name": "Allow edit"},
                {"optionId": "deny", "kind": "reject_once", "name": "Deny"}
            ],
            "toolCall": {
                "toolCallId": "edit-approval-1",
                "status": "pending",
                "kind": "edit",
                "title": "Approve edit: /tmp/spikework/perm_demo.txt",
                "content": [{
                    "type": "diff",
                    "path": "/tmp/spikework/perm_demo.txt",
                    "newText": "banana"
                }],
                "rawInput": {
                    "tool": "write_file",
                    "arguments": {"path": "/tmp/spikework/perm_demo.txt", "content": "banana"}
                }
            }
        }
    })
}

/// The per-turn token report every `session/prompt` answers with.
fn prompt_result(stop_reason: &str) -> Value {
    json!({
        "stopReason": stop_reason,
        "usage": {
            "inputTokens": 47491, "outputTokens": 148, "thoughtTokens": 47,
            "cachedReadTokens": 33280, "totalTokens": 47639
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

/// A `session/prompt` whose answer is deferred: either until the user
/// answers a permission request, or until a `session/cancel` arrives.
struct HeldPrompt {
    rpc_id: Value,
    session_id: String,
}

fn main() {
    let profile = launch_profile();
    journal(&format!(
        "launch profile={}",
        profile.as_deref().unwrap_or("default")
    ));
    // The session id carries the profile so a test can see WHICH child
    // answered — the evidence that a profile change restarted the session
    // rather than reconfiguring the running one.
    let session_id = format!("hermes-{}-session", profile.as_deref().unwrap_or("default"));
    let offered_auth_ids: Vec<String> = auth_methods()
        .as_array()
        .map(|methods| {
            methods
                .iter()
                .filter_map(|method| method.get("id").and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();

    let stdin = std::io::stdin();
    let mut stdin = BufReader::new(stdin.lock());
    let mut line = String::new();
    let mut next_request_id = 0_i64;
    let mut awaiting_cancel: Option<HeldPrompt> = None;
    let mut awaiting_permission: Option<(Value, HeldPrompt)> = None;

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
                eprintln!("fake_hermes_acp: malformed line: {error}");
                continue;
            }
        };
        let id = message.get("id").cloned();
        let Some(method) = message.get("method").and_then(Value::as_str) else {
            // A response to one of our server-initiated requests. The only
            // one we send is the edit approval, and the turn it belongs to
            // is still open waiting for it.
            let Some((request_id, held)) = awaiting_permission.take() else {
                continue;
            };
            if id.as_ref() != Some(&request_id) {
                awaiting_permission = Some((request_id, held));
                continue;
            }
            let option = message
                .pointer("/result/outcome/optionId")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            journal(&format!("permission-outcome {option}"));
            if option == "allow_once" {
                write_line(&update(
                    &held.session_id,
                    json!({
                        "sessionUpdate": "tool_call",
                        "toolCallId": "edit-approval-1",
                        "kind": "edit",
                        "title": "write: /tmp/spikework/perm_demo.txt",
                        "locations": [{"path": "/tmp/spikework/perm_demo.txt"}],
                        "content": [{
                            "type": "diff",
                            "path": "/tmp/spikework/perm_demo.txt",
                            "newText": "banana"
                        }]
                    }),
                ));
                write_line(&update(
                    &held.session_id,
                    json!({
                        "sessionUpdate": "tool_call_update",
                        "toolCallId": "edit-approval-1",
                        "status": "completed",
                        "content": [{"type": "content", "content": {
                            "type": "text",
                            "text": "write_file completed for `/tmp/spikework/perm_demo.txt`"
                        }}]
                    }),
                ));
            } else {
                write_line(&update(
                    &held.session_id,
                    json!({
                        "sessionUpdate": "tool_call_update",
                        "toolCallId": "edit-approval-1",
                        "status": "failed",
                        "content": [{"type": "content", "content": {
                            "type": "text",
                            "text": "write_file rejected by the user"
                        }}]
                    }),
                ));
            }
            write_line(&json!({
                "jsonrpc": "2.0",
                "id": held.rpc_id,
                "result": prompt_result("end_turn")
            }));
            continue;
        };
        journal(method);
        let params = message.get("params").cloned().unwrap_or(Value::Null);

        match method {
            "initialize" => {
                // `clientInfo` is schema-validated upstream and `version`
                // is REQUIRED — a name-only block fails the handshake with
                // `-32602` before any session exists. Rejecting it here
                // too keeps that from regressing behind a fixture that is
                // more forgiving than the agent.
                let client_info_ok = match params.get("clientInfo") {
                    Some(info) => info
                        .get("version")
                        .and_then(Value::as_str)
                        .is_some_and(|version| !version.is_empty()),
                    None => true,
                };
                if let Some(id) = id {
                    if client_info_ok {
                        write_line(
                            &json!({"jsonrpc": "2.0", "id": id, "result": initialize_result()}),
                        );
                    } else {
                        write_line(&json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "error": {
                                "code": -32602,
                                "message": "Invalid params: clientInfo.version is required"
                            }
                        }));
                    }
                }
            }
            "authenticate" => {
                // A method id that was never offered is an error, not a
                // shrug: the point of reading `authMethods` is that a
                // guessed login id cannot work.
                let requested = params
                    .get("methodId")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                let known = offered_auth_ids.iter().any(|offered| offered == requested);
                if let Some(id) = id {
                    if known {
                        write_line(&json!({"jsonrpc": "2.0", "id": id, "result": {}}));
                    } else {
                        write_line(&json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "error": {
                                "code": -32602,
                                "message": format!("unknown auth method: {requested}")
                            }
                        }));
                    }
                }
            }
            "session/new" => {
                if let Some(id) = id {
                    write_line(&json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": session_payload(&session_id)
                    }));
                }
                push_session_startup_frames(&session_id);
            }
            "session/load" => {
                let requested = params
                    .get("sessionId")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                // An id this child never minted is answered with `{}` and
                // no replay — the live behaviour, and the reason a
                // successful load is not proof the session existed.
                if requested != session_id {
                    if let Some(id) = id {
                        write_line(&json!({"jsonrpc": "2.0", "id": id, "result": {}}));
                    }
                    continue;
                }
                // Burst first, response second. A client that starts
                // listening only once the load resolves sees none of it.
                replay_transcript(&requested);
                if let Some(id) = id {
                    write_line(&json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": session_payload(&requested)
                    }));
                }
                push_session_startup_frames(&requested);
            }
            "session/set_mode" | "session/set_model" => {
                if let Some(id) = id {
                    write_line(&json!({"jsonrpc": "2.0", "id": id, "result": {}}));
                }
            }
            "session/list" => {
                if let Some(id) = id {
                    write_line(&json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {"sessions": [{
                            "sessionId": session_id,
                            "cwd": "/tmp",
                            "title": "Create /tmp/spike_demo.txt and run echo spike-ok",
                            "updatedAt": "2026-08-28T13:40:28.751873+00:00"
                        }]}
                    }));
                }
            }
            "session/close" => {
                // A no-op upstream: it returns `{}` and frees nothing.
                if let Some(id) = id {
                    write_line(&json!({"jsonrpc": "2.0", "id": id, "result": {}}));
                }
            }
            "session/prompt" => {
                let prompt_session = params
                    .get("sessionId")
                    .and_then(Value::as_str)
                    .unwrap_or(&session_id)
                    .to_string();
                let text = prompt_text(&params);
                let Some(rpc_id) = id else {
                    continue;
                };
                if text.contains("await-cancel") {
                    awaiting_cancel = Some(HeldPrompt {
                        rpc_id,
                        session_id: prompt_session,
                    });
                    continue;
                }
                if text.contains("permission") {
                    next_request_id += 1;
                    write_line(&permission_request(next_request_id, &prompt_session));
                    awaiting_permission = Some((
                        json!(next_request_id),
                        HeldPrompt {
                            rpc_id,
                            session_id: prompt_session,
                        },
                    ));
                    continue;
                }
                if text.contains("diffs") {
                    emit_edit_transcript(&prompt_session);
                }
                if text.contains("unknown-event") {
                    // Neither of these may take the session down: an
                    // update kind this build has never heard of, and a
                    // whole notification method it has never heard of.
                    write_line(&update(
                        &prompt_session,
                        json!({"sessionUpdate": "quantum_flux_update", "flux": 3}),
                    ));
                    write_line(&notification(
                        "hermes/telemetry",
                        json!({"sessionId": prompt_session, "counter": 1}),
                    ));
                }
                write_line(&update(
                    &prompt_session,
                    json!({
                        "sessionUpdate": "agent_message_chunk",
                        "content": {"type": "text", "text": "DONE"}
                    }),
                ));
                write_line(&update(
                    &prompt_session,
                    json!({"sessionUpdate": "usage_update", "size": 272_000, "used": 14839}),
                ));
                write_line(&json!({
                    "jsonrpc": "2.0",
                    "id": rpc_id,
                    "result": prompt_result("end_turn")
                }));
            }
            "session/cancel" => {
                // A cancel ends whatever the turn is waiting on, an
                // unanswered approval included — the client is then
                // expected to answer that orphaned request with a
                // `cancelled` outcome, which lands here as a response to
                // a request this side has already dropped.
                let held = awaiting_cancel
                    .take()
                    .or_else(|| awaiting_permission.take().map(|(_, held)| held));
                if let Some(held) = held {
                    write_line(&json!({
                        "jsonrpc": "2.0",
                        "id": held.rpc_id,
                        "result": prompt_result("cancelled")
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
