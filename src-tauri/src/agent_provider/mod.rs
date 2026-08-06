//! Provider-agnostic trait and shared types for CLI-backed coding agents.
//!
//! This module is the seam between the chat runtime and any concrete agent
//! integration (Claude Code, Codex, …). It is currently scaffolding — no
//! user-visible behaviour is wired to it yet.
//!
//! See `docs/features/agent-chat.md` for the bigger picture.

pub mod claude;
pub mod codex;
pub mod context_usage;
pub mod errors;
pub mod events;
pub mod instance;
pub mod opencode;
pub mod provider;
pub mod types;

pub use context_usage::ContextUsageTracker;
pub use errors::{ProviderError, SerializableProviderError};
pub use events::{
    child_exit_events, classify_task_kind, CompletedItem, ContentDelta, ContextUsageSnapshot,
    ProviderRuntimeEvent, RequestResponseFailureReason, SubagentSnapshot, SubagentStatus,
    SubagentTaskKind, TaskSnapshotItem, TaskStatus, TasksSnapshot, TurnStatus, TurnUsage,
    UserMessageImage, WorkflowPhaseSnapshot, WorkflowSnapshot, CHILD_EXITED_SUBTYPE,
    WATCH_LOOP_TASK_TYPES,
};
pub use instance::ProviderInstanceId;
pub use provider::{AgentProvider, ProviderEventStream};
pub use types::{
    ApprovalDecision, ChatModelInfo, ContextWindowOption, EffortGranularity, ImageInput,
    PermissionModeOption, ProviderCapabilities, ProviderChatCapabilities, ProviderKind,
    ProviderSession, ProviderSessionId, RequestId, SendOutcome, SendTurnInput, SessionStatus,
    StartSessionInput, ThreadId, TurnId, TurnStartResult,
};
