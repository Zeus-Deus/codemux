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

use std::collections::HashMap;
use std::sync::LazyLock;

use regex::Regex;

use crate::agent_provider::{
    CompletedItem, ContentDelta, ContextUsageTracker, ProviderRuntimeEvent, ProviderSessionId,
    RequestId, SessionStatus, SubagentSnapshot, SubagentStatus, TaskSnapshotItem, TaskStatus,
    TasksSnapshot, ThreadId, TurnId, TurnStatus, TurnUsage, WorkflowPhaseSnapshot,
    WorkflowSnapshot,
};

use super::protocol::{SidecarError, SidecarNotification};

// ---------------------------------------------------------------------------
// Subagent demultiplexing state
// ---------------------------------------------------------------------------

/// Per-session state the Claude translator needs to demultiplex
/// subagents out of the otherwise-flat SDK message stream.
///
/// The translator is otherwise pure; this is the one bit of state it
/// carries across messages. [`super::session`] owns a single instance
/// per live session (created before the notification loop) and passes
/// `&mut` in on every message. Single-message unit tests can keep using
/// the stateless [`translate_sdk_message`] / [`translate_notification`]
/// wrappers, which allocate a throwaway demux.
#[derive(Debug, Default)]
pub struct SubagentDemux {
    /// `task_id` → spawning `tool_use_id` (== `subagent_id`). Built from
    /// `task_started` / `task_progress` / `task_notification` events that
    /// carry both ids; consulted for `task_updated`, which carries only a
    /// `task_id`.
    task_to_tool_use: HashMap<String, String>,
    /// Maps any Agent tool_use id to the ROOT (top-level) subagent id its
    /// stream flattens into. Top-level launches map to themselves;
    /// grandchildren map to their ancestor so v1 flattens nested
    /// subagents into the child's stream (no recursive drill-in yet).
    root_of: HashMap<String, String>,
    /// The `tool_use_id` (== `workflow_id`) of the currently in-flight
    /// top-level `Workflow` tool call, when one is active. `None` when no
    /// workflow is running. While `Some`, every top-level subagent
    /// snapshot is stamped with this id (see [`stamp_workflow_attribution`])
    /// so the frontend can route it into the workflow's phase view.
    active_workflow: Option<String>,
    /// Context-window accounting for this session's thread. Lives here
    /// because it needs exactly the same treatment as the subagent maps
    /// — per-session, mutated across messages, owned by the single
    /// notification task — so it rides the `&mut` the translator is
    /// already handed rather than inventing a second channel.
    context: ContextUsageTracker,
    /// In-flight Claude TaskCreate/TaskUpdate/TaskList calls, keyed by
    /// tool_use_id until their structured result arrives.
    task_tools: HashMap<String, (String, serde_json::Value)>,
    /// Current task state accumulated from Claude's modern Task* tools.
    claude_tasks: Vec<TaskSnapshotItem>,
}

impl SubagentDemux {
    /// Record a top-level `Agent`/`Task` launch (a `tool_use` block in a
    /// message with `parent_tool_use_id == null`). It becomes its own
    /// root.
    fn register_top_level_launch(&mut self, tool_use_id: &str) {
        self.root_of
            .insert(tool_use_id.to_string(), tool_use_id.to_string());
    }

    /// Record a nested (grandchild) launch seen inside a sub-transcript.
    /// It flattens into `root` — the top-level subagent that ultimately
    /// contains it.
    fn register_nested_launch(&mut self, tool_use_id: &str, root: &str) {
        self.root_of
            .insert(tool_use_id.to_string(), root.to_string());
    }

    /// Resolve the root subagent id for a `parent_tool_use_id`. Falls
    /// back to the id itself when unseen (defensive: a mid-session attach
    /// that missed the launch still routes inner items under a stable
    /// key).
    fn root_for(&self, parent_tool_use_id: &str) -> String {
        self.root_of
            .get(parent_tool_use_id)
            .cloned()
            .unwrap_or_else(|| parent_tool_use_id.to_string())
    }

    /// Whether `tool_use_id` is a known top-level subagent launch (as
    /// opposed to a grandchild, or an ordinary tool). Only top-level
    /// launches get a completion card; grandchild results flatten.
    fn is_top_level_launch(&self, tool_use_id: &str) -> bool {
        self.root_of
            .get(tool_use_id)
            .map(|r| r == tool_use_id)
            .unwrap_or(false)
    }

    /// Remember `task_id → tool_use_id` so a later `task_updated` (which
    /// omits the tool_use_id) can still be routed.
    fn record_task(&mut self, task_id: &str, tool_use_id: Option<&str>) {
        if let Some(tu) = tool_use_id {
            self.task_to_tool_use
                .insert(task_id.to_string(), tu.to_string());
        }
    }

    /// Resolve the subagent id for a task event, preferring an explicit
    /// `tool_use_id` and falling back to the recorded task→tool_use map.
    fn subagent_for_task(&self, task_id: &str, tool_use_id: Option<&str>) -> Option<String> {
        if let Some(tu) = tool_use_id {
            return Some(tu.to_string());
        }
        self.task_to_tool_use.get(task_id).cloned()
    }
}

/// Read the wire-level `parent_tool_use_id` (top-level on the SDK
/// message, NOT inside `message`). Empty / null / absent all map to
/// `None`, i.e. "this is a parent-thread message".
fn parent_tool_use_id(msg: &serde_json::Value) -> Option<String> {
    msg.get("parent_tool_use_id")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

/// The Agent-spawning tool was renamed `Agent` in CLI v2.1.63 but
/// `system:init.tools` still lists `Task`; match both.
fn is_agent_tool(name: &str) -> bool {
    name == "Agent" || name == "Task"
}

/// The dynamic-workflow orchestration tool. A top-level `tool_use` block
/// naming it launches a `Workflow` run (see [`translate_assistant`]).
fn is_workflow_tool(name: &str) -> bool {
    name == "Workflow"
}

fn is_claude_task_tool(name: &str) -> bool {
    matches!(name, "TaskCreate" | "TaskUpdate" | "TaskList")
}

fn task_status(value: Option<&str>) -> TaskStatus {
    match value {
        Some("completed") | Some("cancelled") => TaskStatus::Completed,
        Some("in_progress") | Some("inProgress") => TaskStatus::InProgress,
        _ => TaskStatus::Pending,
    }
}

fn string_array(value: Option<&serde_json::Value>) -> Vec<String> {
    value
        .and_then(|value| value.as_array())
        .map(|values| {
            values
                .iter()
                .filter_map(|value| value.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

fn tasks_event(thread_id: &ThreadId, tasks: Vec<TaskSnapshotItem>) -> ProviderRuntimeEvent {
    ProviderRuntimeEvent::TasksUpdated {
        thread_id: thread_id.clone(),
        tasks: TasksSnapshot {
            explanation: None,
            tasks,
        },
    }
}

/// Legacy TodoWrite is already a complete replacement snapshot, so it can
/// publish as soon as the assistant's finalized tool input arrives.
fn todo_write_event(
    thread_id: &ThreadId,
    input: &serde_json::Value,
) -> Option<ProviderRuntimeEvent> {
    let todos = input.get("todos")?.as_array()?;
    let tasks = todos
        .iter()
        .enumerate()
        .filter_map(|(index, todo)| {
            let title = todo
                .get("content")
                .and_then(|value| value.as_str())
                .unwrap_or("Task")
                .trim();
            let title = if title.is_empty() { "Task" } else { title };
            Some(TaskSnapshotItem {
                task_id: todo
                    .get("id")
                    .and_then(|value| value.as_str())
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("claude-todo-{index}")),
                title: title.to_string(),
                status: task_status(todo.get("status").and_then(|value| value.as_str())),
                detail: todo
                    .get("activeForm")
                    .and_then(|value| value.as_str())
                    .filter(|value| !value.trim().is_empty())
                    .map(str::to_string),
                blocked_by: Vec::new(),
            })
        })
        .collect();
    Some(tasks_event(thread_id, tasks))
}

fn task_from_value(value: &serde_json::Value) -> Option<TaskSnapshotItem> {
    let task_id = value.get("id")?.as_str()?.to_string();
    let title = value.get("subject")?.as_str()?.trim().to_string();
    if task_id.is_empty() || title.is_empty() {
        return None;
    }
    Some(TaskSnapshotItem {
        task_id,
        title,
        status: task_status(value.get("status").and_then(|status| status.as_str())),
        detail: value
            .get("activeForm")
            .and_then(|detail| detail.as_str())
            .map(str::to_string),
        blocked_by: string_array(value.get("blockedBy")),
    })
}

/// Apply a successful modern Task* result to the per-session task map.
/// Returns a complete replacement snapshot when the tool supplied enough
/// structured data to change state.
fn apply_claude_task_result(
    thread_id: &ThreadId,
    tool_use_id: &str,
    result: Option<&serde_json::Value>,
    is_error: bool,
    demux: &mut SubagentDemux,
) -> Option<ProviderRuntimeEvent> {
    let (tool_name, input) = demux.task_tools.remove(tool_use_id)?;
    if is_error {
        return None;
    }
    match tool_name.as_str() {
        "TaskList" => {
            let values = result?.get("tasks")?.as_array()?;
            demux.claude_tasks = values.iter().filter_map(task_from_value).collect();
        }
        "TaskCreate" => {
            let task = result
                .and_then(|value| value.get("task"))
                .and_then(task_from_value)
                .or_else(|| {
                    let id = result
                        .and_then(|value| value.get("task"))
                        .and_then(|task| task.get("id"))
                        .and_then(|id| id.as_str())?;
                    let title = input.get("subject")?.as_str()?.trim();
                    if title.is_empty() {
                        return None;
                    }
                    Some(TaskSnapshotItem {
                        task_id: id.to_string(),
                        title: title.to_string(),
                        status: TaskStatus::Pending,
                        detail: input
                            .get("activeForm")
                            .and_then(|value| value.as_str())
                            .map(str::to_string),
                        blocked_by: string_array(input.get("blockedBy")),
                    })
                })?;
            if let Some(index) = demux
                .claude_tasks
                .iter()
                .position(|existing| existing.task_id == task.task_id)
            {
                demux.claude_tasks[index] = task;
            } else {
                demux.claude_tasks.push(task);
            }
        }
        "TaskUpdate" => {
            let task_id = input
                .get("taskId")
                .and_then(|value| value.as_str())
                .or_else(|| result.and_then(|value| value.get("taskId")).and_then(|v| v.as_str()))?;
            let task = demux
                .claude_tasks
                .iter_mut()
                .find(|task| task.task_id == task_id)?;
            if let Some(title) = input.get("subject").and_then(|value| value.as_str()) {
                if !title.trim().is_empty() {
                    task.title = title.trim().to_string();
                }
            }
            if let Some(status) = input.get("status").and_then(|value| value.as_str()) {
                task.status = task_status(Some(status));
            }
            if let Some(detail) = input.get("activeForm").and_then(|value| value.as_str()) {
                task.detail = Some(detail.to_string());
            }
            for dependency in string_array(input.get("addBlockedBy")) {
                if !task.blocked_by.contains(&dependency) {
                    task.blocked_by.push(dependency);
                }
            }
            let removed = string_array(input.get("removeBlockedBy"));
            task.blocked_by.retain(|dependency| !removed.contains(dependency));
        }
        _ => return None,
    }
    Some(tasks_event(thread_id, demux.claude_tasks.clone()))
}

// ---------------------------------------------------------------------------
// Top-level entry point
// ---------------------------------------------------------------------------

/// Translate one sidecar notification into zero or more canonical
/// events, using a throwaway subagent demux.
///
/// Convenience entry point for single-message contexts (tests, callers
/// that do not care about cross-message subagent state). Live sessions
/// use [`translate_notification_with`] with a persistent
/// [`SubagentDemux`] so multi-message subagent routing works.
pub fn translate_notification(
    thread_id: &ThreadId,
    notification: SidecarNotification,
) -> Vec<ProviderRuntimeEvent> {
    translate_notification_with(thread_id, notification, &mut SubagentDemux::default())
}

/// Translate one sidecar notification into zero or more canonical
/// events, threading the session's [`SubagentDemux`] so subagent launch
/// / routing / completion spans multiple messages.
pub fn translate_notification_with(
    thread_id: &ThreadId,
    notification: SidecarNotification,
    demux: &mut SubagentDemux,
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
            translate_sdk_message_with(thread_id, &message, demux)
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
                // Claude's `request-opened` notification does not yet
                // carry the spawning `parent_tool_use_id`, so a subagent's
                // own approval cannot be attributed here in Stage 1. Left
                // `None`; a later sidecar enrichment can populate it.
                subagent_id: None,
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
                subagent_id: None,
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
                subagent_id: None,
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
        SidecarNotification::TurnInterrupted { .. } => {
            // An explicit interrupt RPC ended the active turn but the
            // session survives (the sidecar rebuilds a resumed query on
            // the next send-turn). Mirror the `session-ended` shape —
            // a `TurnCompleted(Error)` closes out the interrupted turn —
            // but keep the session `Ready`, NOT `Closed`.
            vec![
                ProviderRuntimeEvent::TurnCompleted {
                    thread_id: thread_id.clone(),
                    turn_id: TurnId(String::new()),
                    status: TurnStatus::Error {
                        subtype: "interrupted".into(),
                        message: "turn interrupted".into(),
                    },
                    usage: None,
                },
                ProviderRuntimeEvent::SessionStateChanged {
                    thread_id: thread_id.clone(),
                    status: SessionStatus::Ready,
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
        SidecarNotification::ResumeFallback { .. } => vec![
            // A `null` resume cursor is the "clear the persisted id"
            // signal (the stale session's on-disk JSONL was gone). The
            // persist path clears the DB column on exactly JSON null; the
            // rebuilt query emits a fresh `sdk-session-id` shortly, which
            // repopulates it.
            ProviderRuntimeEvent::ResumeCursorUpdated {
                thread_id: thread_id.clone(),
                resume_cursor: serde_json::Value::Null,
            },
            // The `"resume-fallback: "` prefix is a contract with the
            // frontend classifier, which promotes this warning to an
            // inline transcript notice (the remainder after the prefix).
            ProviderRuntimeEvent::RuntimeWarning {
                thread_id: Some(thread_id.clone()),
                message: "resume-fallback: Previous session context couldn't be restored, so this turn continues in a fresh session. Your chat history is preserved.".into(),
                original_payload: None,
            },
        ],
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
/// more canonical events, using a throwaway subagent demux. Never
/// panics; never silently drops.
///
/// Live sessions call [`translate_sdk_message_with`] with a persistent
/// [`SubagentDemux`]; this stateless wrapper serves single-message
/// tests and callers that do not track subagents.
pub fn translate_sdk_message(
    thread_id: &ThreadId,
    msg: &serde_json::Value,
) -> Vec<ProviderRuntimeEvent> {
    translate_sdk_message_with(thread_id, msg, &mut SubagentDemux::default())
}

/// Inspect an SDK-emitted message (opaque JSON), threading the
/// session's [`SubagentDemux`] so subagent launch / inner-transcript
/// routing / progress / completion work across messages.
pub fn translate_sdk_message_with(
    thread_id: &ThreadId,
    msg: &serde_json::Value,
    demux: &mut SubagentDemux,
) -> Vec<ProviderRuntimeEvent> {
    let Some(ty) = msg.get("type").and_then(|v| v.as_str()) else {
        return warning(thread_id, "sdk message missing `type`", msg);
    };
    match ty {
        "assistant" => translate_assistant(thread_id, msg, demux),
        "user" => translate_user(thread_id, msg, demux),
        "user-replay" => {
            // Replays are transcript-rebuild hints; we forward them
            // as a warning so the orchestrator can decide whether to
            // render them.
            warning(thread_id, "sdk user-replay message", msg)
        }
        "result" => translate_result(thread_id, msg, demux),
        "system" => translate_system(thread_id, msg, demux),
        "stream_event" => translate_stream_event(thread_id, msg, demux),
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
///
/// Subagent handling:
/// * A top-level `Agent`/`Task` `tool_use` block (in a message with
///   `parent_tool_use_id == null`) becomes a [`SubagentUpdated`] card
///   and its generic `ToolUse` item is suppressed.
/// * When the message itself carries a non-null `parent_tool_use_id`,
///   every item is tagged with the resolved subagent id so it lands in
///   that subagent's sub-transcript. A nested `Agent`/`Task` launch is
///   registered so its own children flatten into the same root, and it
///   renders as an ordinary (flattened) tool item.
fn translate_assistant(
    thread_id: &ThreadId,
    msg: &serde_json::Value,
    demux: &mut SubagentDemux,
) -> Vec<ProviderRuntimeEvent> {
    let mut out = Vec::new();
    let turn_id = extract_turn_id(msg);
    let parent = parent_tool_use_id(msg);
    let subagent_id = parent.as_ref().map(|p| demux.root_for(p));
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
                            subagent_id: subagent_id.clone(),
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
                            subagent_id: subagent_id.clone(),
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
                    if subagent_id.is_none() {
                        // Claude integrations occasionally namespace tool
                        // names (for example `mcp__...__TodoWrite`). Match
                        // the semantic suffix just as the SDK adapter does.
                        if tool_name.to_ascii_lowercase().contains("todowrite") {
                            if let Some(event) = todo_write_event(thread_id, &input) {
                                out.push(event);
                            }
                        } else if is_claude_task_tool(&tool_name) {
                            demux.task_tools.insert(
                                tool_use_id.clone(),
                                (tool_name.clone(), input.clone()),
                            );
                        }
                    }
                    // A top-level `Workflow` launch becomes a
                    // WorkflowUpdated card and suppresses the generic
                    // ToolUse, exactly like a top-level Agent launch does
                    // for subagents. Nested (subagent-spawned) Workflow
                    // calls are not expected and fall through unchanged.
                    if is_workflow_tool(&tool_name) && subagent_id.is_none() {
                        demux.active_workflow = Some(tool_use_id.clone());
                        out.push(ProviderRuntimeEvent::WorkflowUpdated {
                            thread_id: thread_id.clone(),
                            workflow: workflow_snapshot_from_launch(&tool_use_id, &input),
                        });
                        continue;
                    }
                    if is_agent_tool(&tool_name) {
                        match subagent_id.as_ref() {
                            // Top-level launch: emit the card, suppress the
                            // generic ToolUse (the card replaces it).
                            None => {
                                demux.register_top_level_launch(&tool_use_id);
                                let mut snap = snapshot_from_launch(&tool_use_id, &input);
                                stamp_workflow_attribution(&mut snap, demux);
                                out.push(ProviderRuntimeEvent::SubagentUpdated {
                                    thread_id: thread_id.clone(),
                                    subagent: snap,
                                });
                                continue;
                            }
                            // Nested (grandchild) launch: register so its
                            // own children flatten into the same root, then
                            // fall through to render it as an ordinary
                            // flattened tool item.
                            Some(root) => {
                                demux.register_nested_launch(&tool_use_id, root);
                            }
                        }
                    }
                    out.push(ProviderRuntimeEvent::ItemCompleted {
                        thread_id: thread_id.clone(),
                        turn_id: turn_id.clone(),
                        item: CompletedItem::ToolUse {
                            tool_name,
                            input,
                            tool_use_id,
                        },
                        subagent_id: subagent_id.clone(),
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
    // Live context-occupancy update, from the `message.usage` block the
    // SDK stamps on every assistant message.
    //
    // Subagent messages are excluded: a subagent runs against its OWN
    // context window, so folding its tokens into the parent's meter
    // would overstate the parent. This is the same usage-hygiene rule
    // that drops subagent `message_delta` events in
    // `translate_stream_event`.
    if subagent_id.is_none() {
        if let Some(usage) = msg.get("message").and_then(|m| m.get("usage")) {
            out.extend(demux.context.events(
                thread_id,
                usage_block_tokens(usage),
                None,
                CLAUDE_COMPACTS_AUTOMATICALLY,
            ));
        }
    }
    out
}

/// Claude Code compacts the context automatically once the window
/// fills, and Codemux exposes no toggle to turn that off — so every
/// Claude snapshot reports automatic compaction as on.
const CLAUDE_COMPACTS_AUTOMATICALLY: Option<bool> = Some(true);

/// Sum an Anthropic `usage` block into live context occupancy.
///
/// All four token classes count: fresh input, both cache tiers, and
/// output. Cache reads in particular are *not* free context — they are
/// prompt content the model sees on this request, so they occupy the
/// window exactly like uncached input does.
fn usage_block_tokens(usage: &serde_json::Value) -> u64 {
    [
        "input_tokens",
        "cache_creation_input_tokens",
        "cache_read_input_tokens",
        "output_tokens",
    ]
    .iter()
    .filter_map(|key| usage.get(key).and_then(|v| v.as_u64()))
    .fold(0u64, |acc, v| acc.saturating_add(v))
}

/// Build the initial [`SubagentSnapshot`] from an `Agent`/`Task`
/// tool_use block's `input`. Name precedence: `name` → `subagent_type`
/// → `description`.
fn snapshot_from_launch(tool_use_id: &str, input: &serde_json::Value) -> SubagentSnapshot {
    let get = |k: &str| {
        input
            .get(k)
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
    };
    let name = get("name").or_else(|| get("subagent_type")).or_else(|| get("description"));
    SubagentSnapshot {
        subagent_id: tool_use_id.to_string(),
        parent_item_id: Some(tool_use_id.to_string()),
        name,
        agent_type: get("subagent_type"),
        model: get("model"),
        status: SubagentStatus::Running,
        activity: None,
        result_text: None,
        tool_use_count: None,
        total_tokens: None,
        duration_ms: None,
        provider_ref: None,
        workflow_id: None,
        phase: None,
        // A launch snapshot only ever comes from a real `Agent`/`Task`
        // tool_use block, so it is by definition not a background task.
        background_task: false,
    }
}

/// Build the initial [`WorkflowSnapshot`] from a top-level `Workflow`
/// tool_use block's `input`. `input.script` carries the workflow's JS
/// source, which opens with an `export const meta = { name, description,
/// phases: [{ title, detail }, ...] }` literal; [`parse_workflow_meta`]
/// best-effort extracts it. `input.name` (a saved workflow's name) is the
/// fallback when the script has no `meta.name`.
fn workflow_snapshot_from_launch(tool_use_id: &str, input: &serde_json::Value) -> WorkflowSnapshot {
    let script = input.get("script").and_then(|v| v.as_str()).unwrap_or("");
    let meta = parse_workflow_meta(script);
    let input_name = input
        .get("name")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string());
    WorkflowSnapshot {
        workflow_id: tool_use_id.to_string(),
        status: "running".to_string(),
        name: meta.name.or(input_name),
        description: meta.description,
        script: if script.is_empty() {
            None
        } else {
            Some(script.to_string())
        },
        phases: meta.phases,
        result_text: None,
        total_tokens: None,
        agent_count: None,
        duration_ms: None,
    }
}

/// Best-effort fields extracted from a workflow script's
/// `export const meta = { ... }` literal.
#[derive(Debug, Default)]
struct ParsedWorkflowMeta {
    name: Option<String>,
    description: Option<String>,
    phases: Option<Vec<WorkflowPhaseSnapshot>>,
}

static META_DECL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"export\s+const\s+meta\s*=\s*\{").unwrap());
static META_NAME_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"name\s*:\s*['"]([^'"]*)['"]"#).unwrap());
static META_DESCRIPTION_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"description\s*:\s*['"]([^'"]*)['"]"#).unwrap());
static META_PHASE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"\{\s*title\s*:\s*['"]([^'"]*)['"](?:\s*,\s*detail\s*:\s*['"]([^'"]*)['"])?[^{}]*\}"#)
        .unwrap()
});

/// Scan `text` starting at `open_idx` (the index of an opening `open`
/// delimiter) and return the slice up to and including its matching
/// `close`, tracking nesting depth. `None` if the delimiter never closes
/// (malformed / truncated script — never panics on bad input).
fn extract_delimited(text: &str, open_idx: usize, open: char, close: char) -> Option<&str> {
    let mut depth = 0i32;
    for (i, ch) in text[open_idx..].char_indices() {
        if ch == open {
            depth += 1;
        } else if ch == close {
            depth -= 1;
            if depth == 0 {
                return Some(&text[open_idx..open_idx + i + ch.len_utf8()]);
            }
        }
    }
    None
}

/// Tolerant, regex-based extraction of `name` / `description` / `phases`
/// out of a workflow script's `meta` literal. Never panics — any parse
/// failure just leaves the corresponding field `None` so the launch still
/// gets a card (with the raw script text always preserved separately).
fn parse_workflow_meta(script: &str) -> ParsedWorkflowMeta {
    let mut out = ParsedWorkflowMeta::default();
    let Some(decl) = META_DECL_RE.find(script) else {
        return out;
    };
    // `decl` matches up to and including the opening `{`.
    let brace_idx = decl.end() - 1;
    let Some(block) = extract_delimited(script, brace_idx, '{', '}') else {
        return out;
    };
    out.name = META_NAME_RE
        .captures(block)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string());
    out.description = META_DESCRIPTION_RE
        .captures(block)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string());
    if let Some(phases_key) = block.find("phases") {
        if let Some(bracket_rel) = block[phases_key..].find('[') {
            let bracket_idx = phases_key + bracket_rel;
            if let Some(array_text) = extract_delimited(block, bracket_idx, '[', ']') {
                let phases: Vec<WorkflowPhaseSnapshot> = META_PHASE_RE
                    .captures_iter(array_text)
                    .filter_map(|c| {
                        let title = c.get(1)?.as_str().to_string();
                        let detail = c.get(2).map(|m| m.as_str().to_string());
                        Some(WorkflowPhaseSnapshot { title, detail })
                    })
                    .collect();
                if !phases.is_empty() {
                    out.phases = Some(phases);
                }
            }
        }
    }
    out
}

/// `phase:X` hint scanner, applied to a subagent's activity / result text
/// while a workflow is active. Case-insensitive; the value runs to the
/// next whitespace or closing bracket/paren/comma. `None` when no hint is
/// present — the frontend falls back to the workflow's last planned phase.
static PHASE_HINT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)phase\s*:\s*([^\s\]\),]+)").unwrap());

fn extract_phase_hint(text: &str) -> Option<String> {
    PHASE_HINT_RE
        .captures(text)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())
        .filter(|s| !s.is_empty())
}

/// While a `Workflow` run is active, stamp its id (and a best-effort
/// phase hint derived from the snapshot's own activity/result text) onto
/// a top-level subagent snapshot. No-op when no workflow is active, so
/// existing subagent behavior is byte-identical outside a workflow run.
fn stamp_workflow_attribution(snap: &mut SubagentSnapshot, demux: &SubagentDemux) {
    let Some(workflow_id) = demux.active_workflow.clone() else {
        return;
    };
    snap.workflow_id = Some(workflow_id);
    snap.phase = snap
        .activity
        .as_deref()
        .and_then(extract_phase_hint)
        .or_else(|| snap.result_text.as_deref().and_then(extract_phase_hint));
}

/// Join / stringify a `tool_result` content payload for the workflow's
/// `result_text`, truncated to `max_chars` (character count, not bytes)
/// so a runaway report can't bloat the transcript. `None` for empty
/// content.
fn stringify_content_truncated(content: &serde_json::Value, max_chars: usize) -> Option<String> {
    let text = match content {
        serde_json::Value::Null => return None,
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(arr) => {
            let mut joined = String::new();
            for block in arr {
                if let Some(t) = block.get("text").and_then(|v| v.as_str()) {
                    if !joined.is_empty() {
                        joined.push('\n');
                    }
                    joined.push_str(t);
                }
            }
            if joined.is_empty() {
                content.to_string()
            } else {
                joined
            }
        }
        other => other.to_string(),
    };
    if text.is_empty() {
        return None;
    }
    if text.chars().count() > max_chars {
        let truncated: String = text.chars().take(max_chars).collect();
        Some(format!("{truncated}…"))
    } else {
        Some(text)
    }
}

/// A `SubagentSnapshot` carrying only identity (id + `parent_item_id`)
/// and a default `Running` status. Callers overwrite the fields a given
/// event actually reports; the frontend merges non-`None` fields.
fn base_snapshot(subagent_id: &str) -> SubagentSnapshot {
    SubagentSnapshot {
        subagent_id: subagent_id.to_string(),
        parent_item_id: Some(subagent_id.to_string()),
        name: None,
        agent_type: None,
        model: None,
        status: SubagentStatus::Running,
        activity: None,
        result_text: None,
        tool_use_count: None,
        total_tokens: None,
        duration_ms: None,
        provider_ref: None,
        workflow_id: None,
        phase: None,
        // Task-event snapshots stamp this from the demux — see
        // [`stamp_background_task`].
        background_task: false,
    }
}

/// Mark `snap` as a provider background task when its id is not a
/// registered top-level `Agent`/`Task` launch.
///
/// Claude emits the same `system.task_*` family for a real delegated
/// subagent **and** for a background tool run (`Bash { run_in_background:
/// true }`), keyed by the spawning `tool_use_id`. Only a real launch is
/// registered in the demux ([`SubagentDemux::register_top_level_launch`]),
/// so "not a top-level launch" is the exact discriminator: those rows can
/// outlive the turn forever (a dev server never exits, so its terminal
/// `task_notification` never arrives) and must never pin the workspace to
/// `Working` after `TurnCompleted`.
fn stamp_background_task(snap: &mut SubagentSnapshot, demux: &SubagentDemux) {
    snap.background_task = !demux.is_top_level_launch(&snap.subagent_id);
}

/// `type: "user"` — sent by the SDK when a tool_result is appended
/// to the transcript. Emit `ItemCompleted::ToolResult` for each
/// tool_result block.
///
/// Subagent handling:
/// * Synthetic `origin: { kind: "task-notification" }` user messages are
///   suppressed entirely — the card already reflects the state and they
///   must not render as user bubbles.
/// * A parent-level `tool_result` whose `tool_use_id` is a known
///   top-level subagent launch is a completion: the wire message's
///   structured `tool_use_result` (`AgentOutput`) drives a
///   [`SubagentUpdated`] and the raw `ToolResult` item is suppressed.
///   The **background** variant (`status: "async_launched"`) is NOT a
///   completion — it is suppressed and the subagent stays `Running`
///   until a later `task_notification`.
/// * `tool_result`s inside a sub-transcript (message-level
///   `parent_tool_use_id` set) are tagged with the resolved subagent id.
fn translate_user(
    thread_id: &ThreadId,
    msg: &serde_json::Value,
    demux: &mut SubagentDemux,
) -> Vec<ProviderRuntimeEvent> {
    if is_task_notification_synthetic(msg) {
        // Suppress silently — not a warning; the card owns this state.
        return Vec::new();
    }
    let turn_id = extract_turn_id(msg);
    let parent = parent_tool_use_id(msg);
    let subagent_id = parent.as_ref().map(|p| demux.root_for(p));
    let Some(blocks) = msg
        .get("message")
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_array())
    else {
        return warning(thread_id, "user message without content array", msg);
    };
    let mut out = Vec::new();
    let mut saw_tool_result = false;
    for block in blocks {
        if block.get("type").and_then(|v| v.as_str()) == Some("tool_result") {
            saw_tool_result = true;
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
            if parent.is_none() {
                if let Some(event) = apply_claude_task_result(
                    thread_id,
                    &tool_use_id,
                    msg.get("tool_use_result"),
                    is_error,
                    demux,
                ) {
                    out.push(event);
                }
            }
            // Parent-level completion of the active Workflow tool run.
            if parent.is_none() && demux.active_workflow.as_deref() == Some(tool_use_id.as_str())
            {
                let result_text = stringify_content_truncated(&content, 4000);
                out.push(ProviderRuntimeEvent::WorkflowUpdated {
                    thread_id: thread_id.clone(),
                    workflow: WorkflowSnapshot {
                        workflow_id: tool_use_id.clone(),
                        status: if is_error { "failed" } else { "completed" }.to_string(),
                        result_text,
                        ..Default::default()
                    },
                });
                demux.active_workflow = None;
                // Suppress the raw ToolResult item.
                continue;
            }
            // Parent-level completion of a top-level subagent.
            if parent.is_none() && demux.is_top_level_launch(&tool_use_id) {
                let tur = msg.get("tool_use_result");
                match completion_from_agent_output(&tool_use_id, tur, is_error, demux) {
                    // Background launch: do not mark done; card stays
                    // Running until task_notification arrives.
                    CompletionOutcome::Pending => {}
                    CompletionOutcome::Done(snapshot) => {
                        out.push(ProviderRuntimeEvent::SubagentUpdated {
                            thread_id: thread_id.clone(),
                            subagent: snapshot,
                        });
                    }
                }
                // Suppress the raw ToolResult item either way.
                continue;
            }
            out.push(ProviderRuntimeEvent::ItemCompleted {
                thread_id: thread_id.clone(),
                turn_id: turn_id.clone(),
                item: CompletedItem::ToolResult {
                    tool_use_id,
                    content,
                    is_error,
                },
                subagent_id: subagent_id.clone(),
            });
        }
    }
    if out.is_empty() {
        // A message whose only tool_result was a suppressed
        // subagent completion (e.g. async_launched) is legitimately
        // empty — not a warning. A message with no tool_result blocks
        // at all still warns.
        if saw_tool_result {
            return Vec::new();
        }
        return warning(thread_id, "user message without tool_result blocks", msg);
    }
    out
}

/// Whether a `type: "user"` message is a synthetic task-notification
/// injection (`origin.kind == "task-notification"`) that must not render
/// as a user bubble.
fn is_task_notification_synthetic(msg: &serde_json::Value) -> bool {
    msg.get("origin")
        .and_then(|o| o.get("kind"))
        .and_then(|k| k.as_str())
        == Some("task-notification")
}

/// Outcome of inspecting a subagent `tool_result`'s structured
/// `tool_use_result` (`AgentOutput`).
enum CompletionOutcome {
    /// Background launch (`status: "async_launched"`) — not done yet.
    Pending,
    /// Terminal completion snapshot to emit.
    Done(SubagentSnapshot),
}

/// Map a subagent completion `tool_result` to a [`CompletionOutcome`].
///
/// `tur` is the wire message's top-level `tool_use_result` field
/// (structured `AgentOutput`). When it is absent or unstructured we fall
/// back to a plain completion honouring the tool_result `is_error` flag.
fn completion_from_agent_output(
    tool_use_id: &str,
    tur: Option<&serde_json::Value>,
    is_error: bool,
    demux: &SubagentDemux,
) -> CompletionOutcome {
    let Some(tur) = tur else {
        // No structured output — mark done from the tool_result flag.
        let mut snap = base_snapshot(tool_use_id);
        snap.status = if is_error {
            SubagentStatus::Failed
        } else {
            SubagentStatus::Completed
        };
        stamp_workflow_attribution(&mut snap, demux);
        return CompletionOutcome::Done(snap);
    };
    let status = tur.get("status").and_then(|v| v.as_str());
    if status == Some("async_launched") {
        return CompletionOutcome::Pending;
    }
    let mut snap = base_snapshot(tool_use_id);
    snap.status = if is_error {
        SubagentStatus::Failed
    } else {
        SubagentStatus::Completed
    };
    snap.provider_ref = tur
        .get("agentId")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    snap.agent_type = tur
        .get("agentType")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    snap.tool_use_count = tur.get("totalToolUseCount").and_then(|v| v.as_u64());
    snap.total_tokens = tur.get("totalTokens").and_then(|v| v.as_u64());
    snap.duration_ms = tur.get("totalDurationMs").and_then(|v| v.as_u64());
    snap.result_text = agent_output_text(tur);
    stamp_workflow_attribution(&mut snap, demux);
    CompletionOutcome::Done(snap)
}

/// Join the `content: [{type:"text", text}]` blocks of an `AgentOutput`
/// into the subagent's final report text.
fn agent_output_text(tur: &serde_json::Value) -> Option<String> {
    let arr = tur.get("content").and_then(|v| v.as_array())?;
    let mut out = String::new();
    for block in arr {
        if block.get("type").and_then(|v| v.as_str()) == Some("text") {
            if let Some(t) = block.get("text").and_then(|v| v.as_str()) {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(t);
            }
        }
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// `type: "result"` — turn terminus. 4 error subtypes plus success.
fn translate_result(
    thread_id: &ThreadId,
    msg: &serde_json::Value,
    demux: &mut SubagentDemux,
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
    // Context usage first, so the meter is already correct by the time
    // the UI processes the turn settling.
    let mut out = result_context_usage(thread_id, msg, &mut demux.context);
    out.push(ProviderRuntimeEvent::TurnCompleted {
        thread_id: thread_id.clone(),
        turn_id,
        status,
        usage,
    });
    out
}

/// Turn-end context accounting, derived from the `result` message.
///
/// Two things only the result knows:
///
/// * `modelUsage` — a map keyed by model id whose entries carry the
///   `contextWindow` that model ran with. This is the ONLY place Claude
///   states a window size, so it is the sole source for `max_tokens`;
///   the largest window across the entries wins, since a turn that
///   spilled across models is bounded by the biggest one it used.
/// * `usage` — the turn's *cumulative* token consumption, which is the
///   per-turn figure that feeds the lifetime accumulator. It is
///   deliberately NOT used as live occupancy: cache reads repeat in
///   full on every iteration within a turn, so the cumulative sum runs
///   far ahead of what actually sits in the window.
///
/// The emitted snapshot therefore re-states the last live occupancy
/// observed from an assistant message, now stamped with the freshly
/// learned window and lifetime total. When no assistant reading exists
/// (a single-iteration turn whose assistant message carried no usage)
/// the turn figure is the occupancy, so it is used as the fallback.
fn result_context_usage(
    thread_id: &ThreadId,
    msg: &serde_json::Value,
    tracker: &mut crate::agent_provider::ContextUsageTracker,
) -> Vec<ProviderRuntimeEvent> {
    tracker.observe_max_tokens(model_usage_context_window(msg));
    let mut turn_tokens = 0;
    if let Some(usage) = msg.get("usage") {
        turn_tokens = usage_block_tokens(usage);
        // An explicit total wins over the summed breakdown when the
        // result states one — it is the provider's own arithmetic.
        let processed = usage
            .get("total_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(turn_tokens);
        // Once per turn. Accumulating this per assistant message
        // instead would multiply every repeated cache read by the
        // iteration count.
        tracker.add_processed(processed);
    }
    // A result with no `usage` at all still re-emits, so a window
    // learned from `modelUsage` reaches the meter.
    let live = match tracker.last_live_used() {
        0 => turn_tokens,
        seen => seen,
    };
    tracker.events(thread_id, live, None, CLAUDE_COMPACTS_AUTOMATICALLY)
}

/// Largest `modelUsage[*].contextWindow` reported on a `result`.
fn model_usage_context_window(msg: &serde_json::Value) -> Option<u64> {
    msg.get("modelUsage")
        .and_then(|v| v.as_object())
        .and_then(|entries| {
            entries
                .values()
                .filter_map(|entry| entry.get("contextWindow").and_then(|v| v.as_u64()))
                .max()
        })
}

/// `type: "system"` family — discriminated by `subtype`.
fn translate_system(
    thread_id: &ThreadId,
    msg: &serde_json::Value,
    demux: &mut SubagentDemux,
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
        // Subagent lifecycle system events — translated into
        // SubagentUpdated snapshots rather than dropped as warnings.
        "task_started" => translate_task_started(thread_id, msg, demux),
        "task_progress" => translate_task_progress(thread_id, msg, demux),
        "task_updated" => translate_task_updated(thread_id, msg, demux),
        "task_notification" => translate_task_notification(thread_id, msg, demux),
        // A compaction just rewrote the window. Emit the token facts
        // first, then keep surfacing the raw system event so anything
        // already keyed off the warning still sees it.
        "compact_boundary" => {
            let mut out = translate_compact_boundary(thread_id, msg, &mut demux.context);
            out.extend(warning(thread_id, "sdk system.compact_boundary", msg));
            out
        }
        "hook_started"
        | "hook_progress"
        | "hook_response"
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

/// `system.compact_boundary` — the SDK just compacted the thread.
///
/// `compact_metadata` carries the occupancy on both sides of the
/// boundary: `post_tokens` is what survived into the fresh window (the
/// new live occupancy) and `pre_tokens` is the high-water mark it
/// replaced. The lifetime total is untouched and carries forward, so
/// the meter drops while the informational lifetime figure keeps
/// climbing — which is exactly what a compaction means.
///
/// Emits nothing when the post-compaction figure is missing or zero:
/// an unknown occupancy is worse than a stale-but-real one.
fn translate_compact_boundary(
    thread_id: &ThreadId,
    msg: &serde_json::Value,
    tracker: &mut crate::agent_provider::ContextUsageTracker,
) -> Vec<ProviderRuntimeEvent> {
    let meta = msg.get("compact_metadata");
    let field = |snake: &str, camel: &str| {
        meta.and_then(|m| m.get(snake).or_else(|| m.get(camel)))
            .and_then(|v| v.as_u64())
    };
    let post = field("post_tokens", "postTokens").unwrap_or(0);
    let pre = field("pre_tokens", "preTokens");
    tracker.events(thread_id, post, pre, CLAUDE_COMPACTS_AUTOMATICALLY)
}

/// Pull the `usage` block ({ total_tokens, tool_uses, duration_ms })
/// carried by `task_progress` / `task_notification` into a snapshot.
fn apply_task_usage(snap: &mut SubagentSnapshot, usage: Option<&serde_json::Value>) {
    let Some(usage) = usage else { return };
    snap.total_tokens = usage.get("total_tokens").and_then(|v| v.as_u64());
    snap.tool_use_count = usage.get("tool_uses").and_then(|v| v.as_u64());
    snap.duration_ms = usage.get("duration_ms").and_then(|v| v.as_u64());
}

/// `system.task_started { task_id, tool_use_id?, description }` — the
/// subagent has begun. Records the `task_id → tool_use_id` mapping and
/// emits a `Running` snapshot whose activity is the description.
/// Ambient / housekeeping tasks (`skip_transcript: true`) are dropped so
/// they never spawn a card.
///
/// The SDK emits this family for background *tool* runs too (a
/// `Bash { run_in_background: true }`), keyed by the Bash tool_use_id.
/// Those are stamped `background_task` (see [`stamp_background_task`]) so
/// the pane-status tracker never lets them hold the workspace in
/// `Working` after the turn ends.
fn translate_task_started(
    thread_id: &ThreadId,
    msg: &serde_json::Value,
    demux: &mut SubagentDemux,
) -> Vec<ProviderRuntimeEvent> {
    if msg.get("skip_transcript").and_then(|v| v.as_bool()) == Some(true) {
        return Vec::new();
    }
    let task_id = str_field(msg, "task_id");
    let tool_use_id = opt_str_field(msg, "tool_use_id");
    demux.record_task(&task_id, tool_use_id.as_deref());
    let Some(subagent_id) = demux.subagent_for_task(&task_id, tool_use_id.as_deref()) else {
        return warning(thread_id, "task_started without resolvable tool_use_id", msg);
    };
    let mut snap = base_snapshot(&subagent_id);
    snap.status = SubagentStatus::Running;
    let description = str_field(msg, "description");
    if !description.is_empty() {
        snap.activity = Some(description);
    }
    stamp_background_task(&mut snap, demux);
    stamp_workflow_attribution(&mut snap, demux);
    vec![ProviderRuntimeEvent::SubagentUpdated {
        thread_id: thread_id.clone(),
        subagent: snap,
    }]
}

/// `system.task_progress { task_id, tool_use_id?, usage, last_tool_name?,
/// summary? }` — a live progress tick. Activity precedence: pushed
/// `summary` (needs `Options.agentProgressSummaries: true`) → the child's
/// `last_tool_name`.
fn translate_task_progress(
    thread_id: &ThreadId,
    msg: &serde_json::Value,
    demux: &mut SubagentDemux,
) -> Vec<ProviderRuntimeEvent> {
    let task_id = str_field(msg, "task_id");
    let tool_use_id = opt_str_field(msg, "tool_use_id");
    demux.record_task(&task_id, tool_use_id.as_deref());
    let Some(subagent_id) = demux.subagent_for_task(&task_id, tool_use_id.as_deref()) else {
        return warning(thread_id, "task_progress without resolvable tool_use_id", msg);
    };
    let mut snap = base_snapshot(&subagent_id);
    snap.status = SubagentStatus::Running;
    snap.activity = opt_str_field(msg, "summary").or_else(|| opt_str_field(msg, "last_tool_name"));
    apply_task_usage(&mut snap, msg.get("usage"));
    stamp_background_task(&mut snap, demux);
    stamp_workflow_attribution(&mut snap, demux);
    vec![ProviderRuntimeEvent::SubagentUpdated {
        thread_id: thread_id.clone(),
        subagent: snap,
    }]
}

/// `system.task_updated { task_id, patch: { status?, description?,
/// error? } }` — carries NO `tool_use_id`, so it is routed via the
/// recorded `task_id → tool_use_id` map. `patch.status` maps onto
/// [`SubagentStatus`]; an `error` marks the subagent `Failed`.
fn translate_task_updated(
    thread_id: &ThreadId,
    msg: &serde_json::Value,
    demux: &mut SubagentDemux,
) -> Vec<ProviderRuntimeEvent> {
    let task_id = str_field(msg, "task_id");
    let Some(subagent_id) = demux.subagent_for_task(&task_id, None) else {
        return warning(thread_id, "task_updated for unknown task_id", msg);
    };
    let mut snap = base_snapshot(&subagent_id);
    let mut changed = false;
    if let Some(patch) = msg.get("patch") {
        if let Some(status) = patch.get("status").and_then(|v| v.as_str()) {
            snap.status = map_patch_status(status);
            changed = true;
        }
        if let Some(desc) = patch
            .get("description")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
        {
            snap.activity = Some(desc.to_string());
            changed = true;
        }
        if let Some(err) = patch
            .get("error")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
        {
            snap.status = SubagentStatus::Failed;
            snap.result_text = Some(err.to_string());
            changed = true;
        }
    }
    if !changed {
        return Vec::new();
    }
    stamp_background_task(&mut snap, demux);
    stamp_workflow_attribution(&mut snap, demux);
    vec![ProviderRuntimeEvent::SubagentUpdated {
        thread_id: thread_id.clone(),
        subagent: snap,
    }]
}

/// `system.task_notification { task_id, tool_use_id?, status, summary,
/// usage? }` — terminal notice, the completion path for background
/// (`async_launched`) subagents. `status` ∈ completed|failed|stopped.
fn translate_task_notification(
    thread_id: &ThreadId,
    msg: &serde_json::Value,
    demux: &mut SubagentDemux,
) -> Vec<ProviderRuntimeEvent> {
    let task_id = str_field(msg, "task_id");
    let tool_use_id = opt_str_field(msg, "tool_use_id");
    demux.record_task(&task_id, tool_use_id.as_deref());
    let Some(subagent_id) = demux.subagent_for_task(&task_id, tool_use_id.as_deref()) else {
        return warning(thread_id, "task_notification without resolvable tool_use_id", msg);
    };
    let mut snap = base_snapshot(&subagent_id);
    snap.status = match str_field(msg, "status").as_str() {
        "failed" => SubagentStatus::Failed,
        "stopped" => SubagentStatus::Stopped,
        _ => SubagentStatus::Completed,
    };
    let summary = str_field(msg, "summary");
    if !summary.is_empty() {
        snap.result_text = Some(summary);
    }
    apply_task_usage(&mut snap, msg.get("usage"));
    stamp_background_task(&mut snap, demux);
    stamp_workflow_attribution(&mut snap, demux);
    vec![ProviderRuntimeEvent::SubagentUpdated {
        thread_id: thread_id.clone(),
        subagent: snap,
    }]
}

/// Map a `task_updated.patch.status` string onto [`SubagentStatus`].
fn map_patch_status(status: &str) -> SubagentStatus {
    match status {
        "completed" => SubagentStatus::Completed,
        "failed" => SubagentStatus::Failed,
        "killed" => SubagentStatus::Stopped,
        "pending" => SubagentStatus::Pending,
        // "running" and any unknown future value stay Running.
        _ => SubagentStatus::Running,
    }
}

fn str_field(msg: &serde_json::Value, key: &str) -> String {
    msg.get(key)
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string()
}

fn opt_str_field(msg: &serde_json::Value, key: &str) -> Option<String> {
    msg.get(key)
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

/// `type: "stream_event"` — partial SDK delta. Extract the delta
/// payload and emit the right `ContentDelta`.
///
/// When the wire message carries a non-null `parent_tool_use_id` the
/// delta belongs to a subagent's sub-transcript and is tagged with the
/// resolved subagent id. Non-render subagent stream events (message
/// start/stop/delta, incl. `message_delta` usage) are dropped silently —
/// this is the usage-hygiene step that keeps subagent token deltas out
/// of the parent's accounting.
fn translate_stream_event(
    thread_id: &ThreadId,
    msg: &serde_json::Value,
    demux: &mut SubagentDemux,
) -> Vec<ProviderRuntimeEvent> {
    let turn_id = extract_turn_id(msg);
    let parent = parent_tool_use_id(msg);
    let subagent_id = parent.as_ref().map(|p| demux.root_for(p));
    let Some(event) = msg.get("event") else {
        return warning(thread_id, "stream_event without event payload", msg);
    };
    let event_type = event.get("type").and_then(|v| v.as_str()).unwrap_or("");
    if event_type != "content_block_delta" {
        // Subagent non-render stream events (message_start/message_delta/
        // …) are dropped silently — dropping message_delta here is the
        // usage-hygiene step. Parent-thread ones stay a warning.
        if subagent_id.is_some() {
            return Vec::new();
        }
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
        subagent_id,
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
    use crate::agent_provider::ContextUsageSnapshot;
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
    fn notification_turn_interrupted_emits_error_and_ready() {
        // An explicit interrupt RPC keeps the session alive: the turn
        // closes out as an interrupted error but the session returns to
        // Ready (NOT Closed like a spontaneous `session-ended`).
        let n = SidecarNotification::TurnInterrupted {
            thread_id: "t".into(),
        };
        let events = translate_notification(&tid(), n);
        assert_eq!(events.len(), 2);
        match &events[0] {
            ProviderRuntimeEvent::TurnCompleted { status, .. } => match status {
                TurnStatus::Error { subtype, message } => {
                    assert_eq!(subtype, "interrupted");
                    assert_eq!(message, "turn interrupted");
                }
                _ => panic!("expected Error"),
            },
            _ => panic!("expected TurnCompleted"),
        }
        assert!(matches!(
            &events[1],
            ProviderRuntimeEvent::SessionStateChanged {
                status: SessionStatus::Ready,
                ..
            }
        ));
    }

    #[test]
    fn notification_resume_fallback_clears_cursor_and_warns() {
        // The sidecar rebuilt a fresh query after a stale-session resume
        // failure. Translation must emit exactly two events: a null
        // resume cursor (clear the persisted id) and a `resume-fallback:`
        // -prefixed warning the frontend promotes to an inline notice.
        let n = SidecarNotification::ResumeFallback {
            thread_id: "t".into(),
            stale_session_id: Some("dead-uuid".into()),
        };
        let events = translate_notification(&tid(), n);
        assert_eq!(events.len(), 2);
        match &events[0] {
            ProviderRuntimeEvent::ResumeCursorUpdated { resume_cursor, .. } => {
                assert_eq!(*resume_cursor, serde_json::Value::Null);
            }
            _ => panic!("expected ResumeCursorUpdated(null)"),
        }
        match &events[1] {
            ProviderRuntimeEvent::RuntimeWarning {
                message,
                original_payload,
                ..
            } => {
                assert!(message.starts_with("resume-fallback: "));
                assert_eq!(
                    message,
                    "resume-fallback: Previous session context couldn't be restored, so this turn continues in a fresh session. Your chat history is preserved."
                );
                assert!(original_payload.is_none());
            }
            _ => panic!("expected RuntimeWarning"),
        }
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

    // =======================================================================
    // Subagent mapping (Stage 1)
    // =======================================================================

    /// Assistant message launching an `Agent`/`Task` subagent.
    fn launch_msg(name: &str, tool_use_id: &str, input: serde_json::Value) -> serde_json::Value {
        json!({
            "type": "assistant",
            "parent_tool_use_id": null,
            "turn_id": "turn-1",
            "message": { "content": [{
                "type": "tool_use",
                "id": tool_use_id,
                "name": name,
                "input": input
            }]}
        })
    }

    fn only_subagent(events: &[ProviderRuntimeEvent]) -> &SubagentSnapshot {
        assert_eq!(events.len(), 1, "expected exactly one event: {events:?}");
        match &events[0] {
            ProviderRuntimeEvent::SubagentUpdated { subagent, .. } => subagent,
            other => panic!("expected SubagentUpdated, got {other:?}"),
        }
    }

    #[test]
    fn agent_tool_use_launches_subagent_and_suppresses_tool_use() {
        let mut demux = SubagentDemux::default();
        let msg = launch_msg(
            "Agent",
            "toolu_root",
            json!({
                "description": "explore the repo",
                "subagent_type": "Explore",
                "model": "claude-sonnet-4",
                "prompt": "look around"
            }),
        );
        let events = translate_sdk_message_with(&tid(), &msg, &mut demux);
        // No generic ToolUse item — the card replaces it.
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, ProviderRuntimeEvent::ItemCompleted { .. })),
            "generic ToolUse must be suppressed for an Agent launch"
        );
        let snap = only_subagent(&events);
        assert_eq!(snap.subagent_id, "toolu_root");
        assert_eq!(snap.parent_item_id.as_deref(), Some("toolu_root"));
        // name falls back to subagent_type when `name` is absent.
        assert_eq!(snap.name.as_deref(), Some("Explore"));
        assert_eq!(snap.agent_type.as_deref(), Some("Explore"));
        assert_eq!(snap.model.as_deref(), Some("claude-sonnet-4"));
        assert_eq!(snap.status, SubagentStatus::Running);
    }

    #[test]
    fn legacy_task_tool_name_also_launches_subagent() {
        // CLI < v2.1.63 named the tool `Task`; match it too.
        let mut demux = SubagentDemux::default();
        let msg = launch_msg("Task", "toolu_task", json!({"name": "Nickname"}));
        let snap_events = translate_sdk_message_with(&tid(), &msg, &mut demux);
        let snap = only_subagent(&snap_events);
        assert_eq!(snap.subagent_id, "toolu_task");
        // Explicit `name` wins over subagent_type/description.
        assert_eq!(snap.name.as_deref(), Some("Nickname"));
    }

    #[test]
    fn subagent_name_precedence_falls_back_to_description() {
        let mut demux = SubagentDemux::default();
        let msg = launch_msg("Agent", "toolu_d", json!({"description": "just a desc"}));
        let snap_events = translate_sdk_message_with(&tid(), &msg, &mut demux);
        let snap = only_subagent(&snap_events);
        assert_eq!(snap.name.as_deref(), Some("just a desc"));
    }

    #[test]
    fn inner_assistant_message_routes_to_subagent_id() {
        let mut demux = SubagentDemux::default();
        // Launch first so the demux knows toolu_root is a subagent.
        translate_sdk_message_with(
            &tid(),
            &launch_msg("Agent", "toolu_root", json!({"subagent_type": "Explore"})),
            &mut demux,
        );
        let inner = json!({
            "type": "assistant",
            "parent_tool_use_id": "toolu_root",
            "turn_id": "turn-1",
            "message": { "content": [
                {"type": "text", "text": "working on it"},
                {"type": "tool_use", "id": "tu-bash", "name": "Bash", "input": {"command": "ls"}}
            ]}
        });
        let events = translate_sdk_message_with(&tid(), &inner, &mut demux);
        assert_eq!(events.len(), 2);
        for e in &events {
            match e {
                ProviderRuntimeEvent::ItemCompleted { subagent_id, .. } => {
                    assert_eq!(subagent_id.as_deref(), Some("toolu_root"));
                }
                other => panic!("expected ItemCompleted, got {other:?}"),
            }
        }
    }

    #[test]
    fn inner_stream_event_delta_routes_to_subagent_id() {
        let mut demux = SubagentDemux::default();
        translate_sdk_message_with(
            &tid(),
            &launch_msg("Agent", "toolu_root", json!({"subagent_type": "Explore"})),
            &mut demux,
        );
        let delta = json!({
            "type": "stream_event",
            "parent_tool_use_id": "toolu_root",
            "turn_id": "turn-1",
            "event": {
                "type": "content_block_delta",
                "delta": {"type": "text_delta", "text": "hel"}
            }
        });
        let events = translate_sdk_message_with(&tid(), &delta, &mut demux);
        match &events[0] {
            ProviderRuntimeEvent::ContentDelta { subagent_id, delta, .. } => {
                assert_eq!(subagent_id.as_deref(), Some("toolu_root"));
                assert!(matches!(delta, ContentDelta::Text { text } if text == "hel"));
            }
            other => panic!("expected ContentDelta, got {other:?}"),
        }
    }

    #[test]
    fn subagent_message_delta_is_dropped_for_usage_hygiene() {
        // A subagent's message_delta (carrying usage) must not reach the
        // parent's accounting nor render — drop it silently.
        let mut demux = SubagentDemux::default();
        translate_sdk_message_with(
            &tid(),
            &launch_msg("Agent", "toolu_root", json!({"subagent_type": "Explore"})),
            &mut demux,
        );
        let md = json!({
            "type": "stream_event",
            "parent_tool_use_id": "toolu_root",
            "event": {
                "type": "message_delta",
                "usage": {"output_tokens": 42}
            }
        });
        let events = translate_sdk_message_with(&tid(), &md, &mut demux);
        assert!(events.is_empty(), "subagent message_delta must be dropped");
    }

    #[test]
    fn parent_level_message_start_still_warns() {
        // Regression guard: the usage-hygiene drop only applies to
        // subagent stream events; parent-thread ones keep warning.
        let mut demux = SubagentDemux::default();
        let md = json!({
            "type": "stream_event",
            "parent_tool_use_id": null,
            "event": {"type": "message_start"}
        });
        let events = translate_sdk_message_with(&tid(), &md, &mut demux);
        assert!(matches!(
            &events[0],
            ProviderRuntimeEvent::RuntimeWarning { .. }
        ));
    }

    #[test]
    fn task_started_then_progress_then_updated_route_via_task_map() {
        let mut demux = SubagentDemux::default();
        translate_sdk_message_with(
            &tid(),
            &launch_msg("Agent", "toolu_root", json!({"subagent_type": "Explore"})),
            &mut demux,
        );
        // task_started carries both ids — records task_1 → toolu_root.
        let started = json!({
            "type": "system", "subtype": "task_started",
            "task_id": "task_1", "tool_use_id": "toolu_root",
            "description": "Exploring the tree"
        });
        let snap = only_subagent(&translate_sdk_message_with(&tid(), &started, &mut demux)).clone();
        assert_eq!(snap.subagent_id, "toolu_root");
        assert_eq!(snap.status, SubagentStatus::Running);
        assert_eq!(snap.activity.as_deref(), Some("Exploring the tree"));

        // task_progress: summary wins over last_tool_name; usage flows.
        let progress = json!({
            "type": "system", "subtype": "task_progress",
            "task_id": "task_1", "tool_use_id": "toolu_root",
            "usage": {"total_tokens": 1200, "tool_uses": 3, "duration_ms": 5000},
            "last_tool_name": "Read", "summary": "Reading files"
        });
        let snap = only_subagent(&translate_sdk_message_with(&tid(), &progress, &mut demux)).clone();
        assert_eq!(snap.activity.as_deref(), Some("Reading files"));
        assert_eq!(snap.tool_use_count, Some(3));
        assert_eq!(snap.total_tokens, Some(1200));
        assert_eq!(snap.duration_ms, Some(5000));

        // task_updated carries NO tool_use_id — must resolve via the map.
        let updated = json!({
            "type": "system", "subtype": "task_updated",
            "task_id": "task_1", "patch": {"status": "completed"}
        });
        let snap = only_subagent(&translate_sdk_message_with(&tid(), &updated, &mut demux)).clone();
        assert_eq!(snap.subagent_id, "toolu_root");
        assert_eq!(snap.status, SubagentStatus::Completed);
    }

    // A background *tool* run (`Bash { run_in_background: true }`) rides
    // the same `system.task_*` family as a real subagent, keyed by the
    // Bash tool_use_id — which is never registered as a top-level
    // `Agent`/`Task` launch. Those snapshots must carry `background_task`
    // so the pane-status tracker refuses to let a never-exiting dev server
    // hold the workspace in "Working" after the turn ends.
    #[test]
    fn background_shell_task_events_are_flagged_as_background_tasks() {
        let mut demux = SubagentDemux::default();
        // No `register_top_level_launch` — a Bash tool_use never registers.
        let started = json!({
            "type": "system", "subtype": "task_started",
            "task_id": "task_bg", "tool_use_id": "toolu_bash",
            "description": "npm run dev"
        });
        let snap = only_subagent(&translate_sdk_message_with(&tid(), &started, &mut demux)).clone();
        assert_eq!(snap.subagent_id, "toolu_bash");
        assert!(
            snap.background_task,
            "an unregistered tool_use id is a background task, not a subagent"
        );

        // The same flag rides progress / updated / notification, so no
        // later event can quietly re-enter the running-set unflagged.
        let progress = json!({
            "type": "system", "subtype": "task_progress",
            "task_id": "task_bg", "tool_use_id": "toolu_bash",
            "last_tool_name": "Bash"
        });
        let snap =
            only_subagent(&translate_sdk_message_with(&tid(), &progress, &mut demux)).clone();
        assert!(snap.background_task);

        let updated = json!({
            "type": "system", "subtype": "task_updated",
            "task_id": "task_bg", "patch": {"status": "running"}
        });
        let snap = only_subagent(&translate_sdk_message_with(&tid(), &updated, &mut demux)).clone();
        assert!(snap.background_task);

        let note = json!({
            "type": "system", "subtype": "task_notification",
            "task_id": "task_bg", "tool_use_id": "toolu_bash",
            "status": "completed", "summary": "server stopped"
        });
        let snap = only_subagent(&translate_sdk_message_with(&tid(), &note, &mut demux)).clone();
        assert!(snap.background_task);
        assert_eq!(snap.status, SubagentStatus::Completed);
    }

    // The counterpart: a real `Agent`/`Task` launch is registered in the
    // demux, so its task events stay unflagged and keep the deliberate
    // deferred-`Review` behavior.
    #[test]
    fn real_agent_launch_task_events_are_not_background_tasks() {
        let mut demux = SubagentDemux::default();
        let launch = translate_sdk_message_with(
            &tid(),
            &launch_msg("Agent", "toolu_root", json!({"subagent_type": "Explore"})),
            &mut demux,
        );
        assert!(
            !only_subagent(&launch).background_task,
            "the launch snapshot itself is never a background task"
        );
        let started = json!({
            "type": "system", "subtype": "task_started",
            "task_id": "task_1", "tool_use_id": "toolu_root",
            "description": "Exploring the tree"
        });
        let snap = only_subagent(&translate_sdk_message_with(&tid(), &started, &mut demux)).clone();
        assert!(!snap.background_task);

        let note = json!({
            "type": "system", "subtype": "task_notification",
            "task_id": "task_1", "tool_use_id": "toolu_root",
            "status": "completed", "summary": "done"
        });
        let snap = only_subagent(&translate_sdk_message_with(&tid(), &note, &mut demux)).clone();
        assert!(!snap.background_task);
    }

    #[test]
    fn task_progress_falls_back_to_last_tool_name_without_summary() {
        let mut demux = SubagentDemux::default();
        translate_sdk_message_with(
            &tid(),
            &launch_msg("Agent", "toolu_root", json!({"subagent_type": "Explore"})),
            &mut demux,
        );
        let progress = json!({
            "type": "system", "subtype": "task_progress",
            "task_id": "task_1", "tool_use_id": "toolu_root",
            "usage": {"total_tokens": 10, "tool_uses": 1, "duration_ms": 20},
            "last_tool_name": "Grep"
        });
        let snap = only_subagent(&translate_sdk_message_with(&tid(), &progress, &mut demux)).clone();
        assert_eq!(snap.activity.as_deref(), Some("Grep"));
    }

    #[test]
    fn foreground_completion_emits_completed_and_suppresses_tool_result() {
        let mut demux = SubagentDemux::default();
        translate_sdk_message_with(
            &tid(),
            &launch_msg("Agent", "toolu_root", json!({"subagent_type": "Explore"})),
            &mut demux,
        );
        let done = json!({
            "type": "user",
            "parent_tool_use_id": null,
            "message": { "content": [{
                "type": "tool_result",
                "tool_use_id": "toolu_root",
                "content": "Report",
                "is_error": false
            }]},
            "tool_use_result": {
                "status": "completed",
                "agentId": "agent_xyz",
                "agentType": "Explore",
                "totalToolUseCount": 7,
                "totalDurationMs": 42000,
                "totalTokens": 9000,
                "content": [{"type": "text", "text": "Final report line"}],
                "prompt": "look around"
            }
        });
        let events = translate_sdk_message_with(&tid(), &done, &mut demux);
        // No raw ToolResult item — the card owns completion.
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, ProviderRuntimeEvent::ItemCompleted { .. })),
            "raw ToolResult must be suppressed for a subagent completion"
        );
        let snap = only_subagent(&events);
        assert_eq!(snap.subagent_id, "toolu_root");
        assert_eq!(snap.status, SubagentStatus::Completed);
        assert_eq!(snap.provider_ref.as_deref(), Some("agent_xyz"));
        assert_eq!(snap.tool_use_count, Some(7));
        assert_eq!(snap.total_tokens, Some(9000));
        assert_eq!(snap.duration_ms, Some(42000));
        assert_eq!(snap.result_text.as_deref(), Some("Final report line"));
    }

    #[test]
    fn background_async_launched_does_not_complete_until_task_notification() {
        let mut demux = SubagentDemux::default();
        translate_sdk_message_with(
            &tid(),
            &launch_msg("Agent", "toolu_root", json!({"subagent_type": "Explore"})),
            &mut demux,
        );
        // Background launch: tool_result arrives immediately with
        // status async_launched — must NOT mark done, must suppress.
        let launched = json!({
            "type": "user",
            "parent_tool_use_id": null,
            "message": { "content": [{
                "type": "tool_result",
                "tool_use_id": "toolu_root",
                "content": "launched in background",
                "is_error": false
            }]},
            "tool_use_result": {
                "status": "async_launched",
                "agentId": "agent_xyz",
                "description": "bg task",
                "prompt": "p",
                "outputFile": "/tmp/out"
            }
        });
        let events = translate_sdk_message_with(&tid(), &launched, &mut demux);
        assert!(
            events.is_empty(),
            "async_launched must not emit a completion or a ToolResult"
        );

        // Later, task_notification delivers the real completion.
        let note = json!({
            "type": "system", "subtype": "task_notification",
            "task_id": "task_9", "tool_use_id": "toolu_root",
            "status": "completed", "summary": "All done",
            "output_file": "/tmp/out",
            "usage": {"total_tokens": 500, "tool_uses": 2, "duration_ms": 3000}
        });
        let snap = only_subagent(&translate_sdk_message_with(&tid(), &note, &mut demux)).clone();
        assert_eq!(snap.subagent_id, "toolu_root");
        assert_eq!(snap.status, SubagentStatus::Completed);
        assert_eq!(snap.result_text.as_deref(), Some("All done"));
        assert_eq!(snap.total_tokens, Some(500));
        assert_eq!(snap.tool_use_count, Some(2));
    }

    #[test]
    fn task_notification_failed_and_stopped_map_status() {
        let mut demux = SubagentDemux::default();
        translate_sdk_message_with(
            &tid(),
            &launch_msg("Agent", "toolu_root", json!({"subagent_type": "Explore"})),
            &mut demux,
        );
        let failed = json!({
            "type": "system", "subtype": "task_notification",
            "task_id": "t", "tool_use_id": "toolu_root",
            "status": "failed", "summary": "boom"
        });
        let snap = only_subagent(&translate_sdk_message_with(&tid(), &failed, &mut demux)).clone();
        assert_eq!(snap.status, SubagentStatus::Failed);

        let stopped = json!({
            "type": "system", "subtype": "task_notification",
            "task_id": "t", "tool_use_id": "toolu_root",
            "status": "stopped", "summary": "cancelled"
        });
        let snap = only_subagent(&translate_sdk_message_with(&tid(), &stopped, &mut demux)).clone();
        assert_eq!(snap.status, SubagentStatus::Stopped);
    }

    #[test]
    fn synthetic_task_notification_user_message_is_suppressed() {
        let mut demux = SubagentDemux::default();
        let synthetic = json!({
            "type": "user",
            "parent_tool_use_id": null,
            "isSynthetic": true,
            "origin": {"kind": "task-notification"},
            "message": {"content": [{"type": "text", "text": "Task finished"}]}
        });
        let events = translate_sdk_message_with(&tid(), &synthetic, &mut demux);
        assert!(
            events.is_empty(),
            "synthetic task-notification user messages must not render"
        );
    }

    #[test]
    fn nested_grandchild_flattens_into_root_subagent_stream() {
        let mut demux = SubagentDemux::default();
        // Top-level launch.
        translate_sdk_message_with(
            &tid(),
            &launch_msg("Agent", "toolu_root", json!({"subagent_type": "Explore"})),
            &mut demux,
        );
        // Inside the child's stream, the child spawns a grandchild Agent.
        let nested_launch = json!({
            "type": "assistant",
            "parent_tool_use_id": "toolu_root",
            "turn_id": "turn-1",
            "message": { "content": [{
                "type": "tool_use", "id": "toolu_child", "name": "Agent",
                "input": {"subagent_type": "Sub"}
            }]}
        });
        let events = translate_sdk_message_with(&tid(), &nested_launch, &mut demux);
        // A nested launch does NOT create a second card — it flattens as
        // an ordinary tool item tagged with the ROOT subagent id.
        assert_eq!(events.len(), 1);
        match &events[0] {
            ProviderRuntimeEvent::ItemCompleted {
                subagent_id, item, ..
            } => {
                assert_eq!(subagent_id.as_deref(), Some("toolu_root"));
                assert!(matches!(item, CompletedItem::ToolUse { .. }));
            }
            other => panic!("expected flattened ItemCompleted, got {other:?}"),
        }
        // The grandchild's OWN messages (parent = toolu_child) flatten
        // into the root's stream too.
        let grandchild_msg = json!({
            "type": "assistant",
            "parent_tool_use_id": "toolu_child",
            "message": { "content": [{"type": "text", "text": "deep work"}]}
        });
        let events = translate_sdk_message_with(&tid(), &grandchild_msg, &mut demux);
        match &events[0] {
            ProviderRuntimeEvent::ItemCompleted { subagent_id, .. } => {
                assert_eq!(subagent_id.as_deref(), Some("toolu_root"));
            }
            other => panic!("expected ItemCompleted, got {other:?}"),
        }
    }

    #[test]
    fn parent_tool_result_for_non_subagent_stays_a_normal_item() {
        // A tool_result whose id is NOT a known subagent launch must
        // render as an ordinary (untagged) ToolResult item.
        let mut demux = SubagentDemux::default();
        let msg = json!({
            "type": "user",
            "parent_tool_use_id": null,
            "message": { "content": [{
                "type": "tool_result",
                "tool_use_id": "tu-bash",
                "content": "stdout",
                "is_error": false
            }]}
        });
        let events = translate_sdk_message_with(&tid(), &msg, &mut demux);
        match &events[0] {
            ProviderRuntimeEvent::ItemCompleted { subagent_id, item, .. } => {
                assert!(subagent_id.is_none());
                assert!(matches!(item, CompletedItem::ToolResult { .. }));
            }
            other => panic!("expected untagged ItemCompleted, got {other:?}"),
        }
    }

    #[test]
    fn task_event_without_resolvable_id_warns_not_drops() {
        // task_started with no tool_use_id and no prior mapping — surface
        // a warning so drift is visible (never silently dropped).
        let mut demux = SubagentDemux::default();
        let started = json!({
            "type": "system", "subtype": "task_started",
            "task_id": "orphan", "description": "no id"
        });
        let events = translate_sdk_message_with(&tid(), &started, &mut demux);
        assert!(matches!(
            &events[0],
            ProviderRuntimeEvent::RuntimeWarning { .. }
        ));
    }

    #[test]
    fn ambient_skip_transcript_task_started_is_dropped() {
        let mut demux = SubagentDemux::default();
        let started = json!({
            "type": "system", "subtype": "task_started",
            "task_id": "amb", "tool_use_id": "toolu_x",
            "description": "housekeeping", "skip_transcript": true
        });
        let events = translate_sdk_message_with(&tid(), &started, &mut demux);
        assert!(events.is_empty(), "skip_transcript tasks must not spawn a card");
    }

    #[test]
    fn top_level_agent_tool_result_without_structured_output_completes() {
        // Defensive: a subagent tool_result lacking `tool_use_result`
        // still completes (honouring the error flag) rather than leaking
        // a raw ToolResult item.
        let mut demux = SubagentDemux::default();
        translate_sdk_message_with(
            &tid(),
            &launch_msg("Agent", "toolu_root", json!({"subagent_type": "Explore"})),
            &mut demux,
        );
        let done = json!({
            "type": "user",
            "parent_tool_use_id": null,
            "message": { "content": [{
                "type": "tool_result",
                "tool_use_id": "toolu_root",
                "content": "err",
                "is_error": true
            }]}
        });
        let snap = only_subagent(&translate_sdk_message_with(&tid(), &done, &mut demux)).clone();
        assert_eq!(snap.status, SubagentStatus::Failed);
    }

    // =======================================================================
    // Workflow tool mapping
    // =======================================================================

    fn only_workflow(events: &[ProviderRuntimeEvent]) -> &WorkflowSnapshot {
        assert_eq!(events.len(), 1, "expected exactly one event: {events:?}");
        match &events[0] {
            ProviderRuntimeEvent::WorkflowUpdated { workflow, .. } => workflow,
            other => panic!("expected WorkflowUpdated, got {other:?}"),
        }
    }

    fn workflow_launch_msg(tool_use_id: &str, input: serde_json::Value) -> serde_json::Value {
        json!({
            "type": "assistant",
            "parent_tool_use_id": null,
            "turn_id": "turn-1",
            "message": { "content": [{
                "type": "tool_use",
                "id": tool_use_id,
                "name": "Workflow",
                "input": input
            }]}
        })
    }

    const SAMPLE_WORKFLOW_SCRIPT: &str = r#"
export const meta = {
  name: 'Bug Hunt',
  description: 'Find and fix bugs across the repo',
  phases: [
    { title: 'Explore', detail: 'scan the code for suspects' },
    { title: 'Fix', detail: 'apply the fixes' },
  ],
};

async function run() {
  // ... orchestration body ...
}
"#;

    #[test]
    fn workflow_launch_parses_meta_and_emits_running_snapshot_suppressing_tool_use() {
        let mut demux = SubagentDemux::default();
        let msg = workflow_launch_msg(
            "toolu_wf1",
            json!({"script": SAMPLE_WORKFLOW_SCRIPT}),
        );
        let events = translate_sdk_message_with(&tid(), &msg, &mut demux);
        // No generic ToolUse item — the workflow card replaces it.
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, ProviderRuntimeEvent::ItemCompleted { .. })),
            "generic ToolUse must be suppressed for a Workflow launch"
        );
        let wf = only_workflow(&events);
        assert_eq!(wf.workflow_id, "toolu_wf1");
        assert_eq!(wf.status, "running");
        assert_eq!(wf.name.as_deref(), Some("Bug Hunt"));
        assert_eq!(
            wf.description.as_deref(),
            Some("Find and fix bugs across the repo")
        );
        assert_eq!(wf.script.as_deref(), Some(SAMPLE_WORKFLOW_SCRIPT));
        let phases = wf.phases.as_ref().expect("phases parsed");
        assert_eq!(phases.len(), 2);
        assert_eq!(phases[0].title, "Explore");
        assert_eq!(phases[0].detail.as_deref(), Some("scan the code for suspects"));
        assert_eq!(phases[1].title, "Fix");
        assert_eq!(phases[1].detail.as_deref(), Some("apply the fixes"));
        // The demux now tracks this workflow as active.
        assert_eq!(demux.active_workflow.as_deref(), Some("toolu_wf1"));
    }

    #[test]
    fn workflow_launch_falls_back_to_input_name_when_meta_missing() {
        let mut demux = SubagentDemux::default();
        let msg = workflow_launch_msg(
            "toolu_wf2",
            json!({"script": "console.log('no meta here')", "name": "Saved Workflow"}),
        );
        let wf = only_workflow(&translate_sdk_message_with(&tid(), &msg, &mut demux)).clone();
        assert_eq!(wf.name.as_deref(), Some("Saved Workflow"));
        assert!(wf.phases.is_none());
        assert!(wf.description.is_none());
    }

    #[test]
    fn workflow_tool_result_completes_workflow_and_suppresses_tool_result() {
        let mut demux = SubagentDemux::default();
        translate_sdk_message_with(
            &tid(),
            &workflow_launch_msg("toolu_wf1", json!({"script": SAMPLE_WORKFLOW_SCRIPT})),
            &mut demux,
        );
        let done = json!({
            "type": "user",
            "parent_tool_use_id": null,
            "message": { "content": [{
                "type": "tool_result",
                "tool_use_id": "toolu_wf1",
                "content": "All phases completed successfully.",
                "is_error": false
            }]}
        });
        let events = translate_sdk_message_with(&tid(), &done, &mut demux);
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, ProviderRuntimeEvent::ItemCompleted { .. })),
            "raw ToolResult must be suppressed for a workflow completion"
        );
        let wf = only_workflow(&events);
        assert_eq!(wf.workflow_id, "toolu_wf1");
        assert_eq!(wf.status, "completed");
        assert_eq!(
            wf.result_text.as_deref(),
            Some("All phases completed successfully.")
        );
        // The demux clears the active workflow on completion.
        assert!(demux.active_workflow.is_none());
    }

    #[test]
    fn workflow_tool_result_error_marks_failed() {
        let mut demux = SubagentDemux::default();
        translate_sdk_message_with(
            &tid(),
            &workflow_launch_msg("toolu_wf1", json!({"script": SAMPLE_WORKFLOW_SCRIPT})),
            &mut demux,
        );
        let failed = json!({
            "type": "user",
            "parent_tool_use_id": null,
            "message": { "content": [{
                "type": "tool_result",
                "tool_use_id": "toolu_wf1",
                "content": "boom",
                "is_error": true
            }]}
        });
        let wf = only_workflow(&translate_sdk_message_with(&tid(), &failed, &mut demux)).clone();
        assert_eq!(wf.status, "failed");
        assert_eq!(wf.result_text.as_deref(), Some("boom"));
    }

    #[test]
    fn workflow_result_text_is_truncated_to_4000_chars() {
        let mut demux = SubagentDemux::default();
        translate_sdk_message_with(
            &tid(),
            &workflow_launch_msg("toolu_wf1", json!({"script": SAMPLE_WORKFLOW_SCRIPT})),
            &mut demux,
        );
        let long_text = "x".repeat(5000);
        let done = json!({
            "type": "user",
            "parent_tool_use_id": null,
            "message": { "content": [{
                "type": "tool_result",
                "tool_use_id": "toolu_wf1",
                "content": long_text,
                "is_error": false
            }]}
        });
        let wf = only_workflow(&translate_sdk_message_with(&tid(), &done, &mut demux)).clone();
        let result = wf.result_text.expect("result text present");
        // 4000 chars + the truncation ellipsis marker.
        assert_eq!(result.chars().count(), 4001);
        assert!(result.ends_with('…'));
    }

    #[test]
    fn subagent_task_progress_during_active_workflow_gets_workflow_id_and_phase() {
        let mut demux = SubagentDemux::default();
        translate_sdk_message_with(
            &tid(),
            &workflow_launch_msg("toolu_wf1", json!({"script": SAMPLE_WORKFLOW_SCRIPT})),
            &mut demux,
        );
        translate_sdk_message_with(
            &tid(),
            &launch_msg("Agent", "toolu_root", json!({"subagent_type": "Explore"})),
            &mut demux,
        );
        let progress = json!({
            "type": "system", "subtype": "task_progress",
            "task_id": "task_1", "tool_use_id": "toolu_root",
            "usage": {"total_tokens": 10, "tool_uses": 1, "duration_ms": 20},
            "summary": "scanning [phase:Explore] for bugs"
        });
        let snap = only_subagent(&translate_sdk_message_with(&tid(), &progress, &mut demux)).clone();
        assert_eq!(snap.workflow_id.as_deref(), Some("toolu_wf1"));
        assert_eq!(snap.phase.as_deref(), Some("Explore"));
    }

    #[test]
    fn subagent_events_without_active_workflow_have_no_workflow_attribution() {
        // Regression guard: outside a workflow run, subagent snapshots
        // must be byte-identical to pre-workflow behavior (no
        // `workflow_id` / `phase` stamped).
        let mut demux = SubagentDemux::default();
        translate_sdk_message_with(
            &tid(),
            &launch_msg("Agent", "toolu_root", json!({"subagent_type": "Explore"})),
            &mut demux,
        );
        let progress = json!({
            "type": "system", "subtype": "task_progress",
            "task_id": "task_1", "tool_use_id": "toolu_root",
            "usage": {"total_tokens": 10, "tool_uses": 1, "duration_ms": 20},
            "summary": "scanning phase:Explore for bugs"
        });
        let snap = only_subagent(&translate_sdk_message_with(&tid(), &progress, &mut demux)).clone();
        assert!(snap.workflow_id.is_none());
        assert!(snap.phase.is_none());
    }

    #[test]
    fn workflow_launch_top_level_agent_gets_workflow_id_immediately() {
        // The initial Agent-launch snapshot (before any task_* event)
        // should already carry the active workflow's id.
        let mut demux = SubagentDemux::default();
        translate_sdk_message_with(
            &tid(),
            &workflow_launch_msg("toolu_wf1", json!({"script": SAMPLE_WORKFLOW_SCRIPT})),
            &mut demux,
        );
        let snap = only_subagent(&translate_sdk_message_with(
            &tid(),
            &launch_msg("Agent", "toolu_root", json!({"subagent_type": "Explore"})),
            &mut demux,
        ))
        .clone();
        assert_eq!(snap.workflow_id.as_deref(), Some("toolu_wf1"));
    }

    // ── Context-window usage ──
    //
    // The meter's numerator is *live occupancy*: fresh input + both
    // cache tiers + output of the latest report. Cache reads count —
    // they are prompt content the model sees, not free context.

    /// Extract the single `ContextUsageUpdated` snapshot from a batch,
    /// asserting exactly one was emitted.
    fn only_usage(events: &[ProviderRuntimeEvent]) -> ContextUsageSnapshot {
        let mut found = events.iter().filter_map(|e| match e {
            ProviderRuntimeEvent::ContextUsageUpdated { usage, .. } => Some(usage.clone()),
            _ => None,
        });
        let snap = found.next().expect("expected a ContextUsageUpdated");
        assert!(found.next().is_none(), "expected exactly one snapshot");
        snap
    }

    fn usage_snapshots(events: &[ProviderRuntimeEvent]) -> Vec<ContextUsageSnapshot> {
        events
            .iter()
            .filter_map(|e| match e {
                ProviderRuntimeEvent::ContextUsageUpdated { usage, .. } => Some(usage.clone()),
                _ => None,
            })
            .collect()
    }

    fn assistant_with_usage(usage: serde_json::Value) -> serde_json::Value {
        json!({
            "type": "assistant",
            "turn_id": "turn-1",
            "message": {
                "content": [{"type": "text", "text": "hi"}],
                "usage": usage
            }
        })
    }

    fn result_msg(usage: serde_json::Value, model_usage: serde_json::Value) -> serde_json::Value {
        json!({
            "type": "result",
            "subtype": "success",
            "turn_id": "turn-1",
            "duration_ms": 1000,
            "num_turns": 1,
            "usage": usage,
            "modelUsage": model_usage
        })
    }

    #[test]
    fn assistant_usage_sums_all_four_token_classes() {
        let msg = assistant_with_usage(json!({
            "input_tokens": 4,
            "cache_creation_input_tokens": 2715,
            "cache_read_input_tokens": 21144,
            "output_tokens": 679
        }));
        let events = translate_sdk_message(&tid(), &msg);
        let snap = only_usage(&events);
        assert_eq!(snap.used_tokens, 24_542);
        // No window reported yet, and nothing accumulated.
        assert!(snap.max_tokens.is_none());
        assert!(snap.total_processed_tokens.is_none());
        assert_eq!(snap.compacts_automatically, Some(true));
        // The transcript item still lands alongside the snapshot.
        assert!(events
            .iter()
            .any(|e| matches!(e, ProviderRuntimeEvent::ItemCompleted { .. })));
    }

    #[test]
    fn assistant_without_usage_block_emits_no_snapshot() {
        let events = translate_sdk_message(&tid(), &assistant_text("hi"));
        assert!(usage_snapshots(&events).is_empty());
    }

    #[test]
    fn zero_usage_emits_no_snapshot() {
        let msg = assistant_with_usage(json!({"input_tokens": 0, "output_tokens": 0}));
        let events = translate_sdk_message(&tid(), &msg);
        assert!(usage_snapshots(&events).is_empty());
    }

    #[test]
    fn repeated_identical_usage_is_suppressed() {
        let mut demux = SubagentDemux::default();
        let msg = assistant_with_usage(json!({"input_tokens": 100, "output_tokens": 20}));
        let first = translate_sdk_message_with(&tid(), &msg, &mut demux);
        assert_eq!(only_usage(&first).used_tokens, 120);
        let second = translate_sdk_message_with(&tid(), &msg, &mut demux);
        assert!(
            usage_snapshots(&second).is_empty(),
            "an identical repeat snapshot must not re-emit"
        );
    }

    #[test]
    fn subagent_assistant_usage_does_not_feed_the_parent_meter() {
        // Usage hygiene: a subagent burns its OWN context window, so
        // its tokens must never move the parent's meter.
        let mut demux = SubagentDemux::default();
        translate_sdk_message_with(
            &tid(),
            &launch_msg("Agent", "toolu_root", json!({"subagent_type": "Explore"})),
            &mut demux,
        );
        let inner = json!({
            "type": "assistant",
            "parent_tool_use_id": "toolu_root",
            "turn_id": "turn-1",
            "message": {
                "content": [{"type": "text", "text": "working"}],
                "usage": {"input_tokens": 500_000, "output_tokens": 1000}
            }
        });
        let events = translate_sdk_message_with(&tid(), &inner, &mut demux);
        assert!(
            usage_snapshots(&events).is_empty(),
            "subagent usage must not emit a parent context snapshot"
        );
    }

    #[test]
    fn result_model_usage_supplies_the_context_window() {
        let mut demux = SubagentDemux::default();
        translate_sdk_message_with(
            &tid(),
            &assistant_with_usage(json!({"input_tokens": 30_000, "output_tokens": 500})),
            &mut demux,
        );
        let result = result_msg(
            json!({"input_tokens": 30_000, "output_tokens": 500}),
            json!({"claude-opus-4-8": {"contextWindow": 200_000, "inputTokens": 30_000}}),
        );
        let events = translate_sdk_message_with(&tid(), &result, &mut demux);
        let snap = only_usage(&events);
        assert_eq!(snap.max_tokens, Some(200_000));
        // Live occupancy is the assistant reading, not the turn total.
        assert_eq!(snap.used_tokens, 30_500);
        // The turn settles after the meter is already correct.
        assert!(matches!(
            events.last(),
            Some(ProviderRuntimeEvent::TurnCompleted { .. })
        ));
    }

    #[test]
    fn result_takes_the_largest_window_across_models() {
        let mut demux = SubagentDemux::default();
        let result = result_msg(
            json!({"input_tokens": 10, "output_tokens": 5}),
            json!({
                "claude-haiku-4-5": {"contextWindow": 200_000},
                "claude-opus-4-8": {"contextWindow": 1_000_000}
            }),
        );
        let events = translate_sdk_message_with(&tid(), &result, &mut demux);
        assert_eq!(only_usage(&events).max_tokens, Some(1_000_000));
    }

    #[test]
    fn used_is_clamped_to_the_window_while_the_lifetime_total_stays_real() {
        let mut demux = SubagentDemux::default();
        // A reading that overshoots the window (provider drift, or a
        // window that shrank mid-session) must never render >100%.
        translate_sdk_message_with(
            &tid(),
            &assistant_with_usage(json!({"input_tokens": 240_000, "output_tokens": 10_000})),
            &mut demux,
        );
        let result = result_msg(
            json!({"input_tokens": 240_000, "output_tokens": 10_000, "total_tokens": 250_000}),
            json!({"claude-opus-4-8": {"contextWindow": 200_000}}),
        );
        let events = translate_sdk_message_with(&tid(), &result, &mut demux);
        let snap = only_usage(&events);
        assert_eq!(snap.used_tokens, 200_000, "clamped to the window");
        assert_eq!(
            snap.total_processed_tokens,
            Some(250_000),
            "the lifetime figure keeps the real number"
        );
    }

    #[test]
    fn lifetime_total_accumulates_once_per_turn_and_only_when_above_used() {
        let mut demux = SubagentDemux::default();
        let usage = json!({"input_tokens": 1_000, "output_tokens": 200});
        // Turn 1: the assistant reading is the whole story. At the
        // result the lifetime total merely equals the live occupancy,
        // so it is omitted — leaving a snapshot identical to the one
        // already emitted, which the no-op suppression drops entirely.
        let opening =
            translate_sdk_message_with(&tid(), &assistant_with_usage(usage.clone()), &mut demux);
        let opening = only_usage(&opening);
        assert_eq!(opening.used_tokens, 1_200);
        assert!(opening.total_processed_tokens.is_none());
        let first = translate_sdk_message_with(
            &tid(),
            &result_msg(usage.clone(), json!({})),
            &mut demux,
        );
        assert!(
            usage_snapshots(&first).is_empty(),
            "a result that changes nothing must not re-emit"
        );
        // Turn 2: the same live occupancy repeats (still suppressed),
        // but the second turn's tokens push the lifetime figure past
        // it — so the result snapshot now carries a real total.
        let repeat =
            translate_sdk_message_with(&tid(), &assistant_with_usage(usage.clone()), &mut demux);
        assert!(usage_snapshots(&repeat).is_empty());
        let second =
            translate_sdk_message_with(&tid(), &result_msg(usage, json!({})), &mut demux);
        assert_eq!(
            only_usage(&second).total_processed_tokens,
            Some(2_400),
            "each turn contributes exactly once"
        );
    }

    #[test]
    fn result_explicit_total_wins_over_the_summed_breakdown() {
        let mut demux = SubagentDemux::default();
        translate_sdk_message_with(
            &tid(),
            &assistant_with_usage(json!({"input_tokens": 100, "output_tokens": 10})),
            &mut demux,
        );
        // The breakdown sums to 110 but the provider states 9_000.
        let result = result_msg(
            json!({"input_tokens": 100, "output_tokens": 10, "total_tokens": 9_000}),
            json!({}),
        );
        let events = translate_sdk_message_with(&tid(), &result, &mut demux);
        assert_eq!(only_usage(&events).total_processed_tokens, Some(9_000));
    }

    #[test]
    fn result_without_a_prior_assistant_reading_falls_back_to_the_turn_figure() {
        let mut demux = SubagentDemux::default();
        let events = translate_sdk_message_with(
            &tid(),
            &result_msg(json!({"input_tokens": 800, "output_tokens": 200}), json!({})),
            &mut demux,
        );
        assert_eq!(only_usage(&events).used_tokens, 1_000);
    }

    #[test]
    fn result_without_usage_still_delivers_a_window_learned_from_model_usage() {
        let mut demux = SubagentDemux::default();
        translate_sdk_message_with(
            &tid(),
            &assistant_with_usage(json!({"input_tokens": 5_000, "output_tokens": 100})),
            &mut demux,
        );
        // No `usage` block, but `modelUsage` states the window — the
        // meter must still learn its denominator.
        let result = json!({
            "type": "result",
            "subtype": "success",
            "turn_id": "turn-1",
            "duration_ms": 10,
            "num_turns": 1,
            "modelUsage": {"claude-opus-4-8": {"contextWindow": 200_000}}
        });
        let events = translate_sdk_message_with(&tid(), &result, &mut demux);
        let snap = only_usage(&events);
        assert_eq!(snap.max_tokens, Some(200_000));
        assert_eq!(snap.used_tokens, 5_100);
    }

    #[test]
    fn compact_boundary_drops_used_and_records_the_pre_compaction_mark() {
        let mut demux = SubagentDemux::default();
        translate_sdk_message_with(
            &tid(),
            &assistant_with_usage(json!({"input_tokens": 150_000, "output_tokens": 5_000})),
            &mut demux,
        );
        translate_sdk_message_with(
            &tid(),
            &result_msg(
                json!({"input_tokens": 150_000, "output_tokens": 5_000, "total_tokens": 400_000}),
                json!({"claude-opus-4-8": {"contextWindow": 200_000}}),
            ),
            &mut demux,
        );
        let boundary = json!({
            "type": "system",
            "subtype": "compact_boundary",
            "compact_metadata": {"pre_tokens": 155_000, "post_tokens": 30_000, "trigger": "auto"}
        });
        let events = translate_sdk_message_with(&tid(), &boundary, &mut demux);
        let snap = only_usage(&events);
        assert_eq!(snap.used_tokens, 30_000, "post-compaction occupancy");
        assert_eq!(
            snap.last_used_tokens,
            Some(155_000),
            "pre-compaction high-water mark is preserved"
        );
        assert_eq!(
            snap.total_processed_tokens,
            Some(400_000),
            "the lifetime total carries forward across the compaction"
        );
        assert_eq!(snap.max_tokens, Some(200_000), "the window stays known");
        // The raw system event is still surfaced for existing consumers.
        assert!(events
            .iter()
            .any(|e| matches!(e, ProviderRuntimeEvent::RuntimeWarning { .. })));
    }

    #[test]
    fn compact_boundary_without_post_tokens_emits_no_snapshot() {
        // An unknown post-compaction occupancy is worse than none — the
        // previous (stale but real) reading stands.
        let mut demux = SubagentDemux::default();
        let boundary = json!({
            "type": "system",
            "subtype": "compact_boundary",
            "compact_metadata": {"pre_tokens": 155_000}
        });
        let events = translate_sdk_message_with(&tid(), &boundary, &mut demux);
        assert!(usage_snapshots(&events).is_empty());
        assert!(events
            .iter()
            .any(|e| matches!(e, ProviderRuntimeEvent::RuntimeWarning { .. })));
    }

    #[test]
    fn compact_boundary_accepts_camel_case_metadata() {
        // Wire-drift tolerance: both casings decode.
        let mut demux = SubagentDemux::default();
        let boundary = json!({
            "type": "system",
            "subtype": "compact_boundary",
            "compact_metadata": {"preTokens": 90_000, "postTokens": 12_000}
        });
        let events = translate_sdk_message_with(&tid(), &boundary, &mut demux);
        let snap = only_usage(&events);
        assert_eq!(snap.used_tokens, 12_000);
        assert_eq!(snap.last_used_tokens, Some(90_000));
    }

    #[test]
    fn todo_write_emits_complete_tasks_snapshot() {
        let message = json!({
            "type": "assistant",
            "message": {"id": "turn-1", "content": [{
                "type": "tool_use",
                "id": "todo-1",
                "name": "TodoWrite",
                "input": {"todos": [
                    {"content": "Research", "status": "completed"},
                    {"content": "Implement", "status": "in_progress", "activeForm": "Implementing"}
                ]}
            }]}
        });
        let events = translate_sdk_message(&tid(), &message);
        let tasks = events.iter().find_map(|event| match event {
            ProviderRuntimeEvent::TasksUpdated { tasks, .. } => Some(tasks),
            _ => None,
        }).expect("tasks snapshot");
        assert_eq!(tasks.tasks.len(), 2);
        assert_eq!(tasks.tasks[1].status, TaskStatus::InProgress);
        assert_eq!(tasks.tasks[1].detail.as_deref(), Some("Implementing"));
    }

    #[test]
    fn modern_task_create_result_updates_snapshot() {
        let mut state = SubagentDemux::default();
        let use_message = json!({
            "type": "assistant",
            "message": {"id": "turn-1", "content": [{
                "type": "tool_use", "id": "create-1", "name": "TaskCreate",
                "input": {"subject": "Test the panel", "activeForm": "Testing"}
            }]}
        });
        translate_sdk_message_with(&tid(), &use_message, &mut state);
        let result_message = json!({
            "type": "user",
            "message": {"id": "turn-1", "content": [{
                "type": "tool_result", "tool_use_id": "create-1", "content": "created"
            }]},
            "tool_use_result": {"task": {"id": "42", "subject": "Test the panel", "status": "pending"}}
        });
        let events = translate_sdk_message_with(&tid(), &result_message, &mut state);
        let tasks = events.iter().find_map(|event| match event {
            ProviderRuntimeEvent::TasksUpdated { tasks, .. } => Some(tasks),
            _ => None,
        }).expect("tasks snapshot");
        assert_eq!(tasks.tasks[0].task_id, "42");
        assert_eq!(tasks.tasks[0].title, "Test the panel");
    }
}
