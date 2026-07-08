//! [`AgentProvider`](crate::agent_provider::AgentProvider) implementation
//! for OpenCode.
//!
//! Stage 8 wraps the lifecycle primitives ([`OpenCodeServerManager`],
//! [`OpenCodeSession`]) into a trait-implementing struct slotted into
//! the chat runtime's provider registry. Once registered the
//! `provider_not_configured: OpenCode` error from
//! `commands::agent_chat::lookup_provider` goes away and OpenCode
//! turns route end-to-end.
//!
//! Concurrency model: one [`OpenCodeSession`] per Codemux thread,
//! held under a `RwLock<HashMap<ThreadId, Arc<OpenCodeSession>>>`.
//! All per-session HTTP traffic is serialised inside the session
//! itself; the provider just routes calls.

use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures_core::Stream;
use tokio::sync::{broadcast, RwLock};

use crate::agent_provider::events::ProviderRuntimeEvent;
use crate::agent_provider::provider::{AgentProvider, ProviderEventStream};
use crate::agent_provider::types::{
    ApprovalDecision, ProviderCapabilities, ProviderKind, ProviderSession, RequestId,
    SendTurnInput, SessionStatus, StartSessionInput, ThreadId, TurnId, TurnStartResult,
};
use crate::agent_provider::ProviderError;

use super::manager::OpenCodeServerManager;
use super::session::OpenCodeSession;

/// Configuration knobs for the OpenCode provider. Defaults are
/// fine for production; tests may override `event_channel_capacity`
/// to keep memory tight.
#[derive(Debug, Clone)]
pub struct OpenCodeProviderConfig {
    /// Capacity of the broadcast channel that fans canonical events
    /// out to subscribers. Mirrors the Claude provider's default.
    pub event_channel_capacity: usize,
}

impl Default for OpenCodeProviderConfig {
    fn default() -> Self {
        Self {
            event_channel_capacity: 1024,
        }
    }
}

/// `AgentProvider` for OpenCode, backed by the singleton
/// [`OpenCodeServerManager`] and a per-thread [`OpenCodeSession`] map.
pub struct OpenCodeAgentProvider {
    manager: Arc<OpenCodeServerManager>,
    sessions: Arc<RwLock<HashMap<ThreadId, Arc<OpenCodeSession>>>>,
    event_tx: broadcast::Sender<ProviderRuntimeEvent>,
}

impl OpenCodeAgentProvider {
    pub fn new(manager: Arc<OpenCodeServerManager>, config: OpenCodeProviderConfig) -> Self {
        let capacity = config.event_channel_capacity.max(16);
        let (event_tx, _) = broadcast::channel(capacity);
        Self {
            manager,
            sessions: Arc::new(RwLock::new(HashMap::new())),
            event_tx,
        }
    }

    async fn lookup(&self, thread_id: &ThreadId) -> Result<Arc<OpenCodeSession>, ProviderError> {
        let sessions = self.sessions.read().await;
        sessions
            .get(thread_id)
            .cloned()
            .ok_or_else(|| ProviderError::SessionNotFound {
                thread_id: thread_id.clone(),
            })
    }
}

/// Reaps every live session on drop. Mirrors the Claude/Codex
/// pattern — sessions own their SSE listener tasks and an HTTP
/// client; both wind down cleanly when their `Arc` count hits zero,
/// but we additionally call `shutdown()` to DELETE the OpenCode-side
/// session and free its server-side memory.
impl Drop for OpenCodeAgentProvider {
    fn drop(&mut self) {
        let sessions = Arc::clone(&self.sessions);
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                let map = {
                    let mut guard = sessions.write().await;
                    std::mem::take(&mut *guard)
                };
                for (_, session) in map {
                    session.shutdown().await;
                }
            });
        }
    }
}

#[async_trait]
impl AgentProvider for OpenCodeAgentProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::OpenCode
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            // OpenCode reads the per-turn `model` field on every
            // `prompt_async` call, so the model swap is effectively
            // zero-cost.
            supports_mid_session_model_change: true,
            // No first-class permission-mode RPC today; the provider
            // ignores `set_permission_mode` calls.
            supports_mid_session_permission_change: false,
            // `permission.asked` events do block the turn waiting for
            // the user — a `POST /permissions/{id}` resumes it.
            supports_synchronous_tool_approval: true,
            // `POST /session/{id}/abort` interrupts a running turn.
            supports_interrupt: true,
            // OpenCode persists sessions on disk; resume by sending
            // a fresh `start_session` with the saved provider
            // session id is supported by the server but Codemux does
            // not yet thread the cursor through. False until we wire it.
            supports_session_resume: false,
        }
    }

    async fn start_session(
        &self,
        input: StartSessionInput,
    ) -> Result<ProviderSession, ProviderError> {
        let thread_id = input.thread_id.clone();
        {
            let sessions = self.sessions.read().await;
            if sessions.contains_key(&thread_id) {
                return Err(ProviderError::ValidationError {
                    message: format!(
                        "opencode session already exists for thread {:?}",
                        thread_id.0
                    ),
                });
            }
        }
        let session = OpenCodeSession::start(
            self.manager.clone(),
            thread_id.clone(),
            input.model.clone(),
            input.effort.clone(),
            self.event_tx.clone(),
        )
        .await?;
        let provider_session_id = session.provider_session_id.clone();
        {
            let mut sessions = self.sessions.write().await;
            sessions.insert(thread_id.clone(), Arc::clone(&session));
        }
        Ok(ProviderSession {
            thread_id,
            provider: ProviderKind::OpenCode,
            session_id: provider_session_id,
            status: SessionStatus::Ready,
            resume_cursor: None,
        })
    }

    async fn send_turn(&self, input: SendTurnInput) -> Result<TurnStartResult, ProviderError> {
        let session = self.lookup(&input.thread_id).await?;
        let turn_id = session
            .send_turn(
                input.text,
                input.images,
                input.model_override,
                input.effort_override,
            )
            .await?;
        Ok(TurnStartResult { turn_id })
    }

    async fn interrupt_turn(
        &self,
        thread_id: ThreadId,
        _turn_id: Option<TurnId>,
    ) -> Result<(), ProviderError> {
        // OpenCode's `/abort` is session-scoped — the optional
        // turn_id check is enforced client-side by callers that want
        // to avoid racing a turn that already finished. We don't try
        // to validate it server-side because there is no direct
        // turn->message mapping on the wire.
        let session = self.lookup(&thread_id).await?;
        session.interrupt().await
    }

    async fn respond_to_request(
        &self,
        thread_id: ThreadId,
        request_id: RequestId,
        decision: ApprovalDecision,
    ) -> Result<(), ProviderError> {
        let session = self.lookup(&thread_id).await?;
        session.respond_to_request(request_id, decision).await
    }

    async fn set_model(&self, thread_id: ThreadId, model: String) -> Result<(), ProviderError> {
        let session = self.lookup(&thread_id).await?;
        session.set_model(model).await;
        Ok(())
    }

    async fn set_permission_mode(
        &self,
        _thread_id: ThreadId,
        _mode: String,
    ) -> Result<(), ProviderError> {
        // OpenCode has per-session permission rules baked in at
        // session-create time — there is no live "swap mode" RPC.
        // Surface ValidationError so the UI knows not to render the
        // control rather than silently swallowing the call.
        Err(ProviderError::ValidationError {
            message: "OpenCode does not support runtime permission-mode changes".into(),
        })
    }

    async fn stop_session(&self, thread_id: ThreadId) -> Result<(), ProviderError> {
        let session = {
            let mut sessions = self.sessions.write().await;
            sessions.remove(&thread_id)
        };
        let session = session.ok_or_else(|| ProviderError::SessionNotFound {
            thread_id: thread_id.clone(),
        })?;
        session.shutdown().await;
        Ok(())
    }

    async fn has_session(&self, thread_id: &ThreadId) -> bool {
        self.sessions.read().await.contains_key(thread_id)
    }

    async fn list_sessions(&self) -> Result<Vec<ProviderSession>, ProviderError> {
        let sessions = self.sessions.read().await;
        let mut out = Vec::with_capacity(sessions.len());
        for (thread_id, session) in sessions.iter() {
            out.push(ProviderSession {
                thread_id: thread_id.clone(),
                provider: ProviderKind::OpenCode,
                session_id: session.provider_session_id.clone(),
                status: SessionStatus::Ready,
                resume_cursor: None,
            });
        }
        Ok(out)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn capabilities_match_documented_surface() {
        let provider = OpenCodeAgentProvider::new(
            Arc::new(OpenCodeServerManager::new()),
            OpenCodeProviderConfig::default(),
        );
        let caps = provider.capabilities();
        assert!(caps.supports_mid_session_model_change);
        assert!(!caps.supports_mid_session_permission_change);
        assert!(caps.supports_synchronous_tool_approval);
        assert!(caps.supports_interrupt);
        assert!(!caps.supports_session_resume);
    }

    #[tokio::test]
    async fn kind_returns_opencode() {
        let provider = OpenCodeAgentProvider::new(
            Arc::new(OpenCodeServerManager::new()),
            OpenCodeProviderConfig::default(),
        );
        assert_eq!(provider.kind(), ProviderKind::OpenCode);
    }

    #[tokio::test]
    async fn unknown_thread_returns_session_not_found() {
        let provider = OpenCodeAgentProvider::new(
            Arc::new(OpenCodeServerManager::new()),
            OpenCodeProviderConfig::default(),
        );
        let err = provider
            .interrupt_turn(ThreadId("nope".into()), None)
            .await
            .expect_err("must fail");
        match err {
            ProviderError::SessionNotFound { thread_id } => {
                assert_eq!(thread_id.0, "nope");
            }
            other => panic!("wrong error: {other:?}"),
        }
    }

    #[tokio::test]
    async fn set_permission_mode_returns_validation_error() {
        let provider = OpenCodeAgentProvider::new(
            Arc::new(OpenCodeServerManager::new()),
            OpenCodeProviderConfig::default(),
        );
        let err = provider
            .set_permission_mode(ThreadId("t1".into()), "bypass".into())
            .await
            .expect_err("must fail");
        assert!(matches!(err, ProviderError::ValidationError { .. }));
    }

    #[tokio::test]
    async fn list_sessions_returns_empty_on_fresh_provider() {
        let provider = OpenCodeAgentProvider::new(
            Arc::new(OpenCodeServerManager::new()),
            OpenCodeProviderConfig::default(),
        );
        assert!(provider.list_sessions().await.unwrap().is_empty());
    }
}
