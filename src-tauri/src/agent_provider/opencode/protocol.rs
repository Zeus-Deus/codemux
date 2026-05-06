//! Wire-format types for the OpenCode HTTP server.
//!
//! These structs mirror `@opencode-ai/sdk@1.14.31` at
//! `dist/v2/gen/types.gen.d.ts`, trimmed to the subset Codemux needs
//! to drive a chat session end-to-end. Field names match the SDK's
//! camelCase names verbatim (`messageID`, `sessionID`, `providerID`,
//! `modelID`) so JSON round-trips without rename annotations.
//!
//! Decoder tolerance: every type uses `#[serde(default)]` on optional
//! fields and ignores unknown fields by default. A future OpenCode
//! release adding new keys does not break the chat path; the parts /
//! events Codemux does not understand are surfaced as
//! [`crate::agent_provider::ProviderRuntimeEvent::RuntimeWarning`]
//! by the [`super::translate`] layer, never silently dropped.

use serde::{Deserialize, Serialize};

// ── Request bodies (Codemux → OpenCode) ─────────────────────────────

/// Body of `POST /session`. Every field is optional — OpenCode picks
/// defaults for any that are omitted.
#[derive(Debug, Clone, Default, Serialize)]
pub struct SessionCreateRequest {
    /// Optional parent session id when forking. Codemux always passes
    /// `None` from chat; the field exists for completeness.
    #[serde(rename = "parentID", skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    /// Human-readable title. OpenCode auto-generates one when absent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
}

/// Reference to an OpenCode model — the federated `{providerID, modelID}`
/// pair the prompt body expects.
///
/// Codemux model ids look like `"openai/gpt-5"` or
/// `"anthropic/claude-sonnet-4-6"`; the [`super::translate`] layer
/// splits them into this shape before sending.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelRef {
    #[serde(rename = "providerID")]
    pub provider_id: String,
    #[serde(rename = "modelID")]
    pub model_id: String,
}

/// One element of the `parts` array on a prompt body.
///
/// Codemux today only emits text parts (the chat composer is plain
/// text + image attachments, and image attachments map to `file`
/// parts). The variants below mirror `TextPartInput` and
/// `FilePartInput` from the SDK.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum PartInput {
    Text {
        text: String,
    },
    File {
        mime: String,
        url: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        filename: Option<String>,
    },
}

/// Body of `POST /session/{sessionID}/prompt_async`.
///
/// `prompt_async` returns 204 immediately and emits the assistant
/// response as `message.part.delta` / `message.part.updated` events on
/// the global `/event` SSE stream. Codemux uses this rather than the
/// streaming `POST /session/{id}/message` endpoint because the
/// adapter's event stream is the SSE channel anyway — we'd be parsing
/// the same events twice.
#[derive(Debug, Clone, Serialize)]
pub struct PromptAsyncRequest {
    /// Optional client-supplied user-message id. OpenCode mints one
    /// when absent. Codemux passes `None` — we track turns by the
    /// minted assistant-message id observed via SSE.
    #[serde(rename = "messageID", skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    /// Federated model selector. Required for the chat use-case so
    /// the user's picker choice is honoured.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<ModelRef>,
    /// Optional agent name (e.g. `"build"`, `"plan"`). When `None`
    /// OpenCode picks its default agent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// Conversation parts. Always at least one text part for the
    /// user's message; image attachments append `File` parts.
    pub parts: Vec<PartInput>,
}

/// Body of `POST /session/{sessionID}/permissions/{permissionID}`.
///
/// The three reply tokens map onto Codemux's
/// [`crate::agent_provider::ApprovalDecision`] in
/// [`super::translate::approval_decision_to_permission_reply`].
#[derive(Debug, Clone, Serialize)]
pub struct PermissionRespondRequest {
    pub response: PermissionReply,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PermissionReply {
    Once,
    Always,
    Reject,
}

// ── Response shapes (OpenCode → Codemux) ────────────────────────────

/// Response from `POST /session`. Codemux only consumes `id`; other
/// fields are accepted to keep the decoder tolerant of upstream
/// additions.
#[derive(Debug, Clone, Deserialize)]
pub struct SessionResponse {
    pub id: String,
    #[serde(default)]
    pub title: Option<String>,
}

// ── Event payloads (OpenCode `/event` SSE → Codemux) ────────────────
//
// The SSE stream emits events as `{ "type": "...", "properties":
// {...} }`. Each variant below carries the `properties` shape for the
// types Codemux acts on; everything else falls through to
// `OpenCodeEvent::Other` and the translate layer surfaces it as a
// `RuntimeWarning` so adapter drift is observable.

/// Top-level decode for a single SSE event payload.
///
/// Two layers: [`KnownEvent`] enumerates every event variant Codemux
/// understands; the outer `untagged` `OpenCodeEvent` falls through to
/// `Other(serde_json::Value)` for anything new in a future SDK release
/// (or for events the chat path simply does not act on, like
/// `vcs.branch.updated`). The translate layer surfaces `Other` as a
/// `RuntimeWarning` so adapter drift stays observable.
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum OpenCodeEvent {
    Known(KnownEvent),
    Other(serde_json::Value),
}

/// Closed enumeration of the events Codemux acts on. Variant names
/// rendered as the SDK's dotted strings (`"message.part.delta"` etc.)
/// via `#[serde(tag = "type", content = "properties")]`.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", content = "properties")]
pub enum KnownEvent {
    #[serde(rename = "session.created")]
    SessionCreated(SessionEnvelope),
    #[serde(rename = "session.updated")]
    SessionUpdated(SessionEnvelope),
    #[serde(rename = "session.deleted")]
    SessionDeleted(SessionEnvelope),
    #[serde(rename = "session.idle")]
    SessionIdle(SessionIdle),
    #[serde(rename = "session.status")]
    SessionStatus(SessionStatusEvent),
    #[serde(rename = "session.error")]
    SessionError(SessionErrorEvent),
    #[serde(rename = "message.updated")]
    MessageUpdated(MessageEnvelope),
    #[serde(rename = "message.removed")]
    MessageRemoved(MessageRemoved),
    #[serde(rename = "message.part.updated")]
    MessagePartUpdated(MessagePartUpdated),
    #[serde(rename = "message.part.delta")]
    MessagePartDelta(MessagePartDelta),
    #[serde(rename = "message.part.removed")]
    MessagePartRemoved(MessagePartRemoved),
    #[serde(rename = "permission.asked")]
    PermissionAsked(PermissionAskedEvent),
    #[serde(rename = "permission.replied")]
    PermissionReplied(PermissionRepliedEvent),
}

/// Wraps a session payload — emitted by `session.created/updated/deleted`.
/// We only need the id from these.
#[derive(Debug, Clone, Deserialize)]
pub struct SessionEnvelope {
    pub info: SessionInfo,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SessionInfo {
    pub id: String,
    #[serde(default)]
    pub title: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SessionIdle {
    #[serde(rename = "sessionID")]
    pub session_id: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SessionStatusEvent {
    #[serde(rename = "sessionID")]
    pub session_id: String,
    pub status: SessionStatusValue,
}

/// Mirrors the SDK's `SessionStatus` discriminated union.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum SessionStatusValue {
    Idle,
    Busy,
    Retry {
        attempt: u32,
        message: String,
        next: u64,
    },
}

#[derive(Debug, Clone, Deserialize)]
pub struct SessionErrorEvent {
    #[serde(default, rename = "sessionID")]
    pub session_id: Option<String>,
    #[serde(default)]
    pub error: Option<OpenCodeApiError>,
}

/// Discriminated union of the error shapes OpenCode tags into
/// `session.error.properties.error` and `assistant.error`. Codemux
/// projects all of them to a single `(name, message)` pair via
/// [`OpenCodeApiError::display_pair`].
#[derive(Debug, Clone, Deserialize)]
pub struct OpenCodeApiError {
    pub name: String,
    #[serde(default)]
    pub data: serde_json::Value,
}

impl OpenCodeApiError {
    /// Pull a human-friendly `(subtype, message)` pair out of the
    /// upstream payload. The SDK's error variants all carry a
    /// `data.message` string except `MessageOutputLengthError`, which
    /// has no message at all.
    pub fn display_pair(&self) -> (String, String) {
        let message = self
            .data
            .get("message")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| match self.name.as_str() {
                "MessageOutputLengthError" => "model output length cap reached".into(),
                _ => "unknown error".into(),
            });
        (self.name.clone(), message)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct MessageEnvelope {
    pub info: MessageInfo,
}

/// Trimmed projection of `Message` — Codemux only needs the routing
/// fields (`sessionID`, `id`, `role`) plus the optional `error` to
/// detect mid-turn assistant failures.
#[derive(Debug, Clone, Deserialize)]
pub struct MessageInfo {
    pub id: String,
    #[serde(rename = "sessionID")]
    pub session_id: String,
    pub role: String,
    #[serde(default)]
    pub error: Option<OpenCodeApiError>,
    #[serde(default)]
    pub time: Option<MessageTime>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MessageTime {
    #[serde(default)]
    pub created: Option<u64>,
    #[serde(default)]
    pub completed: Option<u64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MessageRemoved {
    #[serde(rename = "sessionID")]
    pub session_id: String,
    #[serde(rename = "messageID")]
    pub message_id: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MessagePartUpdated {
    pub part: PartPayload,
}

/// One Part as it appears on `message.part.updated`. The SDK
/// distinguishes ~12 part types — Codemux acts on `text`, `reasoning`,
/// and `tool`; everything else falls through to `Other` and emits a
/// diagnostic warning.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum PartPayload {
    #[serde(rename = "text")]
    Text(TextPart),
    #[serde(rename = "reasoning")]
    Reasoning(ReasoningPart),
    #[serde(rename = "tool")]
    Tool(ToolPart),
    /// Catch-all for `step-start`, `step-finish`, `snapshot`, `patch`,
    /// `agent`, `retry`, `compaction`, `subtask`, `file` — none of
    /// which the chat UI renders.
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Deserialize)]
pub struct TextPart {
    pub id: String,
    #[serde(rename = "sessionID")]
    pub session_id: String,
    #[serde(rename = "messageID")]
    pub message_id: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub synthetic: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ReasoningPart {
    pub id: String,
    #[serde(rename = "sessionID")]
    pub session_id: String,
    #[serde(rename = "messageID")]
    pub message_id: String,
    #[serde(default)]
    pub text: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ToolPart {
    pub id: String,
    #[serde(rename = "sessionID")]
    pub session_id: String,
    #[serde(rename = "messageID")]
    pub message_id: String,
    #[serde(rename = "callID")]
    pub call_id: String,
    pub tool: String,
    pub state: ToolStateValue,
}

/// Mirrors `ToolState` from the SDK — we only act on `completed` and
/// `error` (the terminal states); `pending` and `running` are
/// in-flight markers Codemux ignores at item-level.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "status", rename_all = "lowercase")]
pub enum ToolStateValue {
    Pending {
        #[serde(default)]
        input: serde_json::Value,
    },
    Running {
        #[serde(default)]
        input: serde_json::Value,
        #[serde(default)]
        title: Option<String>,
    },
    Completed {
        #[serde(default)]
        input: serde_json::Value,
        #[serde(default)]
        output: String,
        #[serde(default)]
        title: Option<String>,
    },
    Error {
        #[serde(default)]
        input: serde_json::Value,
        #[serde(default)]
        error: String,
    },
}

#[derive(Debug, Clone, Deserialize)]
pub struct MessagePartDelta {
    #[serde(rename = "sessionID")]
    pub session_id: String,
    #[serde(rename = "messageID")]
    pub message_id: String,
    #[serde(rename = "partID")]
    pub part_id: String,
    /// Which field of the underlying part the delta applies to (`"text"`).
    pub field: String,
    pub delta: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MessagePartRemoved {
    #[serde(rename = "sessionID")]
    pub session_id: String,
    #[serde(rename = "messageID")]
    pub message_id: String,
    #[serde(rename = "partID")]
    pub part_id: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PermissionAskedEvent {
    pub id: String,
    #[serde(rename = "sessionID")]
    pub session_id: String,
    pub permission: String,
    #[serde(default)]
    pub patterns: Vec<String>,
    #[serde(default)]
    pub metadata: serde_json::Value,
    #[serde(default)]
    pub tool: Option<PermissionToolRef>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PermissionToolRef {
    #[serde(rename = "messageID")]
    pub message_id: String,
    #[serde(rename = "callID")]
    pub call_id: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PermissionRepliedEvent {
    #[serde(rename = "sessionID")]
    pub session_id: String,
    #[serde(rename = "requestID")]
    pub request_id: String,
    pub reply: PermissionReply,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_create_request_serialises_only_set_fields() {
        let req = SessionCreateRequest {
            parent_id: None,
            title: Some("hello".into()),
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json, serde_json::json!({ "title": "hello" }));
    }

    #[test]
    fn prompt_async_request_emits_camel_case_keys() {
        let req = PromptAsyncRequest {
            message_id: None,
            model: Some(ModelRef {
                provider_id: "openai".into(),
                model_id: "gpt-5".into(),
            }),
            agent: None,
            parts: vec![PartInput::Text {
                text: "hi".into(),
            }],
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "model": { "providerID": "openai", "modelID": "gpt-5" },
                "parts": [{ "type": "text", "text": "hi" }]
            })
        );
    }

    #[test]
    fn permission_respond_request_uses_lowercase_response() {
        let req = PermissionRespondRequest {
            response: PermissionReply::Always,
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json, serde_json::json!({ "response": "always" }));
    }

    #[test]
    fn part_input_file_omits_filename_when_none() {
        let part = PartInput::File {
            mime: "image/png".into(),
            url: "data:image/png;base64,AAA".into(),
            filename: None,
        };
        let json = serde_json::to_value(&part).unwrap();
        assert_eq!(
            json,
            serde_json::json!({
                "type": "file",
                "mime": "image/png",
                "url": "data:image/png;base64,AAA"
            })
        );
    }

    #[test]
    fn session_response_decodes_minimal_payload() {
        let raw = serde_json::json!({ "id": "sess_abc" });
        let resp: SessionResponse = serde_json::from_value(raw).unwrap();
        assert_eq!(resp.id, "sess_abc");
        assert_eq!(resp.title, None);
    }

    #[test]
    fn opencode_event_decodes_message_part_delta() {
        let raw = serde_json::json!({
            "type": "message.part.delta",
            "properties": {
                "sessionID": "s1",
                "messageID": "m1",
                "partID": "p1",
                "field": "text",
                "delta": "Hello "
            }
        });
        let event: OpenCodeEvent = serde_json::from_value(raw).unwrap();
        match event {
            OpenCodeEvent::Known(KnownEvent::MessagePartDelta(d)) => {
                assert_eq!(d.session_id, "s1");
                assert_eq!(d.message_id, "m1");
                assert_eq!(d.part_id, "p1");
                assert_eq!(d.field, "text");
                assert_eq!(d.delta, "Hello ");
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn opencode_event_decodes_session_idle() {
        let raw = serde_json::json!({
            "type": "session.idle",
            "properties": { "sessionID": "s1" }
        });
        let event: OpenCodeEvent = serde_json::from_value(raw).unwrap();
        match event {
            OpenCodeEvent::Known(KnownEvent::SessionIdle(SessionIdle { session_id })) => {
                assert_eq!(session_id, "s1");
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn opencode_event_decodes_session_error_with_inner_error() {
        let raw = serde_json::json!({
            "type": "session.error",
            "properties": {
                "sessionID": "s1",
                "error": {
                    "name": "ProviderAuthError",
                    "data": { "providerID": "openai", "message": "no key" }
                }
            }
        });
        let event: OpenCodeEvent = serde_json::from_value(raw).unwrap();
        match event {
            OpenCodeEvent::Known(KnownEvent::SessionError(payload)) => {
                assert_eq!(payload.session_id.as_deref(), Some("s1"));
                let err = payload.error.expect("error present");
                let (name, message) = err.display_pair();
                assert_eq!(name, "ProviderAuthError");
                assert_eq!(message, "no key");
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn opencode_event_decodes_message_part_updated_text() {
        let raw = serde_json::json!({
            "type": "message.part.updated",
            "properties": {
                "part": {
                    "id": "p1",
                    "sessionID": "s1",
                    "messageID": "m1",
                    "type": "text",
                    "text": "Hello, world."
                }
            }
        });
        let event: OpenCodeEvent = serde_json::from_value(raw).unwrap();
        match event {
            OpenCodeEvent::Known(KnownEvent::MessagePartUpdated(MessagePartUpdated {
                part: PartPayload::Text(t),
            })) => {
                assert_eq!(t.text, "Hello, world.");
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn opencode_event_decodes_tool_part_completed() {
        let raw = serde_json::json!({
            "type": "message.part.updated",
            "properties": {
                "part": {
                    "id": "tp1",
                    "sessionID": "s1",
                    "messageID": "m1",
                    "type": "tool",
                    "callID": "call_xyz",
                    "tool": "read",
                    "state": {
                        "status": "completed",
                        "input": { "path": "src/main.rs" },
                        "output": "fn main() {}",
                        "title": "src/main.rs"
                    }
                }
            }
        });
        let event: OpenCodeEvent = serde_json::from_value(raw).unwrap();
        match event {
            OpenCodeEvent::Known(KnownEvent::MessagePartUpdated(MessagePartUpdated {
                part: PartPayload::Tool(tool),
            })) => {
                assert_eq!(tool.tool, "read");
                assert_eq!(tool.call_id, "call_xyz");
                match tool.state {
                    ToolStateValue::Completed { output, .. } => {
                        assert_eq!(output, "fn main() {}");
                    }
                    other => panic!("wrong tool state: {other:?}"),
                }
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn opencode_event_falls_through_unknown_to_other() {
        let raw = serde_json::json!({
            "type": "vcs.branch.updated",
            "properties": { "branch": "main" }
        });
        let event: OpenCodeEvent = serde_json::from_value(raw.clone()).unwrap();
        match event {
            OpenCodeEvent::Other(payload) => {
                assert_eq!(payload, raw);
            }
            other => panic!("expected Other, got {other:?}"),
        }
    }

    #[test]
    fn permission_asked_event_decodes_with_optional_tool() {
        let raw = serde_json::json!({
            "type": "permission.asked",
            "properties": {
                "id": "perm_1",
                "sessionID": "s1",
                "permission": "bash",
                "patterns": ["ls *"],
                "metadata": {},
                "always": [],
                "tool": { "messageID": "m1", "callID": "call_y" }
            }
        });
        let event: OpenCodeEvent = serde_json::from_value(raw).unwrap();
        match event {
            OpenCodeEvent::Known(KnownEvent::PermissionAsked(p)) => {
                assert_eq!(p.id, "perm_1");
                assert_eq!(p.permission, "bash");
                assert_eq!(p.patterns, vec!["ls *"]);
                assert_eq!(p.tool.unwrap().call_id, "call_y");
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }
}
