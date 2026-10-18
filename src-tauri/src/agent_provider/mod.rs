//! Provider-agnostic trait and shared types for CLI-backed coding agents.
//!
//! This module is the seam between the chat runtime and any concrete agent
//! integration (Claude Code, Codex, …). It is currently scaffolding — no
//! user-visible behaviour is wired to it yet.

pub mod acp;
pub mod claude;
pub mod codex;
pub mod questions;
pub use questions::{
    AnswerQuestionInput, QuestionDelivery, QuestionDeliveryError, QuestionResolution, UserQuestion,
    UserQuestionSet,
};
pub mod context_usage;
pub mod cursor;
pub mod custom_acp;
pub mod errors;
pub mod events;
pub mod grok;
pub mod health;
pub mod hermes;
pub mod instance;
pub mod managed;
pub mod managed_bridge;
pub mod opencode;
pub mod pricing;
pub mod provider;
pub mod types;
pub mod side_chat;
pub mod slash_commands;
pub mod usage_limit;

pub use context_usage::ContextUsageTracker;
pub use errors::{ProviderError, SerializableProviderError};
pub use events::{
    CacheWriteTtl, child_exit_events, classify_task_kind, CompletedItem, ContentDelta, ContextUsageSnapshot, CostSource,
    PlanAuthMode, PlanUsageWindow, PlanWindowKind, ProviderRuntimeEvent, RequestResponseFailureReason, SubagentSnapshot, SubagentStatus,
    SubagentTaskKind, TaskSnapshotItem, TaskStatus, TasksSnapshot, TurnStatus, TurnUsage,
    UserMessageImage, WorkflowPhaseSnapshot, WorkflowSnapshot, CHILD_EXITED_SUBTYPE,
    FIVE_HOUR_WINDOW_MINS, SEVEN_DAY_WINDOW_MINS, WATCH_LOOP_TASK_TYPES,
};
pub use health::{ProviderHealthReport, ProviderHealthStatus};
pub use instance::ProviderInstanceId;
pub use provider::{AgentProvider, ProviderEventStream};
pub use types::{
    ApprovalDecision, ChatModelInfo, ContextWindowOption, EffortGranularity, ImageInput,
    PermissionModeOption, ProviderCapabilities, ProviderChatCapabilities, ProviderKind,
    ProviderSession, ProviderSessionId, RequestId, SendOutcome, SendTurnInput, SessionStatus,
    StartSessionInput, ThreadId, ThreadRuntimeStatus, TurnDispatchCheckpoint, TurnId,
    TurnStartResult, UsageBaseline,
};

/// The MCP registry a session publishes tools from and runs tool calls
/// through. A session on a device gets none: whatever runs on the far end of
/// the SSH link must not be able to drive this computer's terminals, git or
/// MCP servers through Codemux's tools.
pub(crate) fn local_tool_registry(
    registry: Option<&crate::mcp::registry::McpRegistry>,
    remote: Option<&types::RemoteSpawnTarget>,
) -> Option<crate::mcp::registry::McpRegistry> {
    match remote {
        Some(_) => None,
        None => registry.cloned(),
    }
}
