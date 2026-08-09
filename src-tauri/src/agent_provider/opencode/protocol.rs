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
    /// Optional model variant — OpenCode's per-prompt reasoning-effort
    /// selector (`POST /session/{id}/prompt_async` accepts `variant`).
    /// The keys come from each model's `variants` map
    /// (`low`/`medium`/`high`/`max`/`xhigh`/…), surfaced to the picker
    /// as the model's `effort_levels`. `None` lets OpenCode use the
    /// model's default reasoning behaviour.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
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
    #[serde(rename = "todo.updated")]
    TodoUpdated(TodoUpdatedEvent),
}

/// OpenCode's complete session todo-list replacement event.
#[derive(Debug, Clone, Deserialize)]
pub struct TodoUpdatedEvent {
    #[serde(rename = "sessionID")]
    pub session_id: String,
    #[serde(default)]
    pub todos: Vec<OpenCodeTodo>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OpenCodeTodo {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub content: String,
    #[serde(default)]
    pub status: String,
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
    /// Parent session id when this session is a subagent's child
    /// session. Present on `session.created` for every subagent the
    /// runtime spawns (`properties.info.parentID`). The SSE listener
    /// grows its watched-session set when this points at a session it
    /// already tracks. `None` for the top-level (user-initiated)
    /// session.
    #[serde(default, rename = "parentID")]
    pub parent_id: Option<String>,
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

/// One element of `GET /session/{id}/message` — a stored message plus
/// its parts. Used by [`super::client::OpenCodeClient::get_session_messages`]
/// to cold-backfill a subagent transcript on mid-session attach (the
/// live SSE stream is the primary path). Reuses the same [`MessageInfo`]
/// / [`PartPayload`] decoders the streaming path uses.
#[derive(Debug, Clone, Deserialize)]
pub struct SessionMessage {
    pub info: MessageInfo,
    #[serde(default)]
    pub parts: Vec<PartPayload>,
}

/// Trimmed projection of `Message` — Codemux needs the routing
/// fields (`sessionID`, `id`, `role`), the optional `error` to detect
/// mid-turn assistant failures, and the assistant `tokens` block that
/// drives the context meter.
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
    /// Token accounting, present on assistant messages only. Absent on
    /// user messages and on early partial assistant envelopes.
    #[serde(default)]
    pub tokens: Option<MessageTokens>,
    /// The upstream provider that served this message (`"anthropic"`,
    /// `"openrouter"`, …). Assistant messages only.
    #[serde(default, rename = "providerID")]
    pub provider_id: Option<String>,
    /// The model that served this message. Assistant messages only.
    ///
    /// Decoded alongside `tokens` on purpose: the usage ledger needs the
    /// model that produced *these* tokens, and OpenCode lets the model
    /// change mid-thread and lets a subagent run a different one than
    /// its parent. Reading it off the same envelope as the token counts
    /// is the only source that stays correct through both.
    #[serde(default, rename = "modelID")]
    pub model_id: Option<String>,
}

impl MessageInfo {
    /// The `"providerID/modelID"` join Codemux uses as a model id
    /// everywhere else (see [`TaskModelRef::as_model_id`]).
    ///
    /// `None` unless both halves are present — a bare model id with no
    /// provider would collide across upstreams that serve the same
    /// model under the same name.
    pub fn qualified_model_id(&self) -> Option<String> {
        match (self.provider_id.as_deref(), self.model_id.as_deref()) {
            (Some(provider), Some(model)) if !provider.is_empty() && !model.is_empty() => {
                Some(format!("{provider}/{model}"))
            }
            _ => None,
        }
    }
}

/// The assistant `tokens` block: `{ input, output, reasoning, cache: {
/// read, write } }`.
///
/// Every field defaults to 0 so a partial envelope (OpenCode updates a
/// message incrementally, and the early updates carry a zeroed or
/// half-filled block) still decodes instead of failing the whole event.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct MessageTokens {
    #[serde(default)]
    pub input: u64,
    #[serde(default)]
    pub output: u64,
    #[serde(default)]
    pub reasoning: u64,
    #[serde(default)]
    pub cache: MessageCacheTokens,
}

/// The `cache` sub-block of [`MessageTokens`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct MessageCacheTokens {
    #[serde(default)]
    pub read: u64,
    #[serde(default)]
    pub write: u64,
}

impl MessageTokens {
    /// Total tokens this message put into the context window.
    ///
    /// Every bucket is summed. OpenCode reports the two cache tiers
    /// separately from `input` (they are the cached / newly-cached
    /// halves of the prompt, not a subset of it) and surfaces
    /// `reasoning` as its own line beside `output`. Cache reads count:
    /// they are prompt content the model sees on this request, so they
    /// occupy the window exactly like fresh input does.
    pub fn total(&self) -> u64 {
        [
            self.input,
            self.cache.read,
            self.cache.write,
            self.output,
            self.reasoning,
        ]
        .iter()
        .fold(0u64, |acc, v| acc.saturating_add(*v))
    }
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
    /// An `@agent` mention chip inside a message (the runtime records
    /// which subagent the turn addressed). Decoded so it does not fall
    /// to `Other`; the chat backend treats it as inert — no runtime
    /// event — while the frontend can render it as a chip.
    #[serde(rename = "agent")]
    Agent(AgentPart),
    /// A `subtask` marker part. The runtime always also materializes a
    /// `task` tool part (which drives the subagent card), so this part
    /// needs no special handling; decoding it just keeps it off the
    /// `Other` warning path.
    #[serde(rename = "subtask")]
    Subtask(SubtaskPart),
    /// Catch-all for `step-start`, `step-finish`, `snapshot`, `patch`,
    /// `retry`, `compaction`, `file` — none of which the chat UI
    /// renders.
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

/// Mirrors `ToolState` from the SDK. For ordinary tools Codemux acts on
/// `completed` and `error` (the terminal states) at item-level; the
/// `task` tool is special-cased in [`super::translate`] and reads
/// `metadata` / `time` off every non-`pending` state to drive the
/// subagent card (`pending` carries no metadata — verified live).
///
/// `metadata` is decoded as a raw [`serde_json::Value`] because its
/// shape is per-tool (`read` reports `{ preview, truncated, … }`, the
/// `task` tool reports `{ parentSessionId, sessionId, model }` — see
/// [`TaskMetadata`]). Keeping it untyped here means one weird-shaped
/// tool cannot break the decode of the whole event.
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
        #[serde(default)]
        metadata: serde_json::Value,
        #[serde(default)]
        time: Option<ToolTime>,
    },
    Completed {
        #[serde(default)]
        input: serde_json::Value,
        #[serde(default)]
        output: String,
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        metadata: serde_json::Value,
        #[serde(default)]
        time: Option<ToolTime>,
    },
    Error {
        #[serde(default)]
        input: serde_json::Value,
        #[serde(default)]
        error: String,
        #[serde(default)]
        metadata: serde_json::Value,
        #[serde(default)]
        time: Option<ToolTime>,
    },
}

/// Wall-clock window a tool state carries (`state.time.{start,end}`,
/// epoch milliseconds). The `task` tool populates both on completion so
/// the subagent card can show an elapsed duration.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ToolTime {
    #[serde(default)]
    pub start: Option<u64>,
    #[serde(default)]
    pub end: Option<u64>,
}

/// Typed projection of the `task` tool's `state.metadata`.
///
/// The keys are camelCase with a **lowercase `d`** (`sessionId`, not
/// `sessionID`) — a deliberate quirk of the task runtime that differs
/// from the `sessionID` casing used everywhere else on the wire, so the
/// rename annotations here are load-bearing. `sessionId` is the child
/// (subagent) session id and doubles as the subagent demux key.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct TaskMetadata {
    #[serde(default, rename = "parentSessionId")]
    pub parent_session_id: Option<String>,
    #[serde(default, rename = "sessionId")]
    pub session_id: Option<String>,
    #[serde(default)]
    pub model: Option<TaskModelRef>,
}

impl TaskMetadata {
    /// Parse the raw `state.metadata` value into the typed task shape.
    /// Non-object / non-task metadata (e.g. the `read` tool's
    /// `{ preview, … }`) decodes to an all-`None` [`TaskMetadata`]
    /// because every field defaults — the caller treats that as "no
    /// subagent identity yet".
    pub fn from_value(value: &serde_json::Value) -> Self {
        serde_json::from_value(value.clone()).unwrap_or_default()
    }

    /// Render the `{ providerID, modelID }` pair as a Codemux model id
    /// (`providerID/modelID`, matching [`super::translate::split_model_id`]).
    pub fn model_id(&self) -> Option<String> {
        self.model.as_ref().map(|m| m.as_model_id())
    }
}

/// The `{ providerID, modelID }` object nested inside task metadata.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct TaskModelRef {
    #[serde(default, rename = "providerID")]
    pub provider_id: String,
    #[serde(default, rename = "modelID")]
    pub model_id: String,
}

impl TaskModelRef {
    /// `providerID/modelID` — the id form the rest of Codemux uses.
    pub fn as_model_id(&self) -> String {
        format!("{}/{}", self.provider_id, self.model_id)
    }
}

/// An `@agent` mention part (`type: "agent"`). Verified live —
/// `{ type, name, messageID, sessionID, id }` with an optional
/// `source` span the mention was typed at.
#[derive(Debug, Clone, Deserialize)]
pub struct AgentPart {
    pub id: String,
    #[serde(rename = "sessionID")]
    pub session_id: String,
    #[serde(rename = "messageID")]
    pub message_id: String,
    /// The mentioned agent slug, e.g. `"explore"`.
    #[serde(default)]
    pub name: Option<String>,
}

/// A `subtask` marker part (`type: "subtask"`). Mirrors the SDK's
/// `SubtaskPart` — carries the delegated prompt/description plus the
/// target agent + model. Decoded for completeness; the paired `task`
/// tool part is what actually drives the subagent card.
#[derive(Debug, Clone, Deserialize)]
pub struct SubtaskPart {
    pub id: String,
    #[serde(rename = "sessionID")]
    pub session_id: String,
    #[serde(rename = "messageID")]
    pub message_id: String,
    #[serde(default)]
    pub prompt: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub model: Option<TaskModelRef>,
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
            variant: None,
            parts: vec![PartInput::Text {
                text: "hi".into(),
            }],
        };
        let json = serde_json::to_value(&req).unwrap();
        // `variant: None` is omitted from the body.
        assert_eq!(
            json,
            serde_json::json!({
                "model": { "providerID": "openai", "modelID": "gpt-5" },
                "parts": [{ "type": "text", "text": "hi" }]
            })
        );
    }

    #[test]
    fn prompt_async_request_serializes_variant_when_set() {
        let req = PromptAsyncRequest {
            message_id: None,
            model: None,
            agent: None,
            variant: Some("high".into()),
            parts: vec![PartInput::Text { text: "hi".into() }],
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["variant"], "high");
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
    fn session_info_decodes_parent_id_for_child_session() {
        // Real `session.created` child payload from the live capture —
        // the child carries `parentID` pointing at the root session.
        let raw = serde_json::json!({
            "type": "session.created",
            "properties": {
                "sessionID": "ses_child",
                "info": {
                    "id": "ses_child",
                    "parentID": "ses_root",
                    "title": "List current directory files (@explore subagent)"
                }
            }
        });
        let event: OpenCodeEvent = serde_json::from_value(raw).unwrap();
        match event {
            OpenCodeEvent::Known(KnownEvent::SessionCreated(env)) => {
                assert_eq!(env.info.id, "ses_child");
                assert_eq!(env.info.parent_id.as_deref(), Some("ses_root"));
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn session_info_parent_id_absent_on_root_session() {
        let raw = serde_json::json!({
            "type": "session.created",
            "properties": { "info": { "id": "ses_root", "title": "root" } }
        });
        let event: OpenCodeEvent = serde_json::from_value(raw).unwrap();
        match event {
            OpenCodeEvent::Known(KnownEvent::SessionCreated(env)) => {
                assert!(env.info.parent_id.is_none());
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn task_tool_running_decodes_metadata_and_input() {
        // The exact `running` state.metadata shape from the live SSE
        // capture — note the camelCase-with-lowercase-`d` keys
        // (`sessionId`, `parentSessionId`) that differ from the
        // `sessionID` casing used elsewhere.
        let raw = serde_json::json!({
            "type": "message.part.updated",
            "properties": {
                "part": {
                    "id": "prt_1",
                    "sessionID": "ses_root",
                    "messageID": "msg_1",
                    "type": "tool",
                    "callID": "call_task",
                    "tool": "task",
                    "state": {
                        "title": "List current directory files",
                        "metadata": {
                            "parentSessionId": "ses_root",
                            "sessionId": "ses_child",
                            "model": { "modelID": "big-pickle", "providerID": "opencode" }
                        },
                        "status": "running",
                        "input": {
                            "description": "List current directory files",
                            "prompt": "List all files in the current directory (/).",
                            "subagent_type": "explore"
                        },
                        "time": { "start": 1783362470235u64 }
                    }
                }
            }
        });
        let event: OpenCodeEvent = serde_json::from_value(raw).unwrap();
        let OpenCodeEvent::Known(KnownEvent::MessagePartUpdated(MessagePartUpdated {
            part: PartPayload::Tool(tool),
        })) = event
        else {
            panic!("expected tool part");
        };
        assert_eq!(tool.tool, "task");
        match tool.state {
            ToolStateValue::Running {
                title,
                metadata,
                time,
                input,
            } => {
                assert_eq!(title.as_deref(), Some("List current directory files"));
                let md = TaskMetadata::from_value(&metadata);
                assert_eq!(md.session_id.as_deref(), Some("ses_child"));
                assert_eq!(md.parent_session_id.as_deref(), Some("ses_root"));
                assert_eq!(md.model_id().as_deref(), Some("opencode/big-pickle"));
                assert_eq!(time.unwrap().start, Some(1783362470235));
                assert_eq!(input["subagent_type"], "explore");
            }
            other => panic!("wrong state: {other:?}"),
        }
    }

    #[test]
    fn task_tool_completed_decodes_envelope_output_and_time() {
        let raw = serde_json::json!({
            "type": "message.part.updated",
            "properties": {
                "part": {
                    "id": "prt_1",
                    "sessionID": "ses_root",
                    "messageID": "msg_1",
                    "type": "tool",
                    "callID": "call_task",
                    "tool": "task",
                    "state": {
                        "status": "completed",
                        "input": { "subagent_type": "explore" },
                        "output": "<task id=\"ses_child\" state=\"completed\">\n<task_result>\nDone.\n</task_result>\n</task>",
                        "metadata": {
                            "parentSessionId": "ses_root",
                            "sessionId": "ses_child",
                            "model": { "modelID": "big-pickle", "providerID": "opencode" },
                            "truncated": false
                        },
                        "title": "List current directory files",
                        "time": { "start": 1783362470235u64, "end": 1783362691634u64 }
                    }
                }
            }
        });
        let event: OpenCodeEvent = serde_json::from_value(raw).unwrap();
        let OpenCodeEvent::Known(KnownEvent::MessagePartUpdated(MessagePartUpdated {
            part: PartPayload::Tool(tool),
        })) = event
        else {
            panic!("expected tool part");
        };
        match tool.state {
            ToolStateValue::Completed {
                output,
                metadata,
                time,
                ..
            } => {
                assert!(output.contains("<task_result>"));
                let md = TaskMetadata::from_value(&metadata);
                assert_eq!(md.session_id.as_deref(), Some("ses_child"));
                let t = time.unwrap();
                assert_eq!(t.start, Some(1783362470235));
                assert_eq!(t.end, Some(1783362691634));
            }
            other => panic!("wrong state: {other:?}"),
        }
    }

    #[test]
    fn task_metadata_from_non_task_tool_metadata_is_all_none() {
        // The `read` tool's metadata (`{ preview, truncated, loaded }`)
        // must decode to an empty TaskMetadata rather than erroring.
        let md = TaskMetadata::from_value(&serde_json::json!({
            "preview": "a\nb\nc",
            "truncated": false,
            "loaded": []
        }));
        assert!(md.session_id.is_none());
        assert!(md.parent_session_id.is_none());
        assert!(md.model_id().is_none());
    }

    #[test]
    fn agent_part_decodes_as_agent_variant() {
        // Real `agent` part payload from the capture — must decode to
        // the Agent variant, not fall through to Other.
        let raw = serde_json::json!({
            "type": "message.part.updated",
            "properties": {
                "part": {
                    "type": "agent",
                    "name": "explore",
                    "messageID": "msg_1",
                    "sessionID": "ses_root",
                    "id": "prt_agent"
                }
            }
        });
        let event: OpenCodeEvent = serde_json::from_value(raw).unwrap();
        match event {
            OpenCodeEvent::Known(KnownEvent::MessagePartUpdated(MessagePartUpdated {
                part: PartPayload::Agent(agent),
            })) => {
                assert_eq!(agent.name.as_deref(), Some("explore"));
                assert_eq!(agent.session_id, "ses_root");
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn subtask_part_decodes_as_subtask_variant() {
        let raw = serde_json::json!({
            "type": "message.part.updated",
            "properties": {
                "part": {
                    "type": "subtask",
                    "id": "prt_sub",
                    "sessionID": "ses_root",
                    "messageID": "msg_1",
                    "prompt": "do the thing",
                    "description": "a thing",
                    "agent": "explore",
                    "model": { "providerID": "opencode", "modelID": "big-pickle" }
                }
            }
        });
        let event: OpenCodeEvent = serde_json::from_value(raw).unwrap();
        match event {
            OpenCodeEvent::Known(KnownEvent::MessagePartUpdated(MessagePartUpdated {
                part: PartPayload::Subtask(sub),
            })) => {
                assert_eq!(sub.agent.as_deref(), Some("explore"));
                assert_eq!(sub.model.unwrap().as_model_id(), "opencode/big-pickle");
            }
            other => panic!("wrong variant: {other:?}"),
        }
    }

    #[test]
    fn session_message_decodes_info_and_parts_for_backfill() {
        // Shape returned by `GET /session/{id}/message` — a list of
        // `{ info, parts }`; used for cold backfill.
        let raw = serde_json::json!([
            {
                "info": {
                    "id": "msg_1",
                    "sessionID": "ses_child",
                    "role": "assistant",
                    "time": { "created": 1, "completed": 2 }
                },
                "parts": [
                    { "type": "text", "id": "p1", "sessionID": "ses_child", "messageID": "msg_1", "text": "hi" },
                    { "type": "step-start", "id": "p2" }
                ]
            }
        ]);
        let msgs: Vec<SessionMessage> = serde_json::from_value(raw).unwrap();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].info.session_id, "ses_child");
        assert_eq!(msgs[0].parts.len(), 2);
        assert!(matches!(msgs[0].parts[0], PartPayload::Text(_)));
        assert!(matches!(msgs[0].parts[1], PartPayload::Other));
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
