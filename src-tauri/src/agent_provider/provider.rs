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

    /// Respond to a pending approval request with the user's decision.
    async fn respond_to_request(
        &self,
        thread_id: ThreadId,
        request_id: RequestId,
        decision: ApprovalDecision,
    ) -> Result<(), ProviderError>;

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

    /// Subscribe to the canonical runtime event stream.
    ///
    /// Each call typically returns a fresh subscription; implementations are
    /// free to multiplex a single underlying broadcaster to many consumers.
    fn event_stream(&self) -> ProviderEventStream;
}
