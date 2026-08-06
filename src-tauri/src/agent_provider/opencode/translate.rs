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

use std::collections::HashMap;

use crate::agent_provider::context_usage::ContextUsageTracker;
use crate::agent_provider::events::{
    CompletedItem, ContentDelta, ProviderRuntimeEvent, SubagentSnapshot, SubagentStatus,
    TaskSnapshotItem, TaskStatus, TasksSnapshot, TurnStatus,
};
use crate::agent_provider::types::{
    ApprovalDecision, ProviderSessionId, RequestId, SessionStatus, ThreadId, TurnId,
};

use super::protocol::{
    KnownEvent, ModelRef, OpenCodeApiError, OpenCodeEvent, PartInput, PartPayload,
    PermissionReply, PromptAsyncRequest, TaskMetadata, ToolStateValue,
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
    /// The active model's context-window size in tokens, when known.
    ///
    /// OpenCode states this only in the upstream provider catalogue
    /// (`limit.context`), never on the event stream, so the session
    /// probes for it in the background and drops the number here.
    /// `None` until (or unless) that lands — the meter then renders a
    /// bare token count rather than a guessed percentage.
    pub context_window_tokens: Option<u64>,
    /// Active turn id Codemux assigned when the user hit send.
    pub turn_id: TurnId,
    /// OpenCode's session id for the active session — used to
    /// surface as the `ProviderSessionId` on `SessionConfigured`.
    pub provider_session_id: ProviderSessionId,
    /// Whether a user turn is currently in flight for this session.
    /// Set true when `send_turn` posts a prompt, cleared when the
    /// parent turn settles (idle → `TurnCompleted`, or a terminal
    /// session state). The SSE give-up path reads it to decide whether
    /// the dead server left a dangling turn that must be settled with a
    /// synthetic `child_exited` `TurnCompleted`.
    pub turn_active: bool,
}

/// Translate one OpenCode SSE event to zero or more Codemux runtime
/// events.
///
/// Returns an empty `Vec` when the event is irrelevant for the chat
/// runtime (e.g. `session.created` for a session we did not initiate).
/// The SSE listener is responsible for filtering events by the
/// watched-session set before calling this, and for resolving
/// `subagent_id`: `None` when the event belongs to the top-level
/// (parent) session, `Some(child_session_id)` when it belongs to a
/// subagent's child session. That id tags every child-scoped event so
/// the frontend can route it into the subagent's own sub-transcript.
pub fn opencode_event_to_runtime(
    event: OpenCodeEvent,
    ctx: &EventContext,
    subagent_id: Option<&str>,
) -> Vec<ProviderRuntimeEvent> {
    opencode_event_to_runtime_with(
        event,
        ctx,
        subagent_id,
        &mut OpenCodeUsageState::default(),
    )
}

/// Per-session token accounting for the OpenCode adapter.
///
/// OpenCode has no lifetime counter of its own, so the adapter keeps
/// one. Assistant messages are updated *incrementally* — the same
/// message id is re-broadcast with a growing `tokens` block — so
/// accumulating each report wholesale would multiply the total. The
/// per-message high-water map fixes that: only the growth since that
/// message's previous report is added.
#[derive(Debug, Default)]
pub struct OpenCodeUsageState {
    tracker: ContextUsageTracker,
    /// Assistant message id → the largest token figure seen for it.
    seen_per_message: HashMap<String, u64>,
}

impl OpenCodeUsageState {
    /// Re-base the meter after a session model swap.
    ///
    /// The context window is a property of the model, so the number
    /// harvested for the previous one is wrong the moment the model
    /// changes — and the tracker's window is sticky by design, so it has
    /// to be dropped explicitly (`observe_max_tokens(None)` is a no-op on
    /// purpose, for the far more common "this message just didn't repeat
    /// the field" case). Swapping a 1M-window model for a 200k one would
    /// otherwise keep dividing by 1M and understate occupancy until the
    /// background catalogue probe happened to land.
    ///
    /// Returns the events that re-publish the current occupancy without a
    /// denominator, so the meter degrades to a bare token count
    /// immediately rather than showing a wrong percentage until the next
    /// assistant message. Empty when there is no reading to correct.
    ///
    /// `seen_per_message` is deliberately **kept**: it is a per-message
    /// high-water map that de-duplicates OpenCode's incremental
    /// re-broadcasts of one assistant message into the lifetime total.
    /// Message ids are model-independent and the session's transcript
    /// survives the swap (OpenCode applies the new model at the next
    /// prompt), so clearing it would let an already-counted message be
    /// re-added in full on its next re-broadcast — double-counting the
    /// lifetime figure. The lifetime total is likewise a thread-lifetime
    /// quantity and does not reset.
    pub fn model_changed(&mut self, thread_id: &ThreadId) -> Vec<ProviderRuntimeEvent> {
        self.tracker.clear_max_tokens();
        let used = self.tracker.last_live_used();
        self.tracker.events(thread_id, used, None, None)
    }
}

/// [`opencode_event_to_runtime`] threading the session's token
/// accounting so context-usage snapshots span multiple events.
pub fn opencode_event_to_runtime_with(
    event: OpenCodeEvent,
    ctx: &EventContext,
    subagent_id: Option<&str>,
    usage: &mut OpenCodeUsageState,
) -> Vec<ProviderRuntimeEvent> {
    match event {
        OpenCodeEvent::Known(known) => translate_known(known, ctx, subagent_id, usage),
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

fn translate_known(
    event: KnownEvent,
    ctx: &EventContext,
    subagent_id: Option<&str>,
    usage: &mut OpenCodeUsageState,
) -> Vec<ProviderRuntimeEvent> {
    match event {
        KnownEvent::MessagePartDelta(delta) => translate_part_delta(delta, ctx, subagent_id),
        KnownEvent::MessagePartUpdated(updated) => {
            translate_part_updated(updated, ctx, subagent_id)
        }
        KnownEvent::MessagePartRemoved(_) => vec![],
        KnownEvent::MessageUpdated(env) => {
            translate_message_updated(env, ctx, subagent_id, usage)
        }
        KnownEvent::MessageRemoved(_) => vec![],
        KnownEvent::SessionIdle(_) => {
            // A child session going idle fires *before* the parent's
            // task tool part flips to `completed` — completing the
            // parent turn here would end the turn while the subagent's
            // result is still in flight. The task part drives subagent
            // completion, so a child idle is a no-op; only the parent
            // session's idle ends the user turn.
            if subagent_id.is_some() {
                return vec![];
            }
            vec![ProviderRuntimeEvent::TurnCompleted {
                thread_id: ctx.thread_id.clone(),
                turn_id: ctx.turn_id.clone(),
                status: TurnStatus::Success,
                usage: None,
            }]
        }
        KnownEvent::SessionStatus(status) => translate_session_status(status, ctx, subagent_id),
        KnownEvent::SessionError(err) => {
            // A child session error marks that subagent failed; it must
            // not complete the parent turn.
            if let Some(id) = subagent_id {
                let activity = err
                    .error
                    .as_ref()
                    .map(|e| e.display_pair().1)
                    .unwrap_or_else(|| "subagent session error".into());
                return vec![subagent_event(
                    ctx,
                    SubagentSnapshot {
                        subagent_id: id.to_string(),
                        status: SubagentStatus::Failed,
                        result_text: Some(activity),
                        provider_ref: Some(id.to_string()),
                        ..empty_snapshot(id)
                    },
                )];
            }
            translate_session_error(err.error, ctx)
        }
        KnownEvent::PermissionAsked(p) => {
            let payload = serde_json::json!({
                "permission": p.permission,
                "patterns": p.patterns,
                "metadata": p.metadata,
            });
            vec![ProviderRuntimeEvent::RequestOpened {
                // Child (subagent) permission requests are tagged so the
                // UI can label them "from subagent X" and surface them
                // inside the drill-in. The reply still targets the child
                // session id — resolved in `session::respond_to_request`
                // from the id → session map the SSE listener records.
                subagent_id: subagent_id.map(str::to_string),
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
        KnownEvent::TodoUpdated(update) => {
            // The shared panel follows the orchestrator. A child session's
            // private todo list must not replace the parent's plan.
            if subagent_id.is_some() {
                return vec![];
            }
            let tasks = update
                .todos
                .into_iter()
                .enumerate()
                .filter_map(|(index, todo)| {
                    let title = todo.content.trim().to_string();
                    if title.is_empty() {
                        return None;
                    }
                    Some(TaskSnapshotItem {
                        task_id: if todo.id.is_empty() {
                            format!("opencode-{index}")
                        } else {
                            todo.id
                        },
                        title,
                        status: match todo.status.as_str() {
                            "completed" | "cancelled" => TaskStatus::Completed,
                            "in_progress" | "inProgress" => TaskStatus::InProgress,
                            _ => TaskStatus::Pending,
                        },
                        detail: None,
                        blocked_by: Vec::new(),
                    })
                })
                .collect();
            vec![ProviderRuntimeEvent::TasksUpdated {
                thread_id: ctx.thread_id.clone(),
                tasks: TasksSnapshot {
                    explanation: None,
                    tasks,
                },
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
    subagent_id: Option<&str>,
) -> Vec<ProviderRuntimeEvent> {
    if delta.field != "text" {
        return vec![];
    }
    vec![ProviderRuntimeEvent::ContentDelta {
        subagent_id: subagent_id.map(str::to_string),
        thread_id: ctx.thread_id.clone(),
        turn_id: ctx.turn_id.clone(),
        delta: ContentDelta::Text { text: delta.delta },
    }]
}

fn translate_part_updated(
    updated: super::protocol::MessagePartUpdated,
    ctx: &EventContext,
    subagent_id: Option<&str>,
) -> Vec<ProviderRuntimeEvent> {
    let sid = || subagent_id.map(str::to_string);
    match updated.part {
        PartPayload::Text(text) => {
            // Synthetic parts are placeholders OpenCode inserts during
            // multi-step replies — they carry no user-visible text.
            if text.synthetic.unwrap_or(false) || text.text.is_empty() {
                return vec![];
            }
            vec![ProviderRuntimeEvent::ItemCompleted {
                subagent_id: sid(),
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
                subagent_id: sid(),
                thread_id: ctx.thread_id.clone(),
                turn_id: ctx.turn_id.clone(),
                item: CompletedItem::AssistantThinking {
                    text: reasoning.text,
                },
            }]
        }
        // The `task` tool spawns a subagent — it drives the subagent
        // card, not a generic tool row, so it is routed away from
        // `translate_tool_part` (whose generic card is suppressed for
        // it) and into the SubagentUpdated lifecycle.
        PartPayload::Tool(tool) if tool.tool == "task" => translate_task_part(tool, ctx),
        PartPayload::Tool(tool) => translate_tool_part(tool, ctx, subagent_id),
        // Decoded but inert: `@agent` mention chips and `subtask`
        // markers produce no runtime event (the paired `task` tool part
        // carries the real signal).
        PartPayload::Agent(_) | PartPayload::Subtask(_) => vec![],
        PartPayload::Other => vec![],
    }
}

fn translate_tool_part(
    tool: super::protocol::ToolPart,
    ctx: &EventContext,
    subagent_id: Option<&str>,
) -> Vec<ProviderRuntimeEvent> {
    let sid = || subagent_id.map(str::to_string);
    match tool.state {
        ToolStateValue::Pending { .. } | ToolStateValue::Running { .. } => vec![],
        ToolStateValue::Completed { input, output, .. } => vec![
            ProviderRuntimeEvent::ItemCompleted {
                subagent_id: sid(),
                thread_id: ctx.thread_id.clone(),
                turn_id: ctx.turn_id.clone(),
                item: CompletedItem::ToolUse {
                    tool_name: tool.tool.clone(),
                    input,
                    tool_use_id: tool.call_id.clone(),
                },
            },
            ProviderRuntimeEvent::ItemCompleted {
                subagent_id: sid(),
                thread_id: ctx.thread_id.clone(),
                turn_id: ctx.turn_id.clone(),
                item: CompletedItem::ToolResult {
                    tool_use_id: tool.call_id,
                    content: serde_json::Value::String(output),
                    is_error: false,
                },
            },
        ],
        ToolStateValue::Error { input, error, .. } => vec![
            ProviderRuntimeEvent::ItemCompleted {
                subagent_id: sid(),
                thread_id: ctx.thread_id.clone(),
                turn_id: ctx.turn_id.clone(),
                item: CompletedItem::ToolUse {
                    tool_name: tool.tool.clone(),
                    input,
                    tool_use_id: tool.call_id.clone(),
                },
            },
            ProviderRuntimeEvent::ItemCompleted {
                subagent_id: sid(),
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

/// Translate a `task` tool part into the subagent lifecycle.
///
/// The `task` tool's lifecycle is: `pending` (no metadata yet — emits
/// nothing) → `running` (first carries `state.metadata.sessionId`, the
/// child session id / demux key, plus model, title, and the delegated
/// input) → `completed` (output wrapped in a
/// `<task …><task_result>…</task_result></task>` envelope; a
/// `<task_error>` / `state="error"` envelope means the subagent failed)
/// or `error` (the task tool itself failed).
fn translate_task_part(
    tool: super::protocol::ToolPart,
    ctx: &EventContext,
) -> Vec<ProviderRuntimeEvent> {
    match tool.state {
        // `pending` carries no metadata — we don't know the child
        // session id yet, so there is nothing to key a row on.
        ToolStateValue::Pending { .. } => vec![],
        ToolStateValue::Running {
            input,
            title,
            metadata,
            ..
        } => {
            let md = TaskMetadata::from_value(&metadata);
            let Some(child) = md.session_id.clone() else {
                return vec![];
            };
            vec![subagent_event(
                ctx,
                SubagentSnapshot {
                    subagent_id: child.clone(),
                    parent_item_id: Some(tool.call_id),
                    name: task_name(title.as_deref(), &input),
                    agent_type: string_field(&input, "subagent_type"),
                    model: md.model_id(),
                    status: SubagentStatus::Running,
                    provider_ref: Some(child),
                    ..empty_snapshot("")
                },
            )]
        }
        ToolStateValue::Completed {
            input,
            output,
            title,
            metadata,
            time,
        } => {
            let md = TaskMetadata::from_value(&metadata);
            let Some(child) = md.session_id.clone() else {
                return vec![];
            };
            let envelope = parse_task_envelope(&output);
            let status = if envelope.is_error {
                SubagentStatus::Failed
            } else {
                SubagentStatus::Completed
            };
            let duration_ms = time.as_ref().and_then(|t| match (t.start, t.end) {
                (Some(start), Some(end)) if end >= start => Some(end - start),
                _ => None,
            });
            vec![subagent_event(
                ctx,
                SubagentSnapshot {
                    subagent_id: child.clone(),
                    parent_item_id: Some(tool.call_id),
                    name: task_name(title.as_deref(), &input),
                    agent_type: string_field(&input, "subagent_type"),
                    model: md.model_id(),
                    status,
                    result_text: Some(envelope.text),
                    duration_ms,
                    provider_ref: Some(child),
                    ..empty_snapshot("")
                },
            )]
        }
        ToolStateValue::Error {
            input,
            error,
            metadata,
            time,
        } => {
            let md = TaskMetadata::from_value(&metadata);
            // Without a child session id we cannot key the row. Fall
            // back to the spawning tool call id so a failure is never
            // silently swallowed.
            let child = md.session_id.clone().unwrap_or_else(|| tool.call_id.clone());
            let duration_ms = time.as_ref().and_then(|t| match (t.start, t.end) {
                (Some(start), Some(end)) if end >= start => Some(end - start),
                _ => None,
            });
            vec![subagent_event(
                ctx,
                SubagentSnapshot {
                    subagent_id: child.clone(),
                    parent_item_id: Some(tool.call_id),
                    agent_type: string_field(&input, "subagent_type"),
                    model: md.model_id(),
                    status: SubagentStatus::Failed,
                    result_text: Some(error),
                    duration_ms,
                    provider_ref: Some(child),
                    ..empty_snapshot("")
                },
            )]
        }
    }
}

/// Result of stripping OpenCode's `<task …>` completion envelope.
struct TaskEnvelope {
    /// Inner report text (`<task_result>` body) or error text
    /// (`<task_error>` body); the raw output when no envelope is found.
    text: String,
    /// `true` when the envelope carried `state="error"` or a
    /// `<task_error>` body.
    is_error: bool,
}

/// Parse the `<task id="…" state="…"><task_result>…</task_result></task>`
/// completion envelope into its inner report + an error flag. Falls back
/// to returning the whole string (as a success) when no envelope tags
/// are present, so a future runtime that drops the wrapper still shows
/// the subagent's report.
fn parse_task_envelope(output: &str) -> TaskEnvelope {
    let opening_is_error = output
        .split_once('>')
        .map(|(head, _)| head)
        .unwrap_or("")
        .contains("state=\"error\"");
    if let Some(inner) = extract_between(output, "<task_error>", "</task_error>") {
        return TaskEnvelope {
            text: inner.trim().to_string(),
            is_error: true,
        };
    }
    if let Some(inner) = extract_between(output, "<task_result>", "</task_result>") {
        return TaskEnvelope {
            text: inner.trim().to_string(),
            is_error: opening_is_error,
        };
    }
    TaskEnvelope {
        text: output.trim().to_string(),
        is_error: opening_is_error,
    }
}

fn extract_between<'a>(haystack: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let start = haystack.find(open)? + open.len();
    let end = haystack[start..].find(close)? + start;
    Some(&haystack[start..end])
}

/// Pick a display name for a subagent: the runtime-provided `title`
/// falls back to the delegated `input.description`.
fn task_name(title: Option<&str>, input: &serde_json::Value) -> Option<String> {
    title
        .map(str::to_string)
        .or_else(|| string_field(input, "description"))
}

fn string_field(value: &serde_json::Value, key: &str) -> Option<String> {
    value.get(key).and_then(|v| v.as_str()).map(str::to_string)
}

/// A `SubagentSnapshot` with every field at its "unknown" default
/// except the id — the frontend merges non-`None` fields, so leaving
/// fields `None` here never clobbers a value learned from an earlier
/// snapshot.
fn empty_snapshot(id: &str) -> SubagentSnapshot {
    SubagentSnapshot {
        subagent_id: id.to_string(),
        parent_item_id: None,
        name: None,
        agent_type: None,
        // Neither provider reports a watch-loop-vs-agent distinction yet;
        // `None` reads as ordinary agent work everywhere downstream.
        task_kind: None,
        model: None,
        status: SubagentStatus::Pending,
        activity: None,
        result_text: None,
        tool_use_count: None,
        total_tokens: None,
        duration_ms: None,
        provider_ref: None,
        workflow_id: None,
        phase: None,
    }
}

fn subagent_event(ctx: &EventContext, subagent: SubagentSnapshot) -> ProviderRuntimeEvent {
    ProviderRuntimeEvent::SubagentUpdated {
        thread_id: ctx.thread_id.clone(),
        subagent,
    }
}

fn translate_session_status(
    status: super::protocol::SessionStatusEvent,
    ctx: &EventContext,
    subagent_id: Option<&str>,
) -> Vec<ProviderRuntimeEvent> {
    use super::protocol::SessionStatusValue as S;
    // Child-session status changes are subagent status ticks — never
    // the parent session's lifecycle. Completion is driven by the task
    // tool part, so a child going idle/retry is a no-op here (avoid
    // regressing a Running row); only `busy` bumps it to Running.
    if let Some(id) = subagent_id {
        return match status.status {
            S::Busy => vec![subagent_event(
                ctx,
                SubagentSnapshot {
                    subagent_id: id.to_string(),
                    status: SubagentStatus::Running,
                    provider_ref: Some(id.to_string()),
                    ..empty_snapshot(id)
                },
            )],
            S::Idle | S::Retry { .. } => vec![],
        };
    }
    match status.status {
        S::Idle => vec![ProviderRuntimeEvent::SessionStateChanged {
            thread_id: ctx.thread_id.clone(),
            status: SessionStatus::Ready,
        }],
        S::Busy => vec![ProviderRuntimeEvent::SessionStateChanged {
            thread_id: ctx.thread_id.clone(),
            status: SessionStatus::Running {
                active_turn: ctx.turn_id.clone(),
            },
        }],
        S::Retry { message, .. } => vec![ProviderRuntimeEvent::RuntimeWarning {
            thread_id: Some(ctx.thread_id.clone()),
            message: format!("opencode_retry: {message}"),
            original_payload: None,
        }],
    }
}

fn translate_message_updated(
    env: super::protocol::MessageEnvelope,
    ctx: &EventContext,
    subagent_id: Option<&str>,
    usage: &mut OpenCodeUsageState,
) -> Vec<ProviderRuntimeEvent> {
    // Only assistant errors matter at this layer; all other content is
    // already covered by the per-part events.
    if env.info.role != "assistant" {
        return vec![];
    }
    if let Some(err) = env.info.error {
        // A child assistant error marks the subagent failed — it must
        // not complete the parent turn (the task part will also flip to
        // a failed envelope; the reducer merges).
        if let Some(id) = subagent_id {
            let (_, message) = err.display_pair();
            return vec![subagent_event(
                ctx,
                SubagentSnapshot {
                    subagent_id: id.to_string(),
                    status: SubagentStatus::Failed,
                    result_text: Some(message),
                    provider_ref: Some(id.to_string()),
                    ..empty_snapshot(id)
                },
            )];
        }
        return translate_assistant_error(err, ctx);
    }
    translate_message_tokens(&env.info, ctx, subagent_id, usage)
}

/// Context-meter update from an assistant message's `tokens` block.
///
/// Subagent messages are excluded: a child session runs against its
/// own context window, so folding its tokens into the parent's meter
/// would overstate the parent.
fn translate_message_tokens(
    info: &super::protocol::MessageInfo,
    ctx: &EventContext,
    subagent_id: Option<&str>,
    usage: &mut OpenCodeUsageState,
) -> Vec<ProviderRuntimeEvent> {
    if subagent_id.is_some() {
        return vec![];
    }
    let Some(tokens) = info.tokens.as_ref() else {
        return vec![];
    };
    let used = tokens.total();
    if used == 0 {
        return vec![];
    }
    // Add only this message's growth since its last report, so the
    // repeated incremental updates of one message don't compound. The
    // stored value is a high-water mark: a report that comes back
    // *lower* (partial re-send) must not let the next one re-add the
    // difference.
    let seen = usage.seen_per_message.entry(info.id.clone()).or_insert(0);
    if used > *seen {
        usage.tracker.add_processed(used - *seen);
        *seen = used;
    }
    usage.tracker.observe_max_tokens(ctx.context_window_tokens);
    // OpenCode does not advertise whether the upstream compacts
    // automatically, and it varies by upstream provider — so it stays
    // unknown rather than being asserted either way.
    usage.tracker.events(&ctx.thread_id, used, None, None)
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
    use serde_json::json;

    fn ctx() -> EventContext {
        EventContext {
            thread_id: ThreadId("thread_1".into()),
            turn_id: TurnId("turn_1".into()),
            provider_session_id: ProviderSessionId("sess_1".into()),
            turn_active: true,
            context_window_tokens: None,
        }
    }

    /// [`ctx`] with a known context window, for the clamping /
    /// denominator assertions.
    fn ctx_with_window(window: u64) -> EventContext {
        EventContext {
            context_window_tokens: Some(window),
            ..ctx()
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
        let out = opencode_event_to_runtime(event, &ctx(), None);
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
        let out = opencode_event_to_runtime(event, &ctx(), None);
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
        let out = opencode_event_to_runtime(event, &ctx(), None);
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
        let out = opencode_event_to_runtime(event, &ctx(), None);
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
        assert!(opencode_event_to_runtime(event, &ctx(), None).is_empty());
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
                    metadata: serde_json::Value::Null,
                    time: None,
                },
            }),
        }));
        let out = opencode_event_to_runtime(event, &ctx(), None);
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
                    metadata: serde_json::Value::Null,
                    time: None,
                },
            }),
        }));
        let out = opencode_event_to_runtime(event, &ctx(), None);
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
        let out = opencode_event_to_runtime(event, &ctx(), None);
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
        let out = opencode_event_to_runtime(event, &ctx(), None);
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
        let out = opencode_event_to_runtime(OpenCodeEvent::Other(serde_json::Value::Null), &ctx(), None);
        assert_eq!(out.len(), 1);
        assert!(matches!(
            out[0],
            ProviderRuntimeEvent::RuntimeWarning { .. }
        ));
    }

    // ── Subagent (task tool) lifecycle ──────────────────────────────

    /// Build a `message.part.updated` event carrying a `task` tool part
    /// whose `state` is the supplied JSON, decoded through the real
    /// wire path so the tests exercise deserialization too.
    fn task_part_event(state: serde_json::Value) -> OpenCodeEvent {
        serde_json::from_value(serde_json::json!({
            "type": "message.part.updated",
            "properties": {
                "part": {
                    "id": "prt_task",
                    "sessionID": "ses_root",
                    "messageID": "msg_1",
                    "type": "tool",
                    "callID": "call_task",
                    "tool": "task",
                    "state": state
                }
            }
        }))
        .expect("task event decodes")
    }

    #[test]
    fn task_pending_emits_nothing_no_metadata_yet() {
        // The pending→running gotcha: `pending` has no metadata, so no
        // child session id — the row must NOT appear yet.
        let event = task_part_event(serde_json::json!({
            "status": "pending",
            "input": {},
            "raw": ""
        }));
        assert!(
            opencode_event_to_runtime(event, &ctx(), None).is_empty(),
            "pending task emits no subagent row"
        );
    }

    #[test]
    fn task_running_emits_running_subagent_from_metadata() {
        let event = task_part_event(serde_json::json!({
            "title": "List current directory files",
            "metadata": {
                "parentSessionId": "ses_root",
                "sessionId": "ses_child",
                "model": { "modelID": "big-pickle", "providerID": "opencode" }
            },
            "status": "running",
            "input": {
                "description": "List current directory files",
                "prompt": "List all files.",
                "subagent_type": "explore"
            },
            "time": { "start": 1000 }
        }));
        let out = opencode_event_to_runtime(event, &ctx(), None);
        assert_eq!(out.len(), 1);
        match &out[0] {
            ProviderRuntimeEvent::SubagentUpdated { subagent, .. } => {
                assert_eq!(subagent.subagent_id, "ses_child");
                assert_eq!(subagent.provider_ref.as_deref(), Some("ses_child"));
                assert_eq!(subagent.parent_item_id.as_deref(), Some("call_task"));
                assert_eq!(subagent.status, SubagentStatus::Running);
                assert_eq!(subagent.name.as_deref(), Some("List current directory files"));
                assert_eq!(subagent.agent_type.as_deref(), Some("explore"));
                assert_eq!(subagent.model.as_deref(), Some("opencode/big-pickle"));
                assert!(subagent.result_text.is_none());
            }
            other => panic!("wrong event: {other:?}"),
        }
    }

    #[test]
    fn task_running_falls_back_to_input_description_for_name() {
        // No `title` on the running state → name falls back to
        // `input.description`.
        let event = task_part_event(serde_json::json!({
            "metadata": { "sessionId": "ses_child" },
            "status": "running",
            "input": { "description": "Investigate the bug", "subagent_type": "explore" }
        }));
        let out = opencode_event_to_runtime(event, &ctx(), None);
        match &out[0] {
            ProviderRuntimeEvent::SubagentUpdated { subagent, .. } => {
                assert_eq!(subagent.name.as_deref(), Some("Investigate the bug"));
            }
            other => panic!("wrong event: {other:?}"),
        }
    }

    #[test]
    fn task_completed_parses_result_envelope_and_duration() {
        let event = task_part_event(serde_json::json!({
            "status": "completed",
            "input": { "subagent_type": "explore" },
            "output": "<task id=\"ses_child\" state=\"completed\">\n<task_result>\nDone: 19 entries.\n</task_result>\n</task>",
            "metadata": {
                "parentSessionId": "ses_root",
                "sessionId": "ses_child",
                "model": { "modelID": "big-pickle", "providerID": "opencode" }
            },
            "title": "List current directory files",
            "time": { "start": 1000, "end": 4500 }
        }));
        let out = opencode_event_to_runtime(event, &ctx(), None);
        assert_eq!(out.len(), 1);
        match &out[0] {
            ProviderRuntimeEvent::SubagentUpdated { subagent, .. } => {
                assert_eq!(subagent.subagent_id, "ses_child");
                assert_eq!(subagent.status, SubagentStatus::Completed);
                assert_eq!(subagent.result_text.as_deref(), Some("Done: 19 entries."));
                assert_eq!(subagent.duration_ms, Some(3500));
            }
            other => panic!("wrong event: {other:?}"),
        }
    }

    #[test]
    fn task_completed_with_error_envelope_marks_failed() {
        let event = task_part_event(serde_json::json!({
            "status": "completed",
            "input": {},
            "output": "<task id=\"ses_child\" state=\"error\">\n<task_error>\nSubagent hit a wall.\n</task_error>\n</task>",
            "metadata": { "sessionId": "ses_child" },
            "time": { "start": 1000, "end": 2000 }
        }));
        let out = opencode_event_to_runtime(event, &ctx(), None);
        match &out[0] {
            ProviderRuntimeEvent::SubagentUpdated { subagent, .. } => {
                assert_eq!(subagent.status, SubagentStatus::Failed);
                assert_eq!(subagent.result_text.as_deref(), Some("Subagent hit a wall."));
            }
            other => panic!("wrong event: {other:?}"),
        }
    }

    #[test]
    fn task_error_state_marks_failed() {
        let event = task_part_event(serde_json::json!({
            "status": "error",
            "input": { "subagent_type": "explore" },
            "error": "spawn failed",
            "metadata": { "sessionId": "ses_child" }
        }));
        let out = opencode_event_to_runtime(event, &ctx(), None);
        match &out[0] {
            ProviderRuntimeEvent::SubagentUpdated { subagent, .. } => {
                assert_eq!(subagent.subagent_id, "ses_child");
                assert_eq!(subagent.status, SubagentStatus::Failed);
                assert_eq!(subagent.result_text.as_deref(), Some("spawn failed"));
            }
            other => panic!("wrong event: {other:?}"),
        }
    }

    #[test]
    fn task_tool_never_emits_generic_tool_card() {
        // Even on completion the `task` tool must not produce
        // ToolUse/ToolResult items — only the subagent card.
        let event = task_part_event(serde_json::json!({
            "status": "completed",
            "input": {},
            "output": "<task id=\"ses_child\" state=\"completed\"><task_result>ok</task_result></task>",
            "metadata": { "sessionId": "ses_child" },
            "time": { "start": 1, "end": 2 }
        }));
        let out = opencode_event_to_runtime(event, &ctx(), None);
        assert!(
            out.iter().all(|e| matches!(e, ProviderRuntimeEvent::SubagentUpdated { .. })),
            "task tool must not emit generic ItemCompleted cards"
        );
    }

    #[test]
    fn parse_task_envelope_extracts_result_and_error_bodies() {
        let ok = parse_task_envelope(
            "<task id=\"ses_x\" state=\"completed\">\n<task_result>\nhello\n</task_result>\n</task>",
        );
        assert_eq!(ok.text, "hello");
        assert!(!ok.is_error);

        let err = parse_task_envelope(
            "<task id=\"ses_x\" state=\"error\">\n<task_error>\nboom\n</task_error>\n</task>",
        );
        assert_eq!(err.text, "boom");
        assert!(err.is_error);

        // No envelope → the whole string is the report, treated as success.
        let bare = parse_task_envelope("just some text");
        assert_eq!(bare.text, "just some text");
        assert!(!bare.is_error);
    }

    // ── Child-session routing ───────────────────────────────────────

    #[test]
    fn child_text_part_routes_with_subagent_id() {
        let event = OpenCodeEvent::Known(KnownEvent::MessagePartUpdated(super::super::protocol::MessagePartUpdated {
            part: PartPayload::Text(super::super::protocol::TextPart {
                id: "p1".into(),
                session_id: "ses_child".into(),
                message_id: "m1".into(),
                text: "child reply".into(),
                synthetic: None,
            }),
        }));
        let out = opencode_event_to_runtime(event, &ctx(), Some("ses_child"));
        assert_eq!(out.len(), 1);
        match &out[0] {
            ProviderRuntimeEvent::ItemCompleted {
                subagent_id,
                item: CompletedItem::AssistantText { text },
                ..
            } => {
                assert_eq!(subagent_id.as_deref(), Some("ses_child"));
                assert_eq!(text, "child reply");
            }
            other => panic!("wrong event: {other:?}"),
        }
    }

    #[test]
    fn child_delta_routes_with_subagent_id() {
        let event = OpenCodeEvent::Known(KnownEvent::MessagePartDelta(super::super::protocol::MessagePartDelta {
            session_id: "ses_child".into(),
            message_id: "m1".into(),
            part_id: "p1".into(),
            field: "text".into(),
            delta: "hi".into(),
        }));
        let out = opencode_event_to_runtime(event, &ctx(), Some("ses_child"));
        match &out[0] {
            ProviderRuntimeEvent::ContentDelta { subagent_id, .. } => {
                assert_eq!(subagent_id.as_deref(), Some("ses_child"));
            }
            other => panic!("wrong event: {other:?}"),
        }
    }

    #[test]
    fn child_tool_part_routes_with_subagent_id() {
        let event = task_child_tool_event();
        let out = opencode_event_to_runtime(event, &ctx(), Some("ses_child"));
        assert_eq!(out.len(), 2);
        for e in &out {
            match e {
                ProviderRuntimeEvent::ItemCompleted { subagent_id, .. } => {
                    assert_eq!(subagent_id.as_deref(), Some("ses_child"));
                }
                other => panic!("wrong event: {other:?}"),
            }
        }
    }

    fn task_child_tool_event() -> OpenCodeEvent {
        serde_json::from_value(serde_json::json!({
            "type": "message.part.updated",
            "properties": {
                "part": {
                    "id": "tp", "sessionID": "ses_child", "messageID": "m1",
                    "type": "tool", "callID": "call_read", "tool": "read",
                    "state": { "status": "completed", "input": {}, "output": "contents" }
                }
            }
        }))
        .unwrap()
    }

    #[test]
    fn child_session_idle_does_not_complete_parent_turn() {
        let event = OpenCodeEvent::Known(KnownEvent::SessionIdle(super::super::protocol::SessionIdle {
            session_id: "ses_child".into(),
        }));
        assert!(
            opencode_event_to_runtime(event, &ctx(), Some("ses_child")).is_empty(),
            "child idle must not emit TurnCompleted"
        );
    }

    #[test]
    fn parent_session_idle_completes_turn() {
        let event = OpenCodeEvent::Known(KnownEvent::SessionIdle(super::super::protocol::SessionIdle {
            session_id: "ses_root".into(),
        }));
        let out = opencode_event_to_runtime(event, &ctx(), None);
        assert!(matches!(out[0], ProviderRuntimeEvent::TurnCompleted { .. }));
    }

    #[test]
    fn child_session_status_busy_ticks_subagent_running() {
        let event = OpenCodeEvent::Known(KnownEvent::SessionStatus(super::super::protocol::SessionStatusEvent {
            session_id: "ses_child".into(),
            status: super::super::protocol::SessionStatusValue::Busy,
        }));
        let out = opencode_event_to_runtime(event, &ctx(), Some("ses_child"));
        match &out[0] {
            ProviderRuntimeEvent::SubagentUpdated { subagent, .. } => {
                assert_eq!(subagent.subagent_id, "ses_child");
                assert_eq!(subagent.status, SubagentStatus::Running);
            }
            other => panic!("wrong event: {other:?}"),
        }
    }

    #[test]
    fn child_session_status_idle_is_noop() {
        let event = OpenCodeEvent::Known(KnownEvent::SessionStatus(super::super::protocol::SessionStatusEvent {
            session_id: "ses_child".into(),
            status: super::super::protocol::SessionStatusValue::Idle,
        }));
        assert!(opencode_event_to_runtime(event, &ctx(), Some("ses_child")).is_empty());
    }

    #[test]
    fn child_permission_asked_tags_subagent_id() {
        let event = OpenCodeEvent::Known(KnownEvent::PermissionAsked(super::super::protocol::PermissionAskedEvent {
            id: "per_1".into(),
            session_id: "ses_child".into(),
            permission: "external_directory".into(),
            patterns: vec!["/*".into()],
            metadata: serde_json::json!({ "filepath": "/" }),
            tool: Some(super::super::protocol::PermissionToolRef {
                message_id: "m1".into(),
                call_id: "call_y".into(),
            }),
        }));
        let out = opencode_event_to_runtime(event, &ctx(), Some("ses_child"));
        match &out[0] {
            ProviderRuntimeEvent::RequestOpened {
                subagent_id,
                request_id,
                ..
            } => {
                assert_eq!(subagent_id.as_deref(), Some("ses_child"));
                assert_eq!(request_id.0, "per_1");
            }
            other => panic!("wrong event: {other:?}"),
        }
    }

    #[test]
    fn agent_mention_part_is_inert() {
        let event = OpenCodeEvent::Known(KnownEvent::MessagePartUpdated(super::super::protocol::MessagePartUpdated {
            part: PartPayload::Agent(super::super::protocol::AgentPart {
                id: "p".into(),
                session_id: "ses_root".into(),
                message_id: "m1".into(),
                name: Some("explore".into()),
            }),
        }));
        assert!(opencode_event_to_runtime(event, &ctx(), None).is_empty());
    }

    // ── Context-window usage (assistant `tokens` block) ──

    fn usage_snapshots(
        events: &[ProviderRuntimeEvent],
    ) -> Vec<crate::agent_provider::ContextUsageSnapshot> {
        events
            .iter()
            .filter_map(|e| match e {
                ProviderRuntimeEvent::ContextUsageUpdated { usage, .. } => Some(usage.clone()),
                _ => None,
            })
            .collect()
    }

    /// A `message.updated` event for an assistant message carrying a
    /// `tokens` block, decoded from the wire shape so the test covers
    /// the decoder as well as the translation.
    fn assistant_tokens_event(
        message_id: &str,
        input: u64,
        cache_read: u64,
        cache_write: u64,
        output: u64,
        reasoning: u64,
    ) -> OpenCodeEvent {
        serde_json::from_value(json!({
            "type": "message.updated",
            "properties": {
                "info": {
                    "id": message_id,
                    "sessionID": "sess_1",
                    "role": "assistant",
                    "tokens": {
                        "input": input,
                        "output": output,
                        "reasoning": reasoning,
                        "cache": {"read": cache_read, "write": cache_write}
                    }
                }
            }
        }))
        .unwrap()
    }

    #[test]
    fn assistant_tokens_block_sums_every_bucket() {
        let mut usage = OpenCodeUsageState::default();
        let events = opencode_event_to_runtime_with(
            assistant_tokens_event("msg_1", 4, 21_144, 2_715, 679, 0),
            &ctx(),
            None,
            &mut usage,
        );
        let snaps = usage_snapshots(&events);
        assert_eq!(snaps.len(), 1);
        assert_eq!(snaps[0].used_tokens, 24_542);
        // No window known yet, and OpenCode never states whether the
        // upstream auto-compacts.
        assert!(snaps[0].max_tokens.is_none());
        assert!(snaps[0].compacts_automatically.is_none());
    }

    #[test]
    fn reasoning_tokens_are_counted_toward_occupancy() {
        let mut usage = OpenCodeUsageState::default();
        let events = opencode_event_to_runtime_with(
            assistant_tokens_event("msg_1", 100, 0, 0, 50, 25),
            &ctx(),
            None,
            &mut usage,
        );
        assert_eq!(usage_snapshots(&events)[0].used_tokens, 175);
    }

    #[test]
    fn context_window_from_the_session_seeds_the_denominator_and_clamps() {
        let mut usage = OpenCodeUsageState::default();
        let events = opencode_event_to_runtime_with(
            assistant_tokens_event("msg_1", 250_000, 0, 0, 1_000, 0),
            &ctx_with_window(200_000),
            None,
            &mut usage,
        );
        let snap = &usage_snapshots(&events)[0];
        assert_eq!(snap.max_tokens, Some(200_000));
        assert_eq!(snap.used_tokens, 200_000, "never renders above 100%");
    }

    #[test]
    fn a_model_swap_drops_the_previous_model_s_window_until_the_probe_lands() {
        // The window is a property of the model, so the number harvested
        // for the old one must not survive the swap — otherwise a 1M →
        // 200k swap keeps dividing by 1M and understates occupancy.
        let mut usage = OpenCodeUsageState::default();
        let thread = ThreadId("thread_1".into());
        let big = ctx_with_window(1_000_000);
        let events = opencode_event_to_runtime_with(
            assistant_tokens_event("msg_1", 300_000, 0, 0, 0, 0),
            &big,
            None,
            &mut usage,
        );
        assert_eq!(usage_snapshots(&events)[0].max_tokens, Some(1_000_000));

        // Swap: the session clears its routing-context copy and calls
        // this, which re-publishes the same occupancy with no denominator.
        let swap = usage.model_changed(&thread);
        let snap = &usage_snapshots(&swap)[0];
        assert!(
            snap.max_tokens.is_none(),
            "no denominator beats a wrong one — the meter degrades to a bare token count"
        );
        assert_eq!(snap.used_tokens, 300_000, "occupancy carries over");

        // A later message before the probe returns still has no window…
        let events = opencode_event_to_runtime_with(
            assistant_tokens_event("msg_2", 310_000, 0, 0, 0, 0),
            // `context_window_tokens` is `None` again after the swap.
            &ctx(),
            None,
            &mut usage,
        );
        assert!(usage_snapshots(&events)[0].max_tokens.is_none());

        // …and the re-probe's smaller window re-establishes the
        // denominator once the catalogue lookup lands.
        let events = opencode_event_to_runtime_with(
            assistant_tokens_event("msg_3", 320_000, 0, 0, 0, 0),
            &ctx_with_window(200_000),
            None,
            &mut usage,
        );
        let snap = &usage_snapshots(&events)[0];
        assert_eq!(snap.max_tokens, Some(200_000));
        assert_eq!(snap.used_tokens, 200_000, "clamped to the new window");
    }

    #[test]
    fn a_model_swap_keeps_the_lifetime_total_and_its_per_message_high_water() {
        // `seen_per_message` is a de-duplication map for OpenCode's
        // incremental re-broadcasts, not model state: clearing it would
        // let an already-counted message be re-added in full.
        let mut usage = OpenCodeUsageState::default();
        let thread = ThreadId("thread_1".into());
        let ctx = ctx_with_window(200_000);
        opencode_event_to_runtime_with(
            assistant_tokens_event("msg_1", 1_000, 0, 0, 0, 0),
            &ctx,
            None,
            &mut usage,
        );
        usage.model_changed(&thread);
        // The same message re-broadcast after the swap adds only growth.
        let events = opencode_event_to_runtime_with(
            assistant_tokens_event("msg_1", 1_000, 0, 0, 200, 0),
            &ctx,
            None,
            &mut usage,
        );
        let snap = &usage_snapshots(&events)[0];
        assert_eq!(snap.used_tokens, 1_200);
        assert_eq!(
            snap.total_processed_tokens, None,
            "lifetime is 1200 — equal to used, so it stays omitted rather than \
             doubling to 2200"
        );
    }

    #[test]
    fn a_model_swap_with_no_reading_yet_emits_nothing() {
        let mut usage = OpenCodeUsageState::default();
        assert!(usage
            .model_changed(&ThreadId("thread_1".into()))
            .is_empty());
    }

    #[test]
    fn incremental_updates_of_one_message_do_not_compound_the_lifetime_total() {
        // OpenCode re-broadcasts the same message id with a growing
        // token block; only the growth may be accumulated.
        let mut usage = OpenCodeUsageState::default();
        let ctx = ctx();
        opencode_event_to_runtime_with(
            assistant_tokens_event("msg_1", 1_000, 0, 0, 0, 0),
            &ctx,
            None,
            &mut usage,
        );
        opencode_event_to_runtime_with(
            assistant_tokens_event("msg_1", 1_000, 0, 0, 500, 0),
            &ctx,
            None,
            &mut usage,
        );
        // A second message adds its own figure on top.
        let events = opencode_event_to_runtime_with(
            assistant_tokens_event("msg_2", 2_000, 0, 0, 100, 0),
            &ctx,
            None,
            &mut usage,
        );
        let snap = &usage_snapshots(&events)[0];
        assert_eq!(snap.used_tokens, 2_100, "live occupancy is this message");
        assert_eq!(
            snap.total_processed_tokens,
            Some(3_600),
            "1500 from msg_1 (not 1000+1500) plus 2100 from msg_2"
        );
    }

    #[test]
    fn a_lower_re_report_does_not_let_the_next_one_re_add() {
        // Defensive: a partial re-send that reports fewer tokens must
        // not reset the high-water mark, or the following report would
        // add the same tokens to the lifetime total twice.
        let mut usage = OpenCodeUsageState::default();
        let ctx = ctx();
        opencode_event_to_runtime_with(
            assistant_tokens_event("msg_1", 5_000, 0, 0, 0, 0),
            &ctx,
            None,
            &mut usage,
        );
        opencode_event_to_runtime_with(
            assistant_tokens_event("msg_1", 1_000, 0, 0, 0, 0),
            &ctx,
            None,
            &mut usage,
        );
        let events = opencode_event_to_runtime_with(
            assistant_tokens_event("msg_1", 6_000, 0, 0, 0, 0),
            &ctx,
            None,
            &mut usage,
        );
        let snap = &usage_snapshots(&events)[0];
        assert_eq!(snap.used_tokens, 6_000);
        // 6_000 total processed == used, so it is omitted rather than
        // surfacing an inflated 11_000/12_000 figure.
        assert!(snap.total_processed_tokens.is_none());
    }

    #[test]
    fn repeated_identical_token_report_is_suppressed() {
        let mut usage = OpenCodeUsageState::default();
        let ctx = ctx();
        let first = opencode_event_to_runtime_with(
            assistant_tokens_event("msg_1", 100, 0, 0, 20, 0),
            &ctx,
            None,
            &mut usage,
        );
        assert_eq!(usage_snapshots(&first).len(), 1);
        let second = opencode_event_to_runtime_with(
            assistant_tokens_event("msg_1", 100, 0, 0, 20, 0),
            &ctx,
            None,
            &mut usage,
        );
        assert!(usage_snapshots(&second).is_empty());
    }

    #[test]
    fn zeroed_token_block_emits_nothing() {
        // Early partial envelopes carry an all-zero block.
        let mut usage = OpenCodeUsageState::default();
        let events = opencode_event_to_runtime_with(
            assistant_tokens_event("msg_1", 0, 0, 0, 0, 0),
            &ctx(),
            None,
            &mut usage,
        );
        assert!(usage_snapshots(&events).is_empty());
    }

    #[test]
    fn assistant_message_without_a_tokens_block_emits_nothing() {
        let mut usage = OpenCodeUsageState::default();
        let event: OpenCodeEvent = serde_json::from_value(json!({
            "type": "message.updated",
            "properties": {
                "info": {"id": "msg_1", "sessionID": "sess_1", "role": "assistant"}
            }
        }))
        .unwrap();
        let events = opencode_event_to_runtime_with(event, &ctx(), None, &mut usage);
        assert!(usage_snapshots(&events).is_empty());
    }

    #[test]
    fn user_message_tokens_are_ignored() {
        let mut usage = OpenCodeUsageState::default();
        let event: OpenCodeEvent = serde_json::from_value(json!({
            "type": "message.updated",
            "properties": {
                "info": {
                    "id": "msg_1", "sessionID": "sess_1", "role": "user",
                    "tokens": {"input": 999, "output": 0, "reasoning": 0,
                               "cache": {"read": 0, "write": 0}}
                }
            }
        }))
        .unwrap();
        let events = opencode_event_to_runtime_with(event, &ctx(), None, &mut usage);
        assert!(usage_snapshots(&events).is_empty());
    }

    #[test]
    fn subagent_tokens_do_not_feed_the_parent_meter() {
        // Usage hygiene: a child session has its own context window.
        let mut usage = OpenCodeUsageState::default();
        let events = opencode_event_to_runtime_with(
            assistant_tokens_event("msg_child", 500_000, 0, 0, 1_000, 0),
            &ctx(),
            Some("child_sess"),
            &mut usage,
        );
        assert!(usage_snapshots(&events).is_empty());
    }

    #[test]
    fn todo_updated_maps_to_parent_tasks_snapshot() {
        let event: OpenCodeEvent = serde_json::from_value(serde_json::json!({
            "type": "todo.updated",
            "properties": {
                "sessionID": "ses_root",
                "todos": [
                    {"id": "a", "content": "Inspect", "status": "completed"},
                    {"id": "b", "content": "Build", "status": "in_progress"}
                ]
            }
        }))
        .unwrap();
        let out = opencode_event_to_runtime(event, &ctx(), None);
        match &out[0] {
            ProviderRuntimeEvent::TasksUpdated { tasks, .. } => {
                assert_eq!(tasks.tasks.len(), 2);
                assert_eq!(tasks.tasks[1].status, TaskStatus::InProgress);
            }
            other => panic!("expected TasksUpdated, got {other:?}"),
        }
    }
}
