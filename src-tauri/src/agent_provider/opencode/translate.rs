//! Bidirectional adapter between Codemux's chat-runtime types and
//! OpenCode's HTTP/SSE protocol.
//!
//! Two halves:
//!
//! 1. **Outbound** (Codemux → OpenCode). Helpers that turn a
//!    [`SendTurnInput`] / [`StartSessionInput`] / [`ApprovalDecision`]
//!    into the matching wire body.
//! 2. **Inbound** (OpenCode → Codemux). [`opencode_event_to_runtime`]
//!    consumes one decoded [`OpenCodeEvent`] plus the [`ThreadId`] +
//!    [`TurnId`] pair the SSE listener resolved for that session and
//!    returns a list of [`ProviderRuntimeEvent`]s to publish.
//!
//! The inbound translator is responsible for surfacing unknown events
//! as [`ProviderRuntimeEvent::RuntimeWarning`] instead of silently
//! dropping them — adapter drift must be observable.

use crate::agent_provider::events::{
    CompletedItem, ContentDelta, ProviderRuntimeEvent, TurnStatus,
};
use crate::agent_provider::types::{
    ApprovalDecision, ProviderSessionId, RequestId, SessionStatus, ThreadId, TurnId,
};

use super::protocol::{
    KnownEvent, ModelRef, OpenCodeApiError, OpenCodeEvent, PartInput, PartPayload,
    PermissionReply, PromptAsyncRequest, ToolStateValue,
};

// ── Outbound translation (Codemux → OpenCode wire) ──────────────────

/// Split a Codemux model id like `"openai/gpt-5"` into the OpenCode
/// federated `{providerID, modelID}` pair.
///
/// Returns `None` when the id has no `/`; the caller should treat that
/// as a validation error since OpenCode requires both halves on the
/// prompt body.
pub fn split_model_id(model_id: &str) -> Option<ModelRef> {
    let (provider, model) = model_id.split_once('/')?;
    if provider.is_empty() || model.is_empty() {
        return None;
    }
    Some(ModelRef {
        provider_id: provider.to_string(),
        model_id: model.to_string(),
    })
}

/// Build a `prompt_async` body for a user turn.
///
/// `text` is the composer text; `images` are appended as `file` parts
/// using `data:` URIs since the SDK accepts any URL — embedding the
/// bytes inline avoids round-tripping through OpenCode's file API for
/// the chat path.
pub fn build_prompt_async_request(
    text: String,
    model_id: Option<&str>,
    variant: Option<&str>,
    images: &[crate::agent_provider::types::ImageInput],
) -> Result<PromptAsyncRequest, String> {
    let model = match model_id {
        Some(id) => Some(split_model_id(id).ok_or_else(|| {
            format!("invalid_model_id: expected providerID/modelID, got {id:?}")
        })?),
        None => None,
    };
    let mut parts = Vec::with_capacity(1 + images.len());
    parts.push(PartInput::Text { text });
    for image in images {
        parts.push(PartInput::File {
            mime: image.media_type.clone(),
            url: data_uri_for_image(&image.media_type, &image.data),
            filename: None,
        });
    }
    Ok(PromptAsyncRequest {
        message_id: None,
        model,
        agent: None,
        // Normalize an empty/whitespace variant to `None` so a cleared
        // reasoning picker doesn't send `variant: ""` (OpenCode would
        // reject that as an unknown variant key).
        variant: variant
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .map(str::to_string),
        parts,
    })
}

fn data_uri_for_image(media_type: &str, bytes: &[u8]) -> String {
    use base64::Engine;
    let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
    format!("data:{media_type};base64,{b64}")
}

/// Map a Codemux approval decision to the SDK's three-token reply.
///
/// `Allow` and `Cancel` collapse onto OpenCode's `once`/`reject`
/// respectively — OpenCode does not model a per-call "deny with
/// message" the way Claude does, so the message text from
/// `ApprovalDecision::Deny` is dropped at the wire layer (it stays
/// available in the runtime event log for the UI).
pub fn approval_decision_to_permission_reply(decision: &ApprovalDecision) -> PermissionReply {
    match decision {
        ApprovalDecision::Allow { .. } => PermissionReply::Once,
        ApprovalDecision::AllowForSession => PermissionReply::Always,
        ApprovalDecision::Deny { .. } => PermissionReply::Reject,
        ApprovalDecision::Cancel => PermissionReply::Reject,
    }
}

// ── Inbound translation (OpenCode SSE → Codemux events) ─────────────

/// Side-effect-free routing context the SSE listener hands the
/// translator for each event.
#[derive(Debug, Clone)]
pub struct EventContext {
    pub thread_id: ThreadId,
    /// Active turn id Codemux assigned when the user hit send.
    pub turn_id: TurnId,
    /// OpenCode's session id for the active session — used to
    /// surface as the `ProviderSessionId` on `SessionConfigured`.
    pub provider_session_id: ProviderSessionId,
}

/// Translate one OpenCode SSE event to zero or more Codemux runtime
/// events.
///
/// Returns an empty `Vec` when the event is irrelevant for the chat
/// runtime (e.g. `session.created` for a session we did not initiate).
/// The SSE listener is responsible for filtering events by session id
/// before calling this; the context is only used for downstream
/// labelling.
pub fn opencode_event_to_runtime(
    event: OpenCodeEvent,
    ctx: &EventContext,
) -> Vec<ProviderRuntimeEvent> {
    match event {
        OpenCodeEvent::Known(known) => translate_known(known, ctx),
        OpenCodeEvent::Other(payload) => {
            let event_type = payload
                .get("type")
                .and_then(|v| v.as_str())
                .unwrap_or("<unknown>")
                .to_string();
            vec![ProviderRuntimeEvent::RuntimeWarning {
                thread_id: Some(ctx.thread_id.clone()),
                message: format!("opencode_event_unhandled: {event_type}"),
                original_payload: Some(payload),
            }]
        }
    }
}

fn translate_known(event: KnownEvent, ctx: &EventContext) -> Vec<ProviderRuntimeEvent> {
    match event {
        KnownEvent::MessagePartDelta(delta) => translate_part_delta(delta, ctx),
        KnownEvent::MessagePartUpdated(updated) => translate_part_updated(updated, ctx),
        KnownEvent::MessagePartRemoved(_) => vec![],
        KnownEvent::MessageUpdated(env) => translate_message_updated(env, ctx),
        KnownEvent::MessageRemoved(_) => vec![],
        KnownEvent::SessionIdle(_) => vec![ProviderRuntimeEvent::TurnCompleted {
            thread_id: ctx.thread_id.clone(),
            turn_id: ctx.turn_id.clone(),
            status: TurnStatus::Success,
            usage: None,
        }],
        KnownEvent::SessionStatus(status) => match status.status {
            super::protocol::SessionStatusValue::Idle => {
                vec![ProviderRuntimeEvent::SessionStateChanged {
                    thread_id: ctx.thread_id.clone(),
                    status: SessionStatus::Ready,
                }]
            }
            super::protocol::SessionStatusValue::Busy => {
                vec![ProviderRuntimeEvent::SessionStateChanged {
                    thread_id: ctx.thread_id.clone(),
                    status: SessionStatus::Running {
                        active_turn: ctx.turn_id.clone(),
                    },
                }]
            }
            super::protocol::SessionStatusValue::Retry { message, .. } => {
                vec![ProviderRuntimeEvent::RuntimeWarning {
                    thread_id: Some(ctx.thread_id.clone()),
                    message: format!("opencode_retry: {message}"),
                    original_payload: None,
                }]
            }
        },
        KnownEvent::SessionError(err) => translate_session_error(err.error, ctx),
        KnownEvent::PermissionAsked(p) => {
            let payload = serde_json::json!({
                "permission": p.permission,
                "patterns": p.patterns,
                "metadata": p.metadata,
            });
            vec![ProviderRuntimeEvent::RequestOpened {
                thread_id: ctx.thread_id.clone(),
                turn_id: ctx.turn_id.clone(),
                request_id: RequestId(p.id),
                request_kind: "tool_permission".to_string(),
                payload,
                tool_use_id: p.tool.map(|t| t.call_id),
            }]
        }
        KnownEvent::PermissionReplied(reply) => {
            let decision = match reply.reply {
                PermissionReply::Once => ApprovalDecision::Allow {
                    updated_input: None,
                    updated_permissions: None,
                },
                PermissionReply::Always => ApprovalDecision::AllowForSession,
                PermissionReply::Reject => ApprovalDecision::Cancel,
            };
            vec![ProviderRuntimeEvent::RequestResolved {
                thread_id: ctx.thread_id.clone(),
                request_id: RequestId(reply.request_id),
                decision,
            }]
        }
        // session.created/updated/deleted — Codemux already knows
        // about the session it initiated; skip to avoid double-emit.
        KnownEvent::SessionCreated(_)
        | KnownEvent::SessionUpdated(_)
        | KnownEvent::SessionDeleted(_) => vec![],
    }
}

fn translate_part_delta(
    delta: super::protocol::MessagePartDelta,
    ctx: &EventContext,
) -> Vec<ProviderRuntimeEvent> {
    if delta.field != "text" {
        return vec![];
    }
    vec![ProviderRuntimeEvent::ContentDelta {
        thread_id: ctx.thread_id.clone(),
        turn_id: ctx.turn_id.clone(),
        delta: ContentDelta::Text { text: delta.delta },
    }]
}

fn translate_part_updated(
    updated: super::protocol::MessagePartUpdated,
    ctx: &EventContext,
) -> Vec<ProviderRuntimeEvent> {
    match updated.part {
        PartPayload::Text(text) => {
            // Synthetic parts are placeholders OpenCode inserts during
            // multi-step replies — they carry no user-visible text.
            if text.synthetic.unwrap_or(false) || text.text.is_empty() {
                return vec![];
            }
            vec![ProviderRuntimeEvent::ItemCompleted {
                thread_id: ctx.thread_id.clone(),
                turn_id: ctx.turn_id.clone(),
                item: CompletedItem::AssistantText { text: text.text },
            }]
        }
        PartPayload::Reasoning(reasoning) => {
            if reasoning.text.is_empty() {
                return vec![];
            }
            vec![ProviderRuntimeEvent::ItemCompleted {
                thread_id: ctx.thread_id.clone(),
                turn_id: ctx.turn_id.clone(),
                item: CompletedItem::AssistantThinking {
                    text: reasoning.text,
                },
            }]
        }
        PartPayload::Tool(tool) => translate_tool_part(tool, ctx),
        PartPayload::Other => vec![],
    }
}

fn translate_tool_part(
    tool: super::protocol::ToolPart,
    ctx: &EventContext,
) -> Vec<ProviderRuntimeEvent> {
    match tool.state {
        ToolStateValue::Pending { .. } | ToolStateValue::Running { .. } => vec![],
        ToolStateValue::Completed { input, output, .. } => vec![
            ProviderRuntimeEvent::ItemCompleted {
                thread_id: ctx.thread_id.clone(),
                turn_id: ctx.turn_id.clone(),
                item: CompletedItem::ToolUse {
                    tool_name: tool.tool.clone(),
                    input,
                    tool_use_id: tool.call_id.clone(),
                },
            },
            ProviderRuntimeEvent::ItemCompleted {
                thread_id: ctx.thread_id.clone(),
                turn_id: ctx.turn_id.clone(),
                item: CompletedItem::ToolResult {
                    tool_use_id: tool.call_id,
                    content: serde_json::Value::String(output),
                    is_error: false,
                },
            },
        ],
        ToolStateValue::Error { input, error } => vec![
            ProviderRuntimeEvent::ItemCompleted {
                thread_id: ctx.thread_id.clone(),
                turn_id: ctx.turn_id.clone(),
                item: CompletedItem::ToolUse {
                    tool_name: tool.tool.clone(),
                    input,
                    tool_use_id: tool.call_id.clone(),
                },
            },
            ProviderRuntimeEvent::ItemCompleted {
                thread_id: ctx.thread_id.clone(),
                turn_id: ctx.turn_id.clone(),
                item: CompletedItem::ToolResult {
                    tool_use_id: tool.call_id,
                    content: serde_json::Value::String(error),
                    is_error: true,
                },
            },
        ],
    }
}

fn translate_message_updated(
    env: super::protocol::MessageEnvelope,
    ctx: &EventContext,
) -> Vec<ProviderRuntimeEvent> {
    // Only assistant errors matter at this layer; all other content is
    // already covered by the per-part events.
    if env.info.role != "assistant" {
        return vec![];
    }
    if let Some(err) = env.info.error {
        return translate_assistant_error(err, ctx);
    }
    vec![]
}

fn translate_session_error(
    error: Option<OpenCodeApiError>,
    ctx: &EventContext,
) -> Vec<ProviderRuntimeEvent> {
    let Some(err) = error else {
        return vec![ProviderRuntimeEvent::TurnCompleted {
            thread_id: ctx.thread_id.clone(),
            turn_id: ctx.turn_id.clone(),
            status: TurnStatus::Error {
                subtype: "unknown".into(),
                message: "session.error with empty payload".into(),
            },
            usage: None,
        }];
    };
    let (subtype, message) = err.display_pair();
    vec![ProviderRuntimeEvent::TurnCompleted {
        thread_id: ctx.thread_id.clone(),
        turn_id: ctx.turn_id.clone(),
        status: TurnStatus::Error { subtype, message },
        usage: None,
    }]
}

fn translate_assistant_error(
    err: OpenCodeApiError,
    ctx: &EventContext,
) -> Vec<ProviderRuntimeEvent> {
    let (subtype, message) = err.display_pair();
    vec![ProviderRuntimeEvent::TurnCompleted {
        thread_id: ctx.thread_id.clone(),
        turn_id: ctx.turn_id.clone(),
        status: TurnStatus::Error { subtype, message },
        usage: None,
    }]
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::protocol::KnownEvent;
    use crate::agent_provider::types::{ImageInput, ProviderSessionId, ThreadId, TurnId};

    fn ctx() -> EventContext {
        EventContext {
            thread_id: ThreadId("thread_1".into()),
            turn_id: TurnId("turn_1".into()),
            provider_session_id: ProviderSessionId("sess_1".into()),
        }
    }

    #[test]
    fn split_model_id_extracts_pair() {
        let r = split_model_id("openai/gpt-5").unwrap();
        assert_eq!(r.provider_id, "openai");
        assert_eq!(r.model_id, "gpt-5");
    }

    #[test]
    fn split_model_id_rejects_missing_separator() {
        assert!(split_model_id("gpt-5").is_none());
        assert!(split_model_id("/gpt-5").is_none());
        assert!(split_model_id("openai/").is_none());
    }

    #[test]
    fn build_prompt_async_request_attaches_text_and_images() {
        let images = vec![ImageInput {
            data: vec![1, 2, 3, 4],
            media_type: "image/png".into(),
        }];
        let req = build_prompt_async_request(
            "hello".into(),
            Some("openai/gpt-5"),
            None,
            &images,
        )
        .unwrap();
        assert_eq!(req.parts.len(), 2);
        assert_eq!(req.variant, None);
        match &req.parts[0] {
            PartInput::Text { text } => assert_eq!(text, "hello"),
            other => panic!("wrong part: {other:?}"),
        }
        match &req.parts[1] {
            PartInput::File { mime, url, .. } => {
                assert_eq!(mime, "image/png");
                assert!(url.starts_with("data:image/png;base64,"));
            }
            other => panic!("wrong part: {other:?}"),
        }
    }

    #[test]
    fn build_prompt_async_request_carries_the_reasoning_variant() {
        // A picked reasoning level rides on `variant` (OpenCode's
        // per-prompt effort selector) and serializes into the body.
        let req =
            build_prompt_async_request("hi".into(), Some("anthropic/claude-sonnet-4-5"), Some("high"), &[])
                .unwrap();
        assert_eq!(req.variant.as_deref(), Some("high"));
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["variant"], "high");
    }

    #[test]
    fn build_prompt_async_request_normalizes_blank_variant_to_none() {
        // A cleared picker (empty/whitespace) must not send `variant: ""`.
        let req =
            build_prompt_async_request("hi".into(), Some("openai/gpt-5"), Some("   "), &[]).unwrap();
        assert_eq!(req.variant, None);
        let json = serde_json::to_value(&req).unwrap();
        assert!(json.get("variant").is_none(), "blank variant must be omitted");
    }

    #[test]
    fn build_prompt_async_request_rejects_invalid_model() {
        let err =
            build_prompt_async_request("hi".into(), Some("invalid"), None, &[]).unwrap_err();
        assert!(err.starts_with("invalid_model_id"));
    }

    #[test]
    fn approval_decision_maps_to_three_replies() {
        assert_eq!(
            approval_decision_to_permission_reply(&ApprovalDecision::Allow {
                updated_input: None,
                updated_permissions: None
            }),
            PermissionReply::Once
        );
        assert_eq!(
            approval_decision_to_permission_reply(&ApprovalDecision::AllowForSession),
            PermissionReply::Always
        );
        assert_eq!(
            approval_decision_to_permission_reply(&ApprovalDecision::Deny {
                message: "no".into()
            }),
            PermissionReply::Reject
        );
        assert_eq!(
            approval_decision_to_permission_reply(&ApprovalDecision::Cancel),
            PermissionReply::Reject
        );
    }

    #[test]
    fn translate_session_idle_emits_turn_completed_success() {
        let event = OpenCodeEvent::Known(KnownEvent::SessionIdle(super::super::protocol::SessionIdle {
            session_id: "s1".into(),
        }));
        let out = opencode_event_to_runtime(event, &ctx());
        assert_eq!(out.len(), 1);
        match &out[0] {
            ProviderRuntimeEvent::TurnCompleted { status, .. } => {
                assert!(matches!(status, TurnStatus::Success));
            }
            other => panic!("wrong event: {other:?}"),
        }
    }

    #[test]
    fn translate_part_delta_emits_content_delta_text() {
        let event = OpenCodeEvent::Known(KnownEvent::MessagePartDelta(super::super::protocol::MessagePartDelta {
            session_id: "s1".into(),
            message_id: "m1".into(),
            part_id: "p1".into(),
            field: "text".into(),
            delta: "hi".into(),
        }));
        let out = opencode_event_to_runtime(event, &ctx());
        assert_eq!(out.len(), 1);
        match &out[0] {
            ProviderRuntimeEvent::ContentDelta {
                delta: ContentDelta::Text { text },
                ..
            } => assert_eq!(text, "hi"),
            other => panic!("wrong event: {other:?}"),
        }
    }

    #[test]
    fn translate_part_delta_skips_non_text_field() {
        let event = OpenCodeEvent::Known(KnownEvent::MessagePartDelta(super::super::protocol::MessagePartDelta {
            session_id: "s1".into(),
            message_id: "m1".into(),
            part_id: "p1".into(),
            field: "title".into(),
            delta: "filename.rs".into(),
        }));
        let out = opencode_event_to_runtime(event, &ctx());
        assert!(out.is_empty());
    }

    #[test]
    fn translate_text_part_updated_emits_assistant_text() {
        let event = OpenCodeEvent::Known(KnownEvent::MessagePartUpdated(super::super::protocol::MessagePartUpdated {
            part: PartPayload::Text(super::super::protocol::TextPart {
                id: "p1".into(),
                session_id: "s1".into(),
                message_id: "m1".into(),
                text: "Hello, world.".into(),
                synthetic: None,
            }),
        }));
        let out = opencode_event_to_runtime(event, &ctx());
        assert_eq!(out.len(), 1);
        match &out[0] {
            ProviderRuntimeEvent::ItemCompleted {
                item: CompletedItem::AssistantText { text },
                ..
            } => assert_eq!(text, "Hello, world."),
            other => panic!("wrong event: {other:?}"),
        }
    }

    #[test]
    fn translate_synthetic_text_part_drops_event() {
        let event = OpenCodeEvent::Known(KnownEvent::MessagePartUpdated(super::super::protocol::MessagePartUpdated {
            part: PartPayload::Text(super::super::protocol::TextPart {
                id: "p1".into(),
                session_id: "s1".into(),
                message_id: "m1".into(),
                text: "synthetic".into(),
                synthetic: Some(true),
            }),
        }));
        assert!(opencode_event_to_runtime(event, &ctx()).is_empty());
    }

    #[test]
    fn translate_completed_tool_part_emits_use_and_result() {
        let event = OpenCodeEvent::Known(KnownEvent::MessagePartUpdated(super::super::protocol::MessagePartUpdated {
            part: PartPayload::Tool(super::super::protocol::ToolPart {
                id: "tp".into(),
                session_id: "s1".into(),
                message_id: "m1".into(),
                call_id: "call_x".into(),
                tool: "read".into(),
                state: ToolStateValue::Completed {
                    input: serde_json::json!({ "path": "src/main.rs" }),
                    output: "fn main() {}".into(),
                    title: None,
                },
            }),
        }));
        let out = opencode_event_to_runtime(event, &ctx());
        assert_eq!(out.len(), 2, "expected ToolUse + ToolResult");
        match &out[0] {
            ProviderRuntimeEvent::ItemCompleted {
                item:
                    CompletedItem::ToolUse {
                        tool_name,
                        tool_use_id,
                        ..
                    },
                ..
            } => {
                assert_eq!(tool_name, "read");
                assert_eq!(tool_use_id, "call_x");
            }
            other => panic!("wrong first event: {other:?}"),
        }
        match &out[1] {
            ProviderRuntimeEvent::ItemCompleted {
                item:
                    CompletedItem::ToolResult {
                        tool_use_id,
                        is_error,
                        ..
                    },
                ..
            } => {
                assert_eq!(tool_use_id, "call_x");
                assert!(!is_error);
            }
            other => panic!("wrong second event: {other:?}"),
        }
    }

    #[test]
    fn translate_error_tool_part_emits_use_and_error_result() {
        let event = OpenCodeEvent::Known(KnownEvent::MessagePartUpdated(super::super::protocol::MessagePartUpdated {
            part: PartPayload::Tool(super::super::protocol::ToolPart {
                id: "tp".into(),
                session_id: "s1".into(),
                message_id: "m1".into(),
                call_id: "call_x".into(),
                tool: "bash".into(),
                state: ToolStateValue::Error {
                    input: serde_json::json!({ "command": "exit 1" }),
                    error: "exit code 1".into(),
                },
            }),
        }));
        let out = opencode_event_to_runtime(event, &ctx());
        assert_eq!(out.len(), 2);
        match &out[1] {
            ProviderRuntimeEvent::ItemCompleted {
                item: CompletedItem::ToolResult { is_error, .. },
                ..
            } => assert!(*is_error),
            other => panic!("wrong: {other:?}"),
        }
    }

    #[test]
    fn translate_session_error_emits_turn_completed_with_error() {
        let event = OpenCodeEvent::Known(KnownEvent::SessionError(super::super::protocol::SessionErrorEvent {
            session_id: Some("s1".into()),
            error: Some(OpenCodeApiError {
                name: "ProviderAuthError".into(),
                data: serde_json::json!({ "providerID": "openai", "message": "no key" }),
            }),
        }));
        let out = opencode_event_to_runtime(event, &ctx());
        assert_eq!(out.len(), 1);
        match &out[0] {
            ProviderRuntimeEvent::TurnCompleted { status, .. } => match status {
                TurnStatus::Error { subtype, message } => {
                    assert_eq!(subtype, "ProviderAuthError");
                    assert_eq!(message, "no key");
                }
                other => panic!("wrong status: {other:?}"),
            },
            other => panic!("wrong event: {other:?}"),
        }
    }

    #[test]
    fn translate_permission_asked_emits_request_opened() {
        let event = OpenCodeEvent::Known(KnownEvent::PermissionAsked(super::super::protocol::PermissionAskedEvent {
            id: "perm_1".into(),
            session_id: "s1".into(),
            permission: "bash".into(),
            patterns: vec!["ls *".into()],
            metadata: serde_json::json!({}),
            tool: Some(super::super::protocol::PermissionToolRef {
                message_id: "m1".into(),
                call_id: "call_x".into(),
            }),
        }));
        let out = opencode_event_to_runtime(event, &ctx());
        assert_eq!(out.len(), 1);
        match &out[0] {
            ProviderRuntimeEvent::RequestOpened {
                request_id,
                request_kind,
                tool_use_id,
                ..
            } => {
                assert_eq!(request_id.0, "perm_1");
                assert_eq!(request_kind, "tool_permission");
                assert_eq!(tool_use_id.as_deref(), Some("call_x"));
            }
            other => panic!("wrong event: {other:?}"),
        }
    }

    #[test]
    fn translate_unknown_event_emits_runtime_warning() {
        let out = opencode_event_to_runtime(OpenCodeEvent::Other(serde_json::Value::Null), &ctx());
        assert_eq!(out.len(), 1);
        assert!(matches!(
            out[0],
            ProviderRuntimeEvent::RuntimeWarning { .. }
        ));
    }
}
