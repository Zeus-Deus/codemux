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

/// The resume cursor for an OpenCode session. The server-side session id IS
/// the resume handle (OpenCode persists sessions on disk and readopts them by
/// id), so the cursor is simply `{"resume": <session_id>}` — the same shape
/// `ensure_live_session` threads back into `StartSessionInput.resume_cursor`
/// and, crucially, the shape `extract_sdk_session_id` in
/// `commands::agent_chat` knows how to persist. Keep those three in lockstep:
/// a key rename here silently breaks the start-time cursor persist.
pub fn resume_cursor_for(session_id: &crate::agent_provider::ProviderSessionId) -> serde_json::Value {
    serde_json::json!({ "resume": session_id.0 })
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
            // OpenCode persists sessions on disk. On a live rebuild after a
            // dead SSE listener, `start_session` readopts the saved server-side
            // session id (validated via `GET /session/{id}/message`) so the
            // resumed session keeps its conversation context, falling back to a
            // fresh session when the id is stale/unknown. Best-effort but wired.
            supports_session_resume: true,
        }
    }

    async fn start_session(
        &self,
        input: StartSessionInput,
    ) -> Result<ProviderSession, ProviderError> {
        let thread_id = input.thread_id.clone();
        // Evict a corpse before rebuilding: if the SSE listener gave up on an
        // unreachable server it flipped the session's `dead` flag but left the
        // entry in the map (only `stop_session` removes). Without this, the
        // rebuild `ensure_live_session` drives after `has_session` reports the
        // dead session absent would hit the `contains_key` guard below and fail
        // permanently. Remove the dead entry under the write lock (atomic
        // check→remove against a concurrent rebuild) and shut it down cleanly
        // below. A still-live session for this thread is a genuine double-start
        // and stays a ValidationError.
        let dead_evicted = {
            let mut sessions = self.sessions.write().await;
            match sessions.get(&thread_id) {
                Some(existing) if existing.is_dead() => sessions.remove(&thread_id),
                Some(_) => {
                    return Err(ProviderError::ValidationError {
                        message: format!(
                            "opencode session already exists for thread {:?}",
                            thread_id.0
                        ),
                    });
                }
                None => None,
            }
        };
        // Drop the corpse WITHOUT calling `shutdown()`: its SSE listener task
        // has already exited (the give-up path returns after flipping `dead`),
        // so there is nothing to abort, and `shutdown()` would
        // `DELETE /session/{id}` — destroying the very server-side session the
        // resume below wants to readopt. Just let the Arc drop.
        drop(dead_evicted);
        // Best-effort resume: `ensure_live_session` passes
        // `{"resume": <opencode_session_id>}` when the persisted row carries the
        // server-side session id. OpenCode keeps sessions on disk, so a rebuilt
        // session can readopt that id and keep its conversation context instead
        // of starting blank. `OpenCodeSession::start` validates the id and falls
        // back to a fresh session when it is stale/unknown.
        let resume_session_id = input
            .resume_cursor
            .as_ref()
            .and_then(super::session::resume_session_id_from_cursor);
        let session = OpenCodeSession::start(
            self.manager.clone(),
            thread_id.clone(),
            input.model.clone(),
            input.effort.clone(),
            resume_session_id,
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
            session_id: provider_session_id.clone(),
            status: SessionStatus::Ready,
            // The OpenCode session id IS the resume cursor (the server keeps
            // sessions on disk and readopts them by id). Returning it here —
            // not only via the async `ResumeCursorUpdated` event — lets
            // `agent_chat_start_session` persist it synchronously after the
            // session row is upserted, so the FIRST dead-run rebuild already
            // finds a cursor. (The event alone races the row upsert: its
            // persist is a plain UPDATE that silently hits 0 rows when the
            // bridge task wins.) Mirrors the Codex adapter's shape.
            resume_cursor: Some(resume_cursor_for(&provider_session_id)),
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
        // OpenCode has no busy guard and no follow-up queue yet — every
        // send starts immediately, so `queued_id` is always `None`.
        Ok(TurnStartResult {
            turn_id,
            queued_id: None,
        })
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

    fn pending_requests_survive_session_restart(&self) -> bool {
        // OpenCode stores permission requests in its HTTP server and accepts
        // replies by session/request id. Re-adopting the server-side session
        // can therefore preserve an outstanding request across a Codemux
        // process restart, unlike Claude/Codex's in-process callbacks.
        true
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
        // A session whose SSE listener gave up on an unreachable server is
        // treated as absent, so `ensure_live_session` rebuilds a fresh one
        // (with the resume cursor) on the next send instead of routing to a
        // dead server.
        self.sessions
            .read()
            .await
            .get(thread_id)
            .is_some_and(|session| !session.is_dead())
    }

    async fn turn_active(&self, thread_id: &ThreadId) -> bool {
        // Cheap in-memory check for the frontend hydrate path: a live
        // (non-dead) session bound to the thread whose SSE routing context has
        // `turn_active` armed. Does not touch the server. A dead session
        // (SSE listener gave up) reports false.
        let session = {
            let sessions = self.sessions.read().await;
            sessions.get(thread_id).cloned()
        };
        let Some(session) = session else {
            return false;
        };
        if session.is_dead() {
            return false;
        }
        session.turn_active().await
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
                // Same cursor `start_session` returns — the session id is
                // the resume handle.
                resume_cursor: Some(resume_cursor_for(&session.provider_session_id)),
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
        assert!(caps.supports_session_resume);
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

    #[test]
    fn resume_cursor_round_trips_through_the_command_layer_extractor() {
        // `agent_chat_start_session` persists the start-time cursor via
        // `extract_sdk_session_id`, and `ensure_live_session` rebuilds with
        // `{"resume": <id>}` — this pins the three shapes in lockstep. A key
        // rename in `resume_cursor_for` would silently break the synchronous
        // start-time persist (the fallback event path races the row upsert),
        // losing the FIRST dead-run rebuild's conversation context.
        use crate::agent_provider::ProviderSessionId;
        let cursor = resume_cursor_for(&ProviderSessionId("ses_abc".into()));
        assert_eq!(
            crate::commands::agent_chat::extract_sdk_session_id(&cursor),
            Some("ses_abc".to_string())
        );
        assert_eq!(
            super::super::session::resume_session_id_from_cursor(&cursor),
            Some("ses_abc".to_string())
        );
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
