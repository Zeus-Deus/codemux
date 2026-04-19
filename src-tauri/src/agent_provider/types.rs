//! Shared types used by the [`AgentProvider`](crate::agent_provider::AgentProvider)
//! trait.
//!
//! These structs and newtypes are the lingua franca between the chat runtime
//! and any concrete provider implementation. The orchestration layer treats
//! them as opaque identifiers and values so new providers can slot in without
//! touching the engine.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// The set of CLI-backed coding agents the chat runtime can drive.
///
/// Serialized as lowercase strings (`"claude"`, `"codex"`) so values round-trip
/// cleanly through JSON settings and IPC surfaces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderKind {
    /// Claude Code via the `claude` CLI / Claude Agent SDK.
    Claude,
    /// Codex via the `codex app-server` JSON-RPC binary.
    Codex,
}

/// Capabilities a provider declares statically so the UI can enable or hide
/// controls without probing.
///
/// Defaults to all `false`; implementations flip the fields they actually
/// support.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ProviderCapabilities {
    /// Model can be swapped mid-session without reconnecting.
    pub supports_mid_session_model_change: bool,
    /// Permission mode (accept-edits, bypass, plan, ...) can be changed
    /// mid-session.
    pub supports_mid_session_permission_change: bool,
    /// Tool approvals can be granted or denied synchronously while a turn is
    /// in flight (vs. fire-and-forget allow-lists).
    pub supports_synchronous_tool_approval: bool,
    /// A running turn can be interrupted without tearing the session down.
    pub supports_interrupt: bool,
    /// A previously-closed session can be resumed from an opaque cursor.
    pub supports_session_resume: bool,
}

/// Opaque identifier a provider hands back for its own internal session.
///
/// Contents depend on the provider — a Claude SDK session UUID, a Codex
/// app-server `thread_id`, etc. The runtime stores this on the thread's
/// resume state but never interprets it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProviderSessionId(pub String);

/// Stable identifier for a chat thread, owned by the runtime.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ThreadId(pub String);

/// Identifier for a single assistant turn within a thread.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TurnId(pub String);

/// Identifier for a provider-side request (tool approval, file change, etc.)
/// that needs an out-of-band response from the user.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RequestId(pub String);

/// A user-supplied image attachment passed alongside a turn's text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageInput {
    /// Raw bytes of the image. The runtime is responsible for persisting or
    /// re-encoding these as the provider requires.
    pub data: Vec<u8>,
    /// Media type (e.g. `"image/png"`). Provider-specific validation happens
    /// inside the adapter.
    pub media_type: String,
}

/// Parameters for starting a brand-new or resumed provider session.
///
/// Providers that do not recognise a field ignore it; the runtime always
/// passes the full structure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartSessionInput {
    /// Runtime-owned thread identifier.
    pub thread_id: ThreadId,
    /// Working directory to hand the agent.
    pub cwd: PathBuf,
    /// Optional initial model identifier. Providers fall back to their
    /// default when absent.
    pub model: Option<String>,
    /// Opaque resume state previously returned by the provider. When
    /// present the provider should attempt to resume instead of starting a
    /// fresh session.
    pub resume_cursor: Option<serde_json::Value>,
    /// Initial permission mode name. String to avoid baking each provider's
    /// enum into the trait.
    pub permission_mode: Option<String>,
    /// Optional list of extra directories the agent should be allowed to
    /// access beyond `cwd`.
    pub additional_directories: Vec<PathBuf>,
    /// Environment variables to overlay onto the spawned child, or inherit
    /// the runtime's env when `None`.
    pub env: Option<std::collections::HashMap<String, String>>,
    /// Free-form provider-specific extras. Adapters parse what they
    /// understand and ignore the rest.
    #[serde(default)]
    pub extra: serde_json::Value,
}

/// Parameters for queueing a user turn on an existing session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SendTurnInput {
    /// Thread the turn belongs to.
    pub thread_id: ThreadId,
    /// Plain-text content of the user message.
    pub text: String,
    /// Inline image attachments, in order.
    #[serde(default)]
    pub images: Vec<ImageInput>,
    /// Optional per-turn model override. Not all providers support this.
    pub model_override: Option<String>,
}

/// Result of a successful [`AgentProvider::send_turn`](super::AgentProvider::send_turn).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnStartResult {
    /// Provider-assigned turn identifier. Subsequent events for this turn
    /// reference it.
    pub turn_id: TurnId,
}

/// Decision the runtime ships back to the provider when a user finishes
/// evaluating a pending approval request.
///
/// Variants intentionally mirror the shapes both providers already expose —
/// Claude's `canUseTool` callback and Codex's `item/*/requestApproval`
/// stream.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum ApprovalDecision {
    /// Approve the request. `updated_input` may override the tool input the
    /// model provided (e.g. the user edited the command before running).
    Allow {
        updated_input: Option<serde_json::Value>,
    },
    /// Approve and remember this decision for the rest of the session.
    AllowForSession,
    /// Reject the request with a user-visible message passed back to the
    /// model.
    Deny { message: String },
    /// Cancel out of the request entirely, halting the turn.
    Cancel,
}

/// Lifecycle phase of a provider session.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum SessionStatus {
    /// Start-up is in progress; no turns accepted yet.
    Starting,
    /// Session is idle and ready for a new turn.
    Ready,
    /// A turn is actively running.
    Running { active_turn: TurnId },
    /// Waiting for the user to resolve an approval request.
    WaitingApproval { request_id: RequestId },
    /// Session is broken and cannot accept further turns.
    Error { message: String },
    /// Session has been shut down (gracefully or otherwise).
    Closed,
}

/// Snapshot of a live provider session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderSession {
    /// Runtime thread this session is bound to.
    pub thread_id: ThreadId,
    /// Which provider produced it.
    pub provider: ProviderKind,
    /// Provider-internal session identifier.
    pub session_id: ProviderSessionId,
    /// Current lifecycle status.
    pub status: SessionStatus,
    /// Opaque resume cursor suitable for passing back into
    /// [`StartSessionInput::resume_cursor`] after a restart.
    pub resume_cursor: Option<serde_json::Value>,
}
