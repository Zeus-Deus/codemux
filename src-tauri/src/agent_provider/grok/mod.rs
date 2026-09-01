//! Grok Build provider using xAI's official ACP stdio server.

pub mod capabilities;
pub mod slash_commands;

use std::collections::HashMap;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures_core::Stream;
use tokio::sync::{broadcast, RwLock};

use crate::agent_provider::acp::session::{AcpDialect, AcpSession, AcpSpawnConfig};
use crate::agent_provider::{
    AgentProvider, ApprovalDecision, ProviderCapabilities, ProviderError, ProviderEventStream,
    ProviderKind, ProviderRuntimeEvent, ProviderSession, RequestId, SendOutcome, SendTurnInput,
    SessionStatus, StartSessionInput, ThreadId, TurnId, TurnStartResult,
};

#[derive(Debug, Clone)]
pub struct GrokProviderConfig {
    pub binary: PathBuf,
    pub event_channel_capacity: usize,
}

impl Default for GrokProviderConfig {
    fn default() -> Self {
        Self {
            binary: PathBuf::from("grok"),
            event_channel_capacity: 1024,
        }
    }
}

pub struct GrokAgentProvider {
    config: GrokProviderConfig,
    slash_command_cache: Arc<slash_commands::GrokSlashCommandCache>,
    sessions: Arc<RwLock<HashMap<ThreadId, Arc<AcpSession>>>>,
    event_tx: broadcast::Sender<ProviderRuntimeEvent>,
}

impl GrokAgentProvider {
    pub fn new(config: GrokProviderConfig) -> Self {
        Self::new_with_slash_command_cache(
            config,
            Arc::new(slash_commands::GrokSlashCommandCache::new()),
        )
    }

    pub fn new_with_slash_command_cache(
        config: GrokProviderConfig,
        slash_command_cache: Arc<slash_commands::GrokSlashCommandCache>,
    ) -> Self {
        let (event_tx, _) = broadcast::channel(config.event_channel_capacity.max(16));
        Self {
            config,
            slash_command_cache,
            sessions: Arc::new(RwLock::new(HashMap::new())),
            event_tx,
        }
    }

    async fn session(&self, thread_id: &ThreadId) -> Result<Arc<AcpSession>, ProviderError> {
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

impl Drop for GrokAgentProvider {
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
impl AgentProvider for GrokAgentProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Grok
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            supports_mid_session_model_change: true,
            // Grok's ask/full permission choice is supplied when the ACP
            // session is created; the live setter reports restart-required.
            supports_mid_session_permission_change: false,
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
        // Keep the provider boundary safe for callers other than the Tauri
        // command layer. An explicit fresh start must never attempt to load
        // the stale native session even if a cursor was supplied alongside
        // it (the incompatible-model recovery path intentionally does this).
        let resume_cursor = if input.fresh_session {
            None
        } else {
            input.resume_cursor
        };
        let dead_evicted = {
            let mut sessions = self.sessions.write().await;
            match sessions.get(&thread_id) {
                Some(existing) if existing.is_dead() => sessions.remove(&thread_id),
                Some(_) => {
                    return Err(ProviderError::ValidationError {
                        message: format!("Grok session already exists for thread {}", thread_id.0),
                    });
                }
                None => None,
            }
        };
        if let Some(dead) = dead_evicted {
            dead.shutdown().await;
        }

        let session = AcpSession::spawn_and_initialize(
            thread_id.clone(),
            input.cwd,
            input.model,
            input.permission_mode,
            input.effort,
            input.context_window,
            input.fast_mode,
            resume_cursor,
            input.env,
            AcpSpawnConfig {
                binary: self.config.binary.clone(),
                dialect: AcpDialect::Grok,
                grok_slash_command_cache: Some(Arc::clone(&self.slash_command_cache)),
            },
            self.event_tx.clone(),
        )
        .await?;

        {
            let mut sessions = self.sessions.write().await;
            if sessions
                .get(&thread_id)
                .is_some_and(|existing| !existing.is_dead())
            {
                drop(sessions);
                session.shutdown().await;
                return Err(ProviderError::ValidationError {
                    message: format!("Grok session already exists for thread {}", thread_id.0),
                });
            }
            sessions.insert(thread_id.clone(), Arc::clone(&session));
        }

        Ok(ProviderSession {
            thread_id,
            provider: ProviderKind::Grok,
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
                provider: ProviderKind::Grok,
                session_id: session.provider_session_id.clone(),
                status: session.state.lock().await.status.clone(),
                resume_cursor: Some(serde_json::json!({
                    "schemaVersion": 1,
                    "sessionId": session.provider_session_id.0,
                })),
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
