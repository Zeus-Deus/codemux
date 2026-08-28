//! Hermes [`AgentProvider`](crate::agent_provider::AgentProvider)
//! integration.
//!
//! Hermes speaks the standard Agent Client Protocol over stdio:
//! `hermes -p <profile> acp` is spawned per chat thread and driven with
//! JSON-RPC over its stdin/stdout. There is no gateway and no WebSocket —
//! the local child IS the transport.
//!
//! Three properties of the live protocol shape everything in this module,
//! and each one is a place where a straight copy of another ACP adapter
//! would misbehave:
//!
//! * **One process per thread.** Upstream `session/close` is a no-op, MCP
//!   registration is process-global, and the agent's turn executor caps at
//!   four workers. Process exit is the only way a session's resources come
//!   back, so sessions are never multiplexed into one child.
//! * **`session/load` replays the whole transcript** as `session/update`
//!   notifications BEFORE the RPC resolves — i.e. while no turn is active.
//!   An adapter that drops updates outside an active turn discards the
//!   entire replay. [`session::HermesSession`] opens an explicit replay
//!   window instead.
//! * **`session/new` costs seconds**, not milliseconds: 4.3 s warm on the
//!   capture machine, far worse with a cold model catalogue. Session start
//!   gets its own generous deadline; the health probe deliberately uses
//!   `hermes acp --check` instead, which validates the entry point without
//!   booting an agent.
//!
//! # Profile
//!
//! The profile is a launch parameter carried on
//! [`StartSessionInput::profile`](crate::agent_provider::StartSessionInput::profile),
//! not a suffix on the model id. Hermes has no protocol call that
//! re-profiles a running child, so changing it RESTARTS the session — see
//! [`HermesAgentProvider::start_session`], which evicts a live session
//! launched under a different profile rather than reusing it.

pub mod capabilities;
pub mod commands;
pub mod discovery;
pub mod protocol;
mod session;

use std::collections::HashMap;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use futures_core::Stream;
use tokio::sync::{broadcast, RwLock};

use crate::agent_provider::{
    AgentProvider, ApprovalDecision, ProviderCapabilities, ProviderError, ProviderEventStream,
    ProviderKind, ProviderRuntimeEvent, ProviderSession, RequestId, SendOutcome, SendTurnInput,
    SessionStatus, StartSessionInput, ThreadId, TurnId, TurnStartResult,
};

use self::capabilities::HermesCapabilityCache;
use self::commands::HermesSlashCommandCache;
use self::session::{resume_profile, HermesSession, HermesSpawnConfig};

/// Binary Codemux launches. Resolved on PATH; Hermes installs itself as a
/// plain `hermes` executable.
pub const HERMES_BINARY: &str = "hermes";

/// The command layer's window onto the vocabulary live sessions have
/// reported.
///
/// A process-global rather than Tauri-managed state because the two ends
/// never meet any other way: `list_chat_slash_commands` is a bare command
/// with no registry handle, and the sessions that fill the cache are owned
/// by a provider it cannot reach into. Keyed by cwd, which is all the pull
/// side knows. See [`commands`] for why the pull cannot simply ask the
/// agent.
pub fn slash_command_cache() -> Arc<HermesSlashCommandCache> {
    static CACHE: OnceLock<Arc<HermesSlashCommandCache>> = OnceLock::new();
    Arc::clone(CACHE.get_or_init(|| Arc::new(HermesSlashCommandCache::new())))
}

/// Per-profile picker catalogue, refreshed from every `session/new`.
///
/// Global for the same reason as [`slash_command_cache`]: the harvest
/// happens inside a session and the reader is a command with no path to
/// the provider. Seeding it from disk stays in
/// [`capabilities::seed_capabilities_for_profile`]; this is where the
/// agent's own 29-entry catalogue replaces that seed once a session has
/// paid for it.
pub fn capability_cache() -> Arc<HermesCapabilityCache> {
    static CACHE: OnceLock<Arc<HermesCapabilityCache>> = OnceLock::new();
    Arc::clone(CACHE.get_or_init(|| Arc::new(HermesCapabilityCache::new())))
}

/// Outcome of the cheap install/liveness probe behind
/// [`check_hermes_availability`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HermesAvailability {
    /// Resolved binary path, `None` when `hermes` is not on PATH.
    pub binary: Option<PathBuf>,
    /// Whether `hermes acp --check` reported a usable ACP server.
    pub acp_ready: bool,
    /// Version string when `hermes --version` surfaced one. Best-effort.
    pub version: Option<String>,
    /// Human-readable reason the probe was not clean; `None` on success.
    pub message: Option<String>,
}

/// Probe the local Hermes install: binary on PATH, version, and whether
/// its ACP server self-check passes.
///
/// `hermes acp --check` is the cheap liveness probe — it validates the
/// ACP entry point and exits without doing any of the expensive session
/// setup, so it is safe to run behind a status banner. Never errors: a
/// failed probe IS the answer.
pub async fn check_hermes_availability() -> HermesAvailability {
    let binary = match which::which(HERMES_BINARY) {
        Ok(path) => path,
        Err(_) => {
            return HermesAvailability {
                binary: None,
                acp_ready: false,
                version: None,
                message: Some(format!(
                    "Hermes CLI (`{HERMES_BINARY}`) is not installed or not on PATH."
                )),
            }
        }
    };

    let version = match tokio::time::timeout(
        Duration::from_secs(5),
        tokio::process::Command::new(&binary)
            .arg("--version")
            .output(),
    )
    .await
    {
        Ok(Ok(output)) if output.status.success() => {
            let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
            (!text.is_empty()).then_some(text)
        }
        _ => None,
    };

    // ~0.35s warm. Deliberately NOT a session start: `session/new` costs
    // seconds and boots a real agent, which is far too much for a probe
    // the UI polls.
    match tokio::time::timeout(
        Duration::from_secs(10),
        tokio::process::Command::new(&binary)
            .args(["acp", "--check"])
            .output(),
    )
    .await
    {
        Ok(Ok(output)) if output.status.success() => HermesAvailability {
            binary: Some(binary),
            acp_ready: true,
            version,
            message: None,
        },
        Ok(Ok(output)) => {
            let detail = format!(
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            let detail = detail.trim().to_string();
            HermesAvailability {
                binary: Some(binary),
                acp_ready: false,
                version,
                message: Some(if detail.is_empty() {
                    "Hermes is installed but its ACP self-check failed.".to_string()
                } else {
                    format!("Hermes is installed but its ACP self-check failed. ({detail})")
                }),
            }
        }
        Ok(Err(error)) => HermesAvailability {
            binary: Some(binary),
            acp_ready: false,
            version,
            message: Some(format!("Hermes is installed but failed to run. ({error})")),
        },
        Err(_) => HermesAvailability {
            binary: Some(binary),
            acp_ready: false,
            version,
            message: Some("Hermes ACP self-check timed out. Sessions may still work.".to_string()),
        },
    }
}

#[derive(Debug, Clone)]
pub struct HermesProviderConfig {
    pub binary: PathBuf,
    pub event_channel_capacity: usize,
}

impl Default for HermesProviderConfig {
    fn default() -> Self {
        Self {
            binary: PathBuf::from(HERMES_BINARY),
            event_channel_capacity: 1024,
        }
    }
}

/// Whether the caller asked this start to ADOPT the transcript the agent
/// replays for a loaded session.
///
/// Off by default, and deliberately so. A thread Codemux itself created
/// and drove already owns authoritative rows in `agent_chat_messages`,
/// and its transcript hydrates from those rows on rebuild — adopting the
/// replay there would duplicate every bubble, permanently, because that
/// table is a pure append with no idempotency key. The flag is for the
/// opposite case: a session created elsewhere (the Hermes CLI, another
/// host, a `session/list` picker), where the replay is the ONLY source of
/// the conversation and Codemux holds nothing.
fn adopt_transcript(extra: &serde_json::Value) -> bool {
    extra
        .get("adoptTranscript")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

/// Normalise the requested profile: an absent or blank value means the
/// root profile, which is what a bare `hermes` uses.
fn requested_profile(profile: Option<&str>) -> Option<String> {
    profile
        .map(str::trim)
        .filter(|profile| !profile.is_empty())
        .map(str::to_string)
}

pub struct HermesAgentProvider {
    config: HermesProviderConfig,
    sessions: Arc<RwLock<HashMap<ThreadId, Arc<HermesSession>>>>,
    event_tx: broadcast::Sender<ProviderRuntimeEvent>,
}

impl HermesAgentProvider {
    pub fn new(config: HermesProviderConfig) -> Self {
        let (event_tx, _) = broadcast::channel(config.event_channel_capacity.max(16));
        Self {
            config,
            sessions: Arc::new(RwLock::new(HashMap::new())),
            event_tx,
        }
    }

    async fn session(&self, thread_id: &ThreadId) -> Result<Arc<HermesSession>, ProviderError> {
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

impl Drop for HermesAgentProvider {
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
impl AgentProvider for HermesAgentProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Hermes
    }

    fn capabilities(&self) -> ProviderCapabilities {
        // Declared from the live protocol capture rather than aspiration:
        // `initialize` reports `loadSession` plus `sessionCapabilities`
        // {resume, fork, list}, and permission requests are answered
        // synchronously mid-turn. `session/close` being a no-op upstream
        // is why one process backs exactly one thread; it does not change
        // what the session itself supports.
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
        let profile = requested_profile(input.profile.as_deref());

        // Evict under the write lock so check→remove is atomic against a
        // concurrent rebuild. A read-lock check followed by a separate
        // write-lock remove lets two starts both observe the same session.
        //
        // Two things get evicted, not one. A dead session is a corpse.
        // A LIVE session whose child was launched under a different
        // profile is the profile-change path: the profile is a launch
        // flag, there is no protocol call that re-profiles a running
        // child, and poking the live session with a model swap would leave
        // it on the old runtime while the UI showed the new profile. So it
        // is torn down and started again — which is what "changing the
        // profile restarts the session" means in practice.
        let evicted = {
            let mut sessions = self.sessions.write().await;
            match sessions.get(&thread_id) {
                Some(existing) if existing.is_dead() || existing.profile != profile => {
                    sessions.remove(&thread_id)
                }
                Some(_) => {
                    return Err(ProviderError::ValidationError {
                        message: format!("Hermes session already exists for thread {}", thread_id.0),
                    });
                }
                None => None,
            }
        };
        if let Some(evicted) = evicted {
            evicted.shutdown().await;
        }

        // A resume envelope is only valid under the profile that created
        // it: the same session id under a different profile addresses a
        // different runtime's store, and `session/load` answers an unknown
        // id with `{}` and a silently fresh session rather than an error.
        // Dropping the cursor here makes that an explicit new session
        // instead of a resume that appears to work and has no history.
        let resume_cursor = match input.resume_cursor.as_ref() {
            Some(cursor) if resume_profile(cursor).is_some() && resume_profile(cursor) != profile => {
                let _ = self.event_tx.send(ProviderRuntimeEvent::RuntimeWarning {
                    thread_id: Some(thread_id.clone()),
                    message: "This conversation was started under a different Hermes profile; \
                              a new session was started instead of resuming it."
                        .into(),
                    original_payload: Some(cursor.clone()),
                });
                None
            }
            other => other.cloned(),
        };

        let started = HermesSession::spawn_and_initialize(
            thread_id.clone(),
            input.cwd,
            input.model,
            input.permission_mode,
            resume_cursor,
            // Opt-in, and only ever set by a caller that has checked the
            // thread owns no transcript rows of its own: `agent_chat_messages`
            // has no idempotency key, so adopting a replay onto a thread that
            // already has rows doubles every bubble with no way back.
            adopt_transcript(&input.extra),
            input.env,
            HermesSpawnConfig {
                binary: self.config.binary.clone(),
                profile,
            },
            slash_command_cache(),
            self.event_tx.clone(),
        )
        .await?;

        // The picker's per-profile catalogue is refreshed from the
        // response the session already paid for. Best-effort: a start that
        // otherwise succeeded must never fail over a menu.
        if !started.catalog.is_empty() {
            let profile_id = started
                .session
                .profile
                .clone()
                .unwrap_or_else(|| discovery::DEFAULT_PROFILE_ID.to_string());
            capability_cache()
                .record_session_catalog(&profile_id, &started.catalog, started.supports_images)
                .await;
        }

        let session = started.session;
        // Spawning is async, so two starts that both got past the eviction
        // above can arrive here with two live children. Never overwrite a
        // live entry: the loser's `hermes` process would stay running with
        // nothing holding a handle to shut it down.
        {
            let mut sessions = self.sessions.write().await;
            if sessions
                .get(&thread_id)
                .is_some_and(|existing| !existing.is_dead())
            {
                drop(sessions);
                session.shutdown().await;
                return Err(ProviderError::ValidationError {
                    message: format!("Hermes session already exists for thread {}", thread_id.0),
                });
            }
            sessions.insert(thread_id.clone(), Arc::clone(&session));
        }
        Ok(ProviderSession {
            thread_id,
            provider: ProviderKind::Hermes,
            session_id: session.provider_session_id(),
            status: SessionStatus::Ready,
            resume_cursor: Some(session.resume_cursor()),
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

    /// Drop the child WITHOUT ending the agent-side conversation.
    ///
    /// Overridden rather than left to the default (which forwards to
    /// `stop_session`) because a Hermes session genuinely is durable: it
    /// survives the process, `session/list` still names it, and
    /// `session/load` picks the transcript back up. The default would send
    /// `session/cancel` on the way out and kill a turn the user only
    /// wanted to stop watching. See [`HermesSession::detach`].
    async fn detach_session(&self, thread_id: ThreadId) -> Result<(), ProviderError> {
        if let Some(session) = self.sessions.write().await.remove(&thread_id) {
            session.detach().await;
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
                provider: ProviderKind::Hermes,
                session_id: session.provider_session_id(),
                status: session.state.lock().await.status.clone(),
                resume_cursor: Some(session.resume_cursor()),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_and_capabilities_match_the_live_protocol_capture() {
        let provider = HermesAgentProvider::new(HermesProviderConfig::default());
        assert_eq!(provider.kind(), ProviderKind::Hermes);
        let caps = provider.capabilities();
        assert!(caps.supports_session_resume);
        assert!(caps.supports_synchronous_tool_approval);
        assert!(!caps.supports_conversation_rollback);
    }

    #[tokio::test]
    async fn stopping_and_detaching_an_unknown_thread_are_both_successes() {
        let provider = HermesAgentProvider::new(HermesProviderConfig::default());
        let thread = ThreadId("thread-1".into());
        assert!(!provider.has_session(&thread).await);
        assert!(!provider.turn_active(&thread).await);
        // Pane close and app quit both route here and must not surface a
        // failure for a session that was never started.
        assert!(provider.stop_session(thread.clone()).await.is_ok());
        assert!(provider.detach_session(thread.clone()).await.is_ok());
        assert!(provider.list_sessions().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn driving_a_thread_with_no_session_reports_it_as_missing() {
        let provider = HermesAgentProvider::new(HermesProviderConfig::default());
        let error = provider
            .set_model(ThreadId("thread-1".into()), "anthropic:claude-opus-5".into())
            .await
            .expect_err("no session is bound to this thread");
        assert!(matches!(error, ProviderError::SessionNotFound { .. }));
    }

    #[test]
    fn a_blank_profile_means_the_root_profile() {
        assert_eq!(requested_profile(None), None);
        assert_eq!(requested_profile(Some("   ")), None);
        assert_eq!(
            requested_profile(Some(" codemuxdev ")).as_deref(),
            Some("codemuxdev")
        );
    }

    #[test]
    fn transcript_adoption_is_opt_in() {
        assert!(!adopt_transcript(&serde_json::json!({})));
        assert!(!adopt_transcript(&serde_json::json!({"adoptTranscript": false})));
        assert!(adopt_transcript(&serde_json::json!({"adoptTranscript": true})));
    }
}
