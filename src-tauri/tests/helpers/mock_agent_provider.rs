//! In-test mock implementation of [`AgentProvider`].
//!
//! Records every trait call on a shared [`MockAgentCalls`] log and
//! lets tests pre-seed the return values for the fallible methods.
//! The broadcaster is exposed via [`MockAgentProvider::event_tx`] so
//! tests can fan canonical events through the provider's stream and
//! observe the event bridge behavior.

use std::collections::HashSet;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures_core::Stream;
use tokio::sync::broadcast;

use codemux_lib::agent_provider::{
    AgentProvider, ApprovalDecision, ProviderCapabilities, ProviderError, ProviderEventStream,
    ProviderKind, ProviderRuntimeEvent, ProviderSession, ProviderSessionId, RequestId,
    SendTurnInput, SessionStatus, StartSessionInput, ThreadId, TurnId, TurnStartResult,
};

/// One recorded trait-method call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MockCall {
    StartSession(ThreadId),
    SendTurn(ThreadId, String),
    InterruptTurn(ThreadId, Option<TurnId>),
    RespondToRequest(ThreadId, RequestId),
    SetModel(ThreadId, String),
    SetPermissionMode(ThreadId, String),
    RollbackConversation(ThreadId, u32),
    StopSession(ThreadId),
    ListSessions,
}

/// Shared log of calls observed by one [`MockAgentProvider`].
#[derive(Default, Clone)]
pub struct MockAgentCalls(pub Arc<Mutex<Vec<MockCall>>>);

impl MockAgentCalls {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn snapshot(&self) -> Vec<MockCall> {
        self.0.lock().unwrap().clone()
    }

    fn push(&self, call: MockCall) {
        self.0.lock().unwrap().push(call);
    }
}

/// Mock provider.
///
/// Construct one per test; it is cheap to clone via `Arc`. The
/// `kind` field determines what
/// [`AgentProvider::kind`](codemux_lib::agent_provider::AgentProvider::kind)
/// returns so the ProviderRegistry routes to the right mock.
pub struct MockAgentProvider {
    kind: ProviderKind,
    pub calls: MockAgentCalls,
    pub event_tx: broadcast::Sender<ProviderRuntimeEvent>,
    /// Live-session registry so `has_session` mirrors a real adapter:
    /// `start_session` inserts, `stop_session` removes.
    live: Arc<Mutex<HashSet<ThreadId>>>,
    rollback_error: Arc<Mutex<Option<String>>>,
}

impl MockAgentProvider {
    pub fn new(kind: ProviderKind) -> Self {
        let (event_tx, _rx) = broadcast::channel(64);
        Self {
            kind,
            calls: MockAgentCalls::new(),
            event_tx,
            live: Arc::new(Mutex::new(HashSet::new())),
            rollback_error: Arc::new(Mutex::new(None)),
        }
    }

    pub fn fail_next_rollback(&self, message: impl Into<String>) {
        *self.rollback_error.lock().unwrap() = Some(message.into());
    }

    /// Convenience: emit a runtime event via the broadcaster, the
    /// same path a real provider would use.
    #[allow(dead_code)]
    pub fn emit(&self, event: ProviderRuntimeEvent) {
        let _ = self.event_tx.send(event);
    }
}

#[async_trait]
impl AgentProvider for MockAgentProvider {
    fn kind(&self) -> ProviderKind {
        self.kind
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            supports_mid_session_model_change: true,
            supports_mid_session_permission_change: true,
            supports_synchronous_tool_approval: true,
            supports_interrupt: true,
            supports_session_resume: true,
            supports_conversation_rollback: true,
        }
    }

    async fn start_session(
        &self,
        input: StartSessionInput,
    ) -> Result<ProviderSession, ProviderError> {
        self.calls.push(MockCall::StartSession(input.thread_id.clone()));
        self.live.lock().unwrap().insert(input.thread_id.clone());
        Ok(ProviderSession {
            thread_id: input.thread_id.clone(),
            provider: self.kind,
            session_id: ProviderSessionId(format!("mock-{}", input.thread_id.0)),
            status: SessionStatus::Ready,
            resume_cursor: None,
        })
    }

    async fn send_turn(&self, input: SendTurnInput) -> Result<TurnStartResult, ProviderError> {
        let checkpoint = input.turn_checkpoint.clone();
        if let Some(checkpoint) = checkpoint.as_ref() {
            checkpoint.prepare().await;
        }
        self.calls
            .push(MockCall::SendTurn(input.thread_id.clone(), input.text));
        if let Some(checkpoint) = checkpoint.as_ref() {
            checkpoint.commit().await;
        }
        Ok(TurnStartResult {
            turn_id: TurnId("mock-turn".into()),
            queued_id: None,
        })
    }

    async fn interrupt_turn(
        &self,
        thread_id: ThreadId,
        turn_id: Option<TurnId>,
    ) -> Result<(), ProviderError> {
        self.calls.push(MockCall::InterruptTurn(thread_id, turn_id));
        Ok(())
    }

    async fn respond_to_request(
        &self,
        thread_id: ThreadId,
        request_id: RequestId,
        _decision: ApprovalDecision,
    ) -> Result<(), ProviderError> {
        self.calls.push(MockCall::RespondToRequest(thread_id, request_id));
        Ok(())
    }

    async fn set_model(&self, thread_id: ThreadId, model: String) -> Result<(), ProviderError> {
        self.calls.push(MockCall::SetModel(thread_id, model));
        Ok(())
    }

    async fn set_permission_mode(
        &self,
        thread_id: ThreadId,
        mode: String,
    ) -> Result<(), ProviderError> {
        self.calls.push(MockCall::SetPermissionMode(thread_id, mode));
        Ok(())
    }

    async fn stop_session(&self, thread_id: ThreadId) -> Result<(), ProviderError> {
        self.live.lock().unwrap().remove(&thread_id);
        self.calls.push(MockCall::StopSession(thread_id));
        Ok(())
    }

    async fn has_session(&self, thread_id: &ThreadId) -> bool {
        self.live.lock().unwrap().contains(thread_id)
    }

    async fn rollback_conversation(
        &self,
        thread_id: ThreadId,
        num_turns: u32,
    ) -> Result<(), ProviderError> {
        self.calls
            .push(MockCall::RollbackConversation(thread_id, num_turns));
        if let Some(message) = self.rollback_error.lock().unwrap().take() {
            return Err(ProviderError::RpcError { message });
        }
        Ok(())
    }

    async fn list_sessions(&self) -> Result<Vec<ProviderSession>, ProviderError> {
        self.calls.push(MockCall::ListSessions);
        Ok(vec![])
    }

    fn event_stream(&self) -> ProviderEventStream {
        let rx = self.event_tx.subscribe();
        let stream = futures_util::stream::unfold(rx, |mut rx| async move {
            loop {
                match rx.recv().await {
                    Ok(item) => return Some((item, rx)),
                    Err(broadcast::error::RecvError::Closed) => return None,
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                }
            }
        });
        Box::pin(stream) as Pin<Box<dyn Stream<Item = ProviderRuntimeEvent> + Send + 'static>>
    }
}
