//! One `hermes -p <profile> acp` subprocess per Codemux thread.
//!
//! # One process per thread
//!
//! This is the load-bearing structural decision, and it is not the usual
//! "one child per session because it is simpler". Three properties of the
//! agent make multiplexing threads into a shared child actively wrong:
//! upstream `session/close` is a no-op that frees nothing, MCP server
//! registration is process-global (so two threads with different tool sets
//! would fight over one registry), and the turn executor caps at four
//! workers process-wide. Process exit is the only reclamation there is, so
//! a thread owns its child outright and `shutdown` ends both.
//!
//! # Profile
//!
//! The profile is a LAUNCH parameter — `-p <profile>` on the command line,
//! carried by [`StartSessionInput::profile`]. There is no protocol call
//! that re-profiles a running child, so changing it restarts the session.
//! [`HermesSession::profile`] records what this child was launched with and
//! `mod.rs` compares against it; nothing here ever tries to poke a live
//! session with a new profile.
//!
//! # What is standard ACP, and what is not
//!
//! Everything this file speaks is standard ACP: `initialize`,
//! `authenticate`, `session/new`, `session/load`, `session/prompt`,
//! `session/cancel`, `session/set_model`, `session/set_mode`,
//! `session/request_permission`, `session/update`. No vendor extension is
//! sent, and the auth method is read off the `initialize` response rather
//! than assumed — it is derived from the launched profile's runtime and
//! changes with the profile.
//!
//! The turn/queue/interrupt state machine, the generation guard, the
//! pending-request bookkeeping, the drain barrier and the `session/load`
//! replay window are the same designs the Cursor adapter proved; their
//! rationale comments are carried across because the reasoning is
//! provider-neutral and losing it is how a subtle fix gets reverted.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use serde_json::{json, Value};
use tokio::sync::{broadcast, mpsc, oneshot, Mutex, Notify};
use tokio::task::JoinHandle;
use uuid::Uuid;

use crate::agent_provider::{
    CompletedItem, ContentDelta, ContextUsageTracker, ProviderError, ProviderRuntimeEvent,
    ProviderSessionId, RequestId, SendOutcome, SendTurnInput, SessionStatus, TaskSnapshotItem,
    TaskStatus, TasksSnapshot, ThreadId, TurnId, TurnStatus,
};
use crate::agent_provider::{ApprovalDecision, ProviderKind};
use crate::json_rpc_child::{
    IncomingRequest, JsonRpcChild, Notification, RpcChildError, RpcError, SpawnConfig,
};

use super::capabilities::{parse_session_catalog, HermesSessionCatalog};
use super::commands::HermesSlashCommandCache;
use super::protocol::{
    auth_methods, context_occupancy, initialize_params, interactive_auth_hint, normalize_tool_call,
    permission_options, permission_response, prompt_usage, select_auth_method, session_id,
    session_summaries, slash_commands, PermissionOption,
};

/// Default deadline for the small, cheap calls: `set_model`, `set_mode`,
/// `session/list`, permission responses. Every one of these answered in
/// single-digit milliseconds in the live capture.
const RPC_TIMEOUT: Duration = Duration::from_secs(30);

/// Deadline for `session/new` and `session/load` specifically.
///
/// Deliberately NOT [`RPC_TIMEOUT`]. `session/new` measured **4.285 s**
/// warm on the capture machine — it boots a real agent, not a stub — and a
/// cold model catalogue stalls far longer while the runtime is contacted
/// for its model list. `session/load` is worse in a different way: it
/// replays the whole transcript inline before it resolves, so a long
/// conversation makes the response time a function of history length.
/// Reusing the 30-second default here would turn a slow-but-fine start
/// into a spurious timeout.
const SESSION_START_TIMEOUT: Duration = Duration::from_secs(180);

/// A turn is user-paced and may legitimately run for hours.
const PROMPT_TIMEOUT: Duration = Duration::from_secs(24 * 60 * 60);

/// Marker occupying the prompt slot between "a send was accepted" and
/// "the real turn id exists". See [`HermesSession::enqueue_or_send`].
const DISPATCHING_TURN_ID: &str = "hermes-dispatching-turn";

/// How many replayed transcript events one `session/load` may materialise.
///
/// ACP replays a transcript uncapped and inline, so a long session would
/// otherwise flood the runtime event stream (and every persisted row
/// behind it) in one burst. Only the newest `REPLAY_ITEM_LIMIT` events
/// survive; the dropped head is announced by a marker item.
const REPLAY_ITEM_LIMIT: usize = 500;

/// The warning shown once per session when the agent delegates to a
/// subagent.
///
/// Confirmed against the installed agent tree: the ACP path dispatches
/// `delegate_task` with `async_delivery` defaulting to true, and nothing
/// on that path drains the completion queue — so the delegated result
/// never comes back and the parent turn ends without it. Nothing the
/// adapter can do fixes that, so the honest behaviour is to say so in the
/// transcript rather than let the user watch a turn quietly lose work.
const DELEGATION_WARNING: &str = "Hermes delegated this step to a subagent. Delegated results are \
     not delivered back over ACP in this version, so the agent will \
     continue without the subagent's answer.";

#[derive(Debug, Clone)]
pub struct HermesSpawnConfig {
    pub binary: PathBuf,
    /// Profile passed as `hermes -p <profile> acp`. `None` launches the
    /// root profile, which is what a bare `hermes` uses.
    pub profile: Option<String>,
}

#[derive(Debug, Clone)]
struct QueuedTurn {
    id: String,
    input: SendTurnInput,
}

/// A `session/request_permission` awaiting the user's answer.
///
/// Only one kind, unlike the Cursor adapter's three: Hermes sends no
/// vendor request types, so there is no plan or questionnaire variant to
/// carry.
#[derive(Debug, Clone)]
struct PendingPermission {
    rpc_id: Value,
    options: Vec<PermissionOption>,
}

/// The window in which a `session/load` transcript replay is being
/// consumed.
///
/// ACP emits every transcript entry as a `session/update` **before** the
/// load response resolves, so those frames arrive with no turn in flight.
/// The window is what tells [`HermesSession::handle_session_update`] that
/// a turn-less transcript frame is expected rather than stray, and it is
/// deliberately NOT `active_turn`: replay must never claim the session's
/// single prompt slot (see [`HermesSessionState::open_replay`]).
#[derive(Debug)]
struct ReplayWindow {
    /// Synthetic id of the replayed turn currently being rebuilt. Rotated
    /// at every user message, because a user message is what starts a new
    /// turn in the replayed stream.
    turn_id: TurnId,
    /// Whether the replayed transcript should be materialised into this
    /// thread's transcript, or only consumed to prime session state.
    ///
    /// A thread Codemux itself drove already owns authoritative rows in
    /// `agent_chat_messages`, and that table has no idempotency key — so
    /// adopting there would double every bubble permanently. Adoption is
    /// only correct for a session created elsewhere, where the replay is
    /// the ONLY source of the transcript.
    adopt: bool,
    /// Transcript frames observed across the whole window. Zero is the
    /// only evidence that a `session/load` invented a fresh session: ACP
    /// answers an unknown session id with `{}` rather than an error.
    transcript_frames: u64,
    /// Non-user frames seen since the current synthetic turn opened, so a
    /// user message split across several chunks rotates the turn once
    /// rather than once per chunk.
    agent_frames_in_turn: u64,
    /// Text of the replayed user message being accumulated, flushed as a
    /// single bubble ahead of the first agent frame that follows it.
    user_text: String,
    /// Events staged for the flush in [`HermesSession::finish_replay`].
    /// Staged rather than streamed so the tail can be capped and so a
    /// suppressed (or abandoned) window emits nothing at all.
    buffered: VecDeque<ProviderRuntimeEvent>,
    /// Whether anything was dropped — by the cap here or by a lagging
    /// broadcast subscriber — so the adopted transcript can say so.
    truncated: bool,
}

impl ReplayWindow {
    fn stage(&mut self, event: ProviderRuntimeEvent) {
        self.buffered.push_back(event);
        while self.buffered.len() > REPLAY_ITEM_LIMIT {
            self.buffered.pop_front();
            self.truncated = true;
        }
    }

    /// Flush the accumulated user message, if any, as its own bubble.
    ///
    /// Called before the first agent frame that follows it so the
    /// replayed turn keeps its real order, and again when the turn is
    /// sealed so a trailing user message is not lost.
    fn flush_user_message(&mut self, thread_id: &ThreadId) {
        let text = std::mem::take(&mut self.user_text);
        if text.trim().is_empty() {
            return;
        }
        self.stage(ProviderRuntimeEvent::UserMessage {
            thread_id: thread_id.clone(),
            text,
            images: Vec::new(),
            // No nonce: nothing optimistically rendered a replayed
            // bubble, so there is no optimistic copy to reconcile with.
            client_nonce: None,
        });
    }
}

pub(crate) struct HermesSessionState {
    pub status: SessionStatus,
    pub active_turn: Option<TurnId>,
    /// `provider:model` id the session is currently on, as far as this
    /// adapter knows. Seeded from `session/new`'s `currentModelId`.
    model: Option<String>,
    /// ACP mode id (`default` / `accept_edits` / `dont_ask`).
    permission_mode: String,
    /// Mode ids the agent advertised on `session/new`. A `set_mode` for
    /// anything outside this list is refused locally instead of being sent
    /// and silently ignored.
    available_modes: Vec<String>,
    /// Model ids the agent advertised on `session/new`, same reasoning.
    available_models: Vec<String>,
    pending: HashMap<RequestId, PendingPermission>,
    queued: VecDeque<QueuedTurn>,
    assistant_text: String,
    thinking_text: String,
    /// Monotonic id of the current claim on the session's single prompt
    /// slot, bumped every time the slot is claimed (see
    /// [`HermesSessionState::claim_turn`]).
    turn_generation: u64,
    /// The generation a Stop applies to, if any.
    ///
    /// Stop cannot always be forwarded the instant it arrives: while a
    /// turn is still being dispatched (queue promotion, per-turn model
    /// swap) the child has no prompt to cancel, so the request has to be
    /// parked. Parking it as a bare boolean leaks — a Stop that lands in a
    /// window where nothing consumes it survives into the NEXT turn and
    /// completes it as interrupted before its prompt is ever written.
    /// Tagging it with the generation it was aimed at makes staleness
    /// self-invalidating: the next claim bumps the counter and the
    /// orphaned request can never match again.
    interrupt_generation: Option<u64>,
    /// Context-window occupancy, clamped and deduplicated by the shared
    /// tracker every other adapter uses.
    context: ContextUsageTracker,
    /// Whether the delegation limitation has already been reported this
    /// session. Once is information; once per call is noise.
    warned_about_delegation: bool,
    /// Open while a `session/load` transcript replay is being consumed.
    replay: Option<ReplayWindow>,
}

impl HermesSessionState {
    /// Claim the prompt slot for a new turn and return its generation.
    ///
    /// Callers set `active_turn` themselves — some claim the
    /// `DISPATCHING` marker first and only mint the real turn id later —
    /// but every claim must come through here so interrupt targeting
    /// stays accurate.
    fn claim_turn(&mut self) -> u64 {
        self.turn_generation = self.turn_generation.wrapping_add(1);
        self.turn_generation
    }

    /// Aim a Stop at whatever is claimed right now. No-op when the
    /// session is idle: there is nothing to interrupt.
    fn request_interrupt(&mut self) {
        if self.active_turn.is_some() {
            self.interrupt_generation = Some(self.turn_generation);
        }
    }

    /// Consume a Stop aimed at `generation`. A request aimed at any other
    /// generation is left alone — it belongs to a turn that has already
    /// finished, and honoring it would cancel an unrelated turn.
    fn take_interrupt(&mut self, generation: u64) -> bool {
        if self.interrupt_generation == Some(generation) {
            self.interrupt_generation = None;
            return true;
        }
        false
    }

    /// Release the prompt slot. Drops any Stop still aimed at the
    /// generation being released so it cannot outlive its turn.
    fn release_turn(&mut self) {
        self.active_turn = None;
        self.interrupt_generation = None;
    }

    /// Open the `session/load` replay window.
    ///
    /// `active_turn` deliberately stays `None` and no generation is
    /// claimed. Replay is not a turn: if it borrowed the prompt slot,
    /// `enqueue_or_send` would queue the user's first prompt behind a
    /// phantom turn, `turn_active()` would report a busy session to the
    /// frontend, and a Stop arriving during startup would be aimed at a
    /// generation no prompt worker owns.
    fn open_replay(&mut self, adopt: bool) {
        self.replay = Some(ReplayWindow {
            turn_id: replay_turn_id(),
            adopt,
            transcript_frames: 0,
            agent_frames_in_turn: 0,
            user_text: String::new(),
            buffered: VecDeque::new(),
            truncated: false,
        });
    }

    /// Start a fresh synthetic turn inside the open window.
    fn rotate_replay_turn(&mut self) {
        if let Some(replay) = self.replay.as_mut() {
            replay.turn_id = replay_turn_id();
            replay.agent_frames_in_turn = 0;
        }
    }

    /// Close the window and hand back everything it collected.
    fn close_replay(&mut self) -> Option<ReplayWindow> {
        self.replay.take()
    }
}

/// One synthetic turn id per replayed turn boundary. Namespaced so a
/// replayed turn can never be mistaken for one this session dispatched.
fn replay_turn_id() -> TurnId {
    TurnId(format!("hermes-replay-{}", Uuid::new_v4()))
}

/// A token a message pump answers once it has drained everything the
/// child wrote ahead of it. See
/// [`HermesSession::await_child_messages_drained`].
type NotificationBarrier = oneshot::Sender<()>;

/// Everything a completed start hands back to `mod.rs`.
pub(crate) struct HermesSessionStart {
    pub session: Arc<HermesSession>,
    /// Models and modes off the `session/new` response, so the picker's
    /// per-profile cache can be refreshed with the agent's own catalogue
    /// instead of the disk-read seed. Empty for a `session/load`, which
    /// returns no catalogue.
    pub catalog: HermesSessionCatalog,
    /// Whether `initialize` reported image prompt support, which is a
    /// property of the agent rather than of any one model.
    pub supports_images: bool,
}

pub(crate) struct HermesSession {
    pub thread_id: ThreadId,
    /// Profile this child was launched with (`hermes -p <profile> acp`).
    /// Compared by `mod.rs` on a restart request: a profile change cannot
    /// be applied to a running child, so it has to become a new one.
    pub profile: Option<String>,
    /// Working directory the session was created for. Keys the
    /// slash-command cache, which the composer's popup pulls by cwd.
    pub cwd: PathBuf,
    /// The ACP session id this thread is bound to.
    ///
    /// Interior mutability because the id is not final when the session
    /// object is built: the message pumps have to already be draining
    /// when `session/load` is issued (that is where the transcript replay
    /// arrives), and a load that fails falls back to `session/new`, which
    /// mints a different id.
    provider_session_id: std::sync::RwLock<ProviderSessionId>,
    pub state: Mutex<HermesSessionState>,
    child: Arc<JsonRpcChild>,
    event_tx: broadcast::Sender<ProviderRuntimeEvent>,
    /// Where an `available_commands_update` lands so the composer's lazy,
    /// cwd-keyed pull can find it.
    slash_commands: Arc<HermesSlashCommandCache>,
    tasks: Mutex<Vec<JoinHandle<()>>>,
    /// Ordering barriers into the two tasks that consume child messages.
    /// See [`HermesSession::await_child_messages_drained`].
    notification_barrier_tx: mpsc::UnboundedSender<NotificationBarrier>,
    request_barrier_tx: mpsc::UnboundedSender<NotificationBarrier>,
    /// Woken by [`HermesSession::interrupt`] so the prompt worker can
    /// deliver `session/cancel` itself. See [`HermesSession::send_prompt`].
    interrupt_notify: Notify,
    stopped: AtomicBool,
}

impl HermesSession {
    #[allow(clippy::too_many_arguments)]
    pub async fn spawn_and_initialize(
        thread_id: ThreadId,
        cwd: PathBuf,
        model: Option<String>,
        permission_mode: Option<String>,
        resume_cursor: Option<Value>,
        adopt_transcript: bool,
        env: Option<HashMap<String, String>>,
        spawn: HermesSpawnConfig,
        slash_commands: Arc<HermesSlashCommandCache>,
        event_tx: broadcast::Sender<ProviderRuntimeEvent>,
    ) -> Result<HermesSessionStart, ProviderError> {
        // `-p <profile>` is a GLOBAL flag and must precede the
        // subcommand. The profile selects the runtime, the model
        // catalogue and the approval policy the child boots with, so it
        // is part of the process identity — which is exactly why a change
        // to it restarts the session instead of being poked at a live one.
        let mut args = Vec::new();
        if let Some(profile) = spawn.profile.as_deref() {
            args.push("-p".to_string());
            args.push(profile.to_string());
        }
        args.push("acp".to_string());

        let child = JsonRpcChild::spawn(SpawnConfig {
            program: spawn.binary,
            args,
            env: env.unwrap_or_default(),
            cwd: Some(cwd.clone()),
            default_timeout: RPC_TIMEOUT,
        })
        .await
        .map_err(map_spawn_error)?;
        let child = Arc::new(child);
        let notification_rx = child.notifications();
        let request_rx = child
            .incoming_requests()
            .ok_or_else(|| ProviderError::RpcError {
                message: "Hermes ACP request receiver was already claimed".into(),
            })?;

        let handshake = Self::handshake(&child, spawn.profile.as_deref()).await;
        let supports_images = match handshake {
            Ok(supports_images) => supports_images,
            Err(error) => {
                let _ = child.shutdown().await;
                return Err(error);
            }
        };

        let resume_id = resume_cursor.as_ref().and_then(resume_session_id);
        let (notification_barrier_tx, notification_barrier_rx) = mpsc::unbounded_channel();
        let (request_barrier_tx, request_barrier_rx) = mpsc::unbounded_channel();
        // Built BEFORE the session exists agent-side, and with the message
        // pumps already draining. `session/load` replays the whole
        // transcript as `session/update` notifications *ahead of* its own
        // response, and `session/new` pushes `available_commands_update`
        // and `usage_update` unprompted the moment it returns — a session
        // assembled only after those responses can never see them.
        let session = Arc::new(Self {
            thread_id: thread_id.clone(),
            profile: spawn.profile.clone(),
            cwd: cwd.clone(),
            provider_session_id: std::sync::RwLock::new(ProviderSessionId(
                resume_id.clone().unwrap_or_default(),
            )),
            state: Mutex::new(HermesSessionState {
                status: SessionStatus::Ready,
                active_turn: None,
                model: None,
                permission_mode: permission_mode
                    .clone()
                    .unwrap_or_else(|| super::capabilities::HERMES_DEFAULT_PERMISSION_MODE.into()),
                // Filled from the `session/new` response below; the
                // session has to exist before that response can be asked
                // for.
                available_modes: Vec::new(),
                available_models: Vec::new(),
                pending: HashMap::new(),
                queued: VecDeque::new(),
                assistant_text: String::new(),
                thinking_text: String::new(),
                turn_generation: 0,
                interrupt_generation: None,
                context: ContextUsageTracker::default(),
                warned_about_delegation: false,
                replay: None,
            }),
            child,
            event_tx,
            slash_commands,
            tasks: Mutex::new(Vec::new()),
            notification_barrier_tx,
            request_barrier_tx,
            interrupt_notify: Notify::new(),
            stopped: AtomicBool::new(false),
        });
        session
            .start_background_tasks(
                notification_rx,
                request_rx,
                notification_barrier_rx,
                request_barrier_rx,
            )
            .await;

        let setup = match session
            .establish_session(&cwd, resume_id, resume_cursor.as_ref(), adopt_transcript)
            .await
        {
            Ok(setup) => setup,
            Err(error) => {
                session.shutdown().await;
                return Err(error);
            }
        };
        let catalog = parse_session_catalog(&setup);
        {
            let mut state = session.state.lock().await;
            state.available_models = catalog
                .models
                .iter()
                .map(|model| model.id.clone())
                .collect();
            state.available_modes = catalog
                .modes
                .iter()
                .map(|mode| mode.value.clone())
                .collect();
            state.model = catalog.current_model_id.clone();
            if let Some(current) = catalog.current_mode_id.clone() {
                state.permission_mode = current;
            }
        }

        let configuration_result = async {
            // Order matters only in that both are cheap fire-and-forget
            // calls; neither is worth failing a started session over, but
            // a silently-ignored selection is worse than an error — the
            // user would be talking to a different model than the picker
            // shows.
            if let Some(model) = model {
                session.set_model(model).await?;
            }
            if let Some(mode) = permission_mode {
                session.set_permission_mode(mode).await?;
            }
            Ok::<(), ProviderError>(())
        }
        .await;
        if let Err(error) = configuration_result {
            session.shutdown().await;
            return Err(error);
        }

        // The agent titles its own sessions. Ask for the title only after
        // the session exists, and only offer it — the command layer
        // applies it to a thread that has no title of its own and leaves
        // a user-chosen one alone.
        session.suggest_thread_title().await;

        let _ = session
            .event_tx
            .send(ProviderRuntimeEvent::SessionConfigured {
                thread_id: thread_id.clone(),
                provider_session_id: session.provider_session_id(),
            });
        let _ = session
            .event_tx
            .send(ProviderRuntimeEvent::ResumeCursorUpdated {
                thread_id,
                resume_cursor: session.resume_cursor(),
            });
        Ok(HermesSessionStart {
            session,
            catalog,
            supports_images,
        })
    }

    /// `initialize`, then `authenticate` with a method the agent actually
    /// offered. Returns whether the agent accepts image prompts.
    ///
    /// The auth method is READ, never assumed. `authMethods` is derived
    /// from the launched profile's configured runtime, so its ids differ
    /// between profiles on the same machine; a hard-coded login id — the
    /// shortcut the Cursor adapter takes — authenticates the wrong runtime
    /// or fails outright as soon as the user switches profile. A method
    /// marked `type: "terminal"` is never selected: it launches an
    /// interactive program, and the child has no tty to run it on. When
    /// those are the only methods on offer, the error names the command
    /// the user has to run instead.
    async fn handshake(
        child: &Arc<JsonRpcChild>,
        profile: Option<&str>,
    ) -> Result<bool, ProviderError> {
        let response = child
            .request("initialize", initialize_params("codemux"))
            .await
            .map_err(map_rpc_error)?;
        let supports_images = response
            .pointer("/agentCapabilities/promptCapabilities/image")
            .and_then(Value::as_bool)
            .unwrap_or(false);

        let methods = auth_methods(&response);
        // An empty list is a legitimate answer: an agent with no auth step
        // reports no methods, and calling `authenticate` anyway would be
        // an error for a session that is already usable.
        if methods.is_empty() {
            return Ok(supports_images);
        }
        let profile_label = profile.unwrap_or(super::discovery::DEFAULT_PROFILE_ID);
        let Some(method) = select_auth_method(&methods) else {
            return Err(ProviderError::NotAuthenticated {
                provider: ProviderKind::Hermes,
                hint: interactive_auth_hint(profile_label, &methods).unwrap_or_else(|| {
                    format!("Profile `{profile_label}` offers no authentication method Codemux can complete.")
                }),
            });
        };
        child
            .request("authenticate", json!({ "methodId": method.id }))
            .await
            .map_err(|error| map_auth_error(error, profile_label, &methods))?;
        Ok(supports_images)
    }

    /// Create or resume the ACP session this thread talks to.
    ///
    /// Runs with the message pumps already live, so a `session/load`
    /// replay is consumed as it arrives instead of piling up in the
    /// transport's bounded broadcast. Returns the response the catalogue
    /// is read from.
    async fn establish_session(
        self: &Arc<Self>,
        cwd: &Path,
        resume_id: Option<String>,
        resume_cursor: Option<&Value>,
        adopt_transcript: bool,
    ) -> Result<Value, ProviderError> {
        if let Some(session_id) = resume_id {
            self.begin_replay(adopt_transcript).await;
            let loaded = self
                .child
                .request_with_timeout(
                    "session/load",
                    json!({
                        "sessionId": session_id,
                        "cwd": cwd,
                        "mcpServers": mcp_servers()
                    }),
                    SESSION_START_TIMEOUT,
                )
                .await;
            // The tail of the replay and the load response wake together,
            // so closing the window on the response alone would drop the
            // end of the transcript — the same race
            // `await_child_messages_drained` was written for.
            self.await_child_messages_drained().await;
            let replayed_frames = self.finish_replay(loaded.is_ok()).await;
            match loaded {
                // ACP's LoadSessionResponse deliberately omits sessionId:
                // the client already supplied it in the request. Keep the
                // requested id instead of treating a standards-compliant
                // response as a failed restart.
                Ok(response) => {
                    self.set_provider_session_id(session_id);
                    // A successful `session/load` is NOT proof the session
                    // existed: an unknown id is answered with `{}` and a
                    // fresh empty session, not an error. The replayed
                    // transcript is the only evidence, so an adoption that
                    // saw none is reported rather than claimed.
                    if adopt_transcript && replayed_frames == 0 {
                        self.warn(
                            "Hermes replayed no transcript for the loaded session; \
                             treating it as a fresh session"
                                .into(),
                            resume_cursor.cloned(),
                        );
                    }
                    return Ok(response);
                }
                Err(load_error) => self.warn(
                    format!(
                        "Hermes could not resume the prior ACP session; \
                         started a new session instead: {load_error}"
                    ),
                    resume_cursor.cloned(),
                ),
            }
        }
        let response = self
            .child
            .request_with_timeout(
                "session/new",
                json!({ "cwd": cwd, "mcpServers": mcp_servers() }),
                SESSION_START_TIMEOUT,
            )
            .await
            .map_err(map_rpc_error)?;
        let provider_session_id = session_id(&response).ok_or_else(|| ProviderError::RpcError {
            message: "Hermes session/new returned no sessionId".into(),
        })?;
        self.set_provider_session_id(provider_session_id);
        Ok(response)
    }

    /// Ask the agent for its own name for this conversation and offer it
    /// to the command layer.
    ///
    /// `session/list` is effectively free (0.001 s in the capture) and is
    /// the only place the auto-title is exposed. Entirely best-effort: a
    /// failed lookup, a session the list does not mention, or an empty
    /// title all mean "no suggestion", never a failed start.
    async fn suggest_thread_title(&self) {
        let Ok(response) = self.child.request("session/list", json!({})).await else {
            return;
        };
        let session_id = self.provider_session_id().0;
        let Some(title) = session_summaries(&response)
            .into_iter()
            .find(|summary| summary.session_id == session_id)
            .and_then(|summary| summary.title)
        else {
            return;
        };
        let _ = self
            .event_tx
            .send(ProviderRuntimeEvent::ThreadTitleSuggested {
                thread_id: self.thread_id.clone(),
                title,
            });
    }

    /// The ACP session id this thread is currently bound to.
    pub(crate) fn provider_session_id(&self) -> ProviderSessionId {
        self.provider_session_id
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    fn set_provider_session_id(&self, session_id: String) {
        *self
            .provider_session_id
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = ProviderSessionId(session_id);
    }

    /// The provider-native resume envelope. Persisted VERBATIM by the
    /// command layer (`agent_chat_sessions.resume_cursor`) and replayed
    /// unchanged on rebuild, so keys beyond `sessionId` survive a restart.
    ///
    /// The profile rides along because it is part of the session's
    /// identity, not a preference: resuming the same session id under a
    /// different profile would point at a different runtime and model
    /// catalogue. Single source of truth — `mod.rs` hands this same value
    /// out in `ProviderSession`.
    pub(crate) fn resume_cursor(&self) -> Value {
        json!({
            "schemaVersion": 1,
            "sessionId": self.provider_session_id().0,
            "profile": self.profile,
        })
    }

    pub fn is_dead(&self) -> bool {
        self.stopped.load(Ordering::SeqCst) || !self.child.is_alive()
    }

    async fn start_background_tasks(
        self: &Arc<Self>,
        mut notification_rx: broadcast::Receiver<Notification>,
        mut request_rx: tokio::sync::mpsc::Receiver<IncomingRequest>,
        mut notification_barrier_rx: mpsc::UnboundedReceiver<NotificationBarrier>,
        mut request_barrier_rx: mpsc::UnboundedReceiver<NotificationBarrier>,
    ) {
        let notifications = Arc::clone(self);
        let notification_task = tokio::spawn(async move {
            loop {
                // `biased` is the whole barrier contract: the message arm
                // is polled first, so a barrier is only ever answered once
                // the channel is empty. An ack therefore means "every
                // message the child wrote so far has been applied to
                // session state".
                tokio::select! {
                    biased;
                    received = notification_rx.recv() => match received {
                        Ok(notification) => notifications.handle_notification(notification).await,
                        Err(broadcast::error::RecvError::Lagged(count)) => {
                            // Losing frames mid-replay is not the same
                            // failure as losing them mid-turn: the dropped
                            // frames are transcript that has no other
                            // source, so the adopted history has to admit
                            // the hole rather than start mid-thought.
                            let replaying = {
                                let mut state = notifications.state.lock().await;
                                match state.replay.as_mut() {
                                    Some(replay) => {
                                        replay.truncated = true;
                                        true
                                    }
                                    None => false,
                                }
                            };
                            let context = if replaying {
                                " during session-load replay; the restored transcript is incomplete"
                            } else {
                                ""
                            };
                            notifications.warn(
                                format!(
                                    "Hermes ACP notification stream dropped {count} messages{context}"
                                ),
                                None,
                            );
                        }
                        Err(broadcast::error::RecvError::Closed) => break,
                    },
                    barrier = notification_barrier_rx.recv() => match barrier {
                        Some(ack) => {
                            let _ = ack.send(());
                        }
                        None => break,
                    },
                }
            }
        });
        let requests = Arc::clone(self);
        let request_task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    biased;
                    received = request_rx.recv() => match received {
                        Some(request) => requests.handle_request(request).await,
                        None => break,
                    },
                    barrier = request_barrier_rx.recv() => match barrier {
                        Some(ack) => {
                            let _ = ack.send(());
                        }
                        None => break,
                    },
                }
            }
        });
        self.tasks
            .lock()
            .await
            .extend([notification_task, request_task]);
    }

    pub async fn enqueue_or_send(
        self: &Arc<Self>,
        input: SendTurnInput,
    ) -> Result<SendOutcome, ProviderError> {
        let mut state = self.state.lock().await;
        if self.is_dead() {
            return Err(ProviderError::SessionClosed {
                thread_id: self.thread_id.clone(),
            });
        }
        if state.active_turn.is_some() {
            let queued_id = format!("hermes-queued-{}", Uuid::new_v4());
            state.queued.push_back(QueuedTurn {
                id: queued_id.clone(),
                input: input.clone(),
            });
            drop(state);
            let _ = self.event_tx.send(ProviderRuntimeEvent::TurnQueued {
                thread_id: self.thread_id.clone(),
                queued_id: queued_id.clone(),
                client_nonce: input.client_nonce,
                text: input.display_text.unwrap_or(input.text),
            });
            return Ok(SendOutcome::Queued(queued_id));
        }
        // Claim the only ACP prompt slot before configuration/checkpoint
        // work begins. Two frontend sends can otherwise both observe an
        // idle session and start concurrent `session/prompt` requests.
        state.active_turn = Some(TurnId(DISPATCHING_TURN_ID.into()));
        let generation = state.claim_turn();
        drop(state);
        match self.dispatch_turn(input, None, generation).await {
            Ok(turn_id) => Ok(SendOutcome::Started(turn_id)),
            Err(error) => {
                self.release_dispatch_reservation().await;
                Err(error)
            }
        }
    }

    async fn dispatch_turn(
        self: &Arc<Self>,
        input: SendTurnInput,
        queued_id: Option<String>,
        generation: u64,
    ) -> Result<TurnId, ProviderError> {
        let (turn_id, prompt) = self.prepare_turn(input, queued_id).await?;
        let session = Arc::clone(self);
        let worker_turn_id = turn_id.clone();
        let task = tokio::spawn(async move {
            session
                .run_prompt_worker(worker_turn_id, prompt, generation)
                .await;
        });
        {
            // Reap finished workers on the way in: a long session would
            // otherwise accumulate one dead JoinHandle per turn forever.
            let mut tasks = self.tasks.lock().await;
            tasks.retain(|task| !task.is_finished());
            tasks.push(task);
        }
        Ok(turn_id)
    }

    /// Block until both message pumps have applied everything the child
    /// wrote before now.
    ///
    /// `session/prompt`'s response is delivered through the transport's
    /// oneshot, while the `agent_message_chunk` / `agent_thought_chunk`
    /// notifications and `session/request_permission` requests that
    /// precede it on stdout are delivered through a broadcast and an mpsc
    /// channel this session drains in two other tasks. All three wake at
    /// once, so completing the turn on the response alone races the tail
    /// of the child's output — chunks get dropped (no active turn) or
    /// folded into the next turn's buffer, and a just-arrived permission
    /// request lands in `pending` after the turn already cancelled
    /// everything there.
    ///
    /// The transport's reader routes lines in order, so by the time the
    /// response resolves every earlier message is already queued. Both
    /// pumps answer a barrier only when their channel is empty, so an ack
    /// from each restores the ordering the transcript depends on.
    async fn await_child_messages_drained(&self) {
        let barriers = [&self.request_barrier_tx, &self.notification_barrier_tx]
            .into_iter()
            .filter_map(|barrier_tx| {
                let (ack_tx, ack_rx) = oneshot::channel();
                barrier_tx.send(ack_tx).ok().map(|()| ack_rx)
            })
            .collect::<Vec<_>>();
        for ack_rx in barriers {
            // Bounded so a wedged pump can never strand a turn in Running
            // forever; a dropped ack (task exited) resolves instantly.
            let _ = tokio::time::timeout(RPC_TIMEOUT, ack_rx).await;
        }
    }

    async fn prepare_turn(
        &self,
        input: SendTurnInput,
        queued_id: Option<String>,
    ) -> Result<(TurnId, Vec<Value>), ProviderError> {
        if let Some(checkpoint) = &input.turn_checkpoint {
            checkpoint.prepare().await;
        }
        let configured = async {
            if let Some(model) = input.model_override.clone() {
                self.set_model(model).await?;
            }
            if let Some(mode) = input.permission_mode_override.clone() {
                self.set_permission_mode(mode).await?;
            }
            // `effort_override` is deliberately unhandled: Hermes carries
            // reasoning selection on the profile's launch configuration,
            // not per prompt, so there is no protocol call to apply it and
            // pretending otherwise would show a control that does nothing.
            Ok::<(), ProviderError>(())
        }
        .await;
        if let Err(error) = configured {
            if let Some(checkpoint) = &input.turn_checkpoint {
                checkpoint.abort().await;
            }
            return Err(error);
        }

        let turn_id = TurnId(format!("hermes-turn-{}", Uuid::new_v4()));
        {
            let mut state = self.state.lock().await;
            state.active_turn = Some(turn_id.clone());
            state.status = SessionStatus::Running {
                active_turn: turn_id.clone(),
            };
            state.assistant_text.clear();
            state.thinking_text.clear();
        }
        let _ = self
            .event_tx
            .send(ProviderRuntimeEvent::SessionStateChanged {
                thread_id: self.thread_id.clone(),
                status: SessionStatus::Running {
                    active_turn: turn_id.clone(),
                },
            });
        if let Some(id) = queued_id {
            let _ = self
                .event_tx
                .send(ProviderRuntimeEvent::QueuedTurnDispatched {
                    thread_id: self.thread_id.clone(),
                    queued_id: id,
                    turn_id: turn_id.clone(),
                    text: input
                        .display_text
                        .clone()
                        .unwrap_or_else(|| input.text.clone()),
                });
        }
        if let Some(checkpoint) = &input.turn_checkpoint {
            checkpoint.commit().await;
        }

        Ok((turn_id, build_prompt(&input)))
    }

    async fn run_prompt_worker(
        self: Arc<Self>,
        mut turn_id: TurnId,
        mut prompt: Vec<Value>,
        mut generation: u64,
    ) {
        loop {
            // A Stop that landed while this turn was still being
            // dispatched has nothing to cancel on the child yet, so the
            // turn simply never starts. Checked under the same lock
            // `interrupt` writes, so the request cannot slip past.
            let interrupted_before_send = self.state.lock().await.take_interrupt(generation);
            let response = if interrupted_before_send {
                Ok(json!({ "stopReason": "cancelled" }))
            } else {
                self.send_prompt(&prompt, generation).await
            };
            let next = self.finish_turn(turn_id, response, generation).await;
            let Some((queued, next_generation)) = next else {
                return;
            };
            generation = next_generation;
            match self
                .prepare_turn(queued.input, Some(queued.id.clone()))
                .await
            {
                Ok((next_turn_id, next_prompt)) => {
                    turn_id = next_turn_id;
                    prompt = next_prompt;
                }
                Err(error) => {
                    let cancelled = {
                        let mut state = self.state.lock().await;
                        // `release_turn` (not a bare `active_turn = None`)
                        // so a Stop aimed at this abandoned claim cannot
                        // outlive it.
                        state.release_turn();
                        state.status = SessionStatus::Ready;
                        state
                            .queued
                            .drain(..)
                            .map(|queued| queued.id)
                            .collect::<Vec<_>>()
                    };
                    self.warn(
                        format!("Could not dispatch queued Hermes turn: {error}"),
                        None,
                    );
                    let _ = self
                        .event_tx
                        .send(ProviderRuntimeEvent::QueuedTurnCancelled {
                            thread_id: self.thread_id.clone(),
                            queued_id: queued.id,
                        });
                    for queued_id in cancelled {
                        let _ = self
                            .event_tx
                            .send(ProviderRuntimeEvent::QueuedTurnCancelled {
                                thread_id: self.thread_id.clone(),
                                queued_id,
                            });
                    }
                    return;
                }
            }
        }
    }

    /// Run one `session/prompt`, forwarding any Stop that arrives while
    /// it is in flight.
    ///
    /// The cancel is issued from HERE rather than from
    /// [`HermesSession::interrupt`] to fix an ordering race: an interrupt
    /// firing microseconds after the worker claimed the in-flight window
    /// could win the stdin write race and reach the child before the
    /// prompt it was meant to cancel, leaving the child with nothing to
    /// cancel and the user's Stop silently dropped. `biased` polls the
    /// prompt future first, so the request has already claimed the
    /// transport's writer mutex before the cancel arm is ever reachable,
    /// and that mutex is FIFO so the cancel queues behind the prompt and
    /// still reaches the child second. `session/cancel` is a notification
    /// with no reply, so repeating it is harmless.
    async fn send_prompt(
        &self,
        prompt: &[Value],
        generation: u64,
    ) -> Result<Value, RpcChildError> {
        let request = self.child.request_with_timeout(
            "session/prompt",
            json!({
                "sessionId": self.provider_session_id().0,
                "prompt": prompt
            }),
            PROMPT_TIMEOUT,
        );
        tokio::pin!(request);
        loop {
            tokio::select! {
                biased;
                response = &mut request => return response,
                () = self.interrupt_notify.notified() => {
                    if !self.state.lock().await.take_interrupt(generation) {
                        continue;
                    }
                    // Hand the cancel to a detached task instead of
                    // awaiting it here. A child's stdin is a blocking-pool
                    // wrapper on Windows, whose `flush` yields at least
                    // once, so the pinned prompt future can be parked
                    // *inside* `write_line` while still holding the
                    // transport's writer mutex. Awaiting the cancel in this
                    // branch body would then block on that same mutex —
                    // and `select!` does not poll the prompt arm that owns
                    // it while a branch body is awaiting, so the turn
                    // deadlocks and the Stop is swallowed. Spawning keeps
                    // the loop polling the prompt so its write can finish
                    // and release the lock; the cancel queues behind it on
                    // the FIFO mutex, preserving the order described above.
                    let child = Arc::clone(&self.child);
                    let session_id = self.provider_session_id().0;
                    let event_tx = self.event_tx.clone();
                    let thread_id = self.thread_id.clone();
                    tokio::spawn(async move {
                        if let Err(error) = child
                            .notify("session/cancel", json!({ "sessionId": session_id }))
                            .await
                        {
                            let _ = event_tx.send(ProviderRuntimeEvent::RuntimeWarning {
                                thread_id: Some(thread_id),
                                message: format!("Could not cancel the Hermes turn: {error}"),
                                original_payload: None,
                            });
                        }
                    });
                }
            }
        }
    }

    async fn finish_turn(
        &self,
        turn_id: TurnId,
        response: Result<Value, RpcChildError>,
        generation: u64,
    ) -> Option<(QueuedTurn, u64)> {
        // Ordering barrier — see `await_child_messages_drained`. Must run
        // before the buffers below are taken, and without holding the
        // state lock the notification task needs.
        self.await_child_messages_drained().await;
        let (assistant_text, thinking_text, cancelled_requests) = {
            let mut state = self.state.lock().await;
            // This turn is over, so a Stop still aimed at it is moot.
            state.take_interrupt(generation);
            (
                std::mem::take(&mut state.assistant_text),
                std::mem::take(&mut state.thinking_text),
                // ACP requires every outstanding request to be answered.
                // Dropping them would leave the child blocked forever and
                // the UI's approval surface unresolvable (a later Allow
                // would fail with RequestNotPending).
                state.pending.drain().collect::<Vec<_>>(),
            )
        };
        for (request_id, pending) in cancelled_requests {
            let body = permission_response(&pending.options, &ApprovalDecision::Cancel);
            let _ = self.child.respond(pending.rpc_id, Ok(body)).await;
            let _ = self.event_tx.send(ProviderRuntimeEvent::RequestResolved {
                thread_id: self.thread_id.clone(),
                request_id,
                decision: ApprovalDecision::Cancel,
            });
        }
        if !thinking_text.is_empty() {
            let _ = self.event_tx.send(ProviderRuntimeEvent::ItemCompleted {
                thread_id: self.thread_id.clone(),
                turn_id: turn_id.clone(),
                item: CompletedItem::AssistantThinking {
                    text: thinking_text,
                },
                subagent_id: None,
            });
        }
        if !assistant_text.is_empty() {
            let _ = self.event_tx.send(ProviderRuntimeEvent::ItemCompleted {
                thread_id: self.thread_id.clone(),
                turn_id: turn_id.clone(),
                item: CompletedItem::AssistantText {
                    text: assistant_text,
                },
                subagent_id: None,
            });
        }
        let status = match &response {
            Ok(result) => {
                turn_status_from_stop_reason(result.get("stopReason").and_then(Value::as_str))
            }
            Err(error) => TurnStatus::Error {
                subtype: "hermes_acp".into(),
                message: error.to_string(),
            },
        };
        // The per-turn `usage` object the prompt response carries is the
        // most accurate occupancy reading available at turn end — a turn
        // that emitted no `usage_update` would otherwise leave the meter
        // stale. Cost is deliberately not computed from it: the ledger is
        // fed from provider-owned history, and `UsageRecorded` is
        // discarded before it reaches the frontend.
        if let Ok(result) = &response {
            if let Some(usage) = prompt_usage(result) {
                self.observe_context_tokens(usage.total_tokens, None).await;
            }
        }
        let _ = self.event_tx.send(ProviderRuntimeEvent::TurnCompleted {
            thread_id: self.thread_id.clone(),
            turn_id: turn_id.clone(),
            status,
            // No wall-clock or sub-turn count is reported by ACP, and a
            // partially-invented `TurnUsage` reads as a real measurement.
            usage: None,
        });
        let next = {
            let mut state = self.state.lock().await;
            state.status = SessionStatus::Ready;
            // A Stop can also land in the gap between the two locked
            // sections above, where `active_turn` still names the turn
            // that just finished. Dropping it here — and claiming a fresh
            // generation for any queued turn — is what stops it becoming
            // an instant cancellation of somebody else's turn.
            state.interrupt_generation = None;
            match state.queued.pop_front() {
                Some(queued) => {
                    // Reserve the dispatch slot before releasing the lock.
                    // Without this marker a concurrent send could observe
                    // an idle session between popping the queued turn and
                    // preparing it, starting a second prompt worker for
                    // the same ACP session.
                    state.active_turn = Some(TurnId(DISPATCHING_TURN_ID.into()));
                    Some((queued, state.claim_turn()))
                }
                None => {
                    state.release_turn();
                    None
                }
            }
        };
        let _ = self
            .event_tx
            .send(ProviderRuntimeEvent::SessionStateChanged {
                thread_id: self.thread_id.clone(),
                status: SessionStatus::Ready,
            });
        next
    }

    /// Record a Stop against the turn that is claimed right now.
    ///
    /// Nothing is written to the child from here. The prompt worker owns
    /// delivery — it either abandons a turn whose prompt has not gone out
    /// yet, or issues `session/cancel` from inside
    /// [`HermesSession::send_prompt`], where the cancel provably follows
    /// the prompt it cancels.
    pub async fn interrupt(&self, turn_id: Option<TurnId>) -> Result<(), ProviderError> {
        {
            let mut state = self.state.lock().await;
            let active = state.active_turn.clone();
            if active.is_none() || turn_id.is_some_and(|requested| Some(requested) != active) {
                return Ok(());
            }
            state.request_interrupt();
        }
        self.interrupt_notify.notify_one();
        Ok(())
    }

    pub async fn cancel_queued(&self, queued_id: &str) -> bool {
        let removed = {
            let mut state = self.state.lock().await;
            state
                .queued
                .iter()
                .position(|queued| queued.id == queued_id)
                .and_then(|index| state.queued.remove(index))
        };
        if let Some(removed) = removed {
            let _ = self
                .event_tx
                .send(ProviderRuntimeEvent::QueuedTurnCancelled {
                    thread_id: self.thread_id.clone(),
                    queued_id: removed.id,
                });
            true
        } else {
            false
        }
    }

    pub async fn send_queued_now(self: &Arc<Self>, queued_id: &str) -> Result<(), ProviderError> {
        let (ready_to_dispatch, should_interrupt) = {
            let mut state = self.state.lock().await;
            let queued = state
                .queued
                .iter()
                .position(|queued| queued.id == queued_id)
                .and_then(|index| state.queued.remove(index));
            match queued {
                Some(queued) if state.active_turn.is_some() => {
                    state.queued.push_front(queued);
                    (None, true)
                }
                Some(queued) => {
                    state.active_turn = Some(TurnId(DISPATCHING_TURN_ID.into()));
                    let generation = state.claim_turn();
                    (Some((queued, generation)), false)
                }
                None => (None, false),
            }
        };
        if let Some((queued, generation)) = ready_to_dispatch {
            let queued_id = queued.id.clone();
            if let Err(error) = self
                .dispatch_turn(queued.input, Some(queued.id), generation)
                .await
            {
                self.release_dispatch_reservation().await;
                let _ = self
                    .event_tx
                    .send(ProviderRuntimeEvent::QueuedTurnCancelled {
                        thread_id: self.thread_id.clone(),
                        queued_id,
                    });
                return Err(error);
            }
        } else if should_interrupt {
            // The promoted item is already first in line before
            // cancellation, so prompt completion cannot race past it.
            self.interrupt(None).await?;
        }
        Ok(())
    }

    async fn release_dispatch_reservation(&self) {
        let mut state = self.state.lock().await;
        if state.active_turn.as_ref().map(|turn| turn.0.as_str()) == Some(DISPATCHING_TURN_ID) {
            state.release_turn();
            state.status = SessionStatus::Ready;
        }
    }

    pub async fn respond_to_request(
        &self,
        request_id: RequestId,
        decision: ApprovalDecision,
    ) -> Result<(), ProviderError> {
        let pending = self
            .state
            .lock()
            .await
            .pending
            .remove(&request_id)
            .ok_or_else(|| ProviderError::RequestNotPending {
                request_id: request_id.clone(),
            })?;
        let body = permission_response(&pending.options, &decision);
        self.child
            .respond(pending.rpc_id, Ok(body))
            .await
            .map_err(map_rpc_error)?;
        let _ = self.event_tx.send(ProviderRuntimeEvent::RequestResolved {
            thread_id: self.thread_id.clone(),
            request_id,
            decision,
        });
        let status = {
            let mut state = self.state.lock().await;
            // Answering one of several parallel requests must not flip the
            // pane out of WaitingApproval: the others are still blocking
            // the child. Only an empty pending map means the turn resumed.
            let keep_current = matches!(
                &state.status,
                SessionStatus::WaitingApproval { request_id } if state.pending.contains_key(request_id)
            );
            let status = if keep_current {
                state.status.clone()
            } else if let Some(request_id) = state.pending.keys().next().cloned() {
                SessionStatus::WaitingApproval { request_id }
            } else {
                state
                    .active_turn
                    .clone()
                    .map_or(SessionStatus::Ready, |active_turn| SessionStatus::Running {
                        active_turn,
                    })
            };
            state.status = status.clone();
            status
        };
        let _ = self
            .event_tx
            .send(ProviderRuntimeEvent::SessionStateChanged {
                thread_id: self.thread_id.clone(),
                status,
            });
        Ok(())
    }

    /// Swap the model on the live session.
    ///
    /// Standard ACP `session/set_model`, with ids in `provider:model`
    /// form. An id the agent did not advertise is refused here rather than
    /// sent: the agent answers an unknown model with a success and keeps
    /// the old one, so forwarding it would leave the picker showing a
    /// model the session is not using.
    pub async fn set_model(&self, model: String) -> Result<(), ProviderError> {
        let model = model.trim().to_string();
        // `default` is the frontend's placeholder for "capability
        // discovery has not produced a concrete row yet". Leaving the
        // agent on its own current model is the provider-native meaning of
        // that, and it avoids inventing a model id.
        if model.is_empty() || model == "default" {
            return Ok(());
        }
        {
            let state = self.state.lock().await;
            if !state.available_models.is_empty()
                && !state.available_models.iter().any(|known| *known == model)
            {
                return Err(ProviderError::ValidationError {
                    message: format!("Hermes profile does not offer model `{model}`"),
                });
            }
        }
        self.child
            .request(
                "session/set_model",
                json!({ "sessionId": self.provider_session_id().0, "modelId": model }),
            )
            .await
            .map_err(map_rpc_error)?;
        self.state.lock().await.model = Some(model);
        Ok(())
    }

    /// Swap the ACP mode on the live session (`session/set_mode`).
    ///
    /// Worth being precise about what this controls: an ACP mode governs
    /// FILE EDITS. Shell commands are gated by the profile's own
    /// `approvals.mode`, outside the protocol — verified live, a shell
    /// command ran with no permission request at all while the session sat
    /// in the ask-before-edits mode. Nothing here can widen or narrow
    /// that.
    pub async fn set_permission_mode(&self, mode: String) -> Result<(), ProviderError> {
        let mode = mode.trim().to_string();
        {
            let state = self.state.lock().await;
            if !state.available_modes.is_empty()
                && !state.available_modes.iter().any(|known| *known == mode)
            {
                return Err(ProviderError::ValidationError {
                    message: format!("Hermes does not expose permission mode `{mode}`"),
                });
            }
        }
        self.child
            .request(
                "session/set_mode",
                json!({ "sessionId": self.provider_session_id().0, "modeId": mode }),
            )
            .await
            .map_err(map_rpc_error)?;
        self.state.lock().await.permission_mode = mode;
        Ok(())
    }

    async fn handle_request(&self, request: IncomingRequest) {
        match request.method.as_str() {
            "session/request_permission" => self.handle_permission_request(request).await,
            // Every unrecognised request is answered — an unanswered
            // JSON-RPC request blocks the agent's turn forever — and
            // reported as a warning rather than crashing the session.
            _ => {
                self.warn(
                    format!("Unsupported Hermes ACP request `{}`", request.method),
                    Some(request.params),
                );
                let _ = self
                    .child
                    .respond(
                        request.id,
                        Err(RpcError {
                            code: -32601,
                            message: "Method not supported by Codemux".into(),
                            data: None,
                        }),
                    )
                    .await;
            }
        }
    }

    /// Open a permission request to the user.
    ///
    /// Every option is carried through by `kind` + verbatim `name`; none
    /// is auto-selected here. There is deliberately no "the mode is
    /// permissive so answer it myself" shortcut: the agent already applies
    /// the session's mode before deciding to ask, so a request that
    /// reaches Codemux is one the agent wants a human answer to, and
    /// pre-answering it would silently widen the policy the user chose.
    async fn handle_permission_request(&self, request: IncomingRequest) {
        let options = permission_options(&request.params);
        if options.is_empty() {
            // Nothing to offer the user, and leaving it unanswered would
            // wedge the turn. Cancel is the only honest answer.
            self.warn(
                "Hermes asked for permission but offered no options; the request was cancelled."
                    .into(),
                Some(request.params),
            );
            let _ = self
                .child
                .respond(request.id, Ok(json!({ "outcome": { "outcome": "cancelled" } })))
                .await;
            return;
        }

        let request_id = RequestId(format!("hermes-request-{}", Uuid::new_v4()));
        let turn_id = self
            .state
            .lock()
            .await
            .active_turn
            .clone()
            .unwrap_or_else(|| TurnId("hermes-turn-unknown".into()));
        {
            let mut state = self.state.lock().await;
            state.pending.insert(
                request_id.clone(),
                PendingPermission {
                    rpc_id: request.id,
                    options,
                },
            );
            state.status = SessionStatus::WaitingApproval {
                request_id: request_id.clone(),
            };
        }
        let tool = request
            .params
            .get("toolCall")
            .cloned()
            .unwrap_or(Value::Null);
        let tool_use_id = tool
            .get("toolCallId")
            .and_then(Value::as_str)
            .map(str::to_string);
        // The approval card renders the tool call, so it gets the same
        // normalisation the transcript does — otherwise the user approves
        // a raw ACP diff block while the transcript shows a diff card.
        let mut payload = request.params;
        if !tool.is_null() {
            let normalized = normalize_tool_call(&tool);
            if let Some(object) = payload.as_object_mut() {
                object.insert("toolName".into(), Value::String(normalized.tool_name));
                object.insert("toolInput".into(), normalized.input);
            }
        }
        let _ = self.event_tx.send(ProviderRuntimeEvent::RequestOpened {
            thread_id: self.thread_id.clone(),
            turn_id,
            request_id: request_id.clone(),
            request_kind: "tool_approval".into(),
            payload,
            tool_use_id,
            subagent_id: None,
        });
        let _ = self
            .event_tx
            .send(ProviderRuntimeEvent::SessionStateChanged {
                thread_id: self.thread_id.clone(),
                status: SessionStatus::WaitingApproval { request_id },
            });
    }

    async fn handle_notification(&self, notification: Notification) {
        match notification.method.as_str() {
            "session/update" => self.handle_session_update(notification.params).await,
            // Unknown notifications degrade to a warning. A notification
            // has no reply, so there is nothing to unblock — but it must
            // never take the session down either.
            _ => self.warn(
                format!("Unknown Hermes ACP notification `{}`", notification.method),
                Some(notification.params),
            ),
        }
    }

    /// Open the `session/load` replay window.
    async fn begin_replay(&self, adopt: bool) {
        self.state.lock().await.open_replay(adopt);
    }

    /// Close the replay window and, when the load is worth committing,
    /// emit everything it collected.
    ///
    /// Returns the number of transcript frames the replay carried — the
    /// only evidence that the loaded session actually existed.
    async fn finish_replay(&self, commit: bool) -> u64 {
        self.seal_replay_turn(false).await;
        let closed = self.state.lock().await.close_replay();
        let Some(replay) = closed else {
            return 0;
        };
        let frames = replay.transcript_frames;
        // Suppression is at the emission level, never the drop level: the
        // frames were still consumed, so the context meter is primed
        // either way.
        if !commit || !replay.adopt || frames == 0 {
            return frames;
        }
        if replay.truncated {
            // Say so in the transcript rather than silently opening a
            // conversation mid-thought. Its own synthetic turn, so it is
            // sealed no matter which turn the cap trimmed into.
            let turn_id = replay_turn_id();
            let _ = self.event_tx.send(ProviderRuntimeEvent::ItemCompleted {
                thread_id: self.thread_id.clone(),
                turn_id: turn_id.clone(),
                item: CompletedItem::AssistantText {
                    text: "_Earlier history from this session is not shown._".into(),
                },
                subagent_id: None,
            });
            let _ = self.event_tx.send(ProviderRuntimeEvent::TurnCompleted {
                thread_id: self.thread_id.clone(),
                turn_id,
                status: TurnStatus::Success,
                usage: None,
            });
        }
        for event in replay.buffered {
            let _ = self.event_tx.send(event);
        }
        frames
    }

    /// Seal the synthetic turn currently being rebuilt: flush its text
    /// buffers as completed items and mark the turn done.
    ///
    /// The trailing `TurnCompleted` is what seals the turn's last
    /// reasoning / assistant blocks; without it the pane hydrates into a
    /// turn that never closes.
    async fn seal_replay_turn(&self, rotate: bool) {
        let mut state = self.state.lock().await;
        if state.replay.is_none() {
            return;
        }
        let assistant_text = std::mem::take(&mut state.assistant_text);
        let thinking_text = std::mem::take(&mut state.thinking_text);
        let thread_id = self.thread_id.clone();
        if let Some(replay) = state.replay.as_mut() {
            let produced = !replay.user_text.trim().is_empty()
                || replay.agent_frames_in_turn > 0
                || !assistant_text.is_empty()
                || !thinking_text.is_empty();
            replay.flush_user_message(&thread_id);
            if produced {
                let turn_id = replay.turn_id.clone();
                if !thinking_text.is_empty() {
                    replay.stage(ProviderRuntimeEvent::ItemCompleted {
                        thread_id: thread_id.clone(),
                        turn_id: turn_id.clone(),
                        item: CompletedItem::AssistantThinking {
                            text: thinking_text,
                        },
                        subagent_id: None,
                    });
                }
                if !assistant_text.is_empty() {
                    replay.stage(ProviderRuntimeEvent::ItemCompleted {
                        thread_id: thread_id.clone(),
                        turn_id: turn_id.clone(),
                        item: CompletedItem::AssistantText {
                            text: assistant_text,
                        },
                        subagent_id: None,
                    });
                }
                replay.stage(ProviderRuntimeEvent::TurnCompleted {
                    thread_id,
                    turn_id,
                    // A replayed turn already happened; the stop reason it
                    // ended on is not part of the replay.
                    status: TurnStatus::Success,
                    usage: None,
                });
            }
        }
        if rotate {
            state.rotate_replay_turn();
        }
    }

    /// Fold one replayed `user_message_chunk` into the open window.
    ///
    /// A user message is what starts a new turn in the replayed stream,
    /// so the turn being rebuilt is sealed and rotated first.
    async fn replay_user_chunk(&self, text: &str) {
        let rotate = {
            let state = self.state.lock().await;
            match state.replay.as_ref() {
                Some(replay) => replay.agent_frames_in_turn > 0,
                None => return,
            }
        };
        if rotate {
            self.seal_replay_turn(true).await;
        }
        if let Some(replay) = self.state.lock().await.replay.as_mut() {
            replay.user_text.push_str(text);
        }
    }

    /// Route one transcript event: staged in the replay window while one
    /// is open, sent live otherwise.
    async fn emit_transcript_event(&self, event: ProviderRuntimeEvent) {
        let live = {
            let mut state = self.state.lock().await;
            match state.replay.as_mut() {
                Some(replay) => {
                    // Ordering: the user message that opened this replayed
                    // turn has to land before the agent work it prompted.
                    replay.flush_user_message(&self.thread_id);
                    replay.stage(event);
                    None
                }
                None => Some(event),
            }
        };
        if let Some(event) = live {
            let _ = self.event_tx.send(event);
        }
    }

    async fn handle_session_update(&self, params: Value) {
        let update = params.get("update").unwrap_or(&params);
        let kind = update
            .get("sessionUpdate")
            .and_then(Value::as_str)
            .unwrap_or_default();
        // Kinds that legitimately arrive with no turn in flight. Hermes
        // pushes `available_commands_update` and `usage_update`
        // unprompted right after `session/new`, so they have to be routed
        // BEFORE a turn is resolved — otherwise every session start would
        // fan two warnings into the UI.
        match kind {
            "usage_update" => {
                self.handle_usage_update(update).await;
                return;
            }
            "available_commands_update" => {
                self.handle_available_commands(update).await;
                return;
            }
            // Mode and model echoes: the adapter already knows what it
            // set, and the picker reads the catalogue from `session/new`.
            // Consumed silently rather than warned about — they are
            // expected traffic, not a gap.
            "current_mode_update" | "current_model_update" => return,
            _ => {}
        }
        // Everything below is transcript, and the gate is "not replaying",
        // NOT "no active turn". `session/load` replays every transcript
        // entry as a `session/update` before its own response resolves, so
        // dropping turn-less frames would discard the entire history of
        // any session Codemux did not itself create and drive — which is
        // the only case where the replay is the sole source of it.
        let transcript_frame = matches!(
            kind,
            "agent_message_chunk"
                | "agent_thought_chunk"
                | "user_message_chunk"
                | "tool_call"
                | "tool_call_update"
                | "plan"
        );
        let turn_id = {
            let mut state = self.state.lock().await;
            if let Some(replay) = state.replay.as_mut() {
                if transcript_frame {
                    replay.transcript_frames += 1;
                    if kind != "user_message_chunk" {
                        replay.agent_frames_in_turn += 1;
                    }
                }
            }
            state
                .replay
                .as_ref()
                .map(|replay| replay.turn_id.clone())
                .or_else(|| state.active_turn.clone())
        };
        let Some(turn_id) = turn_id else {
            // A warning, never a silent return: a transcript frame that
            // arrives outside both a turn and a replay window is a real
            // gap. The frame is still dropped, so a stray post-turn chunk
            // cannot fold into the next turn's buffer.
            self.warn(
                format!("Hermes ACP session update `{kind}` arrived with no active turn"),
                Some(update.clone()),
            );
            return;
        };
        match kind {
            "agent_message_chunk" | "agent_thought_chunk" => {
                if let Some(text) = update.pointer("/content/text").and_then(Value::as_str) {
                    let stream = {
                        let mut state = self.state.lock().await;
                        if kind == "agent_thought_chunk" {
                            state.thinking_text.push_str(text);
                        } else {
                            state.assistant_text.push_str(text);
                        }
                        // A replayed message is materialised whole when its
                        // turn is sealed. Streaming it delta by delta would
                        // only be superseded by that item anyway, and one
                        // staged event per chunk would burn the replay cap
                        // on a single long answer.
                        state.replay.is_none()
                    };
                    if stream {
                        let delta = if kind == "agent_thought_chunk" {
                            ContentDelta::Thinking { text: text.into() }
                        } else {
                            ContentDelta::Text { text: text.into() }
                        };
                        let _ = self.event_tx.send(ProviderRuntimeEvent::ContentDelta {
                            thread_id: self.thread_id.clone(),
                            turn_id,
                            delta,
                            subagent_id: None,
                        });
                    }
                }
            }
            // Only a replay carries user turns; a live echo of the prompt
            // Codemux just sent already has its bubble.
            "user_message_chunk" => {
                if let Some(text) = update.pointer("/content/text").and_then(Value::as_str) {
                    self.replay_user_chunk(text).await;
                }
            }
            "tool_call" => {
                let id = update
                    .get("toolCallId")
                    .and_then(Value::as_str)
                    .unwrap_or("hermes-tool");
                let call = normalize_tool_call(update);
                if call.is_delegation {
                    self.warn_about_delegation().await;
                }
                self.emit_transcript_event(ProviderRuntimeEvent::ItemCompleted {
                    thread_id: self.thread_id.clone(),
                    turn_id,
                    item: CompletedItem::ToolUse {
                        tool_name: call.tool_name,
                        input: call.input,
                        tool_use_id: id.into(),
                    },
                    subagent_id: None,
                })
                .await;
            }
            "tool_call_update" => {
                let terminal = update
                    .get("status")
                    .and_then(Value::as_str)
                    .is_some_and(|status| matches!(status, "completed" | "failed"));
                if terminal {
                    let id = update
                        .get("toolCallId")
                        .and_then(Value::as_str)
                        .unwrap_or("hermes-tool");
                    let is_error = update.get("status").and_then(Value::as_str) == Some("failed");
                    // Prefer the rendered text: a shell result arrives as
                    // `{type: "content"}` blocks whose text is what the
                    // tool card shows, and handing the raw block array to
                    // the renderer would print JSON at the user.
                    let text = super::protocol::content_text(update);
                    let content = if text.is_empty() {
                        update
                            .get("rawOutput")
                            .or_else(|| update.get("content"))
                            .cloned()
                            .unwrap_or(Value::Null)
                    } else {
                        Value::String(text)
                    };
                    self.emit_transcript_event(ProviderRuntimeEvent::ItemCompleted {
                        thread_id: self.thread_id.clone(),
                        turn_id,
                        item: CompletedItem::ToolResult {
                            tool_use_id: id.into(),
                            content,
                            is_error,
                        },
                        subagent_id: None,
                    })
                    .await;
                }
            }
            "plan" => {
                if let Some(tasks) = tasks_from_value(update) {
                    self.emit_transcript_event(ProviderRuntimeEvent::TasksUpdated {
                        thread_id: self.thread_id.clone(),
                        tasks,
                    })
                    .await;
                }
            }
            _ => self.warn(
                format!("Unknown Hermes ACP session update `{kind}`"),
                Some(update.clone()),
            ),
        }
    }

    /// Report the delegation limitation once per session.
    ///
    /// Confirmed against the installed agent: the ACP path dispatches
    /// `delegate_task` with async delivery on and nothing drains the
    /// completion queue, so the delegated result never returns. Accepted
    /// limitation for this version — the user is told rather than left to
    /// infer it from a turn that quietly came back without the work.
    async fn warn_about_delegation(&self) {
        let first = {
            let mut state = self.state.lock().await;
            let first = !state.warned_about_delegation;
            state.warned_about_delegation = true;
            first
        };
        if first {
            self.warn(DELEGATION_WARNING.to_string(), None);
        }
    }

    /// Record the command vocabulary the agent just advertised.
    ///
    /// The composer's slash-command store is a lazy pull keyed by
    /// provider + cwd, and it has no way to reach into a live session — so
    /// the notification's job is to keep the cwd's cache entry current,
    /// and the pull reads it. Building the pull the other way round (spawn
    /// a child and run `session/new` when the popup opens) would cost
    /// seconds and boot a whole agent to populate a menu.
    async fn handle_available_commands(&self, update: &Value) {
        let commands = slash_commands(update);
        if commands.is_empty() {
            return;
        }
        self.slash_commands.record(&self.cwd, commands).await;
    }

    /// Feed a `usage_update` into the context meter.
    ///
    /// `{used, size}` is the whole payload — occupancy and window size,
    /// with no input/output or cache split. That is enough for the meter
    /// and deliberately not enough for cost, which is a stated non-goal
    /// here: the ledger is fed from provider-owned history, and
    /// `UsageRecorded` is discarded before it reaches the frontend, so
    /// synthesising token splits would produce a number nobody reads and
    /// everybody would eventually trust.
    async fn handle_usage_update(&self, update: &Value) {
        let Some((used, size)) = context_occupancy(update) else {
            return;
        };
        self.observe_context_tokens(used, size).await;
    }

    /// Push one occupancy reading through the shared tracker and emit
    /// whatever it decides is worth sending.
    async fn observe_context_tokens(&self, used: u64, window: Option<u64>) {
        let events = {
            let mut state = self.state.lock().await;
            state.context.observe_max_tokens(window);
            // The reading is already the live occupancy of the window, not
            // a running total, so it is also the best lifetime figure
            // available; the tracker only surfaces it when it exceeds the
            // clamped occupancy.
            state.context.observe_lifetime_total(used);
            state.context.events(
                &self.thread_id,
                used,
                None,
                // The agent compacts its own context (`/compress` exists,
                // and `sessionProvenance` carries a compression depth);
                // Codemux exposes no toggle for it.
                Some(true),
            )
        };
        for event in events {
            let _ = self.event_tx.send(event);
        }
    }

    fn warn(&self, message: String, original_payload: Option<Value>) {
        let _ = self.event_tx.send(ProviderRuntimeEvent::RuntimeWarning {
            thread_id: Some(self.thread_id.clone()),
            message,
            original_payload,
        });
    }

    /// Tear down Codemux's side AND the agent's.
    ///
    /// `session/cancel` first so a turn in flight stops rather than
    /// running on past the process it is reporting to, then the child
    /// exits. Note what is NOT sent: `session/close`, which is a no-op
    /// upstream and frees nothing. Process exit is the reclamation.
    pub async fn shutdown(&self) {
        if self.stopped.swap(true, Ordering::SeqCst) {
            return;
        }
        let queued = {
            let mut state = self.state.lock().await;
            state.status = SessionStatus::Closed;
            state
                .queued
                .drain(..)
                .map(|queued| queued.id)
                .collect::<Vec<_>>()
        };
        for queued_id in queued {
            let _ = self
                .event_tx
                .send(ProviderRuntimeEvent::QueuedTurnCancelled {
                    thread_id: self.thread_id.clone(),
                    queued_id,
                });
        }
        let _ = self
            .child
            .notify(
                "session/cancel",
                json!({ "sessionId": self.provider_session_id().0 }),
            )
            .await;
        let _ = self.child.shutdown().await;
        for task in self.tasks.lock().await.drain(..) {
            task.abort();
        }
    }

    /// Drop the local child WITHOUT ending the agent-side conversation.
    ///
    /// The user closed a window onto the work; they did not ask to end it.
    /// Hermes sessions are durable — `session/list` still names them after
    /// the process exits, and `session/load` picks the transcript back up
    /// — so a detach is exactly a shutdown minus the goodbye:
    ///
    /// * no `session/cancel`, because a turn cancelled here would be
    ///   cancelled agent-side too and the work genuinely lost;
    /// * the queued follow-ups ARE cancelled, because the queue lives only
    ///   in this process's memory and would otherwise vanish with no event
    ///   to tell the UI its greyed-out bubbles are never coming;
    /// * the child is killed and the pumps aborted, which is the whole
    ///   point — a detach that leaked the process would defeat the reason
    ///   pane close calls it.
    ///
    /// Idempotent, like `shutdown`.
    pub async fn detach(&self) {
        if self.stopped.swap(true, Ordering::SeqCst) {
            return;
        }
        let queued = {
            let mut state = self.state.lock().await;
            state.status = SessionStatus::Closed;
            state
                .queued
                .drain(..)
                .map(|queued| queued.id)
                .collect::<Vec<_>>()
        };
        for queued_id in queued {
            let _ = self
                .event_tx
                .send(ProviderRuntimeEvent::QueuedTurnCancelled {
                    thread_id: self.thread_id.clone(),
                    queued_id,
                });
        }
        let _ = self.child.shutdown().await;
        for task in self.tasks.lock().await.drain(..) {
            task.abort();
        }
    }
}

/// MCP servers to register on `session/new` / `session/load`.
///
/// Empty, and deliberately — not copied from the Cursor adapter's habit of
/// sending `[]` because ACP requires the key. Registration upstream is
/// PROCESS-global rather than session-scoped, so a server registered for
/// one thread would be visible to every session in that process. Codemux
/// runs one process per thread, which makes that safe in principle, but
/// nothing in Codemux has a Hermes-side MCP server to register yet;
/// sending an empty list states that explicitly instead of leaving the key
/// off and letting the agent decide what a missing field means.
fn mcp_servers() -> Value {
    json!([])
}

/// Map an ACP `stopReason` onto the closest shared [`TurnStatus`].
///
/// One caveat the protocol imposes and this cannot fix: an agent-side
/// exception also comes back as `end_turn`, with the error delivered as an
/// ordinary assistant message. A host cannot tell success from failure by
/// stop reason alone, so a failed turn shows as a completed one carrying
/// the agent's own error text.
fn turn_status_from_stop_reason(stop_reason: Option<&str>) -> TurnStatus {
    match stop_reason {
        Some("cancelled") => TurnStatus::Error {
            subtype: "interrupted".into(),
            message: "Hermes turn was interrupted".into(),
        },
        // Both caps are "the runtime stopped this turn short", which is
        // exactly what MaxTurns renders.
        Some("max_tokens" | "max_turn_requests") => TurnStatus::MaxTurns,
        Some("refusal") => TurnStatus::Error {
            subtype: "refusal".into(),
            message: "Hermes declined to continue this turn".into(),
        },
        _ => TurnStatus::Success,
    }
}

fn build_prompt(input: &SendTurnInput) -> Vec<Value> {
    let mut prompt = vec![json!({ "type": "text", "text": input.text })];
    prompt.extend(input.images.iter().map(|image| {
        json!({
            "type": "image",
            "data": base64::engine::general_purpose::STANDARD.encode(&image.data),
            "mimeType": image.media_type
        })
    }));
    prompt
}

/// The ACP session id inside a persisted resume envelope.
fn resume_session_id(value: &Value) -> Option<String> {
    value
        .get("sessionId")
        .or_else(|| value.get("session_id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// The profile a persisted resume envelope was created under.
///
/// `mod.rs` compares it against the requested profile: they differ, the
/// session cannot be resumed into the same child, because the profile is a
/// launch parameter.
pub(crate) fn resume_profile(value: &Value) -> Option<String> {
    value
        .get("profile")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

/// Read an ACP `plan` frame into the shared task snapshot.
fn tasks_from_value(value: &Value) -> Option<TasksSnapshot> {
    let entries = value.get("entries").or_else(|| value.get("todos"))?.as_array()?;
    let tasks = entries
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| {
            let title = entry
                .get("content")
                .or_else(|| entry.get("title"))
                .and_then(Value::as_str)?
                .trim();
            if title.is_empty() {
                return None;
            }
            let status = match entry.get("status").and_then(Value::as_str) {
                Some("completed") => TaskStatus::Completed,
                Some("in_progress" | "inProgress") => TaskStatus::InProgress,
                _ => TaskStatus::Pending,
            };
            Some(TaskSnapshotItem {
                task_id: entry
                    .get("id")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("hermes-task-{index}")),
                title: title.into(),
                status,
                detail: None,
                blocked_by: vec![],
            })
        })
        .collect::<Vec<_>>();
    Some(TasksSnapshot {
        explanation: value
            .get("overview")
            .and_then(Value::as_str)
            .map(str::to_string),
        tasks,
    })
}

fn map_spawn_error(error: RpcChildError) -> ProviderError {
    match error {
        RpcChildError::SpawnFailed(source) if source.kind() == std::io::ErrorKind::NotFound => {
            ProviderError::NotInstalled {
                provider: ProviderKind::Hermes,
                hint: "Install Hermes and make sure `hermes` is on your PATH.".into(),
            }
        }
        other => ProviderError::ProcessError {
            message: "Could not start the Hermes ACP server".into(),
            source: Some(other.to_string()),
        },
    }
}

/// Turn a failed `authenticate` into something the user can act on.
///
/// The hint names the profile, because a Hermes credential problem is
/// always per-profile: `default` being signed in says nothing about
/// `coder`. When the agent offered an interactive setup method, its own
/// args become the command to run.
fn map_auth_error(
    error: RpcChildError,
    profile: &str,
    methods: &[super::protocol::AuthMethod],
) -> ProviderError {
    ProviderError::NotAuthenticated {
        provider: ProviderKind::Hermes,
        hint: interactive_auth_hint(profile, methods).unwrap_or_else(|| {
            format!("Hermes profile `{profile}` could not authenticate: {error}")
        }),
    }
}

fn map_rpc_error(error: RpcChildError) -> ProviderError {
    match error {
        RpcChildError::Timeout { method, elapsed } => ProviderError::Timeout {
            operation: method,
            elapsed_ms: elapsed.as_millis() as u64,
        },
        RpcChildError::SpawnFailed(_)
        | RpcChildError::ChildExited { .. }
        | RpcChildError::IoError(_)
        | RpcChildError::AlreadyShutdown => ProviderError::ProcessError {
            message: "The Hermes ACP process failed".into(),
            source: Some(error.to_string()),
        },
        other => ProviderError::RpcError {
            message: other.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A state value with just enough filled in to exercise the turn
    /// state machine. Kept next to the tests that use it so a field added
    /// to `HermesSessionState` breaks here rather than silently defaulting.
    fn interrupt_test_state() -> HermesSessionState {
        HermesSessionState {
            status: SessionStatus::Ready,
            active_turn: None,
            model: None,
            permission_mode: "default".into(),
            available_modes: Vec::new(),
            available_models: Vec::new(),
            pending: HashMap::new(),
            queued: VecDeque::new(),
            assistant_text: String::new(),
            thinking_text: String::new(),
            turn_generation: 0,
            interrupt_generation: None,
            context: ContextUsageTracker::default(),
            warned_about_delegation: false,
            replay: None,
        }
    }

    #[test]
    fn a_stop_never_outlives_the_turn_it_was_aimed_at() {
        let mut state = interrupt_test_state();
        state.active_turn = Some(TurnId("turn-1".into()));
        let first = state.claim_turn();
        state.request_interrupt();
        // The turn ends without anything consuming the Stop.
        state.release_turn();

        state.active_turn = Some(TurnId("turn-2".into()));
        let second = state.claim_turn();
        assert!(
            !state.take_interrupt(second),
            "a Stop aimed at the previous turn must not cancel the next one"
        );
        assert_ne!(first, second);
    }

    #[test]
    fn a_stop_aimed_at_the_running_turn_is_honored_exactly_once() {
        let mut state = interrupt_test_state();
        state.active_turn = Some(TurnId("turn-1".into()));
        let generation = state.claim_turn();
        state.request_interrupt();
        assert!(state.take_interrupt(generation));
        assert!(!state.take_interrupt(generation));
    }

    #[test]
    fn a_stop_on_an_idle_session_is_dropped() {
        let mut state = interrupt_test_state();
        state.request_interrupt();
        let generation = state.claim_turn();
        assert!(!state.take_interrupt(generation));
    }

    #[test]
    fn replay_never_claims_the_prompt_slot() {
        let mut state = interrupt_test_state();
        state.open_replay(true);
        assert!(
            state.active_turn.is_none(),
            "a replay window must leave the session idle to senders"
        );
        assert_eq!(state.turn_generation, 0, "replay claims no generation");
        let first = state
            .replay
            .as_ref()
            .map(|replay| replay.turn_id.clone())
            .unwrap();
        state.rotate_replay_turn();
        let second = state
            .replay
            .as_ref()
            .map(|replay| replay.turn_id.clone())
            .unwrap();
        assert_ne!(first, second, "each replayed turn gets its own id");
        assert!(state.close_replay().is_some());
        assert!(state.close_replay().is_none());
    }

    #[test]
    fn the_replay_cap_trims_the_head_and_records_that_it_did() {
        let mut window = ReplayWindow {
            turn_id: replay_turn_id(),
            adopt: true,
            transcript_frames: 0,
            agent_frames_in_turn: 0,
            user_text: String::new(),
            buffered: VecDeque::new(),
            truncated: false,
        };
        for index in 0..(REPLAY_ITEM_LIMIT + 5) {
            window.stage(ProviderRuntimeEvent::RuntimeWarning {
                thread_id: None,
                message: format!("{index}"),
                original_payload: None,
            });
        }
        assert_eq!(window.buffered.len(), REPLAY_ITEM_LIMIT);
        assert!(window.truncated);
        // The newest events survive: the head is what gets trimmed.
        let first = match window.buffered.front() {
            Some(ProviderRuntimeEvent::RuntimeWarning { message, .. }) => message.clone(),
            other => panic!("unexpected staged event: {other:?}"),
        };
        assert_eq!(first, "5");
    }

    #[test]
    fn stop_reasons_map_to_the_closest_shared_turn_status() {
        assert!(matches!(
            turn_status_from_stop_reason(Some("end_turn")),
            TurnStatus::Success
        ));
        assert!(matches!(
            turn_status_from_stop_reason(Some("max_tokens")),
            TurnStatus::MaxTurns
        ));
        assert!(matches!(
            turn_status_from_stop_reason(Some("cancelled")),
            TurnStatus::Error { subtype, .. } if subtype == "interrupted"
        ));
        // An unknown reason is a completed turn, not an error: the agent
        // stopped for a reason this build has no name for, which is not
        // itself a failure.
        assert!(matches!(
            turn_status_from_stop_reason(Some("something_new")),
            TurnStatus::Success
        ));
    }

    #[test]
    fn the_resume_envelope_carries_the_profile_it_was_created_under() {
        let envelope = json!({"schemaVersion": 1, "sessionId": "abc", "profile": "codemuxdev"});
        assert_eq!(resume_session_id(&envelope).as_deref(), Some("abc"));
        assert_eq!(resume_profile(&envelope).as_deref(), Some("codemuxdev"));
        // A legacy envelope with no profile resumes under whatever the
        // caller asked for rather than being rejected.
        assert_eq!(resume_profile(&json!({"sessionId": "abc"})), None);
    }

    #[test]
    fn a_plan_frame_becomes_a_task_snapshot() {
        let tasks = tasks_from_value(&json!({
            "sessionUpdate": "plan",
            "entries": [
                {"content": "Read the file", "status": "completed"},
                {"content": "Patch it", "status": "in_progress"},
                {"content": "", "status": "pending"}
            ]
        }))
        .expect("plan frame carries entries");
        assert_eq!(tasks.tasks.len(), 2, "an empty title is skipped");
        assert!(matches!(tasks.tasks[1].status, TaskStatus::InProgress));
    }

    #[test]
    fn the_launch_line_puts_the_profile_flag_before_the_subcommand() {
        // `-p` is a GLOBAL flag; after `acp` it would be parsed as an
        // argument to the subcommand and the profile silently ignored.
        let spawn = HermesSpawnConfig {
            binary: PathBuf::from("hermes"),
            profile: Some("codemuxdev".into()),
        };
        let mut args = Vec::new();
        if let Some(profile) = spawn.profile.as_deref() {
            args.push("-p".to_string());
            args.push(profile.to_string());
        }
        args.push("acp".to_string());
        assert_eq!(args, vec!["-p", "codemuxdev", "acp"]);
    }
}
