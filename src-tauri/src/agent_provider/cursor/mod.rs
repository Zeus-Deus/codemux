//! Cursor Agent provider using Cursor's official ACP stdio server.

pub mod capabilities;
pub mod managed;

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

use crate::agent_provider::acp::session::{AcpDialect, AcpSession, AcpSpawnConfig};
use crate::agent_provider::acp::slash_commands::AcpSlashCommandCache;

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
    slash_command_cache: Arc<AcpSlashCommandCache>,
    sessions: Arc<RwLock<HashMap<ThreadId, Arc<AcpSession>>>>,
    managed_sessions:
        Arc<RwLock<HashMap<ThreadId, Arc<super::managed_bridge::ManagedBridgeSession>>>>,
    event_tx: broadcast::Sender<ProviderRuntimeEvent>,
}

impl CursorAgentProvider {
    pub fn new(config: CursorProviderConfig) -> Self {
        Self::new_with_slash_command_cache(config, Arc::new(AcpSlashCommandCache::new()))
    }

    pub fn new_with_slash_command_cache(
        config: CursorProviderConfig,
        slash_command_cache: Arc<AcpSlashCommandCache>,
    ) -> Self {
        let (event_tx, _) = broadcast::channel(config.event_channel_capacity.max(16));
        Self {
            config,
            slash_command_cache,
            sessions: Arc::new(RwLock::new(HashMap::new())),
            managed_sessions: Arc::new(RwLock::new(HashMap::new())),
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

impl Drop for CursorAgentProvider {
    fn drop(&mut self) {
        let sessions = Arc::clone(&self.sessions);
        let managed_sessions = Arc::clone(&self.managed_sessions);
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                let sessions = std::mem::take(&mut *sessions.write().await);
                for (_, session) in sessions {
                    session.shutdown().await;
                }
                for (_, session) in std::mem::take(&mut *managed_sessions.write().await) {
                    let _ = session.shutdown_managed().await;
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

    fn managed_capabilities(&self) -> super::managed::ManagedCapabilities {
        super::managed::ManagedCapabilities {
            scoped_tools: true,
            native_fanout_disabled: true,
            enforced_read_only: true,
            isolated_writes: true,
            verified_stop: cfg!(target_os = "linux"),
        }
    }

    async fn stop_managed_session(&self, thread_id: ThreadId) -> Result<(), ProviderError> {
        let session = self
            .managed_sessions
            .read()
            .await
            .get(&thread_id)
            .cloned()
            .ok_or_else(|| ProviderError::SessionNotFound {
                thread_id: thread_id.clone(),
            })?;
        session.shutdown_managed().await?;
        self.managed_sessions.write().await.remove(&thread_id);
        Ok(())
    }

    async fn start_session(
        &self,
        input: StartSessionInput,
    ) -> Result<ProviderSession, ProviderError> {
        let thread_id = input.thread_id.clone();
        if let Some(context) = super::managed::lookup_session(&thread_id) {
            if self.sessions.read().await.contains_key(&thread_id)
                || self.managed_sessions.read().await.contains_key(&thread_id)
            {
                return Err(ProviderError::ValidationError {
                    message: "Cursor thread already has a session".into(),
                });
            }
            let session = managed::spawn(input, context, self.event_tx.clone()).await?;
            let ordinary = self.sessions.read().await;
            let mut sessions = self.managed_sessions.write().await;
            if ordinary.contains_key(&thread_id) || sessions.contains_key(&thread_id) {
                drop(sessions);
                drop(ordinary);
                session.shutdown_managed().await?;
                return Err(ProviderError::ValidationError {
                    message: "Cursor thread already has a session".into(),
                });
            }
            sessions.insert(thread_id, Arc::clone(&session));
            drop(sessions);
            drop(ordinary);
            return Ok(session.provider_session().await);
        }
        if self.managed_sessions.read().await.contains_key(&thread_id) {
            return Err(ProviderError::ValidationError {
                message: "Cursor thread already has a managed session".into(),
            });
        }
        // Evict a corpse under the write lock so check→remove is atomic
        // against a concurrent rebuild. A read-lock check followed by a
        // separate write-lock remove lets two starts both observe the same
        // dead session. A still-live session is a genuine double-start and
        // stays a ValidationError.
        let dead_evicted = {
            let mut sessions = self.sessions.write().await;
            match sessions.get(&thread_id) {
                Some(existing) if existing.is_dead() => sessions.remove(&thread_id),
                Some(_) => {
                    return Err(ProviderError::ValidationError {
                        message: format!(
                            "Cursor session already exists for thread {}",
                            thread_id.0
                        ),
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
            input.resume_cursor,
            input.env,
            AcpSpawnConfig {
                binary: self.config.binary.clone(),
                dialect: AcpDialect::Cursor,
                slash_command_cache: Arc::clone(&self.slash_command_cache),
            },
            self.event_tx.clone(),
        )
        .await?;
        // Spawning is async, so two starts that both got past the eviction
        // above can arrive here with two live children. Never overwrite a
        // live entry: the loser's `cursor-agent` process would stay running
        // with nothing holding a handle to shut it down.
        {
            let mut sessions = self.sessions.write().await;
            if self.managed_sessions.read().await.contains_key(&thread_id)
                || sessions
                    .get(&thread_id)
                    .is_some_and(|existing| !existing.is_dead())
            {
                drop(sessions);
                session.shutdown().await;
                return Err(ProviderError::ValidationError {
                    message: format!("Cursor session already exists for thread {}", thread_id.0),
                });
            }
            sessions.insert(thread_id.clone(), Arc::clone(&session));
        }
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
        if let Some(session) = self
            .managed_sessions
            .read()
            .await
            .get(&input.thread_id)
            .cloned()
        {
            return session.send_turn(input).await;
        }
        let session = self.session(&input.thread_id).await?;
        Ok(match session.enqueue_or_send(input).await? {
            SendOutcome::Started(turn_id) => TurnStartResult {
                steered: false,
                turn_id,
                queued_id: None,
            },
            SendOutcome::Queued(queued_id) => TurnStartResult {
                steered: false,
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
        if let Some(session) = self.managed_sessions.read().await.get(&thread_id).cloned() {
            return session.interrupt().await;
        }
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

    async fn set_fast_mode(
        &self,
        thread_id: ThreadId,
        fast_mode: bool,
    ) -> Result<(), ProviderError> {
        self.session(&thread_id)
            .await?
            .set_fast_mode(fast_mode)
            .await
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
        if self.managed_sessions.read().await.contains_key(&thread_id) {
            self.stop_managed_session(thread_id.clone()).await?;
        }
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
        for session in self.managed_sessions.read().await.values() {
            result.push(session.provider_session().await);
        }
        Ok(result)
    }

    async fn has_session(&self, thread_id: &ThreadId) -> bool {
        self.managed_sessions
            .read()
            .await
            .get(thread_id)
            .is_some_and(|session| !session.is_dead())
            || self
                .sessions
                .read()
                .await
                .get(thread_id)
                .is_some_and(|session| !session.is_dead())
    }

    async fn turn_active(&self, thread_id: &ThreadId) -> bool {
        if let Some(session) = self.managed_sessions.read().await.get(thread_id).cloned() {
            return session.turn_active().await;
        }
        let Some(session) = self.sessions.read().await.get(thread_id).cloned() else {
            return false;
        };
        !session.is_dead() && session.state.lock().await.active_turn.is_some()
    }

    async fn session_slash_commands(
        &self,
        thread_id: ThreadId,
        cwd: &std::path::Path,
    ) -> Result<Vec<super::claude::slash_commands::ProviderSlashCommand>, ProviderError> {
        let session = self.sessions.read().await.get(&thread_id).cloned();
        match session {
            Some(session) if !session.is_dead() => session.slash_commands(cwd).await,
            _ => Ok(Vec::new()),
        }
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

    fn managed_event_stream(&self, thread_id: &ThreadId) -> ProviderEventStream {
        super::managed_bridge::event_stream(&self.event_tx, thread_id)
    }
}
