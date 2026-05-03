//! Pure translation layer: sidecar notifications → canonical
//! [`ProviderRuntimeEvent`].
//!
//! Two paths:
//!
//! * Structural SDK-message classification: the sidecar forwards
//!   every SDK message opaquely as JSON in an `sdk-message`
//!   notification. [`translate_sdk_message`] inspects the JSON shape
//!   (top-level `type`, optional `subtype`) and produces one or more
//!   canonical events. Unknown shapes become a
//!   [`ProviderRuntimeEvent::RuntimeWarning`] with the raw JSON
//!   preserved — we never silently drop.
//!
//! * Direct mapping for the sidecar's purpose-built notifications
//!   (`request-opened`, `user-input-requested`, `plan-proposed`,
//!   `session-ended`, `session-error`, `request-resolved`,
//!   `session-configured`).
//!
//! All functions here are pure. Callers run them inside a
//! `std::panic::catch_unwind` so a translation bug cannot silently
//! kill the notification task.

use crate::agent_provider::{
    CompletedItem, ContentDelta, ProviderRuntimeEvent, ProviderSessionId, RequestId,
    SessionStatus, ThreadId, TurnId, TurnStatus, TurnUsage,
};

use super::protocol::{SidecarError, SidecarNotification};

// ---------------------------------------------------------------------------
// Top-level entry point
// ---------------------------------------------------------------------------

/// Translate one sidecar notification into zero or more canonical
/// events.
pub fn translate_notification(
    thread_id: &ThreadId,
    notification: SidecarNotification,
) -> Vec<ProviderRuntimeEvent> {
    match notification {
        SidecarNotification::SessionConfigured {
            thread_id: _,
            path_to_claude_code_executable: _,
        } => vec![ProviderRuntimeEvent::SessionStateChanged {
            thread_id: thread_id.clone(),
            status: SessionStatus::Ready,
        }],
        SidecarNotification::SdkMessage { message, .. } => {
            translate_sdk_message(thread_id, &message)
        }
        SidecarNotification::RequestOpened {
            thread_id: _,
            request_id,
            tool_name,
            tool_input,
            tool_use_id,
            kind,
        } => {
            let turn_id = extract_turn_id(&tool_input);
            vec![ProviderRuntimeEvent::RequestOpened {
                thread_id: thread_id.clone(),
                turn_id,
                request_id: RequestId(request_id),
                request_kind: if kind.is_empty() {
                    classify_tool(&tool_name)
                } else {
                    kind
                },
                payload: tool_input,
                tool_use_id,
            }]
        }
        SidecarNotification::RequestResolved {
            thread_id: _,
            request_id,
            decision,
        } => {
            // Translate the sidecar-shaped decision (behavior: allow|deny,
            // plus optional message/interrupt) back into the canonical
            // enum. Previously this event was suppressed, which left the
            // UI's optimistic "Submitting decision…" state stuck forever.
            // Emitting it here lets the frontend reducer flip the
            // permission-request row to its final resolved state.
            vec![ProviderRuntimeEvent::RequestResolved {
                thread_id: thread_id.clone(),
                request_id: RequestId(request_id),
                decision: sidecar_decision_to_approval(&decision),
            }]
        }
        SidecarNotification::UserInputRequested {
            thread_id: _,
            tool_use_id,
            input,
        } => {
            vec![ProviderRuntimeEvent::RequestOpened {
                thread_id: thread_id.clone(),
                turn_id: TurnId(String::new()),
                request_id: RequestId(tool_use_id.clone().unwrap_or_default()),
                request_kind: "user-input".into(),
                payload: input,
                tool_use_id,
            }]
        }
        SidecarNotification::PlanProposed {
            thread_id: _,
            tool_use_id,
            plan,
        } => {
            vec![ProviderRuntimeEvent::RequestOpened {
                thread_id: thread_id.clone(),
                turn_id: TurnId(String::new()),
                request_id: RequestId(tool_use_id.clone().unwrap_or_default()),
                request_kind: "plan".into(),
                payload: plan,
                tool_use_id,
            }]
        }
        SidecarNotification::SessionEnded { reason, .. } => {
            let turn_id = TurnId(String::new());
            let status = match reason.as_str() {
                "iteration-complete" => TurnStatus::Success,
                "interrupted" => TurnStatus::Error {
                    subtype: "interrupted".into(),
                    message: "session interrupted".into(),
                },
                other => TurnStatus::Error {
                    subtype: other.to_string(),
                    message: format!("session ended with reason {other}"),
                },
            };
            vec![
                ProviderRuntimeEvent::TurnCompleted {
                    thread_id: thread_id.clone(),
                    turn_id,
                    status,
                    usage: None,
                },
                ProviderRuntimeEvent::SessionStateChanged {
                    thread_id: thread_id.clone(),
                    status: if reason == "iteration-complete" {
                        SessionStatus::Ready
                    } else {
                        SessionStatus::Closed
                    },
                },
            ]
        }
        SidecarNotification::SessionError {
            thread_id: _,
            error: SidecarError { message, .. },
        } => vec![
            ProviderRuntimeEvent::SessionStateChanged {
                thread_id: thread_id.clone(),
                status: SessionStatus::Error {
                    message: message.clone(),
                },
            },
            ProviderRuntimeEvent::RuntimeWarning {
                thread_id: Some(thread_id.clone()),
                message: format!("sidecar session error: {message}"),
                original_payload: None,
            },
        ],
        SidecarNotification::SdkSessionId {
            thread_id: _,
            session_id,
        } => vec![ProviderRuntimeEvent::ResumeCursorUpdated {
            thread_id: thread_id.clone(),
            resume_cursor: serde_json::json!({ "resume": session_id }),
        }],
        SidecarNotification::Unknown { method, params } => {
            vec![ProviderRuntimeEvent::RuntimeWarning {
                thread_id: Some(thread_id.clone()),
                message: format!("unknown sidecar notification: {method}"),
                original_payload: Some(params),
            }]
        }
    }
}

// ---------------------------------------------------------------------------
// SDK message classifier
// ---------------------------------------------------------------------------

/// Inspect an SDK-emitted message (opaque JSON) and emit zero or
/// more canonical events. Never panics; never silently drops.
pub fn translate_sdk_message(
    thread_id: &ThreadId,
    msg: &serde_json::Value,
) -> Vec<ProviderRuntimeEvent> {
    let Some(ty) = msg.get("type").and_then(|v| v.as_str()) else {
        return warning(thread_id, "sdk message missing `type`", msg);
    };
    match ty {
        "assistant" => translate_assistant(thread_id, msg),
        "user" => translate_user(thread_id, msg),
        "user-replay" => {
            // Replays are transcript-rebuild hints; we forward them
            // as a warning so the orchestrator can decide whether to
            // render them.
            warning(thread_id, "sdk user-replay message", msg)
        }
        "result" => translate_result(thread_id, msg),
        "system" => translate_system(thread_id, msg),
        "stream_event" => translate_stream_event(thread_id, msg),
        "auth_status" => warning(thread_id, "auth status changed", msg),
        "tool_progress" | "tool_use_summary" => {
            warning(thread_id, &format!("sdk {ty}"), msg)
        }
        "rate_limit_event" => warning(thread_id, "rate limit event", msg),
        "prompt_suggestion" => warning(thread_id, "prompt suggestion", msg),
        _ => warning(thread_id, &format!("unknown sdk message type: {ty}"), msg),
    }
}

fn warning(
    thread_id: &ThreadId,
    message: &str,
    payload: &serde_json::Value,
) -> Vec<ProviderRuntimeEvent> {
    vec![ProviderRuntimeEvent::RuntimeWarning {
        thread_id: Some(thread_id.clone()),
        message: message.to_string(),
        original_payload: Some(payload.clone()),
    }]
}

/// `type: "assistant"` — may carry text, thinking, and/or tool_use
/// blocks. Each is emitted as an `ItemCompleted`. The assistant
/// `error` field (7 enumerated kinds) is surfaced as a
/// `RuntimeWarning`.
fn translate_assistant(
    thread_id: &ThreadId,
    msg: &serde_json::Value,
) -> Vec<ProviderRuntimeEvent> {
    let mut out = Vec::new();
    let turn_id = extract_turn_id(msg);
    if let Some(err) = msg.get("error").and_then(|v| v.as_str()) {
        out.push(ProviderRuntimeEvent::RuntimeWarning {
            thread_id: Some(thread_id.clone()),
            message: format!("assistant error: {err}"),
            original_payload: Some(msg.clone()),
        });
    }
    let content = msg
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_array());
    if let Some(blocks) = content {
        for block in blocks {
            let Some(bty) = block.get("type").and_then(|v| v.as_str()) else {
                continue;
            };
            match bty {
                "text" => {
                    if let Some(text) = block.get("text").and_then(|v| v.as_str()) {
                        out.push(ProviderRuntimeEvent::ItemCompleted {
                            thread_id: thread_id.clone(),
                            turn_id: turn_id.clone(),
                            item: CompletedItem::AssistantText {
                                text: text.to_string(),
                            },
                        });
                    }
                }
                "thinking" => {
                    if let Some(text) = block.get("thinking").and_then(|v| v.as_str()) {
                        out.push(ProviderRuntimeEvent::ItemCompleted {
                            thread_id: thread_id.clone(),
                            turn_id: turn_id.clone(),
                            item: CompletedItem::AssistantThinking {
                                text: text.to_string(),
                            },
                        });
                    }
                }
                "tool_use" => {
                    let tool_name = block
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string();
                    let tool_use_id = block
                        .get("id")
                        .and_then(|v| v.as_str())
                        .unwrap_or_default()
                        .to_string();
                    let input = block.get("input").cloned().unwrap_or_default();
                    out.push(ProviderRuntimeEvent::ItemCompleted {
                        thread_id: thread_id.clone(),
                        turn_id: turn_id.clone(),
                        item: CompletedItem::ToolUse {
                            tool_name,
                            input,
                            tool_use_id,
                        },
                    });
                }
                _ => {}
            }
        }
    }
    if out.is_empty() {
        // Nothing mapped — emit a warning so we know the variant
        // was seen.
        out = warning(thread_id, "assistant message with no recognized blocks", msg);
    }
    out
}

/// `type: "user"` — sent by the SDK when a tool_result is appended
/// to the transcript. Emit `ItemCompleted::ToolResult` for each
/// tool_result block.
fn translate_user(
    thread_id: &ThreadId,
    msg: &serde_json::Value,
) -> Vec<ProviderRuntimeEvent> {
    let turn_id = extract_turn_id(msg);
    let Some(blocks) = msg
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_array())
    else {
        return warning(thread_id, "user message without content array", msg);
    };
    let mut out = Vec::new();
    for block in blocks {
        if block.get("type").and_then(|v| v.as_str()) == Some("tool_result") {
            let tool_use_id = block
                .get("tool_use_id")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            let content = block.get("content").cloned().unwrap_or_default();
            let is_error = block
                .get("is_error")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            out.push(ProviderRuntimeEvent::ItemCompleted {
                thread_id: thread_id.clone(),
                turn_id: turn_id.clone(),
                item: CompletedItem::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                },
            });
        }
    }
    if out.is_empty() {
        return warning(thread_id, "user message without tool_result blocks", msg);
    }
    out
}

/// `type: "result"` — turn terminus. 4 error subtypes plus success.
fn translate_result(
    thread_id: &ThreadId,
    msg: &serde_json::Value,
) -> Vec<ProviderRuntimeEvent> {
    let turn_id = extract_turn_id(msg);
    let subtype = msg
        .get("subtype")
        .and_then(|v| v.as_str())
        .unwrap_or("success");
    let status = match subtype {
        "success" => TurnStatus::Success,
        "error_max_turns" => TurnStatus::MaxTurns,
        "error_max_budget_usd" => TurnStatus::MaxBudget,
        "error_during_execution" => TurnStatus::Error {
            subtype: subtype.into(),
            message: msg
                .get("errors")
                .and_then(|v| v.as_array())
                .and_then(|a| a.first())
                .and_then(|v| v.as_str())
                .unwrap_or("error during execution")
                .to_string(),
        },
        "error_max_structured_output_retries" => TurnStatus::Error {
            subtype: subtype.into(),
            message: "max structured-output retries exceeded".into(),
        },
        other => TurnStatus::Error {
            subtype: other.into(),
            message: format!("result subtype {other}"),
        },
    };
    let usage = extract_usage(msg);
    vec![ProviderRuntimeEvent::TurnCompleted {
        thread_id: thread_id.clone(),
        turn_id,
        status,
        usage,
    }]
}

/// `type: "system"` family — discriminated by `subtype`.
fn translate_system(
    thread_id: &ThreadId,
    msg: &serde_json::Value,
) -> Vec<ProviderRuntimeEvent> {
    let subtype = msg
        .get("subtype")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    match subtype {
        "init" => {
            let sid = msg
                .get("session_id")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string();
            vec![ProviderRuntimeEvent::SessionConfigured {
                thread_id: thread_id.clone(),
                provider_session_id: ProviderSessionId(sid),
            }]
        }
        "status" => {
            let state = msg
                .get("status")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            let status = if state == "compacting" {
                SessionStatus::Running {
                    active_turn: TurnId("compacting".into()),
                }
            } else {
                SessionStatus::Ready
            };
            vec![ProviderRuntimeEvent::SessionStateChanged {
                thread_id: thread_id.clone(),
                status,
            }]
        }
        "session_state_changed" => {
            let state = msg
                .get("state")
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            let status = match state {
                "running" => SessionStatus::Running {
                    active_turn: TurnId(String::new()),
                },
                "requires_action" => SessionStatus::Ready,
                _ => SessionStatus::Ready,
            };
            vec![ProviderRuntimeEvent::SessionStateChanged {
                thread_id: thread_id.clone(),
                status,
            }]
        }
        "compact_boundary"
        | "hook_started"
        | "hook_progress"
        | "hook_response"
        | "task_started"
        | "task_updated"
        | "task_progress"
        | "task_notification"
        | "files_persisted"
        | "api_retry"
        | "local_command_output"
        | "plugin_install"
        | "notification"
        | "memory_recall"
        | "elicitation_complete"
        | "mirror_error" => {
            warning(thread_id, &format!("sdk system.{subtype}"), msg)
        }
        _ => warning(thread_id, &format!("unknown sdk system subtype: {subtype}"), msg),
    }
}

/// `type: "stream_event"` — partial SDK delta. Extract the delta
/// payload and emit the right `ContentDelta`.
fn translate_stream_event(
    thread_id: &ThreadId,
    msg: &serde_json::Value,
) -> Vec<ProviderRuntimeEvent> {
    let turn_id = extract_turn_id(msg);
    let Some(event) = msg.get("event") else {
        return warning(thread_id, "stream_event without event payload", msg);
    };
    let event_type = event.get("type").and_then(|v| v.as_str()).unwrap_or("");
    if event_type != "content_block_delta" {
        // Not an error — just not one of the shapes we render. Emit
        // a warning so the orchestrator can log it.
        return warning(thread_id, &format!("stream_event {event_type}"), msg);
    }
    let Some(delta) = event.get("delta") else {
        return warning(thread_id, "content_block_delta without delta", msg);
    };
    let delta_type = delta.get("type").and_then(|v| v.as_str()).unwrap_or("");
    let content = match delta_type {
        "text_delta" => ContentDelta::Text {
            text: delta
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
        },
        "thinking_delta" => ContentDelta::Thinking {
            text: delta
                .get("thinking")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
        },
        "input_json_delta" => ContentDelta::ToolInput {
            tool_name: String::new(),
            partial_json: delta
                .get("partial_json")
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_string(),
        },
        _ => {
            return warning(
                thread_id,
                &format!("unknown content_block_delta: {delta_type}"),
                msg,
            );
        }
    };
    vec![ProviderRuntimeEvent::ContentDelta {
        thread_id: thread_id.clone(),
        turn_id,
        delta: content,
    }]
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Pull a turn id out of whatever SDK shape we just got, falling
/// back to an empty TurnId. The SDK doesn't always stamp turn ids on
/// partials, so an empty id is semantically valid.
fn extract_turn_id(msg: &serde_json::Value) -> TurnId {
    msg.get("turn_id")
        .or_else(|| msg.get("turnId"))
        .and_then(|v| v.as_str())
        .map(|s| TurnId(s.to_string()))
        .unwrap_or_else(|| TurnId(String::new()))
}

fn extract_usage(msg: &serde_json::Value) -> Option<TurnUsage> {
    let duration_ms = msg
        .get("duration_ms")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let num_turns = msg
        .get("num_turns")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;
    let cost = msg.get("total_cost_usd").and_then(|v| v.as_f64());
    if duration_ms == 0 && num_turns == 0 && cost.is_none() {
        return None;
    }
    Some(TurnUsage {
        total_cost_usd: cost,
        duration_ms,
        num_turns,
    })
}

/// Turn the sidecar-shaped decision blob (opaque JSON we emitted
/// outbound as a `SidecarDecision`) back into the canonical
/// [`ApprovalDecision`]. The sidecar's `request-resolved`
/// notification carries this same shape — `{behavior, message?,
/// interrupt?, updatedInput?, updatedPermissions?}`. We lose the
/// `AllowForSession` distinction on the way through since the wire
/// shape doesn't preserve it; the UI only needs allow / deny /
/// cancel to render resolution state, so that trade-off is safe.
fn sidecar_decision_to_approval(v: &serde_json::Value) -> crate::agent_provider::ApprovalDecision {
    use crate::agent_provider::ApprovalDecision;
    let behavior = v.get("behavior").and_then(|b| b.as_str()).unwrap_or("");
    match behavior {
        "allow" => {
            let updated_input = v.get("updatedInput").cloned();
            let updated_permissions = v
                .get("updatedPermissions")
                .and_then(|p| p.as_array())
                .map(|arr| arr.clone());
            ApprovalDecision::Allow {
                updated_input,
                updated_permissions,
            }
        }
        "deny" => {
            let interrupt = v.get("interrupt").and_then(|b| b.as_bool()).unwrap_or(false);
            if interrupt {
                ApprovalDecision::Cancel
            } else {
                let message = v
                    .get("message")
                    .and_then(|m| m.as_str())
                    .unwrap_or_default()
                    .to_string();
                ApprovalDecision::Deny { message }
            }
        }
        _ => ApprovalDecision::Cancel,
    }
}

fn classify_tool(tool_name: &str) -> String {
    let n = tool_name.to_ascii_lowercase();
    if n == "bash" || n.contains("shell") || n.contains("command") {
        "command".into()
    } else if n.contains("read") || n == "fileread" {
        "file-read".into()
    } else if n.contains("edit") || n.contains("write") {
        "file-change".into()
    } else {
        "other".into()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn tid() -> ThreadId {
        ThreadId("t-local".into())
    }

    fn assistant_text(text: &str) -> serde_json::Value {
        json!({
            "type": "assistant",
            "turn_id": "turn-1",
            "message": {
                "content": [{"type": "text", "text": text}]
            }
        })
    }

    #[test]
    fn assistant_text_emits_item_completed() {
        let msg = assistant_text("hi");
        let events = translate_sdk_message(&tid(), &msg);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ProviderRuntimeEvent::ItemCompleted { item, .. } => match item {
                CompletedItem::AssistantText { text } => assert_eq!(text, "hi"),
                _ => panic!("expected AssistantText"),
            },
            _ => panic!("expected ItemCompleted"),
        }
    }

    #[test]
    fn assistant_thinking_emits_thinking_item() {
        let msg = json!({
            "type": "assistant",
            "turn_id": "t",
            "message": {"content": [{"type": "thinking", "thinking": "pondering"}]}
        });
        let events = translate_sdk_message(&tid(), &msg);
        assert!(matches!(
            &events[0],
            ProviderRuntimeEvent::ItemCompleted {
                item: CompletedItem::AssistantThinking { .. },
                ..
            }
        ));
    }

    #[test]
    fn assistant_tool_use_emits_tool_use_item() {
        let msg = json!({
            "type": "assistant",
            "turn_id": "t1",
            "message": {"content": [{
                "type": "tool_use",
                "id": "u-1",
                "name": "Bash",
                "input": {"command": "ls"}
            }]}
        });
        let events = translate_sdk_message(&tid(), &msg);
        match &events[0] {
            ProviderRuntimeEvent::ItemCompleted { item, .. } => match item {
                CompletedItem::ToolUse {
                    tool_name,
                    input,
                    tool_use_id,
                } => {
                    assert_eq!(tool_name, "Bash");
                    assert_eq!(input, &json!({"command": "ls"}));
                    assert_eq!(tool_use_id, "u-1");
                }
                _ => panic!("expected ToolUse"),
            },
            _ => panic!("expected ItemCompleted"),
        }
    }

    #[test]
    fn assistant_error_surfaces_as_warning_plus_content() {
        let msg = json!({
            "type": "assistant",
            "error": "rate_limit",
            "turn_id": "t",
            "message": {"content": [{"type": "text", "text": "partial"}]}
        });
        let events = translate_sdk_message(&tid(), &msg);
        assert!(events.iter().any(|e| matches!(
            e,
            ProviderRuntimeEvent::RuntimeWarning { message, .. }
                if message.contains("rate_limit")
        )));
        assert!(events.iter().any(|e| matches!(
            e,
            ProviderRuntimeEvent::ItemCompleted { .. }
        )));
    }

    #[test]
    fn user_tool_result_emits_tool_result_item() {
        let msg = json!({
            "type": "user",
            "turn_id": "t1",
            "message": {"content": [{
                "type": "tool_result",
                "tool_use_id": "u-1",
                "content": "stdout line",
                "is_error": false
            }]}
        });
        let events = translate_sdk_message(&tid(), &msg);
        match &events[0] {
            ProviderRuntimeEvent::ItemCompleted { item, .. } => match item {
                CompletedItem::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } => {
                    assert_eq!(tool_use_id, "u-1");
                    assert_eq!(content, "stdout line");
                    assert!(!*is_error);
                }
                _ => panic!("expected ToolResult"),
            },
            _ => panic!("expected ItemCompleted"),
        }
    }

    #[test]
    fn result_success_emits_turn_completed_success() {
        let msg = json!({
            "type": "result",
            "subtype": "success",
            "turn_id": "t",
            "duration_ms": 150,
            "num_turns": 1,
            "total_cost_usd": 0.001,
            "result": "ok",
            "stop_reason": null,
            "usage": {},
            "modelUsage": {},
            "permission_denials": []
        });
        let events = translate_sdk_message(&tid(), &msg);
        match &events[0] {
            ProviderRuntimeEvent::TurnCompleted { status, usage, .. } => {
                assert!(matches!(status, TurnStatus::Success));
                assert!(usage.is_some());
                assert_eq!(usage.as_ref().unwrap().duration_ms, 150);
            }
            _ => panic!("expected TurnCompleted"),
        }
    }

    #[test]
    fn result_error_during_execution_emits_turn_error() {
        let msg = json!({
            "type": "result",
            "subtype": "error_during_execution",
            "turn_id": "t",
            "errors": ["boom"]
        });
        let events = translate_sdk_message(&tid(), &msg);
        if let ProviderRuntimeEvent::TurnCompleted { status, .. } = &events[0] {
            match status {
                TurnStatus::Error { subtype, message } => {
                    assert_eq!(subtype, "error_during_execution");
                    assert_eq!(message, "boom");
                }
                _ => panic!("expected TurnStatus::Error"),
            }
        } else {
            panic!("expected TurnCompleted");
        }
    }

    #[test]
    fn result_max_turns_emits_turn_max_turns() {
        let msg = json!({"type": "result", "subtype": "error_max_turns", "turn_id": "t"});
        let events = translate_sdk_message(&tid(), &msg);
        assert!(matches!(
            &events[0],
            ProviderRuntimeEvent::TurnCompleted {
                status: TurnStatus::MaxTurns,
                ..
            }
        ));
    }

    #[test]
    fn result_max_budget_emits_turn_max_budget() {
        let msg = json!({"type": "result", "subtype": "error_max_budget_usd", "turn_id": "t"});
        let events = translate_sdk_message(&tid(), &msg);
        assert!(matches!(
            &events[0],
            ProviderRuntimeEvent::TurnCompleted {
                status: TurnStatus::MaxBudget,
                ..
            }
        ));
    }

    #[test]
    fn result_max_structured_output_retries_emits_turn_error() {
        let msg = json!({
            "type": "result",
            "subtype": "error_max_structured_output_retries",
            "turn_id": "t"
        });
        let events = translate_sdk_message(&tid(), &msg);
        if let ProviderRuntimeEvent::TurnCompleted { status, .. } = &events[0] {
            match status {
                TurnStatus::Error { subtype, .. } => {
                    assert_eq!(subtype, "error_max_structured_output_retries")
                }
                _ => panic!("expected Error"),
            }
        } else {
            panic!("expected TurnCompleted");
        }
    }

    #[test]
    fn system_init_emits_session_configured() {
        let msg = json!({
            "type": "system",
            "subtype": "init",
            "session_id": "sdk-sess-1"
        });
        let events = translate_sdk_message(&tid(), &msg);
        match &events[0] {
            ProviderRuntimeEvent::SessionConfigured {
                provider_session_id,
                ..
            } => assert_eq!(provider_session_id.0, "sdk-sess-1"),
            _ => panic!("expected SessionConfigured"),
        }
    }

    #[test]
    fn system_status_emits_state_change() {
        let msg = json!({"type": "system", "subtype": "status", "status": "compacting"});
        let events = translate_sdk_message(&tid(), &msg);
        assert!(matches!(
            &events[0],
            ProviderRuntimeEvent::SessionStateChanged {
                status: SessionStatus::Running { .. },
                ..
            }
        ));
    }

    #[test]
    fn system_hook_started_emits_warning() {
        let msg = json!({
            "type": "system",
            "subtype": "hook_started",
            "hook_id": "h1",
            "hook_name": "PreToolUse"
        });
        let events = translate_sdk_message(&tid(), &msg);
        assert!(matches!(
            &events[0],
            ProviderRuntimeEvent::RuntimeWarning { .. }
        ));
    }

    #[test]
    fn system_files_persisted_emits_warning() {
        let msg = json!({
            "type": "system",
            "subtype": "files_persisted",
            "files": []
        });
        let events = translate_sdk_message(&tid(), &msg);
        assert!(matches!(
            &events[0],
            ProviderRuntimeEvent::RuntimeWarning { .. }
        ));
    }

    #[test]
    fn system_unknown_subtype_emits_warning_with_payload() {
        let msg = json!({"type": "system", "subtype": "brand_new_thing"});
        let events = translate_sdk_message(&tid(), &msg);
        match &events[0] {
            ProviderRuntimeEvent::RuntimeWarning {
                message,
                original_payload,
                ..
            } => {
                assert!(message.contains("brand_new_thing"));
                assert!(original_payload.is_some());
            }
            _ => panic!("expected RuntimeWarning"),
        }
    }

    #[test]
    fn stream_event_text_delta_emits_content_delta_text() {
        let msg = json!({
            "type": "stream_event",
            "turn_id": "t1",
            "event": {
                "type": "content_block_delta",
                "delta": {"type": "text_delta", "text": "hel"}
            }
        });
        let events = translate_sdk_message(&tid(), &msg);
        match &events[0] {
            ProviderRuntimeEvent::ContentDelta { delta, .. } => match delta {
                ContentDelta::Text { text } => assert_eq!(text, "hel"),
                _ => panic!("expected text delta"),
            },
            _ => panic!("expected ContentDelta"),
        }
    }

    #[test]
    fn stream_event_thinking_delta_emits_thinking_delta() {
        let msg = json!({
            "type": "stream_event",
            "turn_id": "t1",
            "event": {
                "type": "content_block_delta",
                "delta": {"type": "thinking_delta", "thinking": "hmm"}
            }
        });
        let events = translate_sdk_message(&tid(), &msg);
        match &events[0] {
            ProviderRuntimeEvent::ContentDelta { delta, .. } => match delta {
                ContentDelta::Thinking { text } => assert_eq!(text, "hmm"),
                _ => panic!("expected thinking delta"),
            },
            _ => panic!("expected ContentDelta"),
        }
    }

    #[test]
    fn stream_event_input_json_delta_emits_tool_input_delta() {
        let msg = json!({
            "type": "stream_event",
            "turn_id": "t1",
            "event": {
                "type": "content_block_delta",
                "delta": {"type": "input_json_delta", "partial_json": "{\"cmd\":"}
            }
        });
        let events = translate_sdk_message(&tid(), &msg);
        match &events[0] {
            ProviderRuntimeEvent::ContentDelta { delta, .. } => match delta {
                ContentDelta::ToolInput { partial_json, .. } => {
                    assert_eq!(partial_json, "{\"cmd\":")
                }
                _ => panic!("expected tool input delta"),
            },
            _ => panic!("expected ContentDelta"),
        }
    }

    #[test]
    fn stream_event_message_start_emits_warning() {
        let msg = json!({
            "type": "stream_event",
            "turn_id": "t",
            "event": {"type": "message_start"}
        });
        let events = translate_sdk_message(&tid(), &msg);
        assert!(matches!(
            &events[0],
            ProviderRuntimeEvent::RuntimeWarning { .. }
        ));
    }

    #[test]
    fn auth_status_emits_warning() {
        let msg = json!({"type": "auth_status", "isAuthenticating": false});
        let events = translate_sdk_message(&tid(), &msg);
        assert!(matches!(
            &events[0],
            ProviderRuntimeEvent::RuntimeWarning { .. }
        ));
    }

    #[test]
    fn rate_limit_event_emits_warning() {
        let msg = json!({
            "type": "rate_limit_event",
            "rate_limit_info": {"status": "rejected"}
        });
        let events = translate_sdk_message(&tid(), &msg);
        assert!(matches!(
            &events[0],
            ProviderRuntimeEvent::RuntimeWarning { .. }
        ));
    }

    #[test]
    fn unknown_sdk_type_emits_warning_with_payload() {
        let msg = json!({"type": "never_seen_this_before", "foo": "bar"});
        let events = translate_sdk_message(&tid(), &msg);
        match &events[0] {
            ProviderRuntimeEvent::RuntimeWarning {
                message,
                original_payload,
                ..
            } => {
                assert!(message.contains("never_seen_this_before"));
                assert_eq!(
                    original_payload.as_ref().unwrap(),
                    &json!({"type": "never_seen_this_before", "foo": "bar"})
                );
            }
            _ => panic!("expected RuntimeWarning"),
        }
    }

    #[test]
    fn missing_type_field_emits_warning() {
        let msg = json!({"no_type": true});
        let events = translate_sdk_message(&tid(), &msg);
        assert!(matches!(
            &events[0],
            ProviderRuntimeEvent::RuntimeWarning { .. }
        ));
    }

    // ----------------------- notification-level -------------------------

    #[test]
    fn notification_session_configured_emits_ready_state_change() {
        let n = SidecarNotification::SessionConfigured {
            thread_id: "t".into(),
            path_to_claude_code_executable: "/usr/bin/claude".into(),
        };
        let events = translate_notification(&tid(), n);
        assert!(matches!(
            &events[0],
            ProviderRuntimeEvent::SessionStateChanged {
                status: SessionStatus::Ready,
                ..
            }
        ));
    }

    #[test]
    fn notification_request_opened_with_kind_passes_through() {
        let n = SidecarNotification::RequestOpened {
            thread_id: "t".into(),
            request_id: "r-1".into(),
            tool_name: "Bash".into(),
            tool_input: json!({"command": "ls"}),
            tool_use_id: Some("tu-abc".into()),
            kind: "command".into(),
        };
        let events = translate_notification(&tid(), n);
        match &events[0] {
            ProviderRuntimeEvent::RequestOpened {
                request_kind,
                request_id,
                tool_use_id,
                ..
            } => {
                assert_eq!(request_kind, "command");
                assert_eq!(request_id.0, "r-1");
                // Stage 1 wire change — tool_use_id flows through so the
                // frontend reducer can merge the approval with its
                // originating tool_call row.
                assert_eq!(tool_use_id.as_deref(), Some("tu-abc"));
            }
            _ => panic!("expected RequestOpened"),
        }
    }

    #[test]
    fn notification_request_opened_without_kind_falls_back_to_classifier() {
        let n = SidecarNotification::RequestOpened {
            thread_id: "t".into(),
            request_id: "r-2".into(),
            tool_name: "Read".into(),
            tool_input: json!({"path": "/a"}),
            tool_use_id: None,
            kind: String::new(),
        };
        let events = translate_notification(&tid(), n);
        match &events[0] {
            ProviderRuntimeEvent::RequestOpened {
                request_kind,
                tool_use_id,
                ..
            } => {
                assert_eq!(request_kind, "file-read");
                // No tool_use_id on this path — standalone approval.
                assert!(tool_use_id.is_none());
            }
            _ => panic!("expected RequestOpened"),
        }
    }

    #[test]
    fn notification_plan_proposed_emits_plan_request_opened() {
        let n = SidecarNotification::PlanProposed {
            thread_id: "t".into(),
            tool_use_id: Some("tu-1".into()),
            plan: json!("step one"),
        };
        let events = translate_notification(&tid(), n);
        match &events[0] {
            ProviderRuntimeEvent::RequestOpened { request_kind, .. } => {
                assert_eq!(request_kind, "plan")
            }
            _ => panic!("expected RequestOpened"),
        }
    }

    #[test]
    fn notification_user_input_requested_emits_user_input_request_opened() {
        let n = SidecarNotification::UserInputRequested {
            thread_id: "t".into(),
            tool_use_id: Some("tu-2".into()),
            input: json!({"questions": []}),
        };
        let events = translate_notification(&tid(), n);
        match &events[0] {
            ProviderRuntimeEvent::RequestOpened { request_kind, .. } => {
                assert_eq!(request_kind, "user-input")
            }
            _ => panic!("expected RequestOpened"),
        }
    }

    #[test]
    fn notification_session_ended_iteration_complete_emits_success_and_ready() {
        let n = SidecarNotification::SessionEnded {
            thread_id: "t".into(),
            reason: "iteration-complete".into(),
        };
        let events = translate_notification(&tid(), n);
        assert!(matches!(
            &events[0],
            ProviderRuntimeEvent::TurnCompleted {
                status: TurnStatus::Success,
                ..
            }
        ));
        assert!(matches!(
            &events[1],
            ProviderRuntimeEvent::SessionStateChanged {
                status: SessionStatus::Ready,
                ..
            }
        ));
    }

    #[test]
    fn notification_session_ended_interrupted_emits_error_and_closed() {
        let n = SidecarNotification::SessionEnded {
            thread_id: "t".into(),
            reason: "interrupted".into(),
        };
        let events = translate_notification(&tid(), n);
        match &events[0] {
            ProviderRuntimeEvent::TurnCompleted { status, .. } => match status {
                TurnStatus::Error { subtype, .. } => assert_eq!(subtype, "interrupted"),
                _ => panic!("expected Error"),
            },
            _ => panic!("expected TurnCompleted"),
        }
        assert!(matches!(
            &events[1],
            ProviderRuntimeEvent::SessionStateChanged {
                status: SessionStatus::Closed,
                ..
            }
        ));
    }

    #[test]
    fn notification_request_resolved_allow_emits_canonical_event() {
        // Stage 1 fix: the sidecar's `request-resolved` notification
        // is now forwarded as a canonical `RequestResolved` event so
        // the frontend reducer can flip the UI out of its optimistic
        // "Submitting decision…" state. Previously suppressed.
        let n = SidecarNotification::RequestResolved {
            thread_id: "t".into(),
            request_id: "r-1".into(),
            decision: json!({"behavior": "allow"}),
        };
        let events = translate_notification(&tid(), n);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ProviderRuntimeEvent::RequestResolved {
                request_id,
                decision,
                ..
            } => {
                assert_eq!(request_id.0, "r-1");
                assert!(matches!(
                    decision,
                    crate::agent_provider::ApprovalDecision::Allow { .. }
                ));
            }
            _ => panic!("expected RequestResolved"),
        }
    }

    #[test]
    fn notification_request_resolved_deny_preserves_message() {
        let n = SidecarNotification::RequestResolved {
            thread_id: "t".into(),
            request_id: "r-2".into(),
            decision: json!({"behavior": "deny", "message": "nope"}),
        };
        let events = translate_notification(&tid(), n);
        match &events[0] {
            ProviderRuntimeEvent::RequestResolved { decision, .. } => match decision {
                crate::agent_provider::ApprovalDecision::Deny { message } => {
                    assert_eq!(message, "nope");
                }
                _ => panic!("expected Deny"),
            },
            _ => panic!("expected RequestResolved"),
        }
    }

    #[test]
    fn notification_request_resolved_deny_with_interrupt_becomes_cancel() {
        // The sidecar's Cancel variant round-trips as
        // `{behavior: "deny", interrupt: true}` — classify it back into
        // `ApprovalDecision::Cancel` so the UI renders "Cancelled".
        let n = SidecarNotification::RequestResolved {
            thread_id: "t".into(),
            request_id: "r-3".into(),
            decision: json!({"behavior": "deny", "interrupt": true}),
        };
        let events = translate_notification(&tid(), n);
        assert!(matches!(
            &events[0],
            ProviderRuntimeEvent::RequestResolved {
                decision: crate::agent_provider::ApprovalDecision::Cancel,
                ..
            }
        ));
    }

    #[test]
    fn notification_session_error_emits_state_change_plus_warning() {
        let n = SidecarNotification::SessionError {
            thread_id: "t".into(),
            error: SidecarError {
                message: "bad".into(),
                stack: None,
            },
        };
        let events = translate_notification(&tid(), n);
        assert!(matches!(
            &events[0],
            ProviderRuntimeEvent::SessionStateChanged {
                status: SessionStatus::Error { .. },
                ..
            }
        ));
        assert!(matches!(
            &events[1],
            ProviderRuntimeEvent::RuntimeWarning { .. }
        ));
    }

    #[test]
    fn notification_unknown_method_emits_warning() {
        let n = SidecarNotification::Unknown {
            method: "mystery".into(),
            params: json!({"x": 1}),
        };
        let events = translate_notification(&tid(), n);
        match &events[0] {
            ProviderRuntimeEvent::RuntimeWarning {
                message,
                original_payload,
                ..
            } => {
                assert!(message.contains("mystery"));
                assert_eq!(original_payload.as_ref().unwrap(), &json!({"x": 1}));
            }
            _ => panic!("expected RuntimeWarning"),
        }
    }
}
