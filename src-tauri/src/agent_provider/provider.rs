//! The [`AgentProvider`] trait itself.
//!
//! Every concrete CLI-backed agent integration implements this trait. The
//! runtime owns a registry of boxed providers and routes session operations
//! by [`ProviderKind`].

use std::pin::Pin;

use async_trait::async_trait;
use futures_core::Stream;
use tokio::sync::broadcast;

use super::errors::ProviderError;
use super::events::ProviderRuntimeEvent;
use super::types::{
    ApprovalDecision, ProviderCapabilities, ProviderKind, ProviderSession, RequestId,
    SendTurnInput, StartSessionInput, ThreadId, ThreadRuntimeStatus, TurnId, TurnStartResult,
};

/// Boxed, pinned, Send-capable stream of runtime events emitted by a
/// provider. Aliased so adapter signatures stay legible.
pub type ProviderEventStream =
    Pin<Box<dyn Stream<Item = ProviderRuntimeEvent> + Send + 'static>>;

/// Broadcast retention is bounded, but transcript consumers must learn about
/// every gap. A lag count cannot identify which threads lost events, so report
/// it as a provider-wide warning rather than guessing a thread to attach it to.
pub(super) fn broadcast_event_stream(
    sender: &broadcast::Sender<ProviderRuntimeEvent>,
    provider: ProviderKind,
) -> ProviderEventStream {
    let receiver = sender.subscribe();
    Box::pin(futures_util::stream::unfold(
        receiver,
        move |mut receiver| async move {
            let event = match receiver.recv().await {
                Ok(event) => event,
                Err(broadcast::error::RecvError::Closed) => return None,
                Err(broadcast::error::RecvError::Lagged(count)) => ProviderRuntimeEvent::RuntimeWarning {
                    thread_id: None,
                    message: format!(
                        "{provider:?} event stream lost {count} events. Conversation history may be incomplete."
                    ),
                    original_payload: Some(serde_json::json!({
                        "code": "provider_event_lag",
                        "provider": provider,
                        "dropped_events": count,
                    })),
                },
            };
            Some((event, receiver))
        },
    ))
}

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

    /// Workflow execution requires guarantees beyond the chat UI controls.
    /// Adapters opt in individually; ignored input fields are not support.
    fn managed_capabilities(&self) -> super::managed::ManagedCapabilities {
        super::managed::ManagedCapabilities::default()
    }

    /// Close an attempt and prove that its owned execution is no longer live.
    /// A normal stop acknowledgement alone must never release a write lease.
    async fn stop_managed_session(&self, _thread_id: ThreadId) -> Result<(), ProviderError> {
        Err(ProviderError::ValidationError {
            message: "provider cannot verify managed execution quiescence".into(),
        })
    }

    /// Bring up a new session (or resume a previous one via `resume_cursor`).
    ///
    /// Returns the canonical snapshot once the session has advanced to at
    /// least the `Starting` state. Further lifecycle events arrive on
    /// [`event_stream`](Self::event_stream).
    async fn start_session(
        &self,
        input: StartSessionInput,
    ) -> Result<ProviderSession, ProviderError>;

    /// Only cancellation-safe pre-dispatch waits may observe this token.
    /// Default adapters keep their existing startup/cleanup ownership; do not
    /// drop an in-flight startup RPC as a generic cancellation fallback.
    async fn start_session_with_cancellation(
        &self, input: StartSessionInput,
        _cancellation: Option<std::sync::Arc<dyn super::types::SessionStartCancellation>>,
    ) -> Result<ProviderSession, ProviderError> {
        self.start_session(input).await
    }

    /// Queue a user turn on an existing session. Returns the newly minted
    /// turn identifier; actual turn output streams through
    /// [`event_stream`](Self::event_stream).
    async fn send_turn(&self, input: SendTurnInput) -> Result<TurnStartResult, ProviderError>;

    /// Non-interrupting guidance. Unsupported adapters may send normally
    /// while idle, but must never silently queue or cancel a running turn.
    async fn steer_turn(&self, input: SendTurnInput) -> Result<TurnStartResult, ProviderError> {
        if self.turn_active(&input.thread_id).await {
            return Err(ProviderError::ValidationError {
                message: "Safe steering is unavailable for this provider. Choose Queue or Interrupt and send.".into(),
            });
        }
        self.send_turn(input).await
    }

    /// Deliver an existing queued message without dropping its attachments
    /// or changing its position on failure.
    async fn steer_queued_turn(
        &self,
        _thread_id: ThreadId,
        _queued_id: String,
    ) -> Result<(), ProviderError> {
        Err(ProviderError::ValidationError {
            message: "Safe steering is unavailable for this provider.".into(),
        })
    }

    /// Interrupt the currently running turn. When `turn_id` is supplied the
    /// provider should only act if that specific turn is active, which lets
    /// the caller avoid racing against a turn that already finished.
    async fn interrupt_turn(
        &self,
        thread_id: ThreadId,
        turn_id: Option<TurnId>,
    ) -> Result<(), ProviderError>;

    /// Cancel a queued (not-yet-dispatched) follow-up turn by its queued
    /// id. Returns whether an item was actually removed. An unknown or
    /// already-dispatched id returns `false`; the default implementation is
    /// a no-op for providers without a follow-up queue.
    async fn cancel_queued_turn(
        &self,
        _thread_id: ThreadId,
        _queued_id: String,
    ) -> Result<bool, ProviderError> {
        Ok(false)
    }

    /// **Interrupt and send:** promote a queued follow-up to the front of
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

    /// Replace a queued turn's text in place, keeping its position and
    /// attachments; see [`queue_edit`](super::queue_edit). Returns whether it
    /// was still queued, emitting [`ProviderRuntimeEvent::QueuedTurnEdited`]
    /// when it was. Every provider that queues turns overrides this.
    async fn edit_queued_turn(
        &self,
        _thread_id: ThreadId,
        _queued_id: String,
        _text: String,
    ) -> Result<bool, ProviderError> {
        Err(ProviderError::ValidationError {
            message: "This provider can't edit queued messages.".into(),
        })
    }

    /// Move a queued turn to just before `before`, or to the back of the
    /// queue. Returns whether it was still queued, emitting
    /// [`ProviderRuntimeEvent::QueuedTurnsReordered`] when it was.
    async fn move_queued_turn(
        &self,
        _thread_id: ThreadId,
        _queued_id: String,
        _before: Option<String>,
    ) -> Result<bool, ProviderError> {
        Err(ProviderError::ValidationError {
            message: "This provider can't reorder queued messages.".into(),
        })
    }

    /// Adapter opt-in, independent of the selected model's tool catalog.
    fn supports_async_questions(&self) -> bool {
        false
    }

    async fn answer_question(
        &self,
        _input: super::AnswerQuestionInput,
    ) -> Result<super::QuestionDelivery, super::QuestionDeliveryError> {
        Err(super::QuestionDeliveryError::Rejected(
            "This provider does not support asynchronous answers.".into(),
        ))
    }

    /// Correlate an uncertain submission against provider-owned history.
    async fn find_question_answer(
        &self,
        _thread_id: ThreadId,
        _target: String,
        _submission_id: String,
    ) -> Result<Option<super::QuestionDelivery>, String> {
        Err("This provider cannot reconcile asynchronous answers.".into())
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

    /// Swap the session's service-speed tier at runtime. Providers that
    /// advertise Fast mode should override this and apply the choice to the
    /// existing provider session rather than forcing a stop/resume cycle.
    /// Standard is a no-op for providers without Fast support, allowing stale
    /// persisted Fast choices to be cleared when capabilities change.
    async fn set_fast_mode(
        &self,
        _thread_id: ThreadId,
        fast_mode: bool,
    ) -> Result<(), ProviderError> {
        if !fast_mode {
            return Ok(());
        }
        Err(ProviderError::ValidationError {
            message: "provider does not support mid-session Fast mode changes".into(),
        })
    }

    /// Service tiers are provider-native identifiers. Boolean adapters accept
    /// only Standard/Fast; they must never silently treat Ultrafast as Fast.
    async fn set_service_tier(
        &self,
        thread_id: ThreadId,
        tier: String,
    ) -> Result<(), ProviderError> {
        let fast = match tier.as_str() {
            "default" => false,
            "fast" => true,
            _ => {
                return Err(ProviderError::ValidationError {
                    message: "provider does not support this service tier".into(),
                })
            }
        };
        self.set_fast_mode(thread_id, fast).await
    }

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

    /// What the live session bound to `thread_id` is doing, or `None`
    /// when there is none. In-memory only. The default reports what every
    /// provider can tell (`turn_active` and the session status); providers
    /// that keep their own follow-up queue override it.
    async fn runtime_status(&self, thread_id: &ThreadId) -> Option<ThreadRuntimeStatus> {
        if !self.has_session(thread_id).await {
            return None;
        }
        let status = self
            .list_sessions()
            .await
            .ok()?
            .into_iter()
            .find(|session| &session.thread_id == thread_id)?
            .status;
        Some(ThreadRuntimeStatus {
            busy: self.turn_active(thread_id).await,
            queued: None,
            compacting: false,
            status: ThreadRuntimeStatus::status_name(&status).into(),
        })
    }

    /// Compact an idle session's context on behalf of a plugin. `Err` is
    /// the stable refusal reason. Only Claude supports it.
    async fn request_compaction(&self, _thread_id: &ThreadId) -> Result<(), &'static str> {
        Err("not_claude")
    }

    /// Remove completed turns from the provider's durable conversation.
    /// The safe default refuses so filesystem-only restoration can never be
    /// exposed as a true revert.
    async fn rollback_conversation(
        &self,
        _thread_id: ThreadId,
        _num_turns: u32,
    ) -> Result<(), ProviderError> {
        Err(ProviderError::ValidationError {
            message: "provider does not support conversation rollback".into(),
        })
    }

    /// Commands published by one live session (providers with profile-scoped
    /// vocabularies must never merge catalogues by cwd alone).
    async fn session_slash_commands(
        &self,
        _thread_id: ThreadId,
        _cwd: &std::path::Path,
    ) -> Result<Vec<super::claude::slash_commands::ProviderSlashCommand>, ProviderError> {
        Ok(Vec::new())
    }

    /// Native lifecycle-hook controls. Adapters opt in explicitly.
    async fn manage_hooks(
        &self,
        _cwd: &std::path::Path,
        _thread_id: Option<ThreadId>,
        _update: Option<super::codex::hooks::HookUpdate>,
    ) -> Result<super::codex::hooks::HooksList, String> {
        Err("This provider does not expose lifecycle-hook controls in chat.".into())
    }

    /// Subscribe to the canonical runtime event stream.
    ///
    /// Each call typically returns a fresh subscription; implementations are
    /// free to multiplex a single underlying broadcaster to many consumers.
    /// Bounded streams must report gaps with a `RuntimeWarning` so consumers
    /// do not mistake a partial transcript for a complete one.
    fn event_stream(&self) -> ProviderEventStream;

    /// Managed subscribers must surface event loss instead of silently
    /// treating a truncated stream as a successful execution.
    fn managed_event_stream(&self, _thread_id: &ThreadId) -> ProviderEventStream {
        self.event_stream()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures_util::StreamExt;

    #[tokio::test]
    async fn provider_event_delivery_shared_stream_reports_repeated_lag_and_closes() {
        let (sender, _) = tokio::sync::broadcast::channel(2);
        let mut stream = broadcast_event_stream(&sender, ProviderKind::Grok);
        let send = |i: usize| {
            sender
                .send(ProviderRuntimeEvent::RuntimeWarning {
                    thread_id: None,
                    message: i.to_string(),
                    original_payload: None,
                })
                .unwrap();
        };
        for i in 0..4 {
            send(i);
        }
        let warning = stream.next().await.unwrap();
        assert!(matches!(warning, ProviderRuntimeEvent::RuntimeWarning {
            original_payload: Some(details), ..
        } if details["provider"] == "grok" && details["dropped_events"] == 2));
        assert!(
            matches!(stream.next().await, Some(ProviderRuntimeEvent::RuntimeWarning { message, .. }) if message == "2")
        );

        for i in 4..7 {
            send(i);
        }
        let warning = stream.next().await.unwrap();
        assert!(matches!(warning, ProviderRuntimeEvent::RuntimeWarning {
            original_payload: Some(details), ..
        } if details["code"] == "provider_event_lag" && details["dropped_events"] == 2));
        drop(sender);
        for i in 5..7 {
            assert!(
                matches!(stream.next().await, Some(ProviderRuntimeEvent::RuntimeWarning { message, .. }) if message == i.to_string())
            );
        }
        assert!(stream.next().await.is_none());
    }
}
