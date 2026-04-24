//! Pure translation layer from Codex protocol messages to the canonical
//! [`ProviderRuntimeEvent`] enum.
//!
//! Keeping this logic in one place (and free of I/O) means the whole
//! protocol-to-event mapping is unit-testable without spawning a
//! subprocess. Every function here is a pure transform — callers are
//! responsible for broadcasting the returned events.

use crate::agent_provider::{
    CompletedItem, ContentDelta, ProviderRuntimeEvent, RequestId, SessionStatus, ThreadId, TurnId,
    TurnStatus,
};

use super::protocol::{
    AgentMessageDeltaParams, AgentMessageParams, ErrorParams, NotificationMessage,
    ServerRequestMessage, ThreadStartedParams, TurnCompletedParams, TurnStartedParams,
};

/// Translate a single notification into zero or more canonical events.
///
/// `thread_id` is the runtime-owned thread identifier (not Codex's own
/// `threadId` — those match semantically but are stored separately to keep
/// the canonical event stream self-contained).
pub fn translate_notification(
    thread_id: &ThreadId,
    msg: NotificationMessage,
) -> Vec<ProviderRuntimeEvent> {
    match msg {
        NotificationMessage::ThreadStarted(ThreadStartedParams { thread_id: codex_tid }) => {
            vec![ProviderRuntimeEvent::SessionConfigured {
                thread_id: thread_id.clone(),
                provider_session_id: crate::agent_provider::ProviderSessionId(codex_tid),
            }]
        }
        NotificationMessage::TurnStarted(TurnStartedParams {
            turn_id,
            thread_id: _codex_tid,
        }) => {
            let turn_id = TurnId(turn_id);
            vec![ProviderRuntimeEvent::SessionStateChanged {
                thread_id: thread_id.clone(),
                status: SessionStatus::Running {
                    active_turn: turn_id,
                },
            }]
        }
        NotificationMessage::TurnCompleted(params) => translate_turn_completed(thread_id, params),
        NotificationMessage::Error(ErrorParams {
            message,
            will_retry,
            thread_id: _,
        }) => {
            if will_retry {
                // Transient error — surface as a warning so the UI can
                // show a soft indicator without tearing down the session.
                vec![ProviderRuntimeEvent::RuntimeWarning {
                    thread_id: Some(thread_id.clone()),
                    message: format!("codex retrying after error: {message}"),
                    original_payload: None,
                }]
            } else {
                vec![ProviderRuntimeEvent::SessionStateChanged {
                    thread_id: thread_id.clone(),
                    status: SessionStatus::Error { message },
                }]
            }
        }
        NotificationMessage::AgentMessageDelta(AgentMessageDeltaParams {
            turn_id,
            text_delta,
            ..
        }) => vec![ProviderRuntimeEvent::ContentDelta {
            thread_id: thread_id.clone(),
            turn_id: TurnId(turn_id),
            delta: ContentDelta::Text { text: text_delta },
        }],
        NotificationMessage::AgentMessage(AgentMessageParams { turn_id, text, .. }) => {
            vec![ProviderRuntimeEvent::ItemCompleted {
                thread_id: thread_id.clone(),
                turn_id: TurnId(turn_id),
                item: CompletedItem::AssistantText { text },
            }]
        }
        NotificationMessage::ToolCall { method, params } => {
            translate_tool_call(thread_id, &method, params)
        }
        NotificationMessage::Unknown { method, params } => {
            vec![ProviderRuntimeEvent::RuntimeWarning {
                thread_id: Some(thread_id.clone()),
                message: format!("unknown codex notification: {method}"),
                original_payload: Some(params),
            }]
        }
    }
}

/// Translate a `turn/completed` notification. May emit both a
/// [`ProviderRuntimeEvent::TurnCompleted`] AND a
/// [`ProviderRuntimeEvent::SessionStateChanged`] depending on outcome.
pub fn translate_turn_completed(
    thread_id: &ThreadId,
    params: TurnCompletedParams,
) -> Vec<ProviderRuntimeEvent> {
    let turn_id = TurnId(params.turn_id);
    let (turn_status, next_session_status) = match params.status.as_str() {
        "succeeded" | "success" => (TurnStatus::Success, Some(SessionStatus::Ready)),
        "failed" | "error" => {
            let msg = params.error.clone().unwrap_or_else(|| "turn failed".into());
            (
                TurnStatus::Error {
                    subtype: params.status.clone(),
                    message: msg.clone(),
                },
                Some(SessionStatus::Error { message: msg }),
            )
        }
        other => (
            TurnStatus::Error {
                subtype: other.to_string(),
                message: params
                    .error
                    .clone()
                    .unwrap_or_else(|| format!("turn ended with status {other}")),
            },
            Some(SessionStatus::Ready),
        ),
    };

    let mut events = vec![ProviderRuntimeEvent::TurnCompleted {
        thread_id: thread_id.clone(),
        turn_id,
        status: turn_status,
        usage: None,
    }];
    if let Some(status) = next_session_status {
        events.push(ProviderRuntimeEvent::SessionStateChanged {
            thread_id: thread_id.clone(),
            status,
        });
    }
    events
}

/// Best-effort translation of `item/toolCall*` notifications. The Codex
/// wire format for these is provider-internal and evolves independently;
/// we recognise the common "started" / "completed" / "delta" variants and
/// fall back to a `RuntimeWarning` for anything else.
fn translate_tool_call(
    thread_id: &ThreadId,
    method: &str,
    params: serde_json::Value,
) -> Vec<ProviderRuntimeEvent> {
    let turn_id_str = params
        .get("turnId")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let tool_use_id = params
        .get("toolUseId")
        .or_else(|| params.get("id"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_default();
    let tool_name = params
        .get("toolName")
        .or_else(|| params.get("name"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_default();

    match (method, turn_id_str) {
        ("item/toolCall/inputDelta", Some(turn_id)) => {
            let partial = params
                .get("inputDelta")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            let partial_json = match partial {
                serde_json::Value::String(s) => s,
                other => other.to_string(),
            };
            vec![ProviderRuntimeEvent::ContentDelta {
                thread_id: thread_id.clone(),
                turn_id: TurnId(turn_id),
                delta: ContentDelta::ToolInput {
                    tool_name,
                    partial_json,
                },
            }]
        }
        ("item/toolCall/completed", Some(turn_id)) => {
            let input = params.get("input").cloned().unwrap_or(serde_json::Value::Null);
            vec![ProviderRuntimeEvent::ItemCompleted {
                thread_id: thread_id.clone(),
                turn_id: TurnId(turn_id),
                item: CompletedItem::ToolUse {
                    tool_name,
                    input,
                    tool_use_id,
                },
            }]
        }
        ("item/toolCall/result", Some(turn_id)) => {
            let content = params.get("content").cloned().unwrap_or(serde_json::Value::Null);
            let is_error = params
                .get("isError")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            vec![ProviderRuntimeEvent::ItemCompleted {
                thread_id: thread_id.clone(),
                turn_id: TurnId(turn_id),
                item: CompletedItem::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                },
            }]
        }
        _ => vec![ProviderRuntimeEvent::RuntimeWarning {
            thread_id: Some(thread_id.clone()),
            message: format!("codex tool-call notification {method} not translated"),
            original_payload: Some(params),
        }],
    }
}

/// Translate a server-initiated request into a
/// [`ProviderRuntimeEvent::RequestOpened`]. The runtime is expected to
/// track the JSON-RPC id separately so it can respond later.
pub fn translate_server_request(
    thread_id: &ThreadId,
    request_id: &RequestId,
    msg: &ServerRequestMessage,
) -> ProviderRuntimeEvent {
    // Try to extract turn_id from the payload; fall back to an empty TurnId
    // so the event is never malformed.
    let turn_id = msg
        .payload()
        .get("turnId")
        .and_then(|v| v.as_str())
        .map(|s| TurnId(s.to_string()))
        .unwrap_or_else(|| TurnId(String::new()));

    ProviderRuntimeEvent::RequestOpened {
        thread_id: thread_id.clone(),
        turn_id,
        request_id: request_id.clone(),
        request_kind: msg.request_kind().to_string(),
        payload: msg.payload().clone(),
        // Codex's server-initiated approval requests are not tied to a
        // provider-side tool_use_id — the frontend renders them as
        // standalone permission rows. Always `None` until Codex surfaces
        // one.
        tool_use_id: None,
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
        ThreadId("thr-local".into())
    }

    #[test]
    fn thread_started_becomes_session_configured() {
        let msg = NotificationMessage::from_raw("thread/started", json!({"threadId": "c1"}));
        let events = translate_notification(&tid(), msg);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ProviderRuntimeEvent::SessionConfigured {
                thread_id,
                provider_session_id,
            } => {
                assert_eq!(thread_id.0, "thr-local");
                assert_eq!(provider_session_id.0, "c1");
            }
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn turn_started_becomes_session_state_running() {
        let msg = NotificationMessage::from_raw(
            "turn/started",
            json!({"threadId":"c1","turnId":"t1"}),
        );
        let events = translate_notification(&tid(), msg);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ProviderRuntimeEvent::SessionStateChanged { status, .. } => match status {
                SessionStatus::Running { active_turn } => {
                    assert_eq!(active_turn.0, "t1")
                }
                _ => panic!("expected Running"),
            },
            _ => panic!("expected SessionStateChanged"),
        }
    }

    #[test]
    fn turn_completed_success_emits_both_events() {
        let msg = NotificationMessage::from_raw(
            "turn/completed",
            json!({"threadId":"c1","turnId":"t1","status":"succeeded"}),
        );
        let events = translate_notification(&tid(), msg);
        assert_eq!(events.len(), 2);
        matches!(events[0], ProviderRuntimeEvent::TurnCompleted { .. })
            .then_some(())
            .expect("first is TurnCompleted");
        if let ProviderRuntimeEvent::TurnCompleted { status, .. } = &events[0] {
            assert!(matches!(status, TurnStatus::Success));
        }
        if let ProviderRuntimeEvent::SessionStateChanged { status, .. } = &events[1] {
            assert!(matches!(status, SessionStatus::Ready));
        } else {
            panic!("second should be SessionStateChanged Ready");
        }
    }

    #[test]
    fn turn_completed_failed_emits_error_for_both_events() {
        let msg = NotificationMessage::from_raw(
            "turn/completed",
            json!({"threadId":"c1","turnId":"t1","status":"failed","error":"bad thing"}),
        );
        let events = translate_notification(&tid(), msg);
        assert_eq!(events.len(), 2);
        if let ProviderRuntimeEvent::TurnCompleted { status, .. } = &events[0] {
            match status {
                TurnStatus::Error { subtype, message } => {
                    assert_eq!(subtype, "failed");
                    assert_eq!(message, "bad thing");
                }
                _ => panic!("expected TurnStatus::Error"),
            }
        } else {
            panic!("first should be TurnCompleted");
        }
        if let ProviderRuntimeEvent::SessionStateChanged { status, .. } = &events[1] {
            match status {
                SessionStatus::Error { message } => assert_eq!(message, "bad thing"),
                _ => panic!("expected SessionStatus::Error"),
            }
        }
    }

    #[test]
    fn error_with_will_retry_false_flips_session_to_error() {
        let msg = NotificationMessage::from_raw(
            "error",
            json!({"message": "boom", "willRetry": false}),
        );
        let events = translate_notification(&tid(), msg);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ProviderRuntimeEvent::SessionStateChanged { status, .. } => match status {
                SessionStatus::Error { message } => assert_eq!(message, "boom"),
                _ => panic!("expected Error"),
            },
            _ => panic!("expected SessionStateChanged"),
        }
    }

    #[test]
    fn error_with_will_retry_true_becomes_runtime_warning() {
        let msg = NotificationMessage::from_raw(
            "error",
            json!({"message":"transient","willRetry":true}),
        );
        let events = translate_notification(&tid(), msg);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ProviderRuntimeEvent::RuntimeWarning { message, .. } => {
                assert!(message.contains("retrying"));
                assert!(message.contains("transient"));
            }
            _ => panic!("expected RuntimeWarning"),
        }
    }

    #[test]
    fn agent_message_delta_becomes_content_delta_text() {
        let msg = NotificationMessage::from_raw(
            "item/agentMessage/delta",
            json!({"threadId":"c1","turnId":"t1","textDelta":"hel"}),
        );
        let events = translate_notification(&tid(), msg);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ProviderRuntimeEvent::ContentDelta { delta, .. } => match delta {
                ContentDelta::Text { text } => assert_eq!(text, "hel"),
                _ => panic!("expected text delta"),
            },
            _ => panic!("expected ContentDelta"),
        }
    }

    #[test]
    fn agent_message_becomes_item_completed() {
        let msg = NotificationMessage::from_raw(
            "item/agentMessage",
            json!({"threadId":"c1","turnId":"t1","text":"final"}),
        );
        let events = translate_notification(&tid(), msg);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ProviderRuntimeEvent::ItemCompleted { item, .. } => match item {
                CompletedItem::AssistantText { text } => assert_eq!(text, "final"),
                _ => panic!("expected AssistantText"),
            },
            _ => panic!("expected ItemCompleted"),
        }
    }

    #[test]
    fn unknown_notification_preserves_payload_as_warning() {
        let msg = NotificationMessage::from_raw(
            "foo/bar/baz",
            json!({"interesting":"data","n":5}),
        );
        let events = translate_notification(&tid(), msg);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ProviderRuntimeEvent::RuntimeWarning {
                message,
                original_payload,
                ..
            } => {
                assert!(message.contains("foo/bar/baz"));
                assert_eq!(
                    original_payload.as_ref().unwrap(),
                    &json!({"interesting":"data","n":5})
                );
            }
            _ => panic!("expected RuntimeWarning"),
        }
    }

    #[test]
    fn tool_call_input_delta_becomes_content_delta() {
        let msg = NotificationMessage::from_raw(
            "item/toolCall/inputDelta",
            json!({"turnId":"t1","toolName":"shell","inputDelta":"{\"cmd\":"}),
        );
        let events = translate_notification(&tid(), msg);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ProviderRuntimeEvent::ContentDelta { delta, .. } => match delta {
                ContentDelta::ToolInput {
                    tool_name,
                    partial_json,
                } => {
                    assert_eq!(tool_name, "shell");
                    assert_eq!(partial_json, "{\"cmd\":");
                }
                _ => panic!("expected ToolInput"),
            },
            _ => panic!("expected ContentDelta"),
        }
    }

    #[test]
    fn tool_call_completed_becomes_item_tool_use() {
        let msg = NotificationMessage::from_raw(
            "item/toolCall/completed",
            json!({
                "turnId":"t1",
                "toolName":"shell",
                "toolUseId":"u-1",
                "input":{"cmd":"ls"}
            }),
        );
        let events = translate_notification(&tid(), msg);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ProviderRuntimeEvent::ItemCompleted { item, .. } => match item {
                CompletedItem::ToolUse {
                    tool_name,
                    input,
                    tool_use_id,
                } => {
                    assert_eq!(tool_name, "shell");
                    assert_eq!(input, &json!({"cmd":"ls"}));
                    assert_eq!(tool_use_id, "u-1");
                }
                _ => panic!("expected ToolUse"),
            },
            _ => panic!("expected ItemCompleted"),
        }
    }

    #[test]
    fn tool_call_result_becomes_item_tool_result() {
        let msg = NotificationMessage::from_raw(
            "item/toolCall/result",
            json!({
                "turnId":"t1",
                "toolUseId":"u-1",
                "content":{"stdout":"hi"},
                "isError":false
            }),
        );
        let events = translate_notification(&tid(), msg);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ProviderRuntimeEvent::ItemCompleted { item, .. } => match item {
                CompletedItem::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                } => {
                    assert_eq!(tool_use_id, "u-1");
                    assert_eq!(content, &json!({"stdout":"hi"}));
                    assert!(!*is_error);
                }
                _ => panic!("expected ToolResult"),
            },
            _ => panic!("expected ItemCompleted"),
        }
    }

    #[test]
    fn tool_call_unknown_subtype_becomes_warning() {
        let msg = NotificationMessage::from_raw(
            "item/toolCall/somethingNew",
            json!({"data":123}),
        );
        let events = translate_notification(&tid(), msg);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ProviderRuntimeEvent::RuntimeWarning {
                message,
                original_payload,
                ..
            } => {
                assert!(message.contains("item/toolCall/somethingNew"));
                assert!(original_payload.is_some());
            }
            _ => panic!("expected RuntimeWarning"),
        }
    }

    #[test]
    fn server_request_command_approval_classified() {
        let msg = ServerRequestMessage::from_raw(
            "item/commandExecution/requestApproval",
            json!({"cmd":"rm -rf /", "turnId":"t1"}),
        );
        let rid = RequestId("r1".into());
        let ev = translate_server_request(&tid(), &rid, &msg);
        match ev {
            ProviderRuntimeEvent::RequestOpened {
                request_kind,
                request_id,
                turn_id,
                ..
            } => {
                assert_eq!(request_kind, "command");
                assert_eq!(request_id.0, "r1");
                assert_eq!(turn_id.0, "t1");
            }
            _ => panic!("expected RequestOpened"),
        }
    }

    #[test]
    fn server_request_file_change_approval_classified() {
        let msg =
            ServerRequestMessage::from_raw("item/fileChange/requestApproval", json!({"x":1}));
        let rid = RequestId("r2".into());
        let ev = translate_server_request(&tid(), &rid, &msg);
        if let ProviderRuntimeEvent::RequestOpened { request_kind, .. } = ev {
            assert_eq!(request_kind, "file-change");
        } else {
            panic!("expected RequestOpened");
        }
    }

    #[test]
    fn server_request_file_read_approval_classified() {
        let msg = ServerRequestMessage::from_raw("item/fileRead/requestApproval", json!({}));
        let rid = RequestId("r3".into());
        let ev = translate_server_request(&tid(), &rid, &msg);
        if let ProviderRuntimeEvent::RequestOpened { request_kind, .. } = ev {
            assert_eq!(request_kind, "file-read");
        } else {
            panic!("expected RequestOpened");
        }
    }

    #[test]
    fn server_request_user_input_classified() {
        let msg = ServerRequestMessage::from_raw("item/tool/requestUserInput", json!({}));
        let rid = RequestId("r4".into());
        let ev = translate_server_request(&tid(), &rid, &msg);
        if let ProviderRuntimeEvent::RequestOpened { request_kind, .. } = ev {
            assert_eq!(request_kind, "user-input");
        } else {
            panic!("expected RequestOpened");
        }
    }

    #[test]
    fn server_request_unknown_method_is_still_surfaced() {
        let msg = ServerRequestMessage::from_raw("item/foo/requestBizarro", json!({}));
        let rid = RequestId("r5".into());
        let ev = translate_server_request(&tid(), &rid, &msg);
        if let ProviderRuntimeEvent::RequestOpened { request_kind, .. } = ev {
            assert_eq!(request_kind, "unknown");
        } else {
            panic!("expected RequestOpened");
        }
    }
}
