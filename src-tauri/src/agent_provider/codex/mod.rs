//! Codex [`AgentProvider`](crate::agent_provider::AgentProvider)
//! implementation.
//!
//! Wraps the `codex app-server` subprocess (JSON-RPC 2.0 over stdio) as a
//! first-class provider. Everything in this module is scaffolding — no
//! Tauri commands, no UI wiring — but it is complete enough to drive
//! through the trait surface once later tasks hook it up.
//!
//! # Architectural notes
//!
//! * One [`session::CodexSession`] per runtime thread. Each owns its own
//!   `codex app-server` child; Codex does not support thread multiplexing
//!   inside one server.
//! * Notifications and server-initiated requests are consumed by
//!   background tasks that call [`translate`] and broadcast canonical
//!   events on a single `tokio::sync::broadcast` channel.
//! * [`event_stream`](CodexAgentProvider::event_stream) hands out fresh
//!   subscribers. The broadcast channel has a fixed capacity
//!   ([`CodexProviderConfig::event_channel_capacity`]); slow subscribers
//!   lose old events — this is a deliberate UI-compatible semantic.
//! * Dropping the provider signals every live session to shut down. Any
//!   background tasks still awaiting on the broadcaster observe
//!   `Closed` and exit cleanly.

pub mod auth;
pub mod protocol;
pub(crate) mod session;
pub mod translate;

use std::collections::HashMap;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures_core::Stream;
use tokio::sync::{broadcast, RwLock};

use crate::agent_provider::{
    AgentProvider, ApprovalDecision, ProviderCapabilities, ProviderError,
    ProviderEventStream, ProviderKind, ProviderRuntimeEvent, ProviderSession,
    RequestId, SendTurnInput, SessionStatus, StartSessionInput, ThreadId,
    TurnId, TurnStartResult,
};

use self::protocol::{ApprovalResponse, ClientInfo};
use self::session::{CodexSession, CodexSpawnConfig};

/// Configuration for the [`CodexAgentProvider`].
#[derive(Debug, Clone)]
pub struct CodexProviderConfig {
    /// Path to the `codex` binary. Defaults to `"codex"` (search on PATH).
    pub codex_binary: PathBuf,
    /// Optional `CODEX_HOME` override applied to every session.
    pub codex_home: Option<PathBuf>,
    /// Capacity of the broadcast channel that fans canonical events out
    /// to subscribers. Larger values absorb bursty traffic; slow
    /// subscribers miss old events when the buffer wraps.
    pub event_channel_capacity: usize,
    /// Client identification to report at `initialize` time. Affects
    /// Codex's server-side logs/tracing only.
    pub client_info: ClientInfo,
}

impl Default for CodexProviderConfig {
    fn default() -> Self {
        Self {
            codex_binary: PathBuf::from("codex"),
            codex_home: None,
            event_channel_capacity: 1024,
            client_info: ClientInfo {
                name: "codemux".to_string(),
                title: "Codemux".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
            },
        }
    }
}

/// `AgentProvider` implementation for the Codex `app-server` binary.
pub struct CodexAgentProvider {
    config: CodexProviderConfig,
    sessions: Arc<RwLock<HashMap<ThreadId, Arc<CodexSession>>>>,
    event_tx: broadcast::Sender<ProviderRuntimeEvent>,
}

impl CodexAgentProvider {
    /// Construct a new, empty provider. Sessions are lazily spawned by
    /// [`start_session`](AgentProvider::start_session).
    pub fn new(config: CodexProviderConfig) -> Self {
        let (event_tx, _) = broadcast::channel(config.event_channel_capacity.max(16));
        Self {
            config,
            sessions: Arc::new(RwLock::new(HashMap::new())),
            event_tx,
        }
    }

    fn spawn_config(&self) -> CodexSpawnConfig {
        CodexSpawnConfig {
            codex_binary: self.config.codex_binary.clone(),
            codex_home: self.config.codex_home.clone(),
            client_info: self.config.client_info.clone(),
        }
    }

    /// Snapshot of the current live-session map. Useful for tests and
    /// graceful shutdown.
    async fn collect_sessions(&self) -> Vec<Arc<CodexSession>> {
        let sessions = self.sessions.read().await;
        sessions.values().cloned().collect()
    }
}

/// Cleanup semantics when the provider is dropped.
///
/// Two paths, both of which reap every live child process:
///
/// 1. **Normal path.** `tokio::runtime::Handle::try_current()` returns
///    `Ok`, meaning the Tokio runtime is still alive. We spawn a cleanup
///    task that iterates every live [`CodexSession`] and calls
///    `session.shutdown().await`, which closes stdin (giving the child a
///    chance to exit cleanly on EOF), then falls through to `kill` if it
///    has not exited within the graceful-shutdown window.
///
/// 2. **Fallback path.** If `try_current()` returns `Err` — the runtime
///    has already been torn down, e.g. deep in process shutdown — the
///    spawned-task branch is skipped entirely. Cleanup then relies on
///    `tokio::process::Command::kill_on_drop(true)` set on every
///    [`JsonRpcChild`]: when each [`CodexSession`] is dropped, its
///    `Arc<JsonRpcChild>` drops, which drops the underlying
///    `tokio::process::Child`, which sends `SIGKILL` to the subprocess.
///
/// Either way, no child processes are leaked. Graceful stdin-EOF
/// shutdown only happens on the normal path; the fallback is SIGKILL.
impl Drop for CodexAgentProvider {
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
impl AgentProvider for CodexAgentProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Codex
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            supports_mid_session_model_change: true,
            supports_mid_session_permission_change: false,
            supports_synchronous_tool_approval: true,
            supports_interrupt: true,
            supports_session_resume: true,
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
                        "codex session already exists for thread {:?}",
                        thread_id.0
                    ),
                });
            }
        }

        let session = CodexSession::spawn_and_initialize(
            thread_id.clone(),
            input.cwd,
            input.model,
            input.resume_cursor.clone(),
            self.spawn_config(),
            self.event_tx.clone(),
        )
        .await?;

        {
            let mut sessions = self.sessions.write().await;
            sessions.insert(thread_id.clone(), Arc::clone(&session));
        }

        Ok(ProviderSession {
            thread_id,
            provider: ProviderKind::Codex,
            session_id: session.provider_session_id.clone(),
            status: SessionStatus::Ready,
            resume_cursor: Some(serde_json::json!({
                "threadId": session.provider_session_id.0,
            })),
        })
    }

    async fn send_turn(&self, input: SendTurnInput) -> Result<TurnStartResult, ProviderError> {
        let session = {
            let sessions = self.sessions.read().await;
            sessions.get(&input.thread_id).cloned()
        };
        let session = session.ok_or_else(|| ProviderError::SessionNotFound {
            thread_id: input.thread_id.clone(),
        })?;
        let turn_id = session.send_turn(input.text, input.model_override).await?;
        Ok(TurnStartResult { turn_id })
    }

    async fn interrupt_turn(
        &self,
        thread_id: ThreadId,
        turn_id: Option<TurnId>,
    ) -> Result<(), ProviderError> {
        let session = {
            let sessions = self.sessions.read().await;
            sessions.get(&thread_id).cloned()
        };
        let session = session.ok_or(ProviderError::SessionNotFound { thread_id })?;
        session.interrupt_turn(turn_id).await
    }

    async fn respond_to_request(
        &self,
        thread_id: ThreadId,
        request_id: RequestId,
        decision: ApprovalDecision,
    ) -> Result<(), ProviderError> {
        let session = {
            let sessions = self.sessions.read().await;
            sessions.get(&thread_id).cloned()
        };
        let session = session.ok_or(ProviderError::SessionNotFound { thread_id })?;
        let response = ApprovalResponse::from(decision);
        session.respond_to_request(request_id, response).await
    }

    async fn set_model(&self, thread_id: ThreadId, model: String) -> Result<(), ProviderError> {
        let session = {
            let sessions = self.sessions.read().await;
            sessions.get(&thread_id).cloned()
        };
        let session = session.ok_or(ProviderError::SessionNotFound { thread_id })?;
        session.set_model(model).await;
        Ok(())
    }

    async fn set_permission_mode(
        &self,
        _thread_id: ThreadId,
        _mode: String,
    ) -> Result<(), ProviderError> {
        Err(ProviderError::ValidationError {
            message: "Codex does not support mid-session permission changes".into(),
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
        let _ = self.event_tx.send(ProviderRuntimeEvent::SessionStateChanged {
            thread_id,
            status: SessionStatus::Closed,
        });
        Ok(())
    }

    async fn list_sessions(&self) -> Result<Vec<ProviderSession>, ProviderError> {
        let sessions = self.collect_sessions().await;
        let mut out = Vec::with_capacity(sessions.len());
        for s in sessions {
            let status = {
                let state = s.state.lock().await;
                state.status.clone()
            };
            out.push(ProviderSession {
                thread_id: s.thread_id.clone(),
                provider: ProviderKind::Codex,
                session_id: s.provider_session_id.clone(),
                status,
                resume_cursor: Some(serde_json::json!({
                    "threadId": s.provider_session_id.0,
                })),
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
