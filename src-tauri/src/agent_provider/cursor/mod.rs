//! Cursor Agent provider using Cursor's official ACP stdio server.

pub mod capabilities;
mod protocol;
mod session;

use std::collections::HashMap;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures_core::Stream;
use tokio::sync::{broadcast, RwLock};

use crate::agent_provider::{
    AgentProvider, ApprovalDecision, ProviderCapabilities, ProviderError, ProviderEventStream,
    ProviderKind, ProviderRuntimeEvent, ProviderSession, RequestId, SendOutcome, SendTurnInput,
    SessionStatus, StartSessionInput, ThreadId, TurnId, TurnStartResult,
};

use self::session::{CursorSession, CursorSpawnConfig};

#[derive(Debug, Clone)]
pub struct CursorProviderConfig {
    pub binary: PathBuf,
    pub event_channel_capacity: usize,
}

impl Default for CursorProviderConfig {
    fn default() -> Self {
        Self {
            binary: PathBuf::from("cursor-agent"),
            event_channel_capacity: 1024,
        }
    }
}

pub struct CursorAgentProvider {
    config: CursorProviderConfig,
    sessions: Arc<RwLock<HashMap<ThreadId, Arc<CursorSession>>>>,
    event_tx: broadcast::Sender<ProviderRuntimeEvent>,
}

impl CursorAgentProvider {
    pub fn new(config: CursorProviderConfig) -> Self {
        let (event_tx, _) = broadcast::channel(config.event_channel_capacity.max(16));
        Self {
            config,
            sessions: Arc::new(RwLock::new(HashMap::new())),
            event_tx,
        }
    }

    async fn session(&self, thread_id: &ThreadId) -> Result<Arc<CursorSession>, ProviderError> {
        self.sessions
            .read()
            .await
            .get(thread_id)
            .cloned()
            .ok_or_else(|| ProviderError::SessionNotFound {
                thread_id: thread_id.clone(),
            })
    }
}

impl Drop for CursorAgentProvider {
    fn drop(&mut self) {
        let sessions = Arc::clone(&self.sessions);
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                let sessions = std::mem::take(&mut *sessions.write().await);
                for (_, session) in sessions {
                    session.shutdown().await;
                }
            });
        }
    }
}

#[async_trait]
impl AgentProvider for CursorAgentProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Cursor
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            supports_mid_session_model_change: true,
            supports_mid_session_permission_change: true,
            supports_synchronous_tool_approval: true,
            supports_interrupt: true,
            supports_session_resume: true,
            supports_conversation_rollback: false,
        }
    }

    async fn start_session(
        &self,
        input: StartSessionInput,
    ) -> Result<ProviderSession, ProviderError> {
        let thread_id = input.thread_id.clone();
        if let Some(existing) = self.sessions.read().await.get(&thread_id).cloned() {
            if !existing.is_dead() {
                return Err(ProviderError::ValidationError {
                    message: format!("Cursor session already exists for thread {}", thread_id.0),
                });
            }
            self.sessions.write().await.remove(&thread_id);
            existing.shutdown().await;
        }
        let session = CursorSession::spawn_and_initialize(
            thread_id.clone(),
            input.cwd,
            input.model,
            input.permission_mode,
            input.effort,
            input.context_window,
            input.fast_mode,
            input.resume_cursor,
            input.env,
            CursorSpawnConfig {
                binary: self.config.binary.clone(),
            },
            self.event_tx.clone(),
        )
        .await?;
        self.sessions
            .write()
            .await
            .insert(thread_id.clone(), Arc::clone(&session));
        Ok(ProviderSession {
            thread_id,
            provider: ProviderKind::Cursor,
            session_id: session.provider_session_id.clone(),
            status: SessionStatus::Ready,
            resume_cursor: Some(serde_json::json!({
                "schemaVersion": 1,
                "sessionId": session.provider_session_id.0,
            })),
        })
    }

    async fn send_turn(&self, input: SendTurnInput) -> Result<TurnStartResult, ProviderError> {
        let session = self.session(&input.thread_id).await?;
        Ok(match session.enqueue_or_send(input).await? {
            SendOutcome::Started(turn_id) => TurnStartResult {
                turn_id,
                queued_id: None,
            },
            SendOutcome::Queued(queued_id) => TurnStartResult {
                turn_id: TurnId(String::new()),
                queued_id: Some(queued_id),
            },
        })
    }

    async fn interrupt_turn(
        &self,
        thread_id: ThreadId,
        turn_id: Option<TurnId>,
    ) -> Result<(), ProviderError> {
        self.session(&thread_id).await?.interrupt(turn_id).await
    }

    async fn cancel_queued_turn(
        &self,
        thread_id: ThreadId,
        queued_id: String,
    ) -> Result<bool, ProviderError> {
        Ok(self
            .session(&thread_id)
            .await?
            .cancel_queued(&queued_id)
            .await)
    }

    async fn send_queued_turn_now(
        &self,
        thread_id: ThreadId,
        queued_id: String,
    ) -> Result<(), ProviderError> {
        self.session(&thread_id)
            .await?
            .send_queued_now(&queued_id)
            .await
    }

    async fn respond_to_request(
        &self,
        thread_id: ThreadId,
        request_id: RequestId,
        decision: ApprovalDecision,
    ) -> Result<(), ProviderError> {
        self.session(&thread_id)
            .await?
            .respond_to_request(request_id, decision)
            .await
    }

    async fn set_model(&self, thread_id: ThreadId, model: String) -> Result<(), ProviderError> {
        self.session(&thread_id).await?.set_model(model).await
    }

    async fn set_permission_mode(
        &self,
        thread_id: ThreadId,
        mode: String,
    ) -> Result<(), ProviderError> {
        self.session(&thread_id)
            .await?
            .set_permission_mode(mode)
            .await
    }

    async fn stop_session(&self, thread_id: ThreadId) -> Result<(), ProviderError> {
        if let Some(session) = self.sessions.write().await.remove(&thread_id) {
            session.shutdown().await;
        }
        let _ = self
            .event_tx
            .send(ProviderRuntimeEvent::SessionStateChanged {
                thread_id,
                status: SessionStatus::Closed,
            });
        Ok(())
    }

    async fn list_sessions(&self) -> Result<Vec<ProviderSession>, ProviderError> {
        let sessions = self
            .sessions
            .read()
            .await
            .values()
            .cloned()
            .collect::<Vec<_>>();
        let mut result = Vec::with_capacity(sessions.len());
        for session in sessions {
            result.push(ProviderSession {
                thread_id: session.thread_id.clone(),
                provider: ProviderKind::Cursor,
                session_id: session.provider_session_id.clone(),
                status: session.state.lock().await.status.clone(),
                resume_cursor: Some(serde_json::json!({ "schemaVersion": 1, "sessionId": session.provider_session_id.0 })),
            });
        }
        Ok(result)
    }

    async fn has_session(&self, thread_id: &ThreadId) -> bool {
        self.sessions
            .read()
            .await
            .get(thread_id)
            .is_some_and(|session| !session.is_dead())
    }

    async fn turn_active(&self, thread_id: &ThreadId) -> bool {
        let Some(session) = self.sessions.read().await.get(thread_id).cloned() else {
            return false;
        };
        !session.is_dead() && session.state.lock().await.active_turn.is_some()
    }

    fn event_stream(&self) -> ProviderEventStream {
        let receiver = self.event_tx.subscribe();
        let stream = futures_util::stream::unfold(receiver, |mut receiver| async move {
            loop {
                match receiver.recv().await {
                    Ok(event) => return Some((event, receiver)),
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => return None,
                }
            }
        });
        Box::pin(stream) as Pin<Box<dyn Stream<Item = ProviderRuntimeEvent> + Send + 'static>>
    }
}
