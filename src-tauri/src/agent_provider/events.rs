//! Canonical provider event stream.
//!
//! Every concrete [`AgentProvider`](super::AgentProvider) implementation
//! translates its upstream message format (Claude SDK messages, Codex
//! app-server notifications, etc.) into this tagged enum. The orchestration
//! engine stays provider-agnostic by consuming only these events.
//!
//! Unknown upstream messages are always surfaced as
//! [`ProviderRuntimeEvent::RuntimeWarning`] rather than silently dropped, so
//! adapter drift never goes unnoticed.

use serde::{Deserialize, Serialize};

use super::types::{ApprovalDecision, ProviderSessionId, RequestId, SessionStatus, ThreadId, TurnId};

/// Lifecycle status of a subagent, as merged into the frontend's
/// per-subagent state.
///
/// Serialised in `snake_case` (`pending` | `running` | `completed` |
/// `failed` | `stopped`) so the TypeScript mirror matches. `Default` is
/// `Pending` so a partially-populated snapshot deserialised from an old
/// payload still yields a valid status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubagentStatus {
    /// Spawn observed but the subagent has not begun working yet.
    #[default]
    Pending,
    /// Subagent is actively working (streaming its own transcript).
    Running,
    /// Subagent finished and reported a result.
    Completed,
    /// Subagent terminated with an error.
    Failed,
    /// Subagent was interrupted / stopped before completing.
    Stopped,
}

/// Why a response to a provider request could not be delivered.
///
/// Kept separate from [`ApprovalDecision`]: a failed response is not a
/// denial/cancellation by the user, and must never be rendered as though the
/// provider received their decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestResponseFailureReason {
    /// Conversation history was resumed, but the provider's process-local
    /// callback for this request no longer exists.
    StaleProviderCallback,
}

/// A merge-able state snapshot for one subagent.
///
/// Providers dribble a subagent's identity out across many events (its
/// name may arrive before its model, its token totals only on
/// completion), so every field except the stable `subagent_id` key and
/// the `status` is optional. The frontend merges non-`None` fields into
/// its per-subagent state; a later snapshot never clobbers an
/// already-known field with `None`.
///
/// Serde is additive — every optional field carries `#[serde(default)]`
/// so a snapshot persisted by an older build (missing fields added
/// later) still deserialises.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SubagentSnapshot {
    /// Stable demux key. Provider-specific: Claude uses the spawning
    /// `parent_tool_use_id`, Codex the child `threadId`, OpenCode the
    /// child `sessionID`.
    #[serde(default)]
    pub subagent_id: String,
    /// The tool_use / tool_call id that spawned this subagent, when the
    /// provider exposes it separately from the demux key.
    #[serde(default)]
    pub parent_item_id: Option<String>,
    /// Display name — "Explore", a nickname, or the agent type.
    #[serde(default)]
    pub name: Option<String>,
    /// `subagent_type` / role / agent slug.
    #[serde(default)]
    pub agent_type: Option<String>,
    /// Model the subagent runs on, when reported.
    #[serde(default)]
    pub model: Option<String>,
    /// Current lifecycle status.
    #[serde(default)]
    pub status: SubagentStatus,
    /// Live "currently doing X" line (provider-pushed summary or latest
    /// child tool call).
    #[serde(default)]
    pub activity: Option<String>,
    /// Final report text, populated on completion.
    #[serde(default)]
    pub result_text: Option<String>,
    /// Number of tool calls the subagent has made so far.
    #[serde(default)]
    pub tool_use_count: Option<u64>,
    /// Total tokens the subagent has consumed so far.
    #[serde(default)]
    pub total_tokens: Option<u64>,
    /// Elapsed wall-clock time for the subagent in milliseconds.
    #[serde(default)]
    pub duration_ms: Option<u64>,
    /// Provider-native identifier (Codex threadId, OpenCode sessionID,
    /// Claude agentId). Distinct from `subagent_id` because some
    /// providers only learn it on completion.
    #[serde(default)]
    pub provider_ref: Option<String>,
    /// When this subagent was spawned while a `Workflow` tool run was
    /// active, the workflow's `workflow_id` (its spawning tool_use id).
    /// Lets the frontend route the snapshot into the workflow's phase
    /// view instead of the generic subagent-run card. `None` for
    /// subagents spawned outside a workflow.
    #[serde(default)]
    pub workflow_id: Option<String>,
    /// Best-effort phase label for this subagent within its workflow,
    /// when the provider's task label carries a `phase:X` hint. `None`
    /// when no hint was found (the frontend falls back to the workflow's
    /// last planned phase title).
    #[serde(default)]
    pub phase: Option<String>,
}

/// One planned phase of a `Workflow` run, parsed best-effort out of the
/// script's `export const meta = { phases: [...] }` literal.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct WorkflowPhaseSnapshot {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub detail: Option<String>,
}

/// A merge-able state snapshot for one `Workflow` tool run.
///
/// Mirrors [`SubagentSnapshot`]'s additive-serde discipline: every field
/// but `workflow_id` carries `#[serde(default)]` so old/partial payloads
/// still deserialise, and the frontend merges non-`None` fields into its
/// per-workflow view state.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct WorkflowSnapshot {
    /// Stable key — the `tool_use_id` of the `Workflow` tool call.
    #[serde(default)]
    pub workflow_id: String,
    /// `"pending_approval" | "running" | "completed" | "failed" | "stopped"`.
    #[serde(default)]
    pub status: String,
    /// Workflow name — parsed from `meta.name`, falling back to a saved
    /// workflow's `input.name` when the script doesn't carry one.
    #[serde(default)]
    pub name: Option<String>,
    /// `meta.description`, when present.
    #[serde(default)]
    pub description: Option<String>,
    /// The raw script text (`input.script`), for the "view source" affordance.
    #[serde(default)]
    pub script: Option<String>,
    /// Planned phases parsed from `meta.phases`, in script order.
    #[serde(default)]
    pub phases: Option<Vec<WorkflowPhaseSnapshot>>,
    /// Final result text on completion/failure, truncated to a sane length.
    #[serde(default)]
    pub result_text: Option<String>,
    #[serde(default)]
    pub total_tokens: Option<u64>,
    #[serde(default)]
    pub agent_count: Option<u32>,
    #[serde(default)]
    pub duration_ms: Option<u64>,
}

/// A streamed fragment produced mid-turn.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ContentDelta {
    /// A chunk of assistant visible text.
    Text { text: String },
    /// A chunk of assistant reasoning / thinking content.
    Thinking { text: String },
    /// Partial JSON for a tool invocation that is still streaming in.
    ToolInput {
        tool_name: String,
        partial_json: String,
    },
}

/// A completed, committed item inside a turn.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CompletedItem {
    /// Finalised assistant text message.
    AssistantText { text: String },
    /// Finalised assistant reasoning block.
    AssistantThinking { text: String },
    /// An assistant tool use has fully materialised.
    ToolUse {
        tool_name: String,
        input: serde_json::Value,
        tool_use_id: String,
    },
    /// A tool result has been observed.
    ToolResult {
        tool_use_id: String,
        content: serde_json::Value,
        is_error: bool,
    },
}

/// Why a turn ended.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TurnStatus {
    /// Turn completed normally.
    Success,
    /// Turn failed with a provider-specific error subtype.
    Error { subtype: String, message: String },
    /// Turn hit the configured max turns cap.
    MaxTurns,
    /// Turn hit the configured cost / budget cap.
    MaxBudget,
}

/// Usage statistics reported at the end of a turn.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnUsage {
    /// Total billable cost in USD, when the provider reports it.
    pub total_cost_usd: Option<f64>,
    /// Elapsed wall-clock time for the turn in milliseconds.
    pub duration_ms: u64,
    /// Number of internal model turns the provider ran to fulfil this
    /// user-level turn.
    pub num_turns: u32,
}

/// The canonical event stream produced by every provider.
///
/// Downstream consumers pattern-match on the top-level tag. New variants are
/// added by bumping this enum — never by inventing out-of-band channels — so
/// wire compatibility is tracked in one place.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProviderRuntimeEvent {
    /// The provider has fully configured the session and assigned its own
    /// internal identifier.
    SessionConfigured {
        thread_id: ThreadId,
        provider_session_id: ProviderSessionId,
    },
    /// A streaming delta landed mid-turn.
    ContentDelta {
        thread_id: ThreadId,
        turn_id: TurnId,
        delta: ContentDelta,
        /// When set, this delta belongs to a subagent's sub-transcript
        /// rather than the parent thread. The value is the subagent's
        /// stable demux key (see [`SubagentSnapshot::subagent_id`]).
        /// `#[serde(default)]` so payloads persisted before subagents
        /// existed still deserialise.
        #[serde(default)]
        subagent_id: Option<String>,
    },
    /// A complete, committed item landed inside a turn.
    ItemCompleted {
        thread_id: ThreadId,
        turn_id: TurnId,
        item: CompletedItem,
        /// When set, this item belongs to a subagent's sub-transcript.
        /// The drill-in view reuses [`CompletedItem`] unchanged, so the
        /// same renderers serve both the parent transcript and every
        /// child transcript. `#[serde(default)]` keeps old payloads
        /// deserialising.
        #[serde(default)]
        subagent_id: Option<String>,
    },
    /// A subagent's merge-able state snapshot changed (spawned,
    /// progressed, or completed). Emitted thread-scoped under the parent
    /// `thread_id`; the `subagent.subagent_id` inside is the demux key.
    SubagentUpdated {
        thread_id: ThreadId,
        subagent: SubagentSnapshot,
    },
    /// A `Workflow` tool run's merge-able state snapshot changed
    /// (launched, or terminated). Emitted thread-scoped under the parent
    /// `thread_id`; `workflow.workflow_id` inside is the demux key (the
    /// `Workflow` tool call's `tool_use_id`).
    WorkflowUpdated {
        thread_id: ThreadId,
        workflow: WorkflowSnapshot,
    },
    /// The turn finished — either successfully or with a terminal error.
    TurnCompleted {
        thread_id: ThreadId,
        turn_id: TurnId,
        status: TurnStatus,
        usage: Option<TurnUsage>,
    },
    /// The provider is asking the user to approve (or deny) something —
    /// typically a tool call or file change.
    RequestOpened {
        thread_id: ThreadId,
        turn_id: TurnId,
        request_id: RequestId,
        request_kind: String,
        payload: serde_json::Value,
        /// The provider's own tool-use identifier when this request maps to
        /// an in-flight tool invocation (Claude's `canUseTool` path). Lets
        /// the UI merge a permission request into its originating
        /// tool-call card. `None` for standalone requests (plan, user-input,
        /// Codex server-initiated requests not tied to a tool_use).
        #[serde(default)]
        tool_use_id: Option<String>,
        /// When set, this approval was raised by a subagent rather than
        /// the parent session. Lets the UI label it "from subagent X"
        /// and surface it inside the drill-in as well as the parent
        /// view. `#[serde(default)]` for old-payload compatibility.
        #[serde(default)]
        subagent_id: Option<String>,
    },
    /// The outstanding request was resolved with the user's decision.
    RequestResolved {
        thread_id: ThreadId,
        request_id: RequestId,
        decision: ApprovalDecision,
    },
    /// A response could not be delivered and the request is terminal from
    /// Codemux's perspective. Persisting this event prevents transcript
    /// hydration from resurrecting an orphaned approval/question forever.
    RequestResponseFailed {
        thread_id: ThreadId,
        request_id: RequestId,
        reason: RequestResponseFailureReason,
        message: String,
    },
    /// The session lifecycle phase changed (starting → ready → running …).
    SessionStateChanged {
        thread_id: ThreadId,
        status: SessionStatus,
    },
    /// A message the adapter could not translate into a canonical event.
    ///
    /// Providers surface these rather than dropping unknowns so orchestration
    /// can log them and surface adapter drift. `thread_id` is optional
    /// because some global warnings aren't session-bound.
    RuntimeWarning {
        thread_id: Option<ThreadId>,
        message: String,
        original_payload: Option<serde_json::Value>,
    },
    /// A resume cursor became available (or changed). Emitted when
    /// the provider observes its own internal session identifier for
    /// the first time, or at subsequent checkpoints where the cursor
    /// is refreshed. Consumers store this and pass it back in
    /// [`StartSessionInput::resume_cursor`] on a later restart.
    ResumeCursorUpdated {
        thread_id: ThreadId,
        resume_cursor: serde_json::Value,
    },
    /// A user turn arrived while a turn was already in flight, so it was
    /// **queued** instead of rejected. The UI renders it greyed-out in
    /// the transcript until it dispatches or is cancelled.
    ///
    /// `client_nonce` echoes the optimistic-send correlation token the
    /// frontend attached to [`SendTurnInput`](super::types::SendTurnInput),
    /// so the reducer can reconcile this event with the already-appended
    /// optimistic bubble instead of duplicating it. `text` is the raw
    /// turn text so a remounted pane (which never saw the optimistic
    /// append) can still reconstruct the queued bubble.
    TurnQueued {
        thread_id: ThreadId,
        queued_id: String,
        #[serde(default)]
        client_nonce: Option<String>,
        text: String,
    },
    /// A previously-queued turn has now been dispatched as the active
    /// turn. Carries the freshly-minted `turn_id` and the original
    /// `text` so the command layer can persist the user-message envelope
    /// at real turn order (queued turns are NOT persisted at enqueue
    /// time). The UI promotes the greyed bubble to a normal user message.
    QueuedTurnDispatched {
        thread_id: ThreadId,
        queued_id: String,
        turn_id: TurnId,
        text: String,
    },
    /// A queued turn was cancelled — either by the user (X button) or
    /// because the session closed / errored with items still queued. The
    /// UI removes the greyed bubble.
    QueuedTurnCancelled {
        thread_id: ThreadId,
        queued_id: String,
    },
    /// Emitted by the stall watchdog when a mid-turn thread has produced no
    /// runtime events for longer than the stall threshold. Advisory only —
    /// the session is NOT killed; the UI renders an amber notice.
    ///
    /// Transient by design: never persisted (a dead run has no durable
    /// "stalled" fact — the durable record is the settled/interrupted turn)
    /// and re-emitted on every sweep tick while the silence continues so the
    /// surfaced duration stays fresh. The frontend clears it on the next
    /// real activity for the thread.
    RunStalled {
        thread_id: ThreadId,
        /// Seconds since the last observed runtime event for this thread.
        silent_for_secs: u64,
    },
}

/// Turn-error subtype stamped on the synthetic [`TurnStatus::Error`] a
/// provider watchdog emits when its child process dies mid-turn. The
/// frontend keys the "Run interrupted" / Continue affordance off this
/// exact string, so it must stay in lockstep with
/// `src/lib/agent-chat/reducer.ts`.
pub const CHILD_EXITED_SUBTYPE: &str = "child_exited";

/// Build the terminal runtime events a provider watchdog should emit when
/// its child process (sidecar / app-server / shared server) dies
/// unexpectedly mid-run.
///
/// Ordering is load-bearing: the synthetic [`TurnCompleted`] is emitted
/// **before** the [`SessionStateChanged::Error`] so the activity and
/// pane-status trackers see the in-flight turn *settle* while the thread
/// state is still intact; the `Error` event then tears tracking down. The
/// `TurnCompleted` is also persisted, so hydrate after a restart replays a
/// settled — not dangling — turn.
///
/// When no turn is in flight (`active_turn` is `None`) only the `Error`
/// event is produced: a child that dies while idle has no dangling turn to
/// settle, so emitting a synthetic completion would strand the frontend on
/// a spurious "interrupted" signal for a thread that was never mid-run.
///
/// [`TurnCompleted`]: ProviderRuntimeEvent::TurnCompleted
/// [`SessionStateChanged::Error`]: ProviderRuntimeEvent::SessionStateChanged
pub fn child_exit_events(
    thread_id: ThreadId,
    active_turn: Option<TurnId>,
    message: String,
) -> Vec<ProviderRuntimeEvent> {
    let mut events = Vec::with_capacity(2);
    if let Some(turn_id) = active_turn {
        events.push(ProviderRuntimeEvent::TurnCompleted {
            thread_id: thread_id.clone(),
            turn_id,
            status: TurnStatus::Error {
                subtype: CHILD_EXITED_SUBTYPE.to_string(),
                message: message.clone(),
            },
            usage: None,
        });
    }
    events.push(ProviderRuntimeEvent::SessionStateChanged {
        thread_id,
        status: SessionStatus::Error { message },
    });
    events
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ── SubagentStatus wire form ──

    #[test]
    fn subagent_status_serializes_snake_case() {
        assert_eq!(
            serde_json::to_value(SubagentStatus::Pending).unwrap(),
            json!("pending")
        );
        assert_eq!(
            serde_json::to_value(SubagentStatus::Running).unwrap(),
            json!("running")
        );
        assert_eq!(
            serde_json::to_value(SubagentStatus::Completed).unwrap(),
            json!("completed")
        );
        assert_eq!(
            serde_json::to_value(SubagentStatus::Failed).unwrap(),
            json!("failed")
        );
        assert_eq!(
            serde_json::to_value(SubagentStatus::Stopped).unwrap(),
            json!("stopped")
        );
    }

    #[test]
    fn subagent_status_default_is_pending() {
        assert_eq!(SubagentStatus::default(), SubagentStatus::Pending);
    }

    // ── SubagentSnapshot round-trip + additive-serde ──

    fn full_snapshot() -> SubagentSnapshot {
        SubagentSnapshot {
            subagent_id: "toolu_root".into(),
            parent_item_id: Some("toolu_root".into()),
            name: Some("Explore".into()),
            agent_type: Some("Explore".into()),
            model: Some("claude-sonnet-4".into()),
            status: SubagentStatus::Running,
            activity: Some("Reading files".into()),
            result_text: None,
            tool_use_count: Some(3),
            total_tokens: Some(1200),
            duration_ms: Some(5000),
            provider_ref: None,
            workflow_id: None,
            phase: None,
        }
    }

    #[test]
    fn subagent_snapshot_round_trips() {
        let snap = full_snapshot();
        let text = serde_json::to_string(&snap).unwrap();
        let back: SubagentSnapshot = serde_json::from_str(&text).unwrap();
        assert_eq!(snap, back);
    }

    #[test]
    fn subagent_snapshot_deserializes_with_only_id_and_status() {
        // A future (or minimal) payload that omits every optional field
        // must still deserialise, defaulting the optionals to None.
        let snap: SubagentSnapshot =
            serde_json::from_value(json!({"subagent_id": "s1", "status": "running"})).unwrap();
        assert_eq!(snap.subagent_id, "s1");
        assert_eq!(snap.status, SubagentStatus::Running);
        assert!(snap.name.is_none());
        assert!(snap.tool_use_count.is_none());
        assert!(snap.provider_ref.is_none());
    }

    #[test]
    fn subagent_snapshot_deserializes_empty_object_to_defaults() {
        // Maximally-additive: even `{}` deserialises (empty id, Pending).
        let snap: SubagentSnapshot = serde_json::from_value(json!({})).unwrap();
        assert_eq!(snap.subagent_id, "");
        assert_eq!(snap.status, SubagentStatus::Pending);
    }

    #[test]
    fn subagent_updated_event_tagged_shape() {
        let event = ProviderRuntimeEvent::SubagentUpdated {
            thread_id: ThreadId("t1".into()),
            subagent: full_snapshot(),
        };
        let v = serde_json::to_value(&event).unwrap();
        assert_eq!(v["type"], "subagent_updated");
        assert_eq!(v["thread_id"], "t1");
        assert_eq!(v["subagent"]["subagent_id"], "toolu_root");
        assert_eq!(v["subagent"]["status"], "running");
        // Round-trips back to a SubagentUpdated.
        let back: ProviderRuntimeEvent = serde_json::from_value(v).unwrap();
        assert!(matches!(back, ProviderRuntimeEvent::SubagentUpdated { .. }));
    }

    // ── Additive serde on the tagged variants ──
    //
    // The whole point of `#[serde(default)]` on `subagent_id`: payloads
    // persisted BEFORE subagents existed (no `subagent_id` key) must
    // still deserialise.

    #[test]
    fn old_item_completed_without_subagent_id_deserializes() {
        let old = json!({
            "type": "item_completed",
            "thread_id": "t1",
            "turn_id": "turn-1",
            "item": {"kind": "assistant_text", "text": "hi"}
        });
        let event: ProviderRuntimeEvent = serde_json::from_value(old).unwrap();
        match event {
            ProviderRuntimeEvent::ItemCompleted { subagent_id, .. } => {
                assert!(subagent_id.is_none());
            }
            other => panic!("expected ItemCompleted, got {other:?}"),
        }
    }

    #[test]
    fn new_item_completed_with_subagent_id_deserializes() {
        let new = json!({
            "type": "item_completed",
            "thread_id": "t1",
            "turn_id": "turn-1",
            "item": {"kind": "assistant_text", "text": "hi"},
            "subagent_id": "toolu_root"
        });
        let event: ProviderRuntimeEvent = serde_json::from_value(new).unwrap();
        match event {
            ProviderRuntimeEvent::ItemCompleted { subagent_id, .. } => {
                assert_eq!(subagent_id.as_deref(), Some("toolu_root"));
            }
            other => panic!("expected ItemCompleted, got {other:?}"),
        }
    }

    #[test]
    fn old_content_delta_without_subagent_id_deserializes() {
        let old = json!({
            "type": "content_delta",
            "thread_id": "t1",
            "turn_id": "turn-1",
            "delta": {"kind": "text", "text": "hi"}
        });
        let event: ProviderRuntimeEvent = serde_json::from_value(old).unwrap();
        match event {
            ProviderRuntimeEvent::ContentDelta { subagent_id, .. } => {
                assert!(subagent_id.is_none());
            }
            other => panic!("expected ContentDelta, got {other:?}"),
        }
    }

    // ── RunStalled wire form ──

    #[test]
    fn run_stalled_serializes_snake_case_tagged() {
        let event = ProviderRuntimeEvent::RunStalled {
            thread_id: ThreadId("t1".into()),
            silent_for_secs: 640,
        };
        let v = serde_json::to_value(&event).unwrap();
        assert_eq!(v["type"], "run_stalled");
        assert_eq!(v["thread_id"], "t1");
        assert_eq!(v["silent_for_secs"], 640);
        // Round-trips back to a RunStalled.
        let back: ProviderRuntimeEvent = serde_json::from_value(v).unwrap();
        match back {
            ProviderRuntimeEvent::RunStalled {
                thread_id,
                silent_for_secs,
            } => {
                assert_eq!(thread_id.0, "t1");
                assert_eq!(silent_for_secs, 640);
            }
            other => panic!("expected RunStalled, got {other:?}"),
        }
    }

    // ── child_exit_events ordering ──

    #[test]
    fn child_exit_events_settles_turn_then_errors() {
        let events = child_exit_events(
            ThreadId("t1".into()),
            Some(TurnId("turn-9".into())),
            "sidecar exited".into(),
        );
        assert_eq!(events.len(), 2);
        match &events[0] {
            ProviderRuntimeEvent::TurnCompleted {
                thread_id,
                turn_id,
                status,
                usage,
            } => {
                assert_eq!(thread_id.0, "t1");
                assert_eq!(turn_id.0, "turn-9");
                assert!(usage.is_none());
                match status {
                    TurnStatus::Error { subtype, message } => {
                        assert_eq!(subtype, CHILD_EXITED_SUBTYPE);
                        assert_eq!(message, "sidecar exited");
                    }
                    other => panic!("expected Error status, got {other:?}"),
                }
            }
            other => panic!("expected TurnCompleted first, got {other:?}"),
        }
        assert!(matches!(
            &events[1],
            ProviderRuntimeEvent::SessionStateChanged {
                status: SessionStatus::Error { .. },
                ..
            }
        ));
    }

    #[test]
    fn child_exit_events_without_active_turn_only_errors() {
        // A child that dies while idle has no dangling turn to settle, so
        // only the Error event is produced — no spurious TurnCompleted that
        // would strand the frontend on a false "interrupted" signal.
        let events = child_exit_events(ThreadId("t1".into()), None, "dead".into());
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            ProviderRuntimeEvent::SessionStateChanged {
                status: SessionStatus::Error { .. },
                ..
            }
        ));
    }

    #[test]
    fn old_request_opened_without_subagent_id_deserializes() {
        // Predates BOTH tool_use_id and subagent_id — both default None.
        let old = json!({
            "type": "request_opened",
            "thread_id": "t1",
            "turn_id": "turn-1",
            "request_id": "r1",
            "request_kind": "command",
            "payload": {"command": "ls"}
        });
        let event: ProviderRuntimeEvent = serde_json::from_value(old).unwrap();
        match event {
            ProviderRuntimeEvent::RequestOpened {
                tool_use_id,
                subagent_id,
                ..
            } => {
                assert!(tool_use_id.is_none());
                assert!(subagent_id.is_none());
            }
            other => panic!("expected RequestOpened, got {other:?}"),
        }
    }
}
