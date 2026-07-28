//! The [`AgentProvider`] trait itself.
//!
//! Every concrete CLI-backed agent integration implements this trait. The
//! runtime owns a registry of boxed providers and routes session operations
//! by [`ProviderKind`].

use std::pin::Pin;

use async_trait::async_trait;
use futures_core::Stream;

use super::errors::ProviderError;
use super::events::ProviderRuntimeEvent;
use super::types::{
    ApprovalDecision, ProviderCapabilities, ProviderKind, ProviderSession, RequestId,
    SendTurnInput, StartSessionInput, ThreadId, TurnId, TurnStartResult,
};

/// Boxed, pinned, Send-capable stream of runtime events emitted by a
/// provider. Aliased so adapter signatures stay legible.
pub type ProviderEventStream =
    Pin<Box<dyn Stream<Item = ProviderRuntimeEvent> + Send + 'static>>;

/// Contract every CLI-backed agent integration must satisfy.
///
/// Implementations are expected to be cheaply cloneable (via `Arc`) and to
/// drive their subprocesses internally. The runtime only interacts with the
/// trait surface; it does not poke at provider internals.
#[async_trait]
pub trait AgentProvider: Send + Sync {
    /// Which provider family this implementation represents.
    fn kind(&self) -> ProviderKind;

    /// Static declaration of what mid-session operations are supported.
    fn capabilities(&self) -> ProviderCapabilities;

    /// Bring up a new session (or resume a previous one via `resume_cursor`).
    ///
    /// Returns the canonical snapshot once the session has advanced to at
    /// least the `Starting` state. Further lifecycle events arrive on
    /// [`event_stream`](Self::event_stream).
    async fn start_session(
        &self,
        input: StartSessionInput,
    ) -> Result<ProviderSession, ProviderError>;

    /// Queue a user turn on an existing session. Returns the newly minted
    /// turn identifier; actual turn output streams through
    /// [`event_stream`](Self::event_stream).
    async fn send_turn(&self, input: SendTurnInput) -> Result<TurnStartResult, ProviderError>;

    /// Interrupt the currently running turn. When `turn_id` is supplied the
    /// provider should only act if that specific turn is active, which lets
    /// the caller avoid racing against a turn that already finished.
    async fn interrupt_turn(
        &self,
        thread_id: ThreadId,
        turn_id: Option<TurnId>,
    ) -> Result<(), ProviderError>;

    /// Cancel a queued (not-yet-dispatched) follow-up turn by its queued
    /// id. Idempotent — cancelling an unknown or already-dispatched id is
    /// a silent success. The default implementation is a no-op for
    /// providers without a follow-up queue (e.g. OpenCode).
    async fn cancel_queued_turn(
        &self,
        _thread_id: ThreadId,
        _queued_id: String,
    ) -> Result<(), ProviderError> {
        Ok(())
    }

    /// **Send now (steer):** promote a queued follow-up to the front of
    /// the queue and dispatch it immediately, soft-interrupting the active
    /// turn if one is running. The interrupt preserves the session,
    /// transcript, and on-disk work — nothing is discarded — and the
    /// promoted message then runs as a normal follow-up turn so the agent
    /// re-plans with the steer. Idempotent — an unknown or
    /// already-dispatched id is a silent success. The default
    /// implementation is a no-op for providers without a follow-up queue
    /// (e.g. OpenCode).
    async fn send_queued_turn_now(
        &self,
        _thread_id: ThreadId,
        _queued_id: String,
    ) -> Result<(), ProviderError> {
        Ok(())
    }

    /// Respond to a pending approval request with the user's decision.
    async fn respond_to_request(
        &self,
        thread_id: ThreadId,
        request_id: RequestId,
        decision: ApprovalDecision,
    ) -> Result<(), ProviderError>;

    /// Whether an outstanding provider request can still be answered after
    /// Codemux rebuilds a missing live session from its resume cursor.
    ///
    /// Claude and Codex callbacks are tied to the sidecar/app-server process,
    /// so the safe default is `false`. Providers whose request state lives in
    /// a durable external service can opt in.
    fn pending_requests_survive_session_restart(&self) -> bool {
        false
    }

    /// Swap the session's model at runtime. Providers that do not support
    /// this return
    /// [`ProviderError::ValidationError`](super::errors::ProviderError::ValidationError).
    async fn set_model(&self, thread_id: ThreadId, model: String) -> Result<(), ProviderError>;

    /// Swap the session's permission mode at runtime.
    async fn set_permission_mode(
        &self,
        thread_id: ThreadId,
        mode: String,
    ) -> Result<(), ProviderError>;

    /// Gracefully terminate the session. Idempotent — closing an
    /// already-closed session is a no-op.
    async fn stop_session(&self, thread_id: ThreadId) -> Result<(), ProviderError>;

    /// Enumerate every currently live session the provider is tracking.
    async fn list_sessions(&self) -> Result<Vec<ProviderSession>, ProviderError>;

    /// Whether a live session is currently bound to `thread_id`.
    ///
    /// Cheap containment check against the provider's in-memory session
    /// registry — it does not touch the subprocess. The backend
    /// auto-resume choke point
    /// (`commands::agent_chat::ensure_live_session`) uses this to decide
    /// whether a turn needs a session rebuilt from the persisted DB row
    /// (e.g. after an app restart, when the map is empty) before the
    /// operation can proceed.
    async fn has_session(&self, thread_id: &ThreadId) -> bool;

    /// Whether a live turn is currently in flight on `thread_id`.
    ///
    /// Cheap in-memory check: true iff a live (non-dead) session is bound to
    /// the thread AND its `active_turn` is set (i.e. a turn is Running or
    /// WaitingApproval). Must not touch the subprocess. The frontend hydrate
    /// path uses this to distinguish "run still in flight" from "run died
    /// mid-turn", so a healthy run is not falsely labeled "Run interrupted"
    /// after a workspace switch remount. Defaults to `false` so providers
    /// without support are safe.
    async fn turn_active(&self, _thread_id: &ThreadId) -> bool {
        false
    }

    /// Subscribe to the canonical runtime event stream.
    ///
    /// Each call typically returns a fresh subscription; implementations are
    /// free to multiplex a single underlying broadcaster to many consumers.
    fn event_stream(&self) -> ProviderEventStream;
}
