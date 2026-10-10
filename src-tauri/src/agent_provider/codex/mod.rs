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
pub mod chatgpt;
pub mod capabilities;
pub mod protocol;
pub mod slash_commands;
pub mod hooks;
pub(crate) mod session;
pub mod translate;

use std::collections::HashMap;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use futures_core::Stream;
use tokio::sync::{broadcast, Mutex, RwLock};

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
    /// Process-wide MCP registry whose tools are exposed as Codex dynamic
    /// tools. `None` keeps the adapter usable in isolated tests.
    pub mcp_registry: Option<crate::mcp::registry::McpRegistry>,
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
            mcp_registry: None,
        }
    }
}

/// `AgentProvider` implementation for the Codex `app-server` binary.
pub struct CodexAgentProvider {
    config: CodexProviderConfig,
    sessions: Arc<RwLock<HashMap<ThreadId, Arc<CodexSession>>>>,
    event_tx: broadcast::Sender<ProviderRuntimeEvent>,
    hooks_operation: Mutex<()>,
    chatgpt_owner: Option<Arc<chatgpt::Owner>>,
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
            hooks_operation: Mutex::new(()),
            chatgpt_owner: None,
        }
    }

    /// Only the default production registry opts into app-owned ChatGPT routing.
    /// Custom configurations and synthetic adapters constructed with `new` are unchanged.
    pub fn new_managed(config:CodexProviderConfig)->Self{
        let mut value=Self::new(config);value.chatgpt_owner=Some(chatgpt::owner());value
    }
    fn spawn_config(&self) -> CodexSpawnConfig {
        CodexSpawnConfig {
            codex_binary: self.config.codex_binary.clone(),
            chatgpt_owner: self.chatgpt_owner.clone(),
            codex_home: self.config.codex_home.clone(),
            client_info: self.config.client_info.clone(),
            mcp_registry: self.config.mcp_registry.clone(),
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
            supports_conversation_rollback: true,
        }
    }

    async fn start_session(
        &self,
        input: StartSessionInput,
    ) -> Result<ProviderSession, ProviderError> {
        let thread_id = input.thread_id.clone();
        // Wait for renewal/dispatch without holding the session map: dispatch
        // can own outbound while waiting for the checkpoint timeline, whose
        // revert owner calls turn_active (and therefore reads the session map).
        let dead_evicted = loop {
            let existing = self.sessions.read().await.get(&thread_id).cloned();
            let Some(existing) = existing else { break None };
            let _lifecycle = existing.lifecycle_guard().await;
            let mut sessions = self.sessions.write().await;
            // Stop or another rebuild may have changed the binding while we
            // waited. Never apply the old Arc's death classification to it.
            if !sessions.get(&thread_id)
                .is_some_and(|current| Arc::ptr_eq(current, &existing)) {
                continue;
            }
            if !existing.is_dead() {
                return Err(ProviderError::ValidationError {
                    message: format!(
                        "codex session already exists for thread {:?}",
                        thread_id.0
                    ),
                });
            }
            break sessions.remove(&thread_id);
        };
        if let Some(dead) = dead_evicted {
            // Best-effort: reap the dead child's tasks before spawning the
            // replacement. The child is already gone; this just tidies handles.
            dead.shutdown().await;
        }

        let session = CodexSession::spawn_and_initialize(
            thread_id.clone(),
            input.cwd,
            input.model,
            input.permission_mode,
            input.effort,
            input.fast_mode,
            input.resume_cursor.clone(),
            input.env,
            input.workspace_id,
            self.spawn_config(),
            self.event_tx.clone(),
            input.recorded_usage_baseline,
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

    fn supports_async_questions(&self) -> bool {
        true
    }

    async fn answer_question(
        &self,
        input: crate::agent_provider::AnswerQuestionInput,
    ) -> Result<crate::agent_provider::QuestionDelivery, crate::agent_provider::QuestionDeliveryError>
    {
        let session = self
            .sessions
            .read()
            .await
            .get(&input.thread_id)
            .cloned()
            .ok_or_else(|| {
                crate::agent_provider::QuestionDeliveryError::Rejected(
                    "The Codex session is not available.".into(),
                )
            })?;
        session.answer_question(input).await
    }

    async fn find_question_answer(
        &self,
        thread_id: ThreadId,
        target: String,
        submission_id: String,
    ) -> Result<Option<crate::agent_provider::QuestionDelivery>, String> {
        let session = self
            .sessions
            .read()
            .await
            .get(&thread_id)
            .cloned()
            .ok_or("The Codex session is not available.")?;
        session.find_question_answer(&target, &submission_id).await
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
                steered: false,
                turn_id,
                queued_id: None,
            },
            crate::agent_provider::SendOutcome::Queued(queued_id) => TurnStartResult {
                steered: false,
                turn_id: TurnId(String::new()),
                queued_id: Some(queued_id),
            },
        })
    }

    async fn steer_turn(&self, input: SendTurnInput) -> Result<TurnStartResult, ProviderError> {
        let session = self
            .sessions
            .read()
            .await
            .get(&input.thread_id)
            .cloned()
            .ok_or_else(|| ProviderError::SessionNotFound {
                thread_id: input.thread_id.clone(),
            })?;
        session.steer_turn(input).await
    }

    async fn steer_queued_turn(
        &self,
        thread_id: ThreadId,
        queued_id: String,
    ) -> Result<(), ProviderError> {
        let session = self
            .sessions
            .read()
            .await
            .get(&thread_id)
            .cloned()
            .ok_or_else(|| ProviderError::SessionNotFound { thread_id })?;
        session.steer_queued(&queued_id).await
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

    async fn rollback_conversation(
        &self,
        thread_id: ThreadId,
        num_turns: u32,
    ) -> Result<(), ProviderError> {
        let session = {
            let sessions = self.sessions.read().await;
            sessions.get(&thread_id).cloned()
        };
        let session = session.ok_or(ProviderError::SessionNotFound { thread_id })?;
        session.rollback_conversation(num_turns).await
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

    async fn set_fast_mode(
        &self,
        thread_id: ThreadId,
        fast_mode: bool,
    ) -> Result<(), ProviderError> {
        let session = {
            let sessions = self.sessions.read().await;
            sessions.get(&thread_id).cloned()
        };
        let session = session.ok_or(ProviderError::SessionNotFound { thread_id })?;
        session.set_fast_mode(fast_mode).await;
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

    async fn has_session(&self, thread_id: &ThreadId) -> bool {
        // A session whose `codex app-server` died unintentionally (child-exit
        // watchdog set `dead`) is treated as absent, so `ensure_live_session`
        // rebuilds a fresh one (with the resume cursor) on the next send
        // instead of routing to a dead child. Mirrors the OpenCode provider.
        let session = self.sessions.read().await.get(thread_id).cloned();
        let Some(session) = session else { return false };
        if !session.is_dead() {
            return true;
        }
        // Do not classify a retired child as an absent session until its
        // managed renewal/dispatch owner releases outbound. Drop the map read
        // lock before waiting so unrelated provider operations remain live.
        let _lifecycle = session.lifecycle_guard().await;
        !session.is_dead()
    }

    async fn turn_active(&self, thread_id: &ThreadId) -> bool {
        // Cheap in-memory check for the frontend hydrate path: a live
        // (non-dead) session bound to the thread with `active_turn` set. Does
        // not touch the `codex app-server`. A dead session (watchdog fired)
        // reports false even if a turn was mid-flight when the child exited.
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

    async fn manage_hooks(
        &self,
        cwd: &std::path::Path,
        thread_id: Option<ThreadId>,
        update: Option<hooks::HookUpdate>,
    ) -> Result<hooks::HooksList, String> {
        let _operation = self.hooks_operation.lock().await;
        let cwd = cwd.canonicalize().map_err(|error| format!("Cannot open hooks for this directory: {error}"))?;
        if let Some(thread_id) = thread_id {
            let session = self.sessions.read().await.get(&thread_id).cloned();
            if let Some(session) = session.filter(|session| !session.is_dead()) {
                if session.cwd.canonicalize().map_err(|error| error.to_string())? != cwd {
                    return Err("Hook discovery must use this conversation's directory.".into());
                }
                return hooks::manage(session.child().await.as_ref(), &cwd, update).await;
            }
        }
        // Home/new drafts have no session. Probe without creating a thread
        // or starting inference, using the same executable and Codex home.
        let lease=self.chatgpt_owner.as_ref().map(|o|o.acquire_runtime().map(Arc::new)).transpose()?;
        let grant=match self.chatgpt_owner.as_ref(){Some(owner)=>owner.usable().await?,None=>None};
        let mut env=HashMap::new();if let Some(home)=self.config.codex_home.as_ref(){env.insert("CODEX_HOME".into(),home.to_string_lossy().into_owned());}
        let(args,env)=match grant.as_ref(){Some(grant)=>grant.launch(env),None=>(vec!["app-server".into()],env)};
        let config=crate::json_rpc_child::SpawnConfig{program:self.config.codex_binary.clone(),args,env,cwd:Some(cwd.clone()),default_timeout:std::time::Duration::from_secs(15)};
        let child=match lease.clone(){Some(lease)=>crate::json_rpc_child::JsonRpcChild::spawn_owned(config,grant.is_some(),lease).await,None=>crate::json_rpc_child::JsonRpcChild::spawn(config).await}.map_err(|e|e.to_string())?;
        let result = async {
            child.request("initialize", serde_json::json!({
                "clientInfo": self.config.client_info,
                "capabilities": {"experimentalApi": true},
            })).await.map_err(|error| error.to_string())?;
            child.notify("initialized", serde_json::json!({})).await.map_err(|error| error.to_string())?;
            hooks::manage(&child, &cwd, update).await
        }.await;
        if lease.is_some(){let _=child.shutdown_owned().await;}else{let _=child.shutdown().await;}
        result
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
