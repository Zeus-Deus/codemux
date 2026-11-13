//! Google Antigravity provider using the `agy acp` Agent Client Protocol server.

pub mod capabilities;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::{broadcast, RwLock};

use crate::agent_provider::acp::session::{AcpDialect, AcpSession, AcpSpawnConfig};
use crate::agent_provider::acp::slash_commands::AcpSlashCommandCache;
use crate::agent_provider::{
    AgentProvider, ApprovalDecision, ProviderCapabilities, ProviderError, ProviderEventStream,
    ProviderKind, ProviderRuntimeEvent, ProviderSession, RequestId, SendOutcome, SendTurnInput,
    SessionStatus, StartSessionInput, ThreadId, TurnId, TurnStartResult,
};

#[derive(Debug, Clone)]
pub struct AntigravityProviderConfig {
    pub binary: PathBuf,
    pub event_channel_capacity: usize,
    /// Registry whose tools local chats receive through the MCP gateway.
    pub mcp_registry: Option<crate::mcp::registry::McpRegistry>,
}

impl Default for AntigravityProviderConfig {
    fn default() -> Self {
        Self {
            binary: PathBuf::from("agy"),
            event_channel_capacity: 1024,
            mcp_registry: None,
        }
    }
}

pub struct AntigravityAgentProvider {
    config: AntigravityProviderConfig,
    slash_command_cache: Arc<AcpSlashCommandCache>,
    sessions: Arc<RwLock<HashMap<ThreadId, Arc<AcpSession>>>>,
    event_tx: broadcast::Sender<ProviderRuntimeEvent>,
}

impl AntigravityAgentProvider {
    pub fn new(config: AntigravityProviderConfig) -> Self {
        Self::new_with_slash_command_cache(config, Arc::new(AcpSlashCommandCache::new()))
    }

    pub fn new_with_slash_command_cache(
        config: AntigravityProviderConfig,
        slash_command_cache: Arc<AcpSlashCommandCache>,
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

impl Drop for AntigravityAgentProvider {
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
impl AgentProvider for AntigravityAgentProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Antigravity
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            supports_mid_session_model_change: true,
            // Ask first / Full access is Codemux's own answer policy, and
            // plan is an Antigravity session mode; both switch live.
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
        let fast_mode = input.boolean_fast_mode()?;
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
                        message: format!("Antigravity session already exists for thread {}", thread_id.0),
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
            fast_mode,
            resume_cursor,
            input.env,
            AcpSpawnConfig {
                binary: self.config.binary.clone(),
                dialect: AcpDialect::Antigravity,
                slash_command_cache: Arc::clone(&self.slash_command_cache),
                remote: input.remote,
                mcp_registry: self.config.mcp_registry.clone(),
                workspace_id: input.workspace_id,
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
                    message: format!("Antigravity session already exists for thread {}", thread_id.0),
                });
            }
            sessions.insert(thread_id.clone(), Arc::clone(&session));
        }

        Ok(ProviderSession {
            thread_id,
            provider: ProviderKind::Antigravity,
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
        let session = self.session(&thread_id).await?;
        // A blanket Stop is the user's Stop button: hold the queue rather
        // than dispatch the next message the moment this turn settles.
        if turn_id.is_none() {
            session.hold_queue().await;
        }
        session.interrupt(turn_id).await
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

    async fn edit_queued_turn(
        &self,
        thread_id: ThreadId,
        queued_id: String,
        text: String,
    ) -> Result<bool, ProviderError> {
        self.session(&thread_id).await?.edit_queued(&queued_id, &text).await
    }

    async fn move_queued_turn(
        &self,
        thread_id: ThreadId,
        queued_id: String,
        before: Option<String>,
    ) -> Result<bool, ProviderError> {
        self.session(&thread_id)
            .await?
            .move_queued(&queued_id, before.as_deref())
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
                provider: ProviderKind::Antigravity,
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
        super::provider::broadcast_event_stream(&self.event_tx, self.kind())
    }
}

#[cfg(test)]
mod tests {
    //! Session-level checks against `fake_antigravity_acp`, which lists the
    //! Model option before Mode and offers thinking levels per model family.
    use super::*;
    use crate::agent_provider::{
        ApprovalDecision, CompletedItem, ProviderRuntimeEvent, SendTurnInput,
    };
    use futures_util::StreamExt;
    use serde_json::json;
    use std::time::Duration;

    fn fixture() -> PathBuf {
        let binary = std::env::current_exe().unwrap().parent().unwrap().parent().unwrap()
            .join(format!("fake_antigravity_acp{}", std::env::consts::EXE_SUFFIX));
        assert!(binary.is_file(), "build the fake_antigravity_acp test fixture first");
        binary
    }

    async fn started(mode: &str) -> (AntigravityAgentProvider, ProviderEventStream, ThreadId, tempfile::TempDir) {
        started_with(mode, None).await
    }

    async fn started_with(
        mode: &str,
        mode_log: Option<&std::path::Path>,
    ) -> (AntigravityAgentProvider, ProviderEventStream, ThreadId, tempfile::TempDir) {
        let provider = AntigravityAgentProvider::new(AntigravityProviderConfig {
            binary: fixture(),
            event_channel_capacity: 256,
            mcp_registry: None,
        });
        let events = provider.event_stream();
        let root = tempfile::tempdir().unwrap();
        let thread = ThreadId(format!("antigravity-{}", uuid::Uuid::new_v4()));
        provider
            .start_session(serde_json::from_value(json!({
                "thread_id": thread, "cwd": root.path(), "model": "gemini-flash",
                "effort": "minimal", "permission_mode": mode, "additional_directories": [],
                "env": mode_log.map(|path| json!({ "FAKE_AGY_MODE_LOG": path })),
            })).unwrap())
            .await
            .expect("start session");
        (provider, events, thread, root)
    }

    fn send(thread: &ThreadId, text: &str) -> SendTurnInput {
        serde_json::from_value(json!({ "thread_id": thread, "text": text })).unwrap()
    }

    /// Run one turn, answering the first request it opens with `answer`.
    async fn turn(
        provider: &AntigravityAgentProvider,
        events: &mut ProviderEventStream,
        thread: &ThreadId,
        text: &str,
        answer: Option<ApprovalDecision>,
    ) -> (Option<String>, String) {
        provider.send_turn(send(thread, text)).await.expect("send turn");
        let mut answer = answer;
        let mut opened = None;
        let mut reply = String::new();
        tokio::time::timeout(Duration::from_secs(20), async {
            while let Some(event) = events.next().await {
                match event {
                    ProviderRuntimeEvent::RequestOpened { request_id, request_kind, .. } => {
                        opened = Some(request_kind);
                        let decision = answer.take().expect("an unexpected request opened");
                        provider.respond_to_request(thread.clone(), request_id, decision).await.unwrap();
                    }
                    ProviderRuntimeEvent::ItemCompleted {
                        item: CompletedItem::AssistantText { text }, ..
                    } => reply = text,
                    ProviderRuntimeEvent::TurnCompleted { .. } => return,
                    _ => {}
                }
            }
        })
        .await
        .expect("turn timed out");
        (opened, reply)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn starts_with_model_first_options_and_per_family_thinking() {
        let (provider, mut events, thread, _root) = started("ask").await;
        let (_, reply) = turn(&provider, &mut events, &thread, "report", None).await;
        assert_eq!(reply, "mode:default model:gemini-flash thinking:minimal");
        provider.stop_session(thread).await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn full_access_answers_once_and_leaves_questions_to_the_user() {
        let (provider, mut events, thread, _root) = started("agent").await;
        let (opened, reply) = turn(&provider, &mut events, &thread, "need-approval", None).await;
        assert_eq!((opened, reply.as_str()), (None, "approval:once"));

        // Without an allow-once choice the request goes to the user flagged
        // for its exact options. A plain Allow never selects the lasting
        // option and is recorded as the cancel it sent; the exact option
        // is sent as chosen.
        let allow = ApprovalDecision::Allow { updated_input: None, updated_permissions: None };
        let (flagged, recorded, reply) = lasting_turn(&provider, &mut events, &thread, allow).await;
        assert!(flagged);
        assert_eq!(reply, "approval:cancelled");
        assert!(matches!(recorded, Some(ApprovalDecision::Cancel)), "{recorded:?}");
        let exact = ApprovalDecision::ProviderOption { option_id: "always".into() };
        let (_, recorded, reply) = lasting_turn(&provider, &mut events, &thread, exact).await;
        assert_eq!(reply, "approval:always");
        assert!(matches!(recorded, Some(ApprovalDecision::AllowForSession)), "{recorded:?}");

        let answer = ApprovalDecision::Allow {
            updated_input: Some(json!({ "answers": { "interaction_1": "SQLite" } })),
            updated_permissions: None,
        };
        let (opened, reply) = turn(&provider, &mut events, &thread, "ask-question", Some(answer)).await;
        assert_eq!((opened.as_deref(), reply.as_str()), (Some("user-input"), "answer:opt-b"));
        provider.stop_session(thread).await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn ask_first_never_grants_allow_always_and_holds_default_mode() {
        let (provider, mut events, thread, _root) = started("ask").await;
        let (opened, reply) = turn(
            &provider, &mut events, &thread, "need-approval", Some(ApprovalDecision::AllowForSession),
        ).await;
        assert_eq!((opened.as_deref(), reply.as_str()), (Some("tool_approval"), "approval:once"));

        // Antigravity widening itself to accept-edits is switched back.
        turn(&provider, &mut events, &thread, "self-mode:accept-edits", None).await;
        let mut mode = String::new();
        for _ in 0..20 {
            mode = turn(&provider, &mut events, &thread, "report", None).await.1;
            if mode.starts_with("mode:default") {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(mode.starts_with("mode:default"), "{mode}");
        provider.stop_session(thread).await.unwrap();
    }

    /// Plan holds until the user leaves it, and a correction that races the
    /// user's own switch to Plan never undoes it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn plan_survives_native_exits_and_racing_corrections() {
        let (provider, mut events, thread, _root) = started("ask").await;
        for _ in 0..5 {
            provider.set_permission_mode(thread.clone(), "ask".into()).await.unwrap();
            // The widening spawns a correction to `default` while the user
            // switches to Plan; whichever runs first, Plan must win.
            provider.send_turn(send(&thread, "self-mode:auto")).await.unwrap();
            provider.set_permission_mode(thread.clone(), "plan".into()).await.unwrap();
            wait_for_turn(&mut events).await;
            assert_eq!(report_mode(&provider, &mut events, &thread).await, "plan");
        }
        // Antigravity leaving plan on its own is put back.
        turn(&provider, &mut events, &thread, "self-mode:default", None).await;
        assert_eq!(report_mode(&provider, &mut events, &thread).await, "plan");
        provider.stop_session(thread).await.unwrap();
    }

    /// Antigravity acknowledges Plan and then switches itself to `auto`; the
    /// acknowledgement is committed only after that newer report has been
    /// applied, the exact interleaving that must not hide the drift.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_newer_native_report_outlives_the_acknowledged_mode() {
        let (provider, mut events, thread, _root) = started("ask").await;
        turn(&provider, &mut events, &thread, "arm-drift", None).await;
        crate::agent_provider::acp::session::antigravity_test_gate::hold_mode_commits_for(&thread.0);
        provider.set_permission_mode(thread.clone(), "plan".into()).await.unwrap();
        assert_eq!(report_mode(&provider, &mut events, &thread).await, "plan");
        provider.stop_session(thread).await.unwrap();
    }

    /// Full access → Plan while Antigravity drifts to `auto` mid-switch: the
    /// drift is judged against the old (Full access) policy, so the switch
    /// itself must correct it before returning, with no prompt involved.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn switching_policy_corrects_a_drift_judged_by_the_old_policy() {
        let log = tempfile::NamedTempFile::new().unwrap();
        let (provider, mut events, thread, _root) = started_with("agent", Some(log.path())).await;
        turn(&provider, &mut events, &thread, "arm-drift", None).await;
        crate::agent_provider::acp::session::antigravity_test_gate::hold_mode_commits_for(&thread.0);
        provider.set_permission_mode(thread.clone(), "plan".into()).await.unwrap();
        let modes = std::fs::read_to_string(log.path()).unwrap();
        assert_eq!(modes.lines().last(), Some("plan"), "{modes}");
        provider.stop_session(thread).await.unwrap();
    }

    /// A `need-lasting-approval` turn answered with `decision`: whether the
    /// request was flagged for exact options, the recorded decision, and
    /// the fixture's reply.
    async fn lasting_turn(
        provider: &AntigravityAgentProvider,
        events: &mut ProviderEventStream,
        thread: &ThreadId,
        decision: ApprovalDecision,
    ) -> (bool, Option<ApprovalDecision>, String) {
        provider.send_turn(send(thread, "need-lasting-approval")).await.unwrap();
        let (mut flagged, mut recorded, mut reply) = (false, None, String::new());
        let mut decision = Some(decision);
        tokio::time::timeout(Duration::from_secs(20), async {
            while let Some(event) = events.next().await {
                match event {
                    ProviderRuntimeEvent::RequestOpened { request_id, payload, .. } => {
                        flagged = payload["_codemuxExactOptions"] == json!(true);
                        let decision = decision.take().expect("one request");
                        provider.respond_to_request(thread.clone(), request_id, decision).await.unwrap();
                    }
                    ProviderRuntimeEvent::RequestResolved { decision, .. } => recorded = Some(decision),
                    ProviderRuntimeEvent::ItemCompleted { item: CompletedItem::AssistantText { text }, .. } => reply = text,
                    ProviderRuntimeEvent::TurnCompleted { .. } => return,
                    _ => {}
                }
            }
        })
        .await
        .expect("turn timed out");
        (flagged, recorded, reply)
    }

    async fn wait_for_turn(events: &mut ProviderEventStream) {
        tokio::time::timeout(Duration::from_secs(20), async {
            while let Some(event) = events.next().await {
                if matches!(event, ProviderRuntimeEvent::TurnCompleted { .. }) {
                    return;
                }
            }
        })
        .await
        .expect("turn timed out");
    }

    /// The fixture's native mode once any correction has landed.
    async fn report_mode(
        provider: &AntigravityAgentProvider,
        events: &mut ProviderEventStream,
        thread: &ThreadId,
    ) -> String {
        // `report` itself enforces the chat's mode before it is sent.
        let reply = turn(provider, events, thread, "report", None).await.1;
        reply
            .strip_prefix("mode:")
            .and_then(|rest| rest.split_whitespace().next())
            .unwrap_or_default()
            .to_string()
    }
}
