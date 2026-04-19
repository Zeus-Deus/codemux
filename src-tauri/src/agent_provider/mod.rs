//! Provider-agnostic trait and shared types for CLI-backed coding agents.
//!
//! This module is the seam between the chat runtime and any concrete agent
//! integration (Claude Code, Codex, …). It is currently scaffolding — no
//! user-visible behaviour is wired to it yet.
//!
//! See `docs/features/agent-chat.md` for the bigger picture.

pub mod claude;
pub mod codex;
pub mod errors;
pub mod events;
pub mod provider;
pub mod types;

pub use errors::{ProviderError, SerializableProviderError};
pub use events::{CompletedItem, ContentDelta, ProviderRuntimeEvent, TurnStatus, TurnUsage};
pub use provider::{AgentProvider, ProviderEventStream};
pub use types::{
    ApprovalDecision, ImageInput, ProviderCapabilities, ProviderKind, ProviderSession,
    ProviderSessionId, RequestId, SendTurnInput, SessionStatus, StartSessionInput, ThreadId,
    TurnId, TurnStartResult,
};
