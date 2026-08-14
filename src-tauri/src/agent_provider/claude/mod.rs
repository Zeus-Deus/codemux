//! Claude [`AgentProvider`](crate::agent_provider::AgentProvider)
//! implementation.
//!
//! Unlike the Codex adapter (which spawns `codex app-server`
//! directly), this adapter drives a small Bun-compiled sidecar
//! (`sidecar/claude-agent/`) that hosts Anthropic's Claude Agent
//! SDK in-process. All Claude inference goes through the SDK — the
//! Rust side never touches the Anthropic API, credential files, or
//! the `claude` binary directly.
//!
//! One sidecar subprocess per session. Per-session memory and crash
//! isolation; same trade-off the Codex adapter makes.

pub mod auth;
pub mod capabilities;
pub mod protocol;
pub mod slash_commands;
pub(crate) mod session;
pub mod sidecar_path;
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
    ProviderEventStream, ProviderKind, ProviderRuntimeEvent, ProviderSession, RequestId,
    SendTurnInput, SessionStatus, StartSessionInput, ThreadId, TurnId, TurnStartResult,
};

use self::session::{ClaudeSession, ClaudeSpawnConfig};

/// Configuration for the Claude provider. Both binary paths are
/// optional — sensible defaults kick in when they're `None`.
#[derive(Debug, Clone, Default)]
pub struct ClaudeProviderConfig {
    /// Path to the bundled claude-agent sidecar. When `None`, the
    /// provider resolves the path via
    /// [`sidecar_path::resolve_sidecar_path`] at construction time.
    pub sidecar_binary: Option<PathBuf>,
    /// Path to the user's local `claude` CLI. When `None`, defaults
    /// to `"claude"` and relies on PATH resolution inside the
    /// sidecar's SDK.
    pub claude_binary: Option<PathBuf>,
    /// Capacity of the broadcast channel that fans canonical events
    /// out to subscribers. Larger values absorb bursty traffic at
    /// the cost of memory; slow subscribers miss old events when
    /// the buffer wraps.
    pub event_channel_capacity: usize,
    /// MCP runtime registry. When provided, the sidecar can RPC back
    /// to Codemux with `mcp-tool-call` requests and they get routed
    /// to the appropriate user-installed MCP child via
    /// [`crate::mcp::registry::McpRegistry::dispatch_tool_call`].
    /// `None` is fine for tests that don't exercise MCPs — sidecar
    /// requests for `mcp-tool-call` will be rejected with
    /// `method-not-found` semantics.
    pub mcp_registry: Option<crate::mcp::registry::McpRegistry>,
}

/// `AgentProvider` implementation for Claude Code, backed by the
/// bundled sidecar.
pub struct ClaudeAgentProvider {
    sidecar_binary: PathBuf,
    claude_binary: PathBuf,
    sessions: Arc<RwLock<HashMap<ThreadId, Arc<ClaudeSession>>>>,
    event_tx: broadcast::Sender<ProviderRuntimeEvent>,
    mcp_registry: Option<crate::mcp::registry::McpRegistry>,
}

impl ClaudeAgentProvider {
    /// Construct a new provider. Resolves the sidecar binary path
    /// eagerly so callers see `NotInstalled` immediately if the
    /// bundle is missing.
    pub async fn new(config: ClaudeProviderConfig) -> Result<Self, ProviderError> {
        let sidecar_binary = match config.sidecar_binary {
            Some(p) => p,
            None => sidecar_path::resolve_sidecar_path()?,
        };
        let claude_binary = config
            .claude_binary
            .unwrap_or_else(|| PathBuf::from("claude"));
        let capacity = if config.event_channel_capacity == 0 {
            1024
        } else {
            config.event_channel_capacity
        };
        let (event_tx, _) = broadcast::channel(capacity.max(16));
        Ok(Self {
            sidecar_binary,
            claude_binary,
            sessions: Arc::new(RwLock::new(HashMap::new())),
            event_tx,
            mcp_registry: config.mcp_registry,
        })
    }

    fn spawn_config(&self) -> ClaudeSpawnConfig {
        ClaudeSpawnConfig {
            sidecar_binary: self.sidecar_binary.clone(),
            claude_binary: self.claude_binary.clone(),
            mcp_registry: self.mcp_registry.clone(),
        }
    }

    async fn collect_sessions(&self) -> Vec<Arc<ClaudeSession>> {
        let sessions = self.sessions.read().await;
        sessions.values().cloned().collect()
    }
}

/// Cleanup semantics when the provider is dropped.
///
/// Two paths, both of which reap every live sidecar subprocess:
///
/// 1. **Normal path.** `tokio::runtime::Handle::try_current()` returns
///    `Ok`, meaning the Tokio runtime is still alive. We spawn a
///    cleanup task that iterates every live [`ClaudeSession`] and
///    calls `session.shutdown().await`, which in turn asks the
///    sidecar to stop cleanly, then closes the
///    [`crate::json_rpc_child::JsonRpcChild`].
///
/// 2. **Fallback path.** If `try_current()` returns `Err` — the
///    runtime has already been torn down — the spawned-task branch is
///    skipped. Cleanup falls back to
///    `tokio::process::Command::kill_on_drop(true)` set on every
///    `JsonRpcChild`: dropping each session's `Arc<JsonRpcChild>`
///    sends `SIGKILL` to the sidecar subprocess.
///
/// Either way, no subprocesses are leaked. Graceful shutdown only
/// happens on the normal path.
impl Drop for ClaudeAgentProvider {
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
impl AgentProvider for ClaudeAgentProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Claude
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            supports_mid_session_model_change: true,
            supports_mid_session_permission_change: true,
            supports_synchronous_tool_approval: true,
            supports_interrupt: true,
            supports_session_resume: true,
            // Truncating the adapter's local cursor is not an upstream rewind.
            supports_conversation_rollback: false,
        }
    }

    async fn start_session(
        &self,
        input: StartSessionInput,
    ) -> Result<ProviderSession, ProviderError> {
        let thread_id = input.thread_id.clone();
        // Evict a corpse before rebuilding: if the child-exit watchdog marked
        // the existing session dead, remove it under the write lock (so the
        // check→remove is atomic against a concurrent rebuild) and shut it down
        // cleanly below. A still-live session for this thread is a genuine
        // double-start and stays a ValidationError.
        let dead_evicted = {
            let mut sessions = self.sessions.write().await;
            match sessions.get(&thread_id) {
                Some(existing) if existing.is_dead() => sessions.remove(&thread_id),
                Some(_) => {
                    return Err(ProviderError::ValidationError {
                        message: format!(
                            "claude session already exists for thread {:?}",
                            thread_id.0
                        ),
                    });
                }
                None => None,
            }
        };
        if let Some(dead) = dead_evicted {
            // Best-effort: reap the dead sidecar's tasks before spawning the
            // replacement. The child is already gone; this just tidies handles.
            dead.shutdown().await;
        }
        let session = ClaudeSession::spawn_and_initialize(
            thread_id.clone(),
            input,
            self.spawn_config(),
            self.event_tx.clone(),
        )
        .await?;
        let session_id = session.provider_session_id.clone();
        let sdk_session_id = session.state.lock().await.sdk_session_id.clone();
        {
            let mut sessions = self.sessions.write().await;
            sessions.insert(thread_id.clone(), Arc::clone(&session));
        }
        Ok(ProviderSession {
            thread_id,
            provider: ProviderKind::Claude,
            session_id: session_id.clone(),
            status: SessionStatus::Ready,
            // `resume_cursor` is the SDK's own session_id, not the
            // runtime thread_id. At start-session time the SDK hasn't
            // yet assigned one — it arrives on the first SDK message
            // as `ResumeCursorUpdated`. Return `None` here; the
            // frontend learns the cursor via the event stream.
            resume_cursor: sdk_session_id
                .map(|id| serde_json::json!({ "resume": id })),
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
        Ok(match session.enqueue_or_send(input).await? {
            crate::agent_provider::SendOutcome::Started(turn_id) => TurnStartResult {
                turn_id,
                queued_id: None,
            },
            crate::agent_provider::SendOutcome::Queued(queued_id) => TurnStartResult {
                // No live turn yet — the real id arrives on
                // QueuedTurnDispatched. Empty placeholder keeps the shape.
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
        let session = {
            let sessions = self.sessions.read().await;
            sessions.get(&thread_id).cloned()
        };
        let session = session.ok_or(ProviderError::SessionNotFound { thread_id })?;
        session.interrupt(turn_id).await
    }

    async fn cancel_queued_turn(
        &self,
        thread_id: ThreadId,
        queued_id: String,
    ) -> Result<bool, ProviderError> {
        let session = {
            let sessions = self.sessions.read().await;
            sessions.get(&thread_id).cloned()
        };
        // A missing session means nothing is queued — treat as a no-op.
        let Some(session) = session else {
            return Ok(false);
        };
        session.cancel_queued(&queued_id).await
    }

    async fn send_queued_turn_now(
        &self,
        thread_id: ThreadId,
        queued_id: String,
    ) -> Result<(), ProviderError> {
        let session = {
            let sessions = self.sessions.read().await;
            sessions.get(&thread_id).cloned()
        };
        // A missing session means nothing is queued — treat as a no-op.
        let Some(session) = session else {
            return Ok(());
        };
        session.send_queued_now(&queued_id).await
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
        session.respond_to_request(request_id, decision).await
    }

    async fn set_model(&self, thread_id: ThreadId, model: String) -> Result<(), ProviderError> {
        let session = {
            let sessions = self.sessions.read().await;
            sessions.get(&thread_id).cloned()
        };
        let session = session.ok_or(ProviderError::SessionNotFound { thread_id })?;
        session.set_model(Some(model)).await
    }

    async fn set_permission_mode(
        &self,
        thread_id: ThreadId,
        mode: String,
    ) -> Result<(), ProviderError> {
        let session = {
            let sessions = self.sessions.read().await;
            sessions.get(&thread_id).cloned()
        };
        let session = session.ok_or(ProviderError::SessionNotFound { thread_id })?;
        session.set_permission_mode(mode).await
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

    async fn has_session(&self, thread_id: &ThreadId) -> bool {
        // A session whose sidecar died unintentionally (child-exit watchdog
        // set `dead`) is treated as absent, so `ensure_live_session` rebuilds
        // a fresh one (with the resume cursor) on the next send instead of
        // routing to a dead child. Mirrors the OpenCode provider.
        self.sessions
            .read()
            .await
            .get(thread_id)
            .is_some_and(|session| !session.is_dead())
    }

    async fn turn_active(&self, thread_id: &ThreadId) -> bool {
        // Cheap in-memory check for the frontend hydrate path: a live
        // (non-dead) session bound to the thread with `active_turn` set. Does
        // not touch the sidecar. A dead session (watchdog fired) reports false
        // even if a turn was mid-flight when the child exited.
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
        let state = session.state.lock().await;
        state.active_turn.is_some()
    }

    async fn list_sessions(&self) -> Result<Vec<ProviderSession>, ProviderError> {
        let sessions = self.collect_sessions().await;
        let mut out = Vec::with_capacity(sessions.len());
        for s in sessions {
            let (status, sdk_session_id) = {
                let state = s.state.lock().await;
                (state.status.clone(), state.sdk_session_id.clone())
            };
            out.push(ProviderSession {
                thread_id: s.thread_id.clone(),
                provider: ProviderKind::Claude,
                session_id: s.provider_session_id.clone(),
                status,
                resume_cursor: sdk_session_id
                    .map(|id| serde_json::json!({ "resume": id })),
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
