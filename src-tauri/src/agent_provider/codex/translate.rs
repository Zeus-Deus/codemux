//! Pure translation layer from Codex protocol messages to the canonical
//! [`ProviderRuntimeEvent`] enum.
//!
//! Keeping this logic in one place (and free of I/O) means the whole
//! protocol-to-event mapping is unit-testable without spawning a
//! subprocess. Every function here is a pure transform — callers are
//! responsible for broadcasting the returned events.
//!
//! # Mapping policy
//!
//! * Streaming text (`agentMessage`, `commandExecution.outputDelta`,
//!   `fileChange.outputDelta`, `plan.delta`) → [`ContentDelta::Text`].
//! * Reasoning streams (`reasoning.textDelta` / `reasoning.summaryTextDelta`)
//!   → [`ContentDelta::Thinking`].
//! * MCP tool progress (`mcpToolCall.progress`) → [`ContentDelta::ToolInput`]
//!   so the UI's tool-call card updates with status text.
//! * `item/started` for `commandExecution` / `fileChange` / `mcpToolCall` /
//!   `dynamicToolCall` → [`CompletedItem::ToolUse`] (the tool has been
//!   called; result lands later via `item/completed`).
//! * `item/completed` for the same set → [`CompletedItem::ToolResult`].
//! * `item/completed` for `agentMessage` / `reasoning` / `plan` → emit the
//!   final-form [`CompletedItem`] variant so the UI replaces the streaming
//!   placeholder.
//! * Anything else surfaces as [`ProviderRuntimeEvent::RuntimeWarning`]
//!   so adapter drift is observable instead of silent.

use crate::agent_provider::{
    CompletedItem, ContentDelta, ProviderRuntimeEvent, RequestId, SessionStatus, ThreadId, TurnId,
    TurnStatus,
};

use super::protocol::{
    DeltaParams, ErrorParams, FileChangePatchUpdatedParams, ItemEnvelope,
    McpToolCallProgressParams, NotificationMessage, ReasoningSummaryPartAddedParams,
    ReasoningSummaryTextDeltaParams, ReasoningTextDeltaParams, ServerRequestMessage,
    TerminalInteractionParams, ThreadStartedParams, TurnCompletedParams, TurnStartedParams,
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
        NotificationMessage::AgentMessageDelta(p) => text_delta(thread_id, p, ContentStream::Text),
        NotificationMessage::CommandExecutionOutputDelta(p) => {
            text_delta(thread_id, p, ContentStream::CommandOutput)
        }
        NotificationMessage::FileChangeOutputDelta(p) => {
            text_delta(thread_id, p, ContentStream::FileChange)
        }
        NotificationMessage::PlanDelta(p) => text_delta(thread_id, p, ContentStream::Plan),
        NotificationMessage::ReasoningTextDelta(p) => translate_reasoning_text(thread_id, p),
        NotificationMessage::ReasoningSummaryTextDelta(p) => {
            translate_reasoning_summary_text(thread_id, p)
        }
        NotificationMessage::ReasoningSummaryPartAdded(p) => {
            // Pure structural marker — no text. Surface as a warning so
            // a future UI consumer that wants to render summary segments
            // can opt in by parsing the warning payload, but today we
            // don't need a per-segment event in the runtime stream.
            translate_reasoning_summary_marker(thread_id, p)
        }
        NotificationMessage::TerminalInteraction(p) => {
            // Stdin echo for an interactive command. Surface as a tool
            // input delta on the owning command-execution item so the UI
            // can show the user-typed text inline.
            translate_terminal_interaction(thread_id, p)
        }
        NotificationMessage::FileChangePatchUpdated(p) => {
            translate_patch_updated(thread_id, p)
        }
        NotificationMessage::McpToolCallProgress(p) => translate_mcp_progress(thread_id, p),
        NotificationMessage::ItemStarted(env) => translate_item_started(thread_id, env),
        NotificationMessage::ItemCompleted(env) => translate_item_completed(thread_id, env),
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

/// Which canonical text stream a delta belongs to. The chat UI doesn't
/// distinguish today, but the kind is forwarded as the `tool_name` on
/// non-text streams so the UI can render command-output / file-change
/// chunks in dedicated cards in a follow-up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContentStream {
    Text,
    CommandOutput,
    FileChange,
    Plan,
}

fn text_delta(
    thread_id: &ThreadId,
    p: DeltaParams,
    kind: ContentStream,
) -> Vec<ProviderRuntimeEvent> {
    if p.delta.is_empty() {
        return vec![];
    }
    match kind {
        ContentStream::Text => vec![ProviderRuntimeEvent::ContentDelta {
            thread_id: thread_id.clone(),
            turn_id: TurnId(p.turn_id),
            delta: ContentDelta::Text { text: p.delta },
        }],
        // Command output and file change deltas are routed to the
        // tool-input stream keyed on the item id — the UI threads them
        // onto the matching tool-call card emitted by the prior
        // `item/started` event.
        ContentStream::CommandOutput => vec![ProviderRuntimeEvent::ContentDelta {
            thread_id: thread_id.clone(),
            turn_id: TurnId(p.turn_id),
            delta: ContentDelta::ToolInput {
                tool_name: format!("commandExecution::{}", p.item_id),
                partial_json: p.delta,
            },
        }],
        ContentStream::FileChange => vec![ProviderRuntimeEvent::ContentDelta {
            thread_id: thread_id.clone(),
            turn_id: TurnId(p.turn_id),
            delta: ContentDelta::ToolInput {
                tool_name: format!("fileChange::{}", p.item_id),
                partial_json: p.delta,
            },
        }],
        ContentStream::Plan => vec![ProviderRuntimeEvent::ContentDelta {
            thread_id: thread_id.clone(),
            turn_id: TurnId(p.turn_id),
            delta: ContentDelta::ToolInput {
                tool_name: format!("plan::{}", p.item_id),
                partial_json: p.delta,
            },
        }],
    }
}

fn translate_reasoning_text(
    thread_id: &ThreadId,
    p: ReasoningTextDeltaParams,
) -> Vec<ProviderRuntimeEvent> {
    if p.delta.is_empty() {
        return vec![];
    }
    vec![ProviderRuntimeEvent::ContentDelta {
        thread_id: thread_id.clone(),
        turn_id: TurnId(p.turn_id),
        delta: ContentDelta::Thinking { text: p.delta },
    }]
}

fn translate_reasoning_summary_text(
    thread_id: &ThreadId,
    p: ReasoningSummaryTextDeltaParams,
) -> Vec<ProviderRuntimeEvent> {
    if p.delta.is_empty() {
        return vec![];
    }
    vec![ProviderRuntimeEvent::ContentDelta {
        thread_id: thread_id.clone(),
        turn_id: TurnId(p.turn_id),
        delta: ContentDelta::Thinking { text: p.delta },
    }]
}

fn translate_reasoning_summary_marker(
    thread_id: &ThreadId,
    p: ReasoningSummaryPartAddedParams,
) -> Vec<ProviderRuntimeEvent> {
    // Surface the marker as a warning so it stays visible for diagnostics
    // without polluting the canonical content stream — the chat UI
    // re-renders the reasoning block as a single accordion regardless of
    // segment boundaries.
    vec![ProviderRuntimeEvent::RuntimeWarning {
        thread_id: Some(thread_id.clone()),
        message: format!(
            "codex reasoning summary part added (item {} index {})",
            p.item_id, p.summary_index
        ),
        original_payload: None,
    }]
}

fn translate_terminal_interaction(
    thread_id: &ThreadId,
    p: TerminalInteractionParams,
) -> Vec<ProviderRuntimeEvent> {
    if p.stdin.is_empty() {
        return vec![];
    }
    vec![ProviderRuntimeEvent::ContentDelta {
        thread_id: thread_id.clone(),
        turn_id: TurnId(p.turn_id),
        delta: ContentDelta::ToolInput {
            tool_name: format!("commandExecution::{}", p.item_id),
            partial_json: p.stdin,
        },
    }]
}

fn translate_patch_updated(
    thread_id: &ThreadId,
    p: FileChangePatchUpdatedParams,
) -> Vec<ProviderRuntimeEvent> {
    // Forward the patch payload onto the file-change tool stream so the
    // UI can render an updated diff snapshot.
    vec![ProviderRuntimeEvent::ContentDelta {
        thread_id: thread_id.clone(),
        turn_id: TurnId(p.turn_id),
        delta: ContentDelta::ToolInput {
            tool_name: format!("fileChange::{}", p.item_id),
            partial_json: p.changes.to_string(),
        },
    }]
}

fn translate_mcp_progress(
    thread_id: &ThreadId,
    p: McpToolCallProgressParams,
) -> Vec<ProviderRuntimeEvent> {
    if p.message.is_empty() {
        return vec![];
    }
    vec![ProviderRuntimeEvent::ContentDelta {
        thread_id: thread_id.clone(),
        turn_id: TurnId(p.turn_id),
        delta: ContentDelta::ToolInput {
            tool_name: format!("mcpToolCall::{}", p.item_id),
            partial_json: p.message,
        },
    }]
}

/// Translate `item/started`. Tool-flavoured items emit a `ToolUse` so the
/// UI can render the card immediately; text/reasoning/plan items emit
/// nothing (the corresponding delta stream carries the actual content).
fn translate_item_started(
    thread_id: &ThreadId,
    env: ItemEnvelope,
) -> Vec<ProviderRuntimeEvent> {
    let turn_id = TurnId(env.turn_id);
    let item_type = env
        .item
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let item_id = env
        .item
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    match item_type.as_str() {
        "commandExecution" => vec![ProviderRuntimeEvent::ItemCompleted {
            thread_id: thread_id.clone(),
            turn_id,
            item: CompletedItem::ToolUse {
                tool_name: "commandExecution".into(),
                input: env.item.clone(),
                tool_use_id: item_id,
            },
        }],
        "fileChange" => vec![ProviderRuntimeEvent::ItemCompleted {
            thread_id: thread_id.clone(),
            turn_id,
            item: CompletedItem::ToolUse {
                tool_name: "fileChange".into(),
                input: env.item.clone(),
                tool_use_id: item_id,
            },
        }],
        "mcpToolCall" => vec![ProviderRuntimeEvent::ItemCompleted {
            thread_id: thread_id.clone(),
            turn_id,
            item: CompletedItem::ToolUse {
                tool_name: "mcpToolCall".into(),
                input: env.item.clone(),
                tool_use_id: item_id,
            },
        }],
        "dynamicToolCall" => vec![ProviderRuntimeEvent::ItemCompleted {
            thread_id: thread_id.clone(),
            turn_id,
            item: CompletedItem::ToolUse {
                tool_name: "dynamicToolCall".into(),
                input: env.item.clone(),
                tool_use_id: item_id,
            },
        }],
        // Text / reasoning / plan items have no started-time payload —
        // their content streams in via the matching delta event. Skip
        // emitting a duplicate event so the UI doesn't show an empty
        // placeholder.
        "agentMessage" | "reasoning" | "plan" | "userMessage" | "hookPrompt" => vec![],
        // Other variants (webSearch, imageGeneration, etc.) — surface as
        // a warning so we know the SDK shipped something new.
        other => vec![ProviderRuntimeEvent::RuntimeWarning {
            thread_id: Some(thread_id.clone()),
            message: format!("codex item/started type not specifically handled: {other}"),
            original_payload: Some(env.item),
        }],
    }
}

/// Translate `item/completed`. For text-bearing items emit the final
/// [`CompletedItem`]; for tool items emit a [`CompletedItem::ToolResult`]
/// carrying the final output payload.
fn translate_item_completed(
    thread_id: &ThreadId,
    env: ItemEnvelope,
) -> Vec<ProviderRuntimeEvent> {
    let turn_id = TurnId(env.turn_id);
    let item_type = env
        .item
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let item_id = env
        .item
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    match item_type.as_str() {
        "agentMessage" => {
            let text = env
                .item
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if text.is_empty() {
                return vec![];
            }
            vec![ProviderRuntimeEvent::ItemCompleted {
                thread_id: thread_id.clone(),
                turn_id,
                item: CompletedItem::AssistantText { text },
            }]
        }
        "reasoning" => {
            // Reasoning items carry both `content` and `summary` arrays
            // of strings. Concatenate into a single thinking block —
            // the chat UI renders this as the model's reasoning trace.
            let mut buf = String::new();
            if let Some(arr) = env.item.get("content").and_then(|v| v.as_array()) {
                for s in arr.iter().filter_map(|v| v.as_str()) {
                    if !buf.is_empty() {
                        buf.push('\n');
                    }
                    buf.push_str(s);
                }
            }
            if let Some(arr) = env.item.get("summary").and_then(|v| v.as_array()) {
                for s in arr.iter().filter_map(|v| v.as_str()) {
                    if !buf.is_empty() {
                        buf.push('\n');
                    }
                    buf.push_str(s);
                }
            }
            if buf.is_empty() {
                return vec![];
            }
            vec![ProviderRuntimeEvent::ItemCompleted {
                thread_id: thread_id.clone(),
                turn_id,
                item: CompletedItem::AssistantThinking { text: buf },
            }]
        }
        "plan" => {
            let text = env
                .item
                .get("text")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if text.is_empty() {
                return vec![];
            }
            vec![ProviderRuntimeEvent::ItemCompleted {
                thread_id: thread_id.clone(),
                turn_id,
                item: CompletedItem::AssistantText { text },
            }]
        }
        // Tool-style items emit a result with the final payload.
        "commandExecution" => {
            let aggregated = env
                .item
                .get("aggregatedOutput")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let exit_code = env.item.get("exitCode").and_then(|v| v.as_i64());
            let is_error = exit_code.map(|c| c != 0).unwrap_or(false);
            vec![ProviderRuntimeEvent::ItemCompleted {
                thread_id: thread_id.clone(),
                turn_id,
                item: CompletedItem::ToolResult {
                    tool_use_id: item_id,
                    content: serde_json::Value::String(aggregated.to_string()),
                    is_error,
                },
            }]
        }
        "fileChange" | "mcpToolCall" | "dynamicToolCall" | "webSearch" => {
            vec![ProviderRuntimeEvent::ItemCompleted {
                thread_id: thread_id.clone(),
                turn_id,
                item: CompletedItem::ToolResult {
                    tool_use_id: item_id,
                    content: env.item,
                    is_error: false,
                },
            }]
        }
        // Other variants — userMessage, hookPrompt, etc. — are not
        // currently rendered by the chat UI. Surface as a warning to
        // make future SDK additions visible.
        other => vec![ProviderRuntimeEvent::RuntimeWarning {
            thread_id: Some(thread_id.clone()),
            message: format!("codex item/completed type not specifically handled: {other}"),
            original_payload: Some(env.item),
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

    fn delta_payload(method_field: &str, delta: &str) -> serde_json::Value {
        let _ = method_field; // kept for readability
        json!({
            "delta": delta,
            "itemId": "i-1",
            "threadId": "c1",
            "turnId": "t1",
        })
    }

    #[test]
    fn agent_message_delta_uses_real_sdk_field_name() {
        // Pin: SDK wire field is `delta`, NOT `textDelta`. Earlier
        // versions of this adapter used `textDelta` and silently
        // dropped every assistant text chunk.
        let msg = NotificationMessage::from_raw(
            "item/agentMessage/delta",
            delta_payload("agentMessage", "hello"),
        );
        let events = translate_notification(&tid(), msg);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ProviderRuntimeEvent::ContentDelta { delta, .. } => match delta {
                ContentDelta::Text { text } => assert_eq!(text, "hello"),
                other => panic!("wrong delta: {other:?}"),
            },
            other => panic!("wrong event: {other:?}"),
        }
    }

    #[test]
    fn reasoning_text_delta_becomes_thinking() {
        let msg = NotificationMessage::from_raw(
            "item/reasoning/textDelta",
            json!({
                "delta": "Let me think...",
                "contentIndex": 0,
                "itemId": "i-2",
                "threadId": "c1",
                "turnId": "t1",
            }),
        );
        let events = translate_notification(&tid(), msg);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ProviderRuntimeEvent::ContentDelta { delta, .. } => match delta {
                ContentDelta::Thinking { text } => assert_eq!(text, "Let me think..."),
                other => panic!("wrong delta: {other:?}"),
            },
            other => panic!("wrong event: {other:?}"),
        }
    }

    #[test]
    fn reasoning_summary_text_delta_becomes_thinking() {
        let msg = NotificationMessage::from_raw(
            "item/reasoning/summaryTextDelta",
            json!({
                "delta": "Summary chunk",
                "summaryIndex": 0,
                "itemId": "i-3",
                "threadId": "c1",
                "turnId": "t1",
            }),
        );
        let events = translate_notification(&tid(), msg);
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            ProviderRuntimeEvent::ContentDelta {
                delta: ContentDelta::Thinking { .. },
                ..
            }
        ));
    }

    #[test]
    fn reasoning_summary_part_added_emits_diagnostic_warning() {
        let msg = NotificationMessage::from_raw(
            "item/reasoning/summaryPartAdded",
            json!({
                "summaryIndex": 1,
                "itemId": "i-3",
                "threadId": "c1",
                "turnId": "t1",
            }),
        );
        let events = translate_notification(&tid(), msg);
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            ProviderRuntimeEvent::RuntimeWarning { .. }
        ));
    }

    #[test]
    fn command_execution_output_delta_routed_to_tool_input_stream() {
        let msg = NotificationMessage::from_raw(
            "item/commandExecution/outputDelta",
            json!({
                "delta": "fn main() {",
                "itemId": "cmd-1",
                "threadId": "c1",
                "turnId": "t1",
            }),
        );
        let events = translate_notification(&tid(), msg);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ProviderRuntimeEvent::ContentDelta { delta, .. } => match delta {
                ContentDelta::ToolInput {
                    tool_name,
                    partial_json,
                } => {
                    assert!(tool_name.starts_with("commandExecution::"));
                    assert!(tool_name.ends_with("cmd-1"));
                    assert_eq!(partial_json, "fn main() {");
                }
                other => panic!("wrong delta: {other:?}"),
            },
            other => panic!("wrong event: {other:?}"),
        }
    }

    #[test]
    fn file_change_output_delta_routed_to_tool_input_stream() {
        let msg = NotificationMessage::from_raw(
            "item/fileChange/outputDelta",
            json!({
                "delta": "+ added line",
                "itemId": "fc-1",
                "threadId": "c1",
                "turnId": "t1",
            }),
        );
        let events = translate_notification(&tid(), msg);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ProviderRuntimeEvent::ContentDelta { delta, .. } => match delta {
                ContentDelta::ToolInput { tool_name, .. } => {
                    assert!(tool_name.starts_with("fileChange::"));
                }
                _ => panic!("wrong delta"),
            },
            _ => panic!("wrong event"),
        }
    }

    #[test]
    fn plan_delta_routed_to_tool_input_stream() {
        let msg = NotificationMessage::from_raw(
            "item/plan/delta",
            json!({
                "delta": "- step 1",
                "itemId": "plan-1",
                "threadId": "c1",
                "turnId": "t1",
            }),
        );
        let events = translate_notification(&tid(), msg);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ProviderRuntimeEvent::ContentDelta { delta, .. } => match delta {
                ContentDelta::ToolInput { tool_name, .. } => {
                    assert!(tool_name.starts_with("plan::"));
                }
                _ => panic!("wrong delta"),
            },
            _ => panic!("wrong event"),
        }
    }

    #[test]
    fn mcp_tool_call_progress_routed_to_tool_input_stream() {
        let msg = NotificationMessage::from_raw(
            "item/mcpToolCall/progress",
            json!({
                "message": "Connecting...",
                "itemId": "mcp-1",
                "threadId": "c1",
                "turnId": "t1",
            }),
        );
        let events = translate_notification(&tid(), msg);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ProviderRuntimeEvent::ContentDelta { delta, .. } => match delta {
                ContentDelta::ToolInput {
                    tool_name,
                    partial_json,
                } => {
                    assert!(tool_name.starts_with("mcpToolCall::"));
                    assert_eq!(partial_json, "Connecting...");
                }
                _ => panic!("wrong delta"),
            },
            _ => panic!("wrong event"),
        }
    }

    #[test]
    fn empty_delta_is_dropped() {
        let msg = NotificationMessage::from_raw(
            "item/agentMessage/delta",
            json!({
                "delta": "",
                "itemId": "i-1",
                "threadId": "c1",
                "turnId": "t1",
            }),
        );
        assert!(translate_notification(&tid(), msg).is_empty());
    }

    #[test]
    fn item_started_command_execution_emits_tool_use() {
        let msg = NotificationMessage::from_raw(
            "item/started",
            json!({
                "threadId": "c1",
                "turnId": "t1",
                "item": {
                    "type": "commandExecution",
                    "id": "cmd-9",
                    "command": "ls -la",
                    "cwd": "/tmp"
                }
            }),
        );
        let events = translate_notification(&tid(), msg);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ProviderRuntimeEvent::ItemCompleted { item, .. } => match item {
                CompletedItem::ToolUse {
                    tool_name,
                    tool_use_id,
                    ..
                } => {
                    assert_eq!(tool_name, "commandExecution");
                    assert_eq!(tool_use_id, "cmd-9");
                }
                _ => panic!("wrong item"),
            },
            _ => panic!("wrong event"),
        }
    }

    #[test]
    fn item_started_text_item_drops() {
        let msg = NotificationMessage::from_raw(
            "item/started",
            json!({
                "threadId": "c1",
                "turnId": "t1",
                "item": {
                    "type": "agentMessage",
                    "id": "am-1",
                    "text": ""
                }
            }),
        );
        assert!(translate_notification(&tid(), msg).is_empty());
    }

    #[test]
    fn item_completed_agent_message_emits_assistant_text() {
        let msg = NotificationMessage::from_raw(
            "item/completed",
            json!({
                "threadId": "c1",
                "turnId": "t1",
                "item": {
                    "type": "agentMessage",
                    "id": "am-1",
                    "text": "Final answer."
                }
            }),
        );
        let events = translate_notification(&tid(), msg);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ProviderRuntimeEvent::ItemCompleted { item, .. } => match item {
                CompletedItem::AssistantText { text } => assert_eq!(text, "Final answer."),
                _ => panic!("wrong item"),
            },
            _ => panic!("wrong event"),
        }
    }

    #[test]
    fn item_completed_reasoning_concatenates_content_and_summary() {
        let msg = NotificationMessage::from_raw(
            "item/completed",
            json!({
                "threadId": "c1",
                "turnId": "t1",
                "item": {
                    "type": "reasoning",
                    "id": "r-1",
                    "content": ["Think step one.", "Think step two."],
                    "summary": ["Summary."]
                }
            }),
        );
        let events = translate_notification(&tid(), msg);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ProviderRuntimeEvent::ItemCompleted { item, .. } => match item {
                CompletedItem::AssistantThinking { text } => {
                    assert!(text.contains("Think step one."));
                    assert!(text.contains("Think step two."));
                    assert!(text.contains("Summary."));
                }
                _ => panic!("wrong item"),
            },
            _ => panic!("wrong event"),
        }
    }

    #[test]
    fn item_completed_command_execution_emits_tool_result_with_exit_code() {
        let msg = NotificationMessage::from_raw(
            "item/completed",
            json!({
                "threadId": "c1",
                "turnId": "t1",
                "item": {
                    "type": "commandExecution",
                    "id": "cmd-9",
                    "command": "false",
                    "cwd": "/tmp",
                    "aggregatedOutput": "",
                    "exitCode": 1,
                    "commandActions": []
                }
            }),
        );
        let events = translate_notification(&tid(), msg);
        assert_eq!(events.len(), 1);
        match &events[0] {
            ProviderRuntimeEvent::ItemCompleted { item, .. } => match item {
                CompletedItem::ToolResult {
                    tool_use_id,
                    is_error,
                    ..
                } => {
                    assert_eq!(tool_use_id, "cmd-9");
                    assert!(*is_error, "non-zero exit code → is_error=true");
                }
                _ => panic!("wrong item"),
            },
            _ => panic!("wrong event"),
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
        assert!(matches!(events[0], ProviderRuntimeEvent::TurnCompleted { .. }));
        assert!(matches!(
            events[1],
            ProviderRuntimeEvent::SessionStateChanged {
                status: SessionStatus::Ready,
                ..
            }
        ));
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
    }

    #[test]
    fn unknown_notification_preserves_payload_as_warning() {
        let msg = NotificationMessage::from_raw("foo/bar/baz", json!({"interesting":"data"}));
        let events = translate_notification(&tid(), msg);
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            ProviderRuntimeEvent::RuntimeWarning { .. }
        ));
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
    fn server_request_permissions_approval_classified() {
        let msg = ServerRequestMessage::from_raw(
            "item/permissions/requestApproval",
            json!({"permissions":{}, "turnId":"t1"}),
        );
        let rid = RequestId("r2".into());
        let ev = translate_server_request(&tid(), &rid, &msg);
        if let ProviderRuntimeEvent::RequestOpened { request_kind, .. } = ev {
            assert_eq!(request_kind, "permissions");
        } else {
            panic!("expected RequestOpened");
        }
    }

    #[test]
    fn server_request_tool_call_classified() {
        let msg = ServerRequestMessage::from_raw(
            "item/tool/call",
            json!({"name":"shell", "turnId":"t1"}),
        );
        let rid = RequestId("r3".into());
        let ev = translate_server_request(&tid(), &rid, &msg);
        if let ProviderRuntimeEvent::RequestOpened { request_kind, .. } = ev {
            assert_eq!(request_kind, "tool-call");
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
