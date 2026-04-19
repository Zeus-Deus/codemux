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
    },
    /// A complete, committed item landed inside a turn.
    ItemCompleted {
        thread_id: ThreadId,
        turn_id: TurnId,
        item: CompletedItem,
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
    },
    /// The outstanding request was resolved with the user's decision.
    RequestResolved {
        thread_id: ThreadId,
        request_id: RequestId,
        decision: ApprovalDecision,
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
}
