//! Per-thread OpenCode chat session.
//!
//! Owns the OpenCode-side session id, a `reqwest`-backed HTTP client,
//! and a long-lived SSE listener task. Each Codemux thread maps to
//! one [`OpenCodeSession`]; the [`super::OpenCodeAgentProvider`] holds
//! the registry of live sessions.
//!
//! Lifecycle:
//!
//! 1. [`start`](OpenCodeSession::start) — bring up the OpenCode HTTP
//!    server (if not already), `POST /session`, spin up the SSE
//!    listener filtered by the new session id.
//! 2. [`send_turn`](OpenCodeSession::send_turn) — assign a fresh
//!    Codemux turn id, swap it into the SSE listener's
//!    [`EventContext`], then `POST /session/{id}/prompt_async`.
//! 3. [`interrupt`](OpenCodeSession::interrupt) —
//!    `POST /session/{id}/abort`.
//! 4. [`respond_to_request`](OpenCodeSession::respond_to_request) —
//!    `POST /session/{id}/permissions/{permID}` with the user's
//!    decision.
//! 5. [`shutdown`](OpenCodeSession::shutdown) — fence admitted input,
//!    interrupt this attachment and confirm idle before emitting `Closed`.

use std::collections::{HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tokio::sync::{broadcast, Mutex};
use tokio::task::JoinHandle;
use uuid::Uuid;

use crate::agent_provider::errors::ProviderError;
use crate::agent_provider::events::ProviderRuntimeEvent;
use crate::agent_provider::types::{
    ApprovalDecision, ImageInput, ProviderKind, ProviderSessionId, RequestId, SessionStatus,
    ThreadId, TurnId,
};
use crate::mcp::registry::McpRegistry;

use super::client::{
    OpenCodeClient, OpenCodeClientConfig, OpenCodeModelCost, OpenCodeProviderEntry,
};
use super::manager::{OpenCodeServerHandle, OpenCodeServerManager};
use super::protocol::{PermissionRespondRequest, SessionCreateRequest, SessionResponse};
use super::sse::{spawn_sse_listener, SsePeer, SseRouter};
use super::translate::{
    approval_decision_to_permission_reply, build_prompt_async_request, EventContext,
    OpenCodeUsageState,
};

const NATIVE_SHUTDOWN_GRACE: std::time::Duration = std::time::Duration::from_secs(3);
type NativeAdmission = tokio::sync::watch::Receiver<Option<Result<(), String>>>;

/// Detached requests keep responsibility for native masked admission after
/// their caller stops waiting. Closing an HTTP connection is not a server ACK.
#[derive(Clone)]
struct NativeAdmissionPeer {
    http: reqwest::Client,
    base: String,
    password: String,
    session: String,
    inputs: Arc<Mutex<HashSet<String>>>,
}

impl NativeAdmissionPeer {
    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        self.http
            .request(method, format!("{}{}", self.base, path))
            .basic_auth("opencode", Some(&self.password))
    }

    async fn acknowledged(request: reqwest::RequestBuilder) -> Result<Vec<u8>, String> {
        let response = request
            .send()
            .await
            .map_err(|e| format!("native_admission_unconfirmed: {e}"))?;
        let status = response.status();
        let body = response
            .bytes()
            .await
            .map_err(|e| format!("native_response_unconfirmed: {e}"))?;
        if !status.is_success() {
            return Err(format!(
                "native_http_status_{}: {}",
                status.as_u16(),
                String::from_utf8_lossy(&body)
            ));
        }
        Ok(body.to_vec())
    }

    async fn interrupt(&self) -> Result<(), String> {
        Self::acknowledged(self.request(
            reqwest::Method::POST,
            &format!("/api/session/{}/interrupt?resume=false", self.session),
        ))
        .await
        .map(|_| ())
    }

    async fn finish(&self) -> Result<(), String> {
        // Only cancel this attachment's acknowledged inputs. Other sessions
        // and the durable conversation are never deleted.
        let body = Self::acknowledged(self.request(
            reqwest::Method::GET,
            &format!("/api/session/{}/inbox", self.session),
        ))
        .await?;
        let value: serde_json::Value = serde_json::from_slice(&body)
            .map_err(|e| format!("native_inbox_decode_failed: {e}"))?;
        let pending = value["data"]
            .as_array()
            .ok_or("native_inbox_missing_data")?;
        let owned = self.inputs.lock().await.clone();
        for item in pending {
            let id = item["id"].as_str().ok_or("native_inbox_missing_identity")?;
            if owned.contains(id) {
                Self::acknowledged(self.request(
                    reqwest::Method::DELETE,
                    &format!(
                        "/api/session/{}/inbox/{}",
                        self.session,
                        urlencoding::encode(id)
                    ),
                ))
                .await?;
            }
        }
        self.interrupt().await?;
        // Interrupt acknowledges cancellation before coordinator settlement.
        Self::acknowledged(self.request(
            reqwest::Method::POST,
            &format!("/api/experimental/session/{}/wait", self.session),
        ))
        .await?;
        Ok(())
    }
}

async fn await_native_admissions(mut requests: Vec<NativeAdmission>) -> Result<(), String> {
    for request in &mut requests {
        loop {
            if let Some(result) = request.borrow().clone() {
                result?;
                break;
            }
            request
                .changed()
                .await
                .map_err(|_| "native_admission_owner_lost".to_string())?;
        }
    }
    Ok(())
}

/// Live handle to an OpenCode session bound to one Codemux thread.
pub struct OpenCodeSession {
    pub thread_id: ThreadId,
    pub provider_session_id: ProviderSessionId,
    /// Cached model identifier in `providerID/modelID` form. Updated
    /// by `set_model`. The chat send path uses this when no per-turn
    /// override is supplied.
    pub current_model: Mutex<Option<String>>,
    /// Session-level reasoning variant (OpenCode's per-prompt effort
    /// selector — `low`/`medium`/`high`/`max`/…). Seeded from the
    /// session's initial effort; the send path uses it when no per-turn
    /// effort override is supplied.
    pub current_variant: Mutex<Option<String>>,
    server_handle: OpenCodeServerHandle,
    http: reqwest::Client,
    /// Routing context shared with the SSE listener — the listener
    /// reads it on each event so updates land before the next
    /// `prompt_async` triggers a new burst of SSE traffic.
    event_ctx: Arc<Mutex<EventContext>>,
    /// Subagent routing state shared with the SSE listener: the
    /// watched-session set and the permission-id → session map. The
    /// reply path reads the latter so a subagent's approval targets the
    /// child session id rather than the root.
    router: Arc<Mutex<SseRouter>>,
    /// Per-session token accounting, shared with the SSE listener that
    /// feeds it. The session itself only touches it on a model swap, to
    /// invalidate the window the previous model's catalogue entry
    /// supplied (see [`OpenCodeUsageState::model_changed`]).
    usage: Arc<Mutex<OpenCodeUsageState>>,
    sse_handle: Mutex<Option<JoinHandle<()>>>,
    event_tx: broadcast::Sender<ProviderRuntimeEvent>,
    /// Set by the SSE listener when it gives up on an unreachable server.
    /// [`is_dead`](OpenCodeSession::is_dead) reports it so the provider's
    /// `has_session` treats the corpse as absent and the next send rebuilds
    /// a fresh session via `ensure_live_session`.
    dead: Arc<AtomicBool>,
    closing: tokio::sync::watch::Sender<bool>,
    ready: tokio::sync::watch::Receiver<bool>,
    outbound: Mutex<()>,
    queued: Mutex<VecDeque<(String, crate::agent_provider::SendTurnInput)>>,
    queue_worker: Mutex<Option<JoinHandle<()>>>,
    native_requests: Mutex<Vec<JoinHandle<()>>>,
    native_admissions: Mutex<Vec<NativeAdmission>>,
    native_inputs: Arc<Mutex<HashSet<String>>>,
}

impl OpenCodeSession {
    /// Bring up a session. Spawns the HTTP server (if absent), creates
    /// the OpenCode-side session, kicks off the SSE listener, and
    /// emits `SessionConfigured` on the provider's broadcast channel.
    pub async fn start(
        manager: Arc<OpenCodeServerManager>,
        mcp_registry: Option<McpRegistry>,
        thread_id: ThreadId,
        initial_model: Option<String>,
        initial_variant: Option<String>,
        resume_session_id: Option<String>,
        event_tx: broadcast::Sender<ProviderRuntimeEvent>,
        // Usage state carried over from a previous session object that
        // used this same OpenCode session id, when there is one. `None`
        // starts fresh. See `OpenCodeAgentProvider::usage_states`.
        carried_usage: Option<Arc<Mutex<OpenCodeUsageState>>>,
        directory: Option<std::path::PathBuf>,
    ) -> Result<Arc<Self>, ProviderError> {
        let server_handle = manager.ensure_running().await.map_err(|err| {
            if err == "opencode_not_installed" {
                ProviderError::NotInstalled {
                    provider: ProviderKind::OpenCode,
                    hint:
                        "Install OpenCode (https://opencode.ai/) and ensure `opencode` is on PATH."
                            .into(),
                }
            } else {
                ProviderError::ProcessError {
                    message: "failed to start OpenCode server".into(),
                    source: Some(err),
                }
            }
        })?;

        let mut headers = reqwest::header::HeaderMap::new();
        if let Some(directory) = directory.as_ref() {
            headers.insert(
                "x-opencode-directory",
                reqwest::header::HeaderValue::from_str(&urlencoding::encode(
                    &directory.to_string_lossy(),
                ))
                .map_err(|error| ProviderError::ValidationError {
                    message: error.to_string(),
                })?,
            );
        }
        let http = reqwest::Client::builder()
            .default_headers(headers)
            .build()
            .map_err(|err| ProviderError::ProcessError {
                message: "failed to build HTTP client".into(),
                source: Some(err.to_string()),
            })?;

        // Shared MCP is best-effort: OpenCode only grew the runtime
        // `POST /mcp` endpoint in 1.18.18, and older binaries must still be
        // able to start a session — just without Codemux-configured servers.
        // Failures surface as a RuntimeWarning instead of killing the session.
        if let Some(registry) = mcp_registry.as_ref() {
            if let Err(error) =
                attach_mcp_gateway_scoped(&http, &server_handle, registry, directory.as_deref())
                    .await
            {
                eprintln!("[codemux::opencode] shared MCP gateway attach failed: {error}");
                let _ = event_tx.send(ProviderRuntimeEvent::RuntimeWarning {
                    thread_id: Some(thread_id.clone()),
                    message: format!(
                        "Codemux MCP servers are unavailable in this OpenCode session \
                         (gateway attach failed: {error}). Update OpenCode to 1.18.18 or later."
                    ),
                    original_payload: None,
                });
            }
        }

        // Best-effort resume: readopt the persisted server-side session id when
        // it still exists on the OpenCode server (it persists sessions on disk,
        // so a rebuilt session — even after a server restart — can keep its
        // conversation context). We probe `GET /session/{id}/message?limit=1`:
        // a success means the session is addressable, so we skip `POST /session`
        // and reuse the id; a 404 / error means it is stale, so we fall through
        // to creating a fresh session (the visible transcript still hydrates
        // from the DB, so the user keeps their history either way).
        let base = server_handle.base_url.trim_end_matches('/').to_string();
        let native_v2 = server_handle.api == super::api::OpenCodeApi::V2;
        let resumed_id = match &resume_session_id {
            Some(id) if native_v2 => {
                let response = http
                    .get(format!("{base}/api/session/{}", urlencoding::encode(id)))
                    .basic_auth("opencode", Some(&server_handle.server_password))
                    .timeout(std::time::Duration::from_secs(15))
                    .send()
                    .await
                    .map_err(|e| ProviderError::RpcError {
                        message: format!("session_resume_send_failed: {e}"),
                    })?;
                if response.status() == reqwest::StatusCode::NOT_FOUND {
                    None
                } else {
                    let response =
                        response
                            .error_for_status()
                            .map_err(|e| ProviderError::RpcError {
                                message: format!("session_resume_failed: {e}"),
                            })?;
                    let value: serde_json::Value =
                        response.json().await.map_err(|e| ProviderError::RpcError {
                            message: format!("session_resume_decode_failed: {e}"),
                        })?;
                    if value["data"]["id"].as_str() != Some(id) {
                        return Err(ProviderError::RpcError {
                            message: "session_resume_identity_mismatch".into(),
                        });
                    }
                    if let (Some(expected), Some(actual)) = (
                        directory.as_ref(),
                        value["data"]["location"]["directory"].as_str(),
                    ) {
                        if expected.to_string_lossy() != actual {
                            return Err(ProviderError::ValidationError {
                                message:
                                    "OpenCode resume directory differs from the persisted session"
                                        .into(),
                            });
                        }
                    }
                    Some(id.clone())
                }
            }
            Some(id)
                if session_is_addressable(&http, &base, &server_handle.server_password, id)
                    .await =>
            {
                Some(id.clone())
            }
            _ => None,
        };
        // Captured before `resumed_id` is consumed below: whether this
        // build readopted an existing OpenCode session decides whether the
        // carried usage state applies.
        let did_resume = resumed_id.is_some();
        let session_id = match resumed_id {
            Some(id) => id,
            None => {
                let create_url = format!(
                    "{base}{}",
                    server_handle.api.route("/session", "/api/session")
                );
                let body = if native_v2 {
                    let cwd = directory
                        .clone()
                        .map(Ok)
                        .unwrap_or_else(std::env::current_dir)
                        .map_err(|e| ProviderError::ValidationError {
                            message: e.to_string(),
                        })?;
                    if !cwd.is_absolute() {
                        return Err(ProviderError::ValidationError {
                            message: "OpenCode session cwd must be absolute".into(),
                        });
                    }
                    super::v2::create_body(
                        &cwd.to_string_lossy(),
                        initial_model.as_deref(),
                        initial_variant.as_deref(),
                    )
                    .map_err(|message| ProviderError::ValidationError { message })?
                } else {
                    serde_json::to_value(SessionCreateRequest::default()).unwrap()
                };
                let response = http
                    .post(&create_url)
                    .basic_auth("opencode", Some(&server_handle.server_password))
                    .json(&body)
                    .send()
                    .await
                    .map_err(|err| ProviderError::RpcError {
                        message: format!("session_create_send_failed: {err}"),
                    })?;

                let status = response.status();
                if !status.is_success() {
                    return Err(ProviderError::RpcError {
                        message: format!("session_create_http_status_{}", status.as_u16()),
                    });
                }
                let value: serde_json::Value =
                    response
                        .json()
                        .await
                        .map_err(|err| ProviderError::RpcError {
                            message: format!("session_create_decode_failed: {err}"),
                        })?;
                let session_resp: SessionResponse = serde_json::from_value(if native_v2 {
                    value["data"].clone()
                } else {
                    value
                })
                .map_err(|err| ProviderError::RpcError {
                    message: format!("session_create_decode_failed: {err}"),
                })?;
                session_resp.id
            }
        };
        let provider_session_id = ProviderSessionId(session_id.clone());

        let initial_turn = TurnId(format!("turn_{}", Uuid::new_v4()));
        let event_ctx = Arc::new(Mutex::new(EventContext {
            thread_id: thread_id.clone(),
            turn_id: initial_turn,
            provider_session_id: provider_session_id.clone(),
            turn_active: false,
            context_window_tokens: None,
            model_cost: None,
        }));
        // Best-effort: the numeric context window lives only in the
        // upstream provider catalogue, so resolve it off the hot path.
        // Failure is non-fatal — the meter simply renders a bare token
        // count until (or unless) a number lands.
        spawn_context_window_probe(
            server_handle.base_url.clone(),
            server_handle.server_password.clone(),
            initial_model.clone(),
            server_handle.api,
            event_ctx.clone(),
        );

        // Shared liveness flag: the SSE listener flips it when it exhausts
        // its reconnect budget (server gone), which makes `has_session`
        // report the session dead so the next send auto-resumes through
        // `ensure_live_session` rather than POSTing to a corpse.
        let dead = Arc::new(AtomicBool::new(false));
        let router = Arc::new(Mutex::new(SseRouter::new(session_id.clone())));
        // Reuse the carried state when this rebuild readopted the same
        // OpenCode session id; a genuinely new session starts clean.
        let usage = match carried_usage {
            Some(carried) if did_resume => carried,
            _ => Arc::new(Mutex::new(OpenCodeUsageState::default())),
        };
        let queue_events = event_tx.subscribe();
        let (ready_tx, ready) =
            tokio::sync::watch::channel(server_handle.api == super::api::OpenCodeApi::Legacy);
        let peer = SsePeer {
            api: server_handle.api,
            ready: ready_tx,
            directory: directory.clone(),
            session_id: session_id.clone(),
            event_ctx: event_ctx.clone(),
            router: router.clone(),
            dead: dead.clone(),
            // Written by the listener task (this session spawns exactly
            // one); the session keeps a handle so `set_model` can
            // invalidate the previous model's context window.
            usage: usage.clone(),
        };
        let sse_handle = spawn_sse_listener(
            server_handle.base_url.clone(),
            server_handle.server_password.clone(),
            peer,
            event_tx.clone(),
        );

        let _ = event_tx.send(ProviderRuntimeEvent::SessionConfigured {
            thread_id: thread_id.clone(),
            provider_session_id: provider_session_id.clone(),
        });
        // Persist the server-side session id (via the command layer's
        // `ResumeCursorUpdated` handler) so a later live rebuild after a dead
        // SSE listener can readopt it and keep the conversation context.
        // OpenCode never emits this on its own (no SDK cursor notification), so
        // the adapter surfaces it here on every session start — fresh or
        // resumed — mirroring the `{"resume": id}` shape Claude/Codex use.
        // This event heals the row on rebuilds; the *initial* session's cursor
        // is persisted synchronously by `agent_chat_start_session` from the
        // `ProviderSession.resume_cursor` the provider returns, because this
        // event races the row upsert across the bridge task.
        let _ = event_tx.send(ProviderRuntimeEvent::ResumeCursorUpdated {
            thread_id: thread_id.clone(),
            resume_cursor: super::agent::resume_cursor_for(&provider_session_id),
        });

        let session = Arc::new(Self {
            thread_id,
            provider_session_id,
            current_model: Mutex::new(initial_model),
            current_variant: Mutex::new(initial_variant),
            server_handle,
            http,
            event_ctx,
            router,
            usage,
            sse_handle: Mutex::new(Some(sse_handle)),
            event_tx,
            dead,
            closing: tokio::sync::watch::channel(false).0,
            ready,
            outbound: Mutex::new(()),
            queued: Mutex::new(VecDeque::new()),
            queue_worker: Mutex::new(None),
            native_requests: Mutex::new(Vec::new()),
            native_admissions: Mutex::new(Vec::new()),
            native_inputs: Arc::new(Mutex::new(HashSet::new())),
        });
        if native_v2 {
            if let Err(error) = session.wait_ready().await {
                let _ = session.shutdown().await;
                return Err(error);
            }
            // A resumed conversation keeps its native history, while explicit
            // picker choices still require a successful native model ACK.
            let model = session.current_model.lock().await.clone();
            let variant = session.current_variant.lock().await.clone();
            if did_resume || (model.is_none() && variant.is_some()) {
                let selected = match model {
                    Some(model) => Some(model),
                    None if variant.is_some() => Some(session.default_native_model().await?),
                    _ => None,
                };
                if let Some(model) = selected {
                    if let Err(error) = session.apply_native_model(&model, variant.as_deref()).await
                    {
                        let _ = session.shutdown().await;
                        return Err(error);
                    }
                    *session.current_model.lock().await = Some(model);
                }
            }
        }
        let active = session.event_ctx.lock().await.turn_active;
        let status = if active {
            SessionStatus::Running {
                active_turn: session.event_ctx.lock().await.turn_id.clone(),
            }
        } else {
            SessionStatus::Ready
        };
        let _ = session
            .event_tx
            .send(ProviderRuntimeEvent::SessionStateChanged {
                thread_id: session.thread_id.clone(),
                status,
            });
        session.start_queue_worker(queue_events).await;
        Ok(session)
    }

    async fn start_queue_worker(
        self: &Arc<Self>,
        mut events: broadcast::Receiver<ProviderRuntimeEvent>,
    ) {
        let weak = Arc::downgrade(self);
        let handle = tokio::spawn(async move {
            loop {
                let event = match events.recv().await {
                    Ok(event) => event,
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        if let Some(session) = weak.upgrade() {
                            session.drain_queue().await;
                        }
                        continue;
                    }
                    Err(_) => break,
                };
                let Some(session) = weak.upgrade() else { break };
                match event {
                    ProviderRuntimeEvent::SessionStateChanged {
                        thread_id,
                        status: SessionStatus::Ready,
                    } if thread_id == session.thread_id => session.drain_queue().await,
                    ProviderRuntimeEvent::SessionStateChanged {
                        thread_id,
                        status: SessionStatus::Closed | SessionStatus::Error { .. },
                    } if thread_id == session.thread_id => session.cancel_all_queued().await,
                    _ => {}
                }
            }
        });
        *self.queue_worker.lock().await = Some(handle);
    }

    pub async fn enqueue_or_send(
        &self,
        input: crate::agent_provider::SendTurnInput,
    ) -> Result<crate::agent_provider::TurnStartResult, ProviderError> {
        let _outbound = self.outbound.lock().await;
        if self.is_dead() {
            return Err(ProviderError::SessionClosed {
                thread_id: self.thread_id.clone(),
            });
        }
        if self.turn_active().await || !self.queued.lock().await.is_empty() {
            return Ok(self.queue_input(input, None).await);
        }
        self.dispatch_input(input, false, None).await
    }

    async fn queue_input(
        &self,
        input: crate::agent_provider::SendTurnInput,
        existing_id: Option<String>,
    ) -> crate::agent_provider::TurnStartResult {
        let is_existing = existing_id.is_some();
        let id = existing_id.unwrap_or_else(|| format!("opencode-queued-{}", Uuid::new_v4()));
        let text = input
            .display_text
            .clone()
            .unwrap_or_else(|| input.text.clone());
        let client_nonce = input.client_nonce.clone();
        let mut queue = self.queued.lock().await;
        if is_existing {
            queue.retain(|(key, _)| key != &id);
            queue.push_front((id.clone(), input));
        } else {
            queue.push_back((id.clone(), input));
            let _ = self.event_tx.send(ProviderRuntimeEvent::TurnQueued {
                thread_id: self.thread_id.clone(),
                queued_id: id.clone(),
                client_nonce,
                text,
            });
        }
        crate::agent_provider::TurnStartResult {
            turn_id: TurnId(String::new()),
            queued_id: Some(id),
            steered: false,
        }
    }

    pub async fn steer_turn(
        &self,
        input: crate::agent_provider::SendTurnInput,
    ) -> Result<crate::agent_provider::TurnStartResult, ProviderError> {
        let _outbound = self.outbound.lock().await;
        self.dispatch_input(input, true, None).await
    }

    async fn dispatch_input(
        &self,
        input: crate::agent_provider::SendTurnInput,
        steer: bool,
        queued_id: Option<String>,
    ) -> Result<crate::agent_provider::TurnStartResult, ProviderError> {
        if self.is_dead() {
            return Err(ProviderError::SessionClosed {
                thread_id: self.thread_id.clone(),
            });
        }
        let steered = steer && self.turn_active().await;
        let checkpoint = input.turn_checkpoint.as_ref().filter(|_| !steered);
        if let Some(checkpoint) = checkpoint {
            checkpoint.prepare().await;
        }
        let result = self
            .post_turn(
                input.text.clone(),
                input.images.clone(),
                input.model_override.clone(),
                input.effort_override.clone(),
                steered,
                input.turn_checkpoint.clone(),
            )
            .await;
        match result {
            Ok(Some(turn_id)) => {
                if let Some(checkpoint) = checkpoint {
                    checkpoint.commit().await;
                }
                Ok(crate::agent_provider::TurnStartResult {
                    turn_id,
                    queued_id: None,
                    steered,
                })
            }
            Ok(None) => {
                if let Some(checkpoint) = checkpoint {
                    checkpoint.abort().await;
                }
                // A background execution can resume while model preparation
                // awaits its ACK. Preserve this input for the next idle turn.
                Ok(self.queue_input(input, queued_id).await)
            }
            Err(error) => {
                if let Some(checkpoint) = checkpoint {
                    checkpoint.abort().await;
                }
                Err(error)
            }
        }
    }

    async fn drain_queue(&self) {
        let _outbound = self.outbound.lock().await;
        if self.is_dead() {
            self.cancel_all_queued().await;
            return;
        }
        while !self.turn_active().await {
            let Some((id, input)) = self.queued.lock().await.pop_front() else {
                return;
            };
            let text = input
                .display_text
                .clone()
                .unwrap_or_else(|| input.text.clone());
            match self.dispatch_input(input, false, Some(id.clone())).await {
                Ok(result) if result.queued_id.is_some() => return,
                Ok(result) => {
                    let _ = self
                        .event_tx
                        .send(ProviderRuntimeEvent::QueuedTurnDispatched {
                            steered: false,
                            thread_id: self.thread_id.clone(),
                            queued_id: id,
                            turn_id: result.turn_id,
                            text,
                        });
                    return;
                }
                Err(error) => {
                    let _ = self.event_tx.send(ProviderRuntimeEvent::RuntimeWarning {
                        thread_id: Some(self.thread_id.clone()),
                        message: format!("Queued message failed: {error}"),
                        original_payload: None,
                    });
                    let _ = self
                        .event_tx
                        .send(ProviderRuntimeEvent::QueuedTurnCancelled {
                            thread_id: self.thread_id.clone(),
                            queued_id: id,
                        });
                }
            }
        }
    }

    pub async fn cancel_queued(&self, id: &str) -> bool {
        let _outbound = self.outbound.lock().await;
        let mut queue = self.queued.lock().await;
        let Some(index) = queue.iter().position(|(key, _)| key == id) else {
            return false;
        };
        queue.remove(index);
        let _ = self
            .event_tx
            .send(ProviderRuntimeEvent::QueuedTurnCancelled {
                thread_id: self.thread_id.clone(),
                queued_id: id.into(),
            });
        true
    }

    async fn cancel_all_queued(&self) {
        for (id, _) in self.queued.lock().await.drain(..) {
            let _ = self
                .event_tx
                .send(ProviderRuntimeEvent::QueuedTurnCancelled {
                    thread_id: self.thread_id.clone(),
                    queued_id: id,
                });
        }
    }

    pub async fn send_queued_now(&self, id: &str, steer: bool) -> Result<(), ProviderError> {
        let outbound = self.outbound.lock().await;
        let input = self
            .queued
            .lock()
            .await
            .iter()
            .find(|(key, _)| key == id)
            .map(|(_, input)| input.clone());
        let Some(input) = input else { return Ok(()) };
        if steer {
            let text = input
                .display_text
                .clone()
                .unwrap_or_else(|| input.text.clone());
            let result = self.dispatch_input(input, true, Some(id.into())).await?;
            if result.queued_id.is_some() {
                return Ok(());
            }
            self.queued.lock().await.retain(|(key, _)| key != id);
            let _ = self
                .event_tx
                .send(ProviderRuntimeEvent::QueuedTurnDispatched {
                    steered: result.steered,
                    thread_id: self.thread_id.clone(),
                    queued_id: id.into(),
                    turn_id: result.turn_id,
                    text,
                });
        } else {
            // Keep the queue unchanged if the abort fails. The outbound lock
            // prevents the completion worker from draining before promotion.
            if self.turn_active().await {
                self.until_closed(self.interrupt()).await?;
            }
            let mut queue = self.queued.lock().await;
            if let Some(index) = queue.iter().position(|(key, _)| key == id) {
                let item = queue.remove(index).unwrap();
                queue.push_front(item);
            }
        }
        drop(outbound);
        self.drain_queue().await;
        Ok(())
    }

    async fn wait_ready(&self) -> Result<(), ProviderError> {
        if self.server_handle.api == super::api::OpenCodeApi::Legacy {
            return Ok(());
        }
        let mut ready = self.ready.clone();
        tokio::time::timeout(std::time::Duration::from_secs(15), async {
            while !*ready.borrow_and_update() {
                ready.changed().await.map_err(|_| ProviderError::RpcError {
                    message: "OpenCode event stream closed before readiness".into(),
                })?;
            }
            Ok(())
        })
        .await
        .map_err(|_| ProviderError::Timeout {
            operation: "OpenCode server.connected handshake".into(),
            elapsed_ms: 15000,
        })?
    }

    async fn default_native_model(&self) -> Result<String, ProviderError> {
        let response = self
            .http
            .get(format!(
                "{}/api/model/default",
                self.server_handle.base_url.trim_end_matches('/')
            ))
            .basic_auth("opencode", Some(&self.server_handle.server_password))
            .timeout(std::time::Duration::from_secs(15))
            .send()
            .await
            .map_err(|e| ProviderError::RpcError {
                message: e.to_string(),
            })?
            .error_for_status()
            .map_err(|e| ProviderError::RpcError {
                message: e.to_string(),
            })?;
        let value: serde_json::Value =
            response.json().await.map_err(|e| ProviderError::RpcError {
                message: e.to_string(),
            })?;
        let data = &value["data"];
        match (data["providerID"].as_str(), data["id"].as_str()) {
            (Some(provider), Some(id)) => Ok(format!("{provider}/{id}")),
            _ => Err(ProviderError::ValidationError {
                message: "Select a model before choosing an OpenCode effort.".into(),
            }),
        }
    }

    async fn apply_native_model(
        &self,
        model: &str,
        variant: Option<&str>,
    ) -> Result<(), ProviderError> {
        let model = super::v2::model_ref(model, variant)
            .map_err(|message| ProviderError::ValidationError { message })?;
        let response = self
            .http
            .post(format!(
                "{}/api/session/{}/model",
                self.server_handle.base_url.trim_end_matches('/'),
                urlencoding::encode(&self.provider_session_id.0)
            ))
            .basic_auth("opencode", Some(&self.server_handle.server_password))
            .json(&serde_json::json!({"model":model}))
            .timeout(std::time::Duration::from_secs(15))
            .send()
            .await
            .map_err(|e| ProviderError::RpcError {
                message: format!("model_send_failed: {e}"),
            })?;
        if !response.status().is_success() {
            return Err(ProviderError::RpcError {
                message: format!("model_http_status_{}", response.status().as_u16()),
            });
        }
        Ok(())
    }

    /// Whether the SSE listener has declared this session's server
    /// unreachable (reconnect budget exhausted). A dead session is
    /// treated as absent by the provider so the next send auto-resumes.
    /// The token-accounting handle this session drives.
    ///
    /// Exposed so the provider can key it by OpenCode session id and hand
    /// it back to a later rebuild that readopts the same id — see
    /// `OpenCodeAgentProvider::usage_states`.
    pub fn usage_state(&self) -> Arc<Mutex<OpenCodeUsageState>> {
        Arc::clone(&self.usage)
    }

    pub fn is_dead(&self) -> bool {
        self.dead.load(Ordering::Relaxed)
    }

    /// Whether a turn is currently in flight on this session. Reads the
    /// SSE routing context's `turn_active` flag (armed on send, cleared when
    /// the turn settles). Cheap in-memory check — does not touch the server.
    pub async fn turn_active(&self) -> bool {
        self.event_ctx.lock().await.turn_active
    }

    /// Send a user turn. Mints a new Codemux turn id, threads it into
    /// the SSE routing context, then `POST`s `prompt_async`.
    pub async fn send_turn(
        &self,
        text: String,
        images: Vec<ImageInput>,
        model_override: Option<String>,
        effort_override: Option<String>,
    ) -> Result<TurnId, ProviderError> {
        let _outbound = self.outbound.lock().await;
        self.post_turn(text, images, model_override, effort_override, false, None)
            .await?
            .ok_or_else(|| ProviderError::ValidationError {
                message: "OpenCode execution resumed before this turn could be admitted. Queue the prompt or steer the active turn.".into(),
            })
    }

    async fn post_turn(
        &self,
        text: String,
        images: Vec<ImageInput>,
        model_override: Option<String>,
        effort_override: Option<String>,
        steer: bool,
        checkpoint: Option<Arc<dyn crate::agent_provider::types::TurnDispatchCheckpoint>>,
    ) -> Result<Option<TurnId>, ProviderError> {
        self.until_closed(self.post_turn_open(
            text,
            images,
            model_override,
            effort_override,
            steer,
            checkpoint,
        ))
        .await
    }

    async fn post_turn_open(
        &self,
        text: String,
        images: Vec<ImageInput>,
        model_override: Option<String>,
        effort_override: Option<String>,
        steer: bool,
        checkpoint: Option<Arc<dyn crate::agent_provider::types::TurnDispatchCheckpoint>>,
    ) -> Result<Option<TurnId>, ProviderError> {
        self.ensure_open()?;
        self.wait_ready().await?;
        self.ensure_open()?;
        let current_model = self.current_model.lock().await.clone();
        let model = match model_override {
            Some(m) => Some(m),
            None => current_model.clone(),
        };
        // Per-turn effort wins; otherwise fall back to the session's
        // variant. OpenCode's `variant` selects the model's reasoning
        // effort on each `prompt_async`.
        let variant = match effort_override {
            Some(v) => Some(v),
            None if self.server_handle.api == super::api::OpenCodeApi::V2
                && model != current_model =>
            {
                None
            }
            None => self.current_variant.lock().await.clone(),
        };
        let native = crate::agent_provider::slash_commands::leading_command(&text);
        if native.is_some() && steer {
            return Err(ProviderError::ValidationError {
                message: "Native commands cannot steer a running turn. Choose Queue or Interrupt and send.".into(),
            });
        }
        if matches!(native,Some(("compact",arguments)) if !arguments.is_empty() || !images.is_empty())
        {
            return Err(ProviderError::ValidationError {
                message: "OpenCode /compact accepts no arguments or images.".into(),
            });
        }
        let prompt =
            build_prompt_async_request(text.clone(), model.as_deref(), variant.as_deref(), &images)
                .map_err(|err| ProviderError::ValidationError { message: err })?;
        let native_v2 = self.server_handle.api == super::api::OpenCodeApi::V2;
        let (endpoint, body) = if native_v2 {
            if let Some(checkpoint) = &checkpoint {
                checkpoint.authorize_dispatch()?;
            }
            let selected_variant = variant.as_deref().filter(|v| !v.is_empty());
            if model != *self.current_model.lock().await
                || selected_variant != self.current_variant.lock().await.as_deref()
            {
                let selected_model = match model.clone() {
                    Some(model) => model,
                    None => self.default_native_model().await?,
                };
                self.apply_native_model(&selected_model, selected_variant)
                    .await?;
                self.ensure_open()?;
                *self.current_model.lock().await = Some(selected_model.clone());
                *self.current_variant.lock().await = selected_variant.map(str::to_string);
                self.refresh_model_context(selected_model).await;
            }
            let files: Vec<_> = prompt
                .parts
                .iter()
                .filter_map(|p| match p {
                    super::protocol::PartInput::File { url, filename, .. } => {
                        let mut file = serde_json::json!({"uri":url});
                        if let Some(name) = filename {
                            file["name"] = serde_json::json!(name);
                        }
                        Some(file)
                    }
                    _ => None,
                })
                .collect();
            match native {
                Some(("compact", arguments)) => {
                    if !arguments.is_empty() || !images.is_empty() {
                        return Err(ProviderError::ValidationError {
                            message: "OpenCode /compact accepts no arguments or images.".into(),
                        });
                    }
                    (
                        "compact",
                        serde_json::json!({"id":format!("msg_codemux_{}",Uuid::new_v4()),"delivery":"queue"}),
                    )
                }
                Some((name, arguments)) => {
                    let mut body =
                        serde_json::json!({"name":name,"text":arguments,"delivery":"queue"});
                    if !files.is_empty() {
                        body["files"] = serde_json::json!(files);
                    }
                    ("command", body)
                }
                None => {
                    let mut body = super::v2::prompt_body(&text, files, steer);
                    body["id"] = serde_json::json!(format!("msg_codemux_{}", Uuid::new_v4()));
                    body["resume"] = serde_json::json!(false);
                    ("prompt", body)
                }
            }
        } else {
            match native {
                Some(("compact", arguments)) => {
                    if !arguments.is_empty() || !images.is_empty() {
                        return Err(ProviderError::ValidationError {
                            message: "OpenCode /compact accepts no arguments or images.".into(),
                        });
                    }
                    let model =
                        prompt
                            .model
                            .as_ref()
                            .ok_or_else(|| ProviderError::ValidationError {
                                message:
                                    "Select a model before compacting this OpenCode conversation."
                                        .into(),
                            })?;
                    (
                        "summarize",
                        serde_json::json!({ "providerID": model.provider_id, "modelID": model.model_id }),
                    )
                }
                Some((name, arguments)) => {
                    // /command expands native templates and accepts image file parts.
                    // Sending /name through prompt_async would only send literal prose.
                    let files: Vec<_> = prompt
                        .parts
                        .iter()
                        .filter(|part| matches!(part, super::protocol::PartInput::File { .. }))
                        .collect();
                    let mut body = serde_json::json!({
                        "command": name, "arguments": arguments, "parts": files,
                    });
                    // Optional Zod fields accept omission, not JSON null.
                    if let Some(model) = model.as_ref() {
                        body["model"] = serde_json::json!(model);
                    }
                    if let Some(variant) = prompt.variant.as_ref() {
                        body["variant"] = serde_json::json!(variant);
                    }
                    ("command", body)
                }
                None => ("prompt_async", serde_json::to_value(&prompt).unwrap()),
            }
        };
        let turn_id = {
            let mut ctx = self.event_ctx.lock().await;
            self.ensure_open()?;
            if native_v2 && !steer && ctx.turn_active {
                return Ok(None);
            }
            if let Some(checkpoint) = &checkpoint {
                checkpoint.authorize_dispatch()?;
            }
            if !steer {
                ctx.turn_id = TurnId(format!("turn_{}", Uuid::new_v4()));
            }
            ctx.turn_active = true;
            ctx.turn_id.clone()
        };
        if !steer {
            let _ = self
                .event_tx
                .send(ProviderRuntimeEvent::SessionStateChanged {
                    thread_id: self.thread_id.clone(),
                    status: SessionStatus::Running {
                        active_turn: turn_id.clone(),
                    },
                });
        }
        if native_v2 {
            return self
                .post_native_admission(endpoint, body, turn_id)
                .await
                .map(Some);
        }
        let url = format!(
            "{}{}/session/{}/{}",
            self.server_handle.base_url.trim_end_matches('/'),
            if native_v2 { "/api" } else { "" },
            urlencoding::encode(&self.provider_session_id.0),
            endpoint
        );
        let request = self
            .http
            .post(&url)
            .basic_auth("opencode", Some(&self.server_handle.server_password))
            .json(&body);
        if endpoint != "prompt_async" && !(native_v2 && matches!(endpoint, "prompt" | "compact")) {
            // Native endpoints respond only after generation completes. Keep
            // the composer/queue responsive while SSE streams the actual turn.
            let context = self.event_ctx.clone();
            let events = self.event_tx.clone();
            let thread_id = self.thread_id.clone();
            let expected_turn = turn_id.clone();
            let dead = self.dead.clone();
            let handle = tokio::spawn(async move {
                let result = request
                    .timeout(std::time::Duration::from_secs(600))
                    .send()
                    .await;
                let failure = match result {
                    Ok(response) if response.status().is_success() => response
                        .bytes()
                        .await
                        .err()
                        .map(|error| format!("{endpoint}_response_failed: {error}")),
                    Ok(response) => {
                        let status = response.status();
                        let body = response.text().await.unwrap_or_default();
                        Some(format!(
                            "{endpoint}_http_status_{}: {body}",
                            status.as_u16()
                        ))
                    }
                    Err(error) => Some(format!("{endpoint}_send_failed: {error}")),
                };
                // Successful generations settle on session.idle in the SSE
                // stream, after their final output. HTTP can arrive first;
                // settling here would route late output/idle to a queued turn.
                let Some(message) = failure else { return };
                let mut live = context.lock().await;
                // SSE may already have settled this turn and dispatched the
                // next. A late HTTP result must never clear that newer turn.
                if !dead.load(Ordering::Relaxed)
                    && live.turn_active
                    && live.turn_id == expected_turn
                {
                    live.turn_active = false;
                    let status = crate::agent_provider::TurnStatus::Error {
                        subtype: "native_command_failed".into(),
                        message,
                    };
                    let _ = events.send(ProviderRuntimeEvent::TurnCompleted {
                        thread_id: thread_id.clone(),
                        turn_id: expected_turn,
                        status,
                        usage: None,
                    });
                    let _ = events.send(ProviderRuntimeEvent::SessionStateChanged {
                        thread_id,
                        status: SessionStatus::Ready,
                    });
                }
            });
            let mut requests = self.native_requests.lock().await;
            requests.retain(|request| !request.is_finished());
            requests.push(handle);
            return Ok(Some(turn_id));
        }
        let response = request
            .timeout(std::time::Duration::from_secs(
                if native_v2 && endpoint == "compact" {
                    600
                } else {
                    15
                },
            ))
            .send()
            .await;
        let response = match response {
            Ok(r) => r,
            Err(err) => {
                // Transport failure does not prove that the server rejected the
                // request. Native admission is tracked separately above.
                if !steer {
                    self.event_ctx.lock().await.turn_active = false;
                    let _ = self
                        .event_tx
                        .send(ProviderRuntimeEvent::SessionStateChanged {
                            thread_id: self.thread_id.clone(),
                            status: SessionStatus::Ready,
                        });
                }
                return Err(ProviderError::RpcError {
                    message: format!("{endpoint}_send_failed: {err}"),
                });
            }
        };
        let status = response.status();
        if !status.is_success() {
            if !steer {
                self.event_ctx.lock().await.turn_active = false;
                let _ = self
                    .event_tx
                    .send(ProviderRuntimeEvent::SessionStateChanged {
                        thread_id: self.thread_id.clone(),
                        status: SessionStatus::Ready,
                    });
            }
            let body = response.text().await.unwrap_or_default();
            return Err(ProviderError::RpcError {
                message: format!("{endpoint}_http_status_{}: {body}", status.as_u16()),
            });
        }
        // SSE may already have completed the turn before this HTTP response.
        // Never re-arm it here. A steer retains the active turn's identity.
        Ok(Some(turn_id))
    }

    pub(crate) async fn has_unconfirmed_native_admissions(&self) -> bool {
        self.native_admissions
            .lock()
            .await
            .iter()
            .any(|request| !matches!(request.borrow().as_ref(), Some(Ok(()))))
    }

    fn native_peer(&self) -> NativeAdmissionPeer {
        NativeAdmissionPeer {
            http: self.http.clone(),
            base: self
                .server_handle
                .base_url
                .trim_end_matches('/')
                .to_string(),
            password: self.server_handle.server_password.clone(),
            session: urlencoding::encode(&self.provider_session_id.0).into_owned(),
            inputs: self.native_inputs.clone(),
        }
    }

    async fn post_native_admission(
        &self,
        endpoint: &'static str,
        mut body: serde_json::Value,
        turn_id: TurnId,
    ) -> Result<TurnId, ProviderError> {
        let peer = self.native_peer();
        let closing = self.closing.subscribe();
        let (done, mut result) = tokio::sync::watch::channel(None);
        let input_id = body["id"].as_str().map(str::to_string);
        if let Some(id) = &input_id {
            self.native_inputs.lock().await.insert(id.clone());
        }
        // Register before spawning/writing. The closure-cancellable caller can
        // leave at any await, but shutdown still owns the original admission.
        let mut admissions = self.native_admissions.lock().await;
        self.ensure_open()?;
        admissions.retain(|request| !matches!(request.borrow().as_ref(), Some(Ok(()))));
        admissions.push(result.clone());
        let events = self.event_tx.clone();
        let thread_id = self.thread_id.clone();
        let dead = self.dead.clone();
        let close = self.closing.clone();
        tokio::spawn(async move {
            let admission = async {
                let path = format!("/api/session/{}/{endpoint}", peer.session);
                let bytes = NativeAdmissionPeer::acknowledged(
                    peer.request(reqwest::Method::POST, &path).json(&body),
                )
                .await?;
                if endpoint == "compact" {
                    let value: serde_json::Value = serde_json::from_slice(&bytes)
                        .map_err(|e| format!("native_compact_decode_failed: {e}"))?;
                    let id = value["data"]["id"]
                        .as_str()
                        .ok_or("native_compact_missing_identity")?;
                    // Compaction may coalesce into an existing pending item.
                    peer.inputs.lock().await.insert(id.to_string());
                }
                if endpoint == "prompt" && !*closing.borrow() {
                    // Reconcile the exact acknowledged input, then wake once.
                    // Never repeat this request after cancellation: cancelled
                    // IDs have no native tombstone and could be re-admitted.
                    body["resume"] = serde_json::json!(true);
                    NativeAdmissionPeer::acknowledged(
                        peer.request(reqwest::Method::POST, &path).json(&body),
                    )
                    .await?;
                }
                if *closing.borrow() {
                    peer.finish().await?;
                }
                Ok(())
            }
            .await;
            if let Err(message) = &admission {
                if !*closing.borrow() {
                    dead.store(true, Ordering::Relaxed);
                    close.send_replace(true);
                    let _ = events.send(ProviderRuntimeEvent::SessionStateChanged {
                        thread_id,
                        status: SessionStatus::Error {
                            message: message.clone(),
                        },
                    });
                }
            }
            done.send_replace(Some(admission));
        });
        drop(admissions);
        if endpoint == "command" {
            // Commands can acknowledge only after their generation finishes.
            // Keep the composer responsive while retaining the teardown fence.
            return Ok(turn_id);
        }
        // This bounds the UI wait, not the server-side masked operation. The
        // owned task continues and shutdown cannot claim Closed before its ACK.
        let response = tokio::time::timeout(std::time::Duration::from_secs(15), async {
            loop {
                if let Some(response) = result.borrow().clone() {
                    return response;
                }
                result
                    .changed()
                    .await
                    .map_err(|_| "native_admission_owner_lost".to_string())?;
            }
        })
        .await
        .unwrap_or_else(|_| Err("native_admission_ack_timeout; cleanup remains pending".into()));
        if let Err(message) = response {
            self.dead.store(true, Ordering::Relaxed);
            self.closing.send_replace(true);
            let _ = self
                .event_tx
                .send(ProviderRuntimeEvent::SessionStateChanged {
                    thread_id: self.thread_id.clone(),
                    status: SessionStatus::Error {
                        message: message.clone(),
                    },
                });
            return Err(ProviderError::RpcError { message });
        }
        Ok(turn_id)
    }

    pub async fn interrupt_selected(&self, turn_id: Option<TurnId>) -> Result<(), ProviderError> {
        // All native prompt assignment and queue drains own outbound. Keep it
        // across the session-scoped abort so a newer turn cannot enter between
        // checking the selected identity and writing the HTTP request.
        let _outbound = self.outbound.lock().await;
        if let Some(turn_id) = turn_id {
            let ctx = self.event_ctx.lock().await;
            if !ctx.turn_active || ctx.turn_id != turn_id {
                return Err(ProviderError::ValidationError {
                    message: "stale_turn: selected OpenCode turn is no longer active".into(),
                });
            }
        }
        self.until_closed(self.interrupt()).await
    }

    /// Abort the active turn. OpenCode's `/abort` endpoint is
    /// idempotent — calling it on an idle session returns 200 with
    /// `false`, which we treat as success.
    pub async fn interrupt(&self) -> Result<(), ProviderError> {
        if self.server_handle.api == super::api::OpenCodeApi::Legacy {
            let ctx = self.event_ctx.lock().await;
            self.router
                .lock()
                .await
                .mark_interrupted(if ctx.turn_active {
                    Some(ctx.turn_id.clone())
                } else {
                    None
                });
        }
        let url = format!(
            "{}{}/session/{}/{}",
            self.server_handle.base_url.trim_end_matches('/'),
            self.server_handle.api.route("", "/api"),
            urlencoding::encode(&self.provider_session_id.0),
            self.server_handle
                .api
                .route("abort", "interrupt?resume=false")
        );
        let response = self
            .http
            .post(&url)
            .timeout(std::time::Duration::from_secs(15))
            .basic_auth("opencode", Some(&self.server_handle.server_password))
            .send()
            .await
            .map_err(|err| ProviderError::RpcError {
                message: format!("abort_send_failed: {err}"),
            })?;
        let status = response.status();
        if !status.is_success() {
            return Err(ProviderError::RpcError {
                message: format!("abort_http_status_{}", status.as_u16()),
            });
        }
        Ok(())
    }

    /// Respond to a permission request. The mapping from Codemux's
    /// rich [`ApprovalDecision`] to OpenCode's three reply tokens
    /// lives in [`approval_decision_to_permission_reply`].
    pub async fn respond_to_request(
        &self,
        request_id: RequestId,
        decision: ApprovalDecision,
    ) -> Result<(), ProviderError> {
        if self.server_handle.api == super::api::OpenCodeApi::V2 {
            let router = self.router.lock().await;
            let target = router
                .session_for_permission(&request_id.0)
                .ok_or_else(|| ProviderError::RequestNotPending {
                    request_id: request_id.clone(),
                })?;
            let form = router.form(&request_id.0);
            drop(router);
            let base = format!(
                "{}/api/session/{}",
                self.server_handle.base_url.trim_end_matches('/'),
                urlencoding::encode(&target)
            );
            let request = if let Some(form) = form {
                match &decision {
                    ApprovalDecision::Allow {
                        updated_input: Some(input),
                        ..
                    } => {
                        let answer = super::v2::question_answer(&form, input)
                            .map_err(|message| ProviderError::ValidationError { message })?;
                        self.http
                            .post(format!(
                                "{base}/form/{}/reply",
                                urlencoding::encode(&request_id.0)
                            ))
                            .json(&serde_json::json!({"answer":answer}))
                    }
                    ApprovalDecision::Cancel => self.http.delete(format!(
                        "{base}/form/{}",
                        urlencoding::encode(&request_id.0)
                    )),
                    ApprovalDecision::Deny { message } => self
                        .http
                        .delete(format!(
                            "{base}/form/{}",
                            urlencoding::encode(&request_id.0)
                        ))
                        .query(&[("message", message)]),
                    _ => {
                        return Err(ProviderError::ValidationError {
                            message: "OpenCode questions require answers or explicit cancellation"
                                .into(),
                        })
                    }
                }
            } else {
                if matches!(&decision, ApprovalDecision::ProviderOption { .. }) {
                    return Err(ProviderError::ValidationError {
                        message: "OpenCode permissions do not advertise provider-option replies"
                            .into(),
                    });
                }
                let mut body = serde_json::json!({"decision":approval_decision_to_permission_reply(&decision)});
                if let ApprovalDecision::Deny { message } = &decision {
                    body["message"] = serde_json::json!(message);
                }
                self.http
                    .post(format!(
                        "{base}/permission/{}/reply",
                        urlencoding::encode(&request_id.0)
                    ))
                    .json(&body)
            };
            let response = request
                .basic_auth("opencode", Some(&self.server_handle.server_password))
                .timeout(std::time::Duration::from_secs(15))
                .send()
                .await
                .map_err(|e| ProviderError::RpcError {
                    message: format!("request_reply_failed: {e}"),
                })?;
            if !response.status().is_success() {
                if matches!(
                    response.status(),
                    reqwest::StatusCode::NOT_FOUND
                        | reqwest::StatusCode::CONFLICT
                        | reqwest::StatusCode::GONE
                ) {
                    return Err(ProviderError::RequestNotPending { request_id });
                }
                return Err(ProviderError::RpcError {
                    message: format!("request_reply_http_status_{}", response.status().as_u16()),
                });
            }
            self.router.lock().await.forget_request(&request_id.0);
            return Ok(());
        }
        let body = PermissionRespondRequest {
            response: approval_decision_to_permission_reply(&decision),
        };
        // A subagent's permission request must be answered on the child
        // session that raised it — the SSE listener recorded which
        // session each permission id belongs to. Fall back to the root
        // session for the ordinary (non-subagent) case.
        let target_session = self
            .router
            .lock()
            .await
            .session_for_permission(&request_id.0)
            .unwrap_or_else(|| self.provider_session_id.0.clone());
        let url = format!(
            "{}/session/{}/permissions/{}",
            self.server_handle.base_url.trim_end_matches('/'),
            urlencoding::encode(&target_session),
            urlencoding::encode(&request_id.0),
        );
        let response = self
            .http
            .post(&url)
            .basic_auth("opencode", Some(&self.server_handle.server_password))
            .json(&body)
            .send()
            .await
            .map_err(|err| ProviderError::RpcError {
                message: format!("permission_send_failed: {err}"),
            })?;
        let status = response.status();
        if !status.is_success() {
            if matches!(
                status,
                reqwest::StatusCode::NOT_FOUND
                    | reqwest::StatusCode::CONFLICT
                    | reqwest::StatusCode::GONE
            ) {
                return Err(ProviderError::RequestNotPending { request_id });
            }
            return Err(ProviderError::RpcError {
                message: format!("permission_http_status_{}", status.as_u16()),
            });
        }
        Ok(())
    }

    /// Update the cached model for subsequent turns. OpenCode does
    /// not have a "session-level model swap" RPC — the change is
    /// applied at the next `prompt_async` body.
    pub async fn set_model(&self, model: String) -> Result<(), ProviderError> {
        let _outbound = self.outbound.lock().await;
        self.until_closed(self.set_model_open(model)).await
    }

    async fn set_model_open(&self, model: String) -> Result<(), ProviderError> {
        if self.is_dead() {
            return Err(ProviderError::SessionClosed {
                thread_id: self.thread_id.clone(),
            });
        }
        if self.server_handle.api == super::api::OpenCodeApi::V2 {
            self.wait_ready().await?;
            self.apply_native_model(&model, None).await?;
            *self.current_variant.lock().await = None;
        }
        {
            let mut current = self.current_model.lock().await;
            *current = Some(model.clone());
        }
        self.refresh_model_context(model).await;
        Ok(())
    }

    async fn refresh_model_context(&self, model: String) {
        // The window is a property of the model, so a swap invalidates
        // the cached number. Clear it immediately (a stale denominator
        // is worse than none) and re-probe in the background. Both
        // copies have to go: the one the SSE listener reads off the
        // routing context, and the sticky one already latched inside the
        // usage tracker — the latter would otherwise keep stamping the
        // old window on every snapshot, since `observe_max_tokens(None)`
        // is a deliberate no-op. Re-publishing the current occupancy
        // without a denominator makes the meter degrade to a bare token
        // count now rather than at the next assistant message.
        {
            let mut ctx = self.event_ctx.lock().await;
            ctx.context_window_tokens = None;
            // The price belongs to the old model too. Dropping it falls
            // the ledger back to the static table until the re-probe
            // lands, which is right: an unpriced row is recoverable,
            // a row billed at another model's rate is not.
            ctx.model_cost = None;
        }
        for event in self.usage.lock().await.model_changed(&self.thread_id) {
            let _ = self.event_tx.send(event);
        }
        spawn_context_window_probe(
            self.server_handle.base_url.clone(),
            self.server_handle.server_password.clone(),
            Some(model),
            self.server_handle.api,
            self.event_ctx.clone(),
        );
    }

    /// Close this attachment only after all native admission/wake requests
    /// are acknowledged, its pending input is cancelled and execution is idle.
    pub async fn shutdown(&self) -> Result<(), ProviderError> {
        self.dead.store(true, Ordering::Relaxed);
        self.closing.send_replace(true);
        let _outbound = self.outbound.lock().await;
        if let Some(worker) = self.queue_worker.lock().await.take() {
            worker.abort();
        }
        self.cancel_all_queued().await;
        if let Some(handle) = self.sse_handle.lock().await.take() {
            handle.abort();
        }
        let result = if self.server_handle.api == super::api::OpenCodeApi::V2 {
            let peer = self.native_peer();
            // Stop an existing execution immediately, but an idle interrupt is
            // not evidence that an outstanding admission cannot arrive later.
            let initial_interrupt =
                tokio::time::timeout(NATIVE_SHUTDOWN_GRACE, peer.interrupt()).await;
            match initial_interrupt {
                Ok(Ok(())) => {
                    let admissions = self.native_admissions.lock().await.clone();
                    tokio::time::timeout(NATIVE_SHUTDOWN_GRACE, async {
                        await_native_admissions(admissions).await?;
                        peer.finish().await
                    })
                    .await
                    .unwrap_or_else(|_| {
                        Err("native_shutdown_admission_unconfirmed; cleanup remains owned".into())
                    })
                }
                Ok(Err(error)) => Err(error),
                Err(_) => Err("native_shutdown_interrupt_unconfirmed".into()),
            }
        } else {
            let requests: Vec<_> = self.native_requests.lock().await.drain(..).collect();
            for request in &requests {
                request.abort();
            }
            for request in requests {
                let _ = request.await;
            }
            tokio::time::timeout(NATIVE_SHUTDOWN_GRACE, self.interrupt())
                .await
                .map_err(|_| "legacy_shutdown_interrupt_timeout".into())
                .and_then(|result| result.map_err(|error| error.to_string()))
        };
        if let Err(message) = result {
            let _ = self
                .event_tx
                .send(ProviderRuntimeEvent::SessionStateChanged {
                    thread_id: self.thread_id.clone(),
                    status: SessionStatus::Error {
                        message: message.clone(),
                    },
                });
            return Err(ProviderError::RpcError { message });
        }
        {
            let mut ctx = self.event_ctx.lock().await;
            if ctx.turn_active {
                ctx.turn_active = false;
                let _ = self.event_tx.send(ProviderRuntimeEvent::TurnCompleted {
                    thread_id: self.thread_id.clone(),
                    turn_id: ctx.turn_id.clone(),
                    status: crate::agent_provider::TurnStatus::Error {
                        subtype: "interrupted".into(),
                        message: "OpenCode attachment closed".into(),
                    },
                    usage: None,
                });
            }
        }
        let _ = self
            .event_tx
            .send(ProviderRuntimeEvent::SessionStateChanged {
                thread_id: self.thread_id.clone(),
                status: SessionStatus::Closed,
            });
        Ok(())
    }

    async fn until_closed<T>(
        &self,
        operation: impl std::future::Future<Output = Result<T, ProviderError>>,
    ) -> Result<T, ProviderError> {
        // Subscribe before checking closure so a close between the check and
        // the first poll is retained, even if no response headers ever arrive.
        let mut closing = self.closing.subscribe();
        self.ensure_open()?;
        tokio::select! {
            biased;
            _ = closing.changed() => Err(ProviderError::SessionClosed {
                thread_id: self.thread_id.clone(),
            }),
            result = operation => result,
        }
    }

    fn ensure_open(&self) -> Result<(), ProviderError> {
        if self.is_dead() {
            Err(ProviderError::SessionClosed {
                thread_id: self.thread_id.clone(),
            })
        } else {
            Ok(())
        }
    }
}

impl Drop for OpenCodeSession {
    fn drop(&mut self) {
        self.dead.store(true, Ordering::Relaxed);
        self.closing.send_replace(true);
        if self.server_handle.api == super::api::OpenCodeApi::V2 {
            let peer = self.native_peer();
            let admissions = self.native_admissions.get_mut().clone();
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                runtime.spawn(async move {
                    if await_native_admissions(admissions).await.is_ok() {
                        let _ = peer.finish().await;
                    }
                });
            }
        }
        if let Some(handle) = self.sse_handle.get_mut().take() {
            handle.abort();
        }
        if let Some(handle) = self.queue_worker.get_mut().take() {
            handle.abort();
        }
        for request in self.native_requests.get_mut().drain(..) {
            request.abort();
        }
    }
}

/// Register Codemux's process-wide MCP facade with the live OpenCode server.
/// The remote config is runtime-only: no provider config files are rewritten,
/// and the bearer credential never leaves this process.
#[cfg(test)]
async fn attach_mcp_gateway(
    http: &reqwest::Client,
    server: &OpenCodeServerHandle,
    registry: &McpRegistry,
) -> Result<(), ProviderError> {
    attach_mcp_gateway_scoped(http, server, registry, None).await
}

async fn attach_mcp_gateway_scoped(
    http: &reqwest::Client,
    server: &OpenCodeServerHandle,
    registry: &McpRegistry,
    directory: Option<&std::path::Path>,
) -> Result<(), ProviderError> {
    let connection =
        registry
            .gateway_connection()
            .await
            .map_err(|message| ProviderError::ProcessError {
                message: "failed to start Codemux MCP gateway".into(),
                source: Some(message),
            })?;
    if server.api == super::api::OpenCodeApi::V2 {
        let mut request = http.put(format!(
            "{}/api/experimental/mcp/codemux",
            server.base_url.trim_end_matches('/')
        ));
        if let Some(directory) = directory {
            request = request.query(&[("location[directory]", directory.to_string_lossy())]);
        }
        let response=request.basic_auth("opencode",Some(&server.server_password)).json(&serde_json::json!({"config":{"type":"remote","url":connection.url,"disabled":false,"oauth":false,"headers":{"Authorization":format!("Bearer {}",connection.bearer_token)}}})).send().await.map_err(|e|ProviderError::RpcError{message:format!("opencode_mcp_attach_failed: {e}")})?;
        if !response.status().is_success() {
            return Err(ProviderError::RpcError {
                message: format!(
                    "opencode_mcp_attach_http_status_{}",
                    response.status().as_u16()
                ),
            });
        }
        let mut request = http.get(format!("{}/api/mcp", server.base_url.trim_end_matches('/')));
        if let Some(directory) = directory {
            request = request.query(&[("location[directory]", directory.to_string_lossy())]);
        }
        let status: serde_json::Value = request
            .basic_auth("opencode", Some(&server.server_password))
            .send()
            .await
            .map_err(|e| ProviderError::RpcError {
                message: e.to_string(),
            })?
            .error_for_status()
            .map_err(|e| ProviderError::RpcError {
                message: e.to_string(),
            })?
            .json()
            .await
            .map_err(|e| ProviderError::RpcError {
                message: e.to_string(),
            })?;
        if !status["data"].as_array().is_some_and(|entries| {
            entries
                .iter()
                .any(|e| e["name"] == "codemux" && e["status"]["status"] == "connected")
        }) {
            return Err(ProviderError::RpcError {
                message: "Codemux MCP gateway was registered but is not connected".into(),
            });
        }
        return Ok(());
    }
    let url = format!("{}/mcp", server.base_url.trim_end_matches('/'));
    let response = http
        .post(url)
        .basic_auth("opencode", Some(&server.server_password))
        .json(&serde_json::json!({
            "name": "codemux",
            "config": {
                "type": "remote",
                "url": connection.url,
                "enabled": true,
                "oauth": false,
                "headers": {
                    "Authorization": format!("Bearer {}", connection.bearer_token)
                }
            }
        }))
        .send()
        .await
        .map_err(|error| ProviderError::RpcError {
            message: format!("opencode_mcp_attach_send_failed: {error}"),
        })?;
    if !response.status().is_success() {
        let status = response.status();
        let detail = response.text().await.unwrap_or_default();
        return Err(ProviderError::RpcError {
            message: format!(
                "opencode_mcp_attach_http_status_{}: {detail}",
                status.as_u16()
            ),
        });
    }
    Ok(())
}

/// Pull the OpenCode server-side session id out of the resume cursor
/// `ensure_live_session` threads in (`{"resume": <session_id>}`). Returns
/// `None` for an absent/mis-shaped cursor so the caller starts fresh.
pub fn resume_session_id_from_cursor(cursor: &serde_json::Value) -> Option<String> {
    cursor
        .get("resume")
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

/// Whether the OpenCode server can still address `session_id`. Probes
/// `GET /session/{id}/message?limit=1`: a success status means the session
/// is known (in memory or loaded from disk) and safe to readopt on a live
/// rebuild; any error / non-success means it is stale, so the caller creates
/// a fresh session instead. Best-effort — a transport error is treated as
/// "not addressable" so we never route a resumed turn at a session the server
/// has forgotten.
async fn session_is_addressable(
    http: &reqwest::Client,
    base_url: &str,
    server_password: &str,
    session_id: &str,
) -> bool {
    let url = format!(
        "{}/session/{}/message?limit=1",
        base_url,
        urlencoding::encode(session_id)
    );
    match http
        .get(&url)
        .basic_auth("opencode", Some(server_password))
        .send()
        .await
    {
        Ok(resp) => resp.status().is_success(),
        Err(_) => false,
    }
}

/// Resolve `model_slug` (`providerID/modelID`) to its numeric context
/// window in a harvested provider catalogue.
///
/// Returns `None` for an unknown provider/model or an upstream that
/// reports no `limit.context` — the meter then degrades to a bare
/// token count rather than rendering a guessed percentage.
pub(crate) fn lookup_context_window(
    providers: &[OpenCodeProviderEntry],
    model_slug: &str,
) -> Option<u64> {
    let (provider_id, model_id) = model_slug.split_once('/')?;
    providers
        .iter()
        .find(|p| p.id == provider_id)?
        .models
        .get(model_id)
        .and_then(|m| m.context_window)
}

/// Look the active model's context window up in the background and
/// drop it into the shared [`EventContext`].
///
/// Deliberately fire-and-forget: the number only sharpens the context
/// meter (it supplies the denominator), so a slow or failed catalogue
/// fetch must never delay session start or fail it. A `None` model or
/// any error simply leaves the window unknown.
fn spawn_context_window_probe(
    base_url: String,
    server_password: String,
    model_slug: Option<String>,
    api: super::api::OpenCodeApi,
    event_ctx: Arc<Mutex<EventContext>>,
) {
    let Some(model_slug) = model_slug else {
        return;
    };
    tokio::spawn(async move {
        let mut config = OpenCodeClientConfig::new(base_url);
        config.server_password = Some(server_password);
        config.api = api;
        let Ok(client) = OpenCodeClient::new(config) else {
            return;
        };
        let Ok(providers) = client.list_models().await else {
            return;
        };
        let window = lookup_context_window(&providers, &model_slug);
        let cost = lookup_model_cost(&providers, &model_slug);
        if window.is_none() && cost.is_none() {
            return;
        }
        let mut ctx = event_ctx.lock().await;
        if let Some(window) = window {
            ctx.context_window_tokens = Some(window);
        }
        if let Some(cost) = cost {
            ctx.model_cost = Some(cost);
        }
    });
}

/// Find the active model's upstream list price in the catalogue.
///
/// Mirrors [`lookup_context_window`] — same `provider/model` slug, same
/// "unknown means leave it alone" contract. `None` when the catalogue
/// reports no `cost` block, which leaves the ledger on the static
/// fallback table rather than pricing the model at zero.
pub(crate) fn lookup_model_cost(
    providers: &[OpenCodeProviderEntry],
    model_slug: &str,
) -> Option<OpenCodeModelCost> {
    let (provider_id, model_id) = model_slug.split_once('/')?;
    providers
        .iter()
        .find(|p| p.id == provider_id)?
        .models
        .get(model_id)
        .and_then(|m| m.cost)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::agent_provider::types::{ImageInput, ThreadId};
    use mockito::Server;

    fn delivery_input(text: &str) -> crate::agent_provider::SendTurnInput {
        crate::agent_provider::SendTurnInput {
            thread_id: ThreadId("thread-1".into()),
            text: text.into(),
            display_text: None,
            images: vec![],
            skill_invocations: vec![],
            model_override: None,
            effort_override: None,
            permission_mode_override: None,
            client_nonce: Some(text.into()),
            turn_checkpoint: None,
        }
    }

    #[tokio::test]
    async fn delivery_queue_waits_and_steer_keeps_running_turn() {
        let mut server = Server::new_async().await;
        let post = server
            .mock("POST", "/session/sess_1/prompt_async")
            .with_status(204)
            .expect(2)
            .create_async()
            .await;
        let abort = server
            .mock("POST", "/session/sess_1/abort")
            .expect(0)
            .create_async()
            .await;
        let (session, _, _) = mock_session(server.url(), "pw".into(), "sess_1").await;
        let original = session
            .enqueue_or_send(delivery_input("Build"))
            .await
            .unwrap();
        let queued = session
            .enqueue_or_send(delivery_input("Later"))
            .await
            .unwrap();
        assert!(queued.queued_id.is_some());
        let guidance = session
            .steer_turn(delivery_input("Correction"))
            .await
            .unwrap();
        assert!(guidance.steered);
        assert_eq!(guidance.turn_id, original.turn_id);
        assert!(session.turn_active().await);
        assert_eq!(session.queued.lock().await.len(), 1);
        assert!(
            session
                .cancel_queued(queued.queued_id.as_ref().unwrap())
                .await
        );
        assert!(
            !session
                .cancel_queued(queued.queued_id.as_ref().unwrap())
                .await
        );
        post.assert_async().await;
        abort.assert_async().await;
    }

    #[tokio::test]
    async fn delivery_failed_queued_steer_preserves_message_and_active_turn() {
        let mut server = Server::new_async().await;
        let post = server
            .mock("POST", "/session/sess_1/prompt_async")
            .with_status(500)
            .create_async()
            .await;
        let (session, _, _) = mock_session(server.url(), "pw".into(), "sess_1").await;
        session.event_ctx.lock().await.turn_active = true;
        let queued = session
            .enqueue_or_send(delivery_input("Keep me"))
            .await
            .unwrap();
        assert!(session
            .send_queued_now(queued.queued_id.as_ref().unwrap(), true)
            .await
            .is_err());
        assert_eq!(session.queued.lock().await.len(), 1);
        assert!(session.turn_active().await);
        post.assert_async().await;
    }

    #[tokio::test]
    async fn delivery_completion_drains_fifo() {
        let mut server = Server::new_async().await;
        let post = server
            .mock("POST", "/session/sess_1/prompt_async")
            .match_body(mockito::Matcher::PartialJson(
                serde_json::json!({"parts":[{"type":"text","text":"First"}]}),
            ))
            .with_status(204)
            .expect(1)
            .create_async()
            .await;
        let (session, tx, mut rx) = mock_session(server.url(), "pw".into(), "sess_1").await;
        session.event_ctx.lock().await.turn_active = true;
        let first = session
            .enqueue_or_send(delivery_input("First"))
            .await
            .unwrap();
        session
            .enqueue_or_send(delivery_input("Second"))
            .await
            .unwrap();
        session.event_ctx.lock().await.turn_active = false;
        let _ = tx.send(ProviderRuntimeEvent::SessionStateChanged {
            thread_id: session.thread_id.clone(),
            status: SessionStatus::Ready,
        });
        tokio::time::timeout(std::time::Duration::from_secs(3), async {
            loop {
                if let ProviderRuntimeEvent::QueuedTurnDispatched {
                    queued_id, steered, ..
                } = rx.recv().await.unwrap()
                {
                    assert_eq!(Some(queued_id), first.queued_id);
                    assert!(!steered);
                    break;
                }
            }
        })
        .await
        .unwrap();
        assert_eq!(session.queued.lock().await.len(), 1);
        post.assert_async().await;
    }

    #[tokio::test]
    async fn attaches_shared_gateway_through_opencode_runtime_api() {
        let mut server = Server::new_async().await;
        let attach = server
            .mock("POST", "/mcp")
            .match_body(mockito::Matcher::PartialJson(serde_json::json!({
                "name": "codemux",
                "config": {"type": "remote", "enabled": true, "oauth": false}
            })))
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body("{}")
            .create_async()
            .await;
        let handle = OpenCodeServerHandle {
            base_url: server.url(),
            server_password: "pw".into(),
            api: super::super::api::OpenCodeApi::Legacy,
        };
        attach_mcp_gateway(&reqwest::Client::new(), &handle, &McpRegistry::new())
            .await
            .unwrap();
        attach.assert_async().await;
    }

    /// An OpenCode binary without the runtime `POST /mcp` endpoint (pre
    /// 1.18.18) must still get a working session — just without Codemux's
    /// shared MCP servers — with the loss reported as a RuntimeWarning.
    #[cfg(unix)]
    #[tokio::test]
    #[serial_test::serial(opencode_path)]
    async fn session_starts_when_gateway_attach_is_unsupported() {
        use super::super::manager::write_fake_opencode_binary;

        let mut backend = Server::new_async().await;
        let attach = backend
            .mock("POST", "/mcp")
            .with_status(404)
            .with_body("not found")
            .create_async()
            .await;
        let create = backend
            .mock("POST", "/session")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"id":"ses_no_mcp"}"#)
            .create_async()
            .await;

        let tmp = tempfile::tempdir().expect("tempdir");
        write_fake_opencode_binary(tmp.path());
        let original_path = std::env::var("PATH").unwrap_or_default();
        // SAFETY: serial_test serialises this against every other opencode
        // test that touches PATH / FAKE_OPENCODE_URL.
        unsafe {
            std::env::set_var("PATH", tmp.path());
            std::env::set_var("FAKE_OPENCODE_URL", backend.url());
        }

        let manager = Arc::new(OpenCodeServerManager::new());
        let (tx, mut rx) = broadcast::channel(64);
        let started = OpenCodeSession::start(
            manager.clone(),
            Some(McpRegistry::new()),
            ThreadId("t-mcp-degraded".into()),
            None,
            None,
            None,
            tx,
            None,
            None,
        )
        .await;

        unsafe {
            std::env::set_var("PATH", original_path);
            std::env::remove_var("FAKE_OPENCODE_URL");
        }
        manager.stop().await;

        let session = started.expect("session must start without the shared MCP gateway");
        assert_eq!(session.provider_session_id.0, "ses_no_mcp");
        attach.assert_async().await;
        create.assert_async().await;
        let mut warned = false;
        while let Ok(event) = rx.try_recv() {
            if let ProviderRuntimeEvent::RuntimeWarning { message, .. } = event {
                warned |= message.contains("gateway attach failed");
            }
        }
        assert!(warned, "a failed attach must surface as a RuntimeWarning");
        let _ = session.shutdown().await;
    }

    /// Developer smoke test against the installed OpenCode binary. Ignored in
    /// CI because OpenCode is an optional external provider.
    #[tokio::test]
    #[ignore]
    async fn live_opencode_server_connects_codemux_gateway() {
        let manager = Arc::new(OpenCodeServerManager::new());
        let registry = McpRegistry::new();
        let (tx, _rx) = broadcast::channel(64);
        let session = OpenCodeSession::start(
            manager.clone(),
            Some(registry.clone()),
            ThreadId("live-mcp-smoke".into()),
            None,
            None,
            None,
            tx,
            None,
            None,
        )
        .await
        .expect("OpenCode session should start with shared MCP gateway");
        let handle = manager.ensure_running().await.unwrap();
        // A second chat session re-registers the same facade; this must be an
        // idempotent update rather than a duplicate-name failure.
        attach_mcp_gateway(&reqwest::Client::new(), &handle, &registry)
            .await
            .expect("reattaching the gateway should be idempotent");
        let status: serde_json::Value = reqwest::Client::new()
            .get(format!("{}/mcp", handle.base_url.trim_end_matches('/')))
            .basic_auth("opencode", Some(&handle.server_password))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        assert_eq!(status["codemux"]["status"], "connected", "{status}");
        let _ = session.shutdown().await;
        manager.stop().await;
    }

    /// Helper: spawn a manager whose `ensure_running` returns a
    /// pre-built handle pointing at the supplied mock URL. Bypasses
    /// the real `opencode serve` process so unit tests don't depend
    /// on the binary.
    pub(crate) async fn mock_session(
        base_url: String,
        password: String,
        session_id: &str,
    ) -> (
        Arc<OpenCodeSession>,
        broadcast::Sender<ProviderRuntimeEvent>,
        broadcast::Receiver<ProviderRuntimeEvent>,
    ) {
        mock_session_api(
            base_url,
            password,
            session_id,
            super::super::api::OpenCodeApi::Legacy,
        )
        .await
    }

    async fn mock_session_api(
        base_url: String,
        password: String,
        session_id: &str,
        api: super::super::api::OpenCodeApi,
    ) -> (
        Arc<OpenCodeSession>,
        broadcast::Sender<ProviderRuntimeEvent>,
        broadcast::Receiver<ProviderRuntimeEvent>,
    ) {
        let (tx, rx) = broadcast::channel(64);
        let (_, ready) = tokio::sync::watch::channel(true);
        let handle = OpenCodeServerHandle {
            base_url,
            server_password: password,
            api,
        };
        // Build the session directly — bypasses
        // OpenCodeServerManager so we can mock the HTTP layer.
        let http = reqwest::Client::new();
        let provider_session_id = ProviderSessionId(session_id.to_string());
        let initial_turn = TurnId("turn_init".into());
        let event_ctx = Arc::new(Mutex::new(EventContext {
            thread_id: ThreadId("t1".into()),
            turn_id: initial_turn,
            provider_session_id: provider_session_id.clone(),
            turn_active: false,
            context_window_tokens: None,
            model_cost: None,
        }));
        let session = Arc::new(OpenCodeSession {
            thread_id: ThreadId("t1".into()),
            provider_session_id: provider_session_id.clone(),
            current_model: Mutex::new(Some("openai/gpt-5".into())),
            current_variant: Mutex::new(None),
            server_handle: handle,
            http,
            event_ctx,
            router: Arc::new(Mutex::new(SseRouter::new(provider_session_id.0.clone()))),
            usage: Arc::new(Mutex::new(OpenCodeUsageState::default())),
            sse_handle: Mutex::new(None),
            event_tx: tx.clone(),
            dead: Arc::new(AtomicBool::new(false)),
            closing: tokio::sync::watch::channel(false).0,
            ready,
            outbound: Mutex::new(()),
            queued: Mutex::new(VecDeque::new()),
            queue_worker: Mutex::new(None),
            native_requests: Mutex::new(Vec::new()),
            native_admissions: Mutex::new(Vec::new()),
            native_inputs: Arc::new(Mutex::new(HashSet::new())),
        });
        session.start_queue_worker(tx.subscribe()).await;
        (session, tx, rx)
    }

    /// Recovery tests use the real HTTP prompt, SSE listener and queue worker,
    /// but never discover/spawn an installed provider or access an account.
    pub(crate) async fn recovery_session(
        base_url: String,
    ) -> (
        Arc<OpenCodeSession>,
        broadcast::Sender<ProviderRuntimeEvent>,
    ) {
        let (session, tx, _) = mock_session(base_url.clone(), "synthetic".into(), "recovery").await;
        let peer = SsePeer {
            api: super::super::api::OpenCodeApi::Legacy,
            ready: tokio::sync::watch::channel(true).0,
            directory: None,
            session_id: "recovery".into(),
            event_ctx: session.event_ctx.clone(),
            router: session.router.clone(),
            dead: session.dead.clone(),
            usage: session.usage.clone(),
        };
        *session.sse_handle.lock().await = Some(spawn_sse_listener(
            base_url,
            "synthetic".into(),
            peer,
            tx.clone(),
        ));
        (session, tx)
    }

    pub(crate) async fn mock_native_peer(
        base_url: String,
    ) -> (
        Arc<OpenCodeSession>,
        SsePeer,
        broadcast::Sender<ProviderRuntimeEvent>,
        broadcast::Receiver<ProviderRuntimeEvent>,
    ) {
        let (session, tx, rx) = mock_session_api(
            base_url,
            "fixture".into(),
            "root",
            super::super::api::OpenCodeApi::V2,
        )
        .await;
        let peer = SsePeer {
            api: super::super::api::OpenCodeApi::V2,
            ready: tokio::sync::watch::channel(true).0,
            directory: None,
            session_id: "root".into(),
            event_ctx: session.event_ctx.clone(),
            router: session.router.clone(),
            dead: session.dead.clone(),
            usage: session.usage.clone(),
        };
        (session, peer, tx, rx)
    }

    #[tokio::test]
    async fn v2_gateway_registration_is_scoped_and_verified_connected() {
        let mut server = Server::new_async().await;
        let directory = tempfile::tempdir().unwrap();
        let query = mockito::Matcher::UrlEncoded(
            "location[directory]".into(),
            directory.path().to_string_lossy().into_owned(),
        );
        let put = server
            .mock("PUT", "/api/experimental/mcp/codemux")
            .match_query(query.clone())
            .match_body(mockito::Matcher::PartialJson(
                serde_json::json!({"config":{"type":"remote","disabled":false,"oauth":false}}),
            ))
            .with_status(204)
            .create_async()
            .await;
        let get = server
            .mock("GET", "/api/mcp")
            .match_query(query)
            .with_status(200)
            .with_body(r#"{"data":[{"name":"codemux","status":{"status":"connected"}}]}"#)
            .create_async()
            .await;
        let server = OpenCodeServerHandle {
            base_url: server.url(),
            server_password: "fixture".into(),
            api: super::super::api::OpenCodeApi::V2,
        };
        attach_mcp_gateway_scoped(
            &reqwest::Client::new(),
            &server,
            &McpRegistry::new(),
            Some(directory.path()),
        )
        .await
        .unwrap();
        put.assert_async().await;
        get.assert_async().await;
    }

    #[tokio::test]
    async fn v2_command_and_compact_keep_native_bodies_and_wait_for_execution_terminal() {
        let mut server = Server::new_async().await;
        let compact = server
            .mock("POST", "/api/session/s/compact")
            .match_body(mockito::Matcher::Json(
                serde_json::json!({"delivery":"queue"}),
            ))
            .with_status(200)
            .with_body(r#"{"data":{"id":"c","sessionID":"s","type":"compaction"}}"#)
            .create_async()
            .await;
        let command = server
            .mock("POST", "/api/session/s/command")
            .match_body(mockito::Matcher::Json(
                serde_json::json!({"name":"review","text":"diff","delivery":"queue"}),
            ))
            .with_status(204)
            .create_async()
            .await;
        let (session, _, mut events) = mock_session_api(
            server.url(),
            "fixture".into(),
            "s",
            super::super::api::OpenCodeApi::V2,
        )
        .await;
        session
            .send_turn("/compact".into(), vec![], None, None)
            .await
            .unwrap();
        compact.assert_async().await;
        assert!(session.turn_active().await);
        session.event_ctx.lock().await.turn_active = false;
        session
            .send_turn("/review diff".into(), vec![], None, None)
            .await
            .unwrap();
        await_native_http(&session).await;
        command.assert_async().await;
        assert!(session.turn_active().await);
        assert!(!std::iter::from_fn(|| events.try_recv().ok())
            .any(|e| matches!(e, ProviderRuntimeEvent::TurnCompleted { .. })));
    }

    #[tokio::test]
    async fn native_model_ack_precedes_prompt_and_failed_ack_keeps_selection() {
        let mut server = Server::new_async().await;
        let model = server
            .mock("POST", "/api/session/s/model")
            .match_body(mockito::Matcher::Json(
                serde_json::json!({"model":{"providerID":"p","id":"m","variant":"high"}}),
            ))
            .with_status(204)
            .create_async()
            .await;
        let prompt = server
            .mock("POST", "/api/session/s/prompt")
            .match_body(mockito::Matcher::Json(
                serde_json::json!({"text":"hi","delivery":"queue","resume":true}),
            ))
            .with_status(200)
            .with_body(r#"{"data":{"id":"u","sessionID":"s","type":"user"}}"#)
            .create_async()
            .await;
        let (session, _, _) = mock_session_api(
            server.url(),
            "pw".into(),
            "s",
            super::super::api::OpenCodeApi::V2,
        )
        .await;
        session
            .send_turn("hi".into(), vec![], Some("p/m".into()), Some("high".into()))
            .await
            .unwrap();
        model.assert_async().await;
        prompt.assert_async().await;
        assert_eq!(session.current_model.lock().await.as_deref(), Some("p/m"));
        assert_eq!(
            session.current_variant.lock().await.as_deref(),
            Some("high")
        );
        let reject = server
            .mock("POST", "/api/session/s/model")
            .with_status(400)
            .create_async()
            .await;
        assert!(session.set_model("p/bad".into()).await.is_err());
        reject.assert_async().await;
        assert_eq!(session.current_model.lock().await.as_deref(), Some("p/m"));
    }

    #[derive(Debug)]
    struct NativeAdmissionRequest {
        method: String,
        path: String,
        body: serde_json::Value,
    }

    #[derive(Default)]
    struct NativeAdmissionState {
        pending: std::collections::HashMap<String, serde_json::Value>,
        admitted: Vec<String>,
        cancelled: Vec<String>,
        wakes: usize,
        interrupts: usize,
        waits: usize,
        active: bool,
    }

    struct DelayedNativeAdmissionPeer {
        base_url: String,
        requests: tokio::sync::mpsc::UnboundedReceiver<NativeAdmissionRequest>,
        release: tokio::sync::watch::Sender<bool>,
        state: Arc<Mutex<NativeAdmissionState>>,
        server: tokio::task::JoinHandle<()>,
    }

    impl Drop for DelayedNativeAdmissionPeer {
        fn drop(&mut self) {
            self.server.abort();
        }
    }

    impl DelayedNativeAdmissionPeer {
        /// Pause after preparation, before durable admission. The server task
        /// remains alive if the HTTP client disconnects, matching native v2's
        /// uninterruptible admission phase rather than delayed response headers.
        async fn start() -> Self {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let (seen, requests) = tokio::sync::mpsc::unbounded_channel();
            let (release, released) = tokio::sync::watch::channel(false);
            let state = Arc::new(Mutex::new(NativeAdmissionState::default()));
            let server_state = state.clone();
            let held = Arc::new(AtomicBool::new(false));
            let server = tokio::spawn(async move {
                let mut connections = tokio::task::JoinSet::new();
                loop {
                    tokio::select! {
                        accepted = listener.accept() => {
                            let (mut socket, _) = accepted.unwrap();
                            let seen = seen.clone();
                            let state = server_state.clone();
                            let held = held.clone();
                            let mut released = released.clone();
                            connections.spawn(async move {
                                let mut bytes = Vec::new();
                                let mut chunk = [0_u8; 2048];
                                let (headers_end, content_length) = loop {
                                    let read = socket.read(&mut chunk).await.unwrap();
                                    if read == 0 { return; }
                                    bytes.extend_from_slice(&chunk[..read]);
                                    let Some(end) = bytes.windows(4).position(|part| part == b"\r\n\r\n") else { continue };
                                    let headers = String::from_utf8_lossy(&bytes[..end]);
                                    let length = headers.lines().filter_map(|line| line.split_once(':'))
                                        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                                        .map(|(_, value)| value.trim().parse::<usize>().unwrap()).unwrap_or(0);
                                    if bytes.len() >= end + 4 + length { break (end, length); }
                                };
                                let headers = String::from_utf8_lossy(&bytes[..headers_end]);
                                let mut first_line = headers.lines().next().unwrap().split_whitespace();
                                let method = first_line.next().unwrap().to_string();
                                let path = first_line.next().unwrap().to_string();
                                let body = if content_length == 0 {
                                    serde_json::Value::Null
                                } else {
                                    serde_json::from_slice(&bytes[headers_end + 4..headers_end + 4 + content_length]).unwrap()
                                };
                                let _ = seen.send(NativeAdmissionRequest {
                                    method: method.clone(), path: path.clone(), body: body.clone(),
                                });
                                if method == "POST" && path == "/api/session/s/prompt"
                                    && !held.swap(true, Ordering::Relaxed)
                                {
                                    while !*released.borrow_and_update() {
                                        if released.changed().await.is_err() { return; }
                                    }
                                }
                                let (status, response) = {
                                    let mut state = state.lock().await;
                                    match (method.as_str(), path.as_str()) {
                                        ("POST", "/api/session/s/prompt") => {
                                            let id = body["id"].as_str().unwrap_or("msg_server_generated").to_string();
                                            let existing = state.pending.contains_key(&id);
                                            let item = state.pending.entry(id.clone()).or_insert_with(|| serde_json::json!({
                                                "id":id,"sessionID":"s","type":"user",
                                                "delivery":body["delivery"],"payload":{"text":body["text"]}
                                            })).clone();
                                            if !existing {
                                                state.admitted.push(id);
                                            }
                                            if body["resume"] != false {
                                                state.wakes += 1;
                                                state.active = true;
                                            }
                                            ("200 OK", serde_json::json!({"data":item}).to_string())
                                        }
                                        ("POST", "/api/session/s/interrupt?resume=false") => {
                                            state.interrupts += 1;
                                            let interrupted = state.active;
                                            state.active = false;
                                            ("200 OK", serde_json::json!({"interrupted":interrupted}).to_string())
                                        }
                                        ("GET", "/api/session/s/inbox") => (
                                            "200 OK", serde_json::json!({"data":state.pending.values().collect::<Vec<_>>()} ).to_string()
                                        ),
                                        ("POST", "/api/experimental/session/s/wait") => {
                                            state.waits += 1;
                                            assert!(!state.active, "wait must follow interrupting native execution");
                                            ("204 No Content", String::new())
                                        }
                                        ("DELETE", target) if target.starts_with("/api/session/s/inbox/") => {
                                            let id = target.strip_prefix("/api/session/s/inbox/").unwrap();
                                            // An absent row is a successful no-op, not a
                                            // cancellation tombstone for future admission.
                                            if state.pending.remove(id).is_some() {
                                                state.cancelled.push(id.to_string());
                                            }
                                            ("204 No Content", String::new())
                                        }
                                        _ => ("404 Not Found", String::new()),
                                    }
                                };
                                let response = format!(
                                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}", response.len()
                                );
                                let _ = socket.write_all(response.as_bytes()).await;
                            });
                        }
                        _ = connections.join_next(), if !connections.is_empty() => {}
                    }
                }
            });
            Self {
                base_url: format!("http://{address}"),
                requests,
                release,
                state,
                server,
            }
        }

        async fn next_request(&mut self, method: &str, path: &str) -> NativeAdmissionRequest {
            tokio::time::timeout(std::time::Duration::from_secs(3), async {
                loop {
                    let request = self.requests.recv().await.unwrap();
                    if request.method == method && request.path == path {
                        return request;
                    }
                }
            })
            .await
            .expect("native admission peer did not receive the expected request")
        }

        async fn stop(&mut self) {
            self.server.abort();
            let _ = (&mut self.server).await;
        }
    }

    #[tokio::test]
    async fn native_shutdown_waits_for_server_admission_then_cancels_without_waking() {
        let mut peer = DelayedNativeAdmissionPeer::start().await;
        let (session, _, mut events) = mock_session_api(
            peer.base_url.clone(),
            "pw".into(),
            "s",
            super::super::api::OpenCodeApi::V2,
        )
        .await;
        let sending = tokio::spawn({
            let session = session.clone();
            async move {
                session
                    .enqueue_or_send(delivery_input("held admission"))
                    .await
            }
        });
        let stage = peer.next_request("POST", "/api/session/s/prompt").await;
        assert_eq!(stage.body["resume"], false);
        let input_id = stage.body["id"].as_str().unwrap().to_string();
        assert!(input_id.starts_with("msg_codemux"));
        assert!(peer.state.lock().await.pending.is_empty());
        let closing = tokio::spawn({
            let session = session.clone();
            async move { session.shutdown().await }
        });
        peer.next_request("POST", "/api/session/s/interrupt?resume=false")
            .await;
        assert!(peer.state.lock().await.pending.is_empty());
        assert!(
            !closing.is_finished(),
            "closure cannot precede server admission acknowledgment"
        );
        assert!(
            !std::iter::from_fn(|| events.try_recv().ok()).any(|event| matches!(
                event,
                ProviderRuntimeEvent::SessionStateChanged {
                    status: SessionStatus::Closed,
                    ..
                }
            ))
        );
        peer.release.send_replace(true);
        tokio::time::timeout(
            NATIVE_SHUTDOWN_GRACE + std::time::Duration::from_secs(2),
            closing,
        )
        .await
        .unwrap()
        .unwrap()
        .unwrap();
        assert!(matches!(
            sending.await.unwrap(),
            Err(ProviderError::SessionClosed { .. })
        ));
        let state = peer.state.lock().await;
        assert_eq!(state.admitted, vec![input_id.clone()]);
        assert_eq!(state.cancelled, vec![input_id]);
        assert!(state.pending.is_empty());
        assert_eq!(
            state.wakes, 0,
            "late server admission must never schedule native execution"
        );
        assert!(!state.active);
        assert!(state.interrupts > 0);
        assert!(state.waits > 0);
        drop(state);
        let emitted: Vec<_> = std::iter::from_fn(|| events.try_recv().ok()).collect();
        assert!(matches!(
            emitted.last(),
            Some(ProviderRuntimeEvent::SessionStateChanged {
                status: SessionStatus::Closed,
                ..
            })
        ));
        assert!(matches!(
            session
                .enqueue_or_send(delivery_input("after closure"))
                .await,
            Err(ProviderError::SessionClosed { .. })
        ));
        assert_eq!(peer.state.lock().await.wakes, 0);
        peer.stop().await;
    }

    #[tokio::test]
    async fn native_shutdown_reports_unknown_admission_then_retries_after_owned_cleanup() {
        let mut peer = DelayedNativeAdmissionPeer::start().await;
        let (session, _, mut events) = mock_session_api(
            peer.base_url.clone(),
            "pw".into(),
            "s",
            super::super::api::OpenCodeApi::V2,
        )
        .await;
        let sending = tokio::spawn({
            let session = session.clone();
            async move {
                session
                    .enqueue_or_send(delivery_input("unknown admission"))
                    .await
            }
        });
        let stage = peer.next_request("POST", "/api/session/s/prompt").await;
        assert_eq!(stage.body["resume"], false);
        let input_id = stage.body["id"].as_str().unwrap().to_string();
        let started = tokio::time::Instant::now();
        let closing = tokio::spawn({
            let session = session.clone();
            async move { session.shutdown().await }
        });
        peer.next_request("POST", "/api/session/s/interrupt?resume=false")
            .await;
        let error = tokio::time::timeout(
            NATIVE_SHUTDOWN_GRACE + std::time::Duration::from_secs(2),
            closing,
        )
        .await
        .unwrap()
        .unwrap()
        .expect_err("unknown server admission cannot claim successful closure");
        assert!(matches!(error, ProviderError::RpcError { .. }));
        assert!(started.elapsed() >= NATIVE_SHUTDOWN_GRACE);
        assert!(matches!(
            sending.await.unwrap(),
            Err(ProviderError::SessionClosed { .. })
        ));
        let emitted: Vec<_> = std::iter::from_fn(|| events.try_recv().ok()).collect();
        assert!(emitted.iter().any(|event| matches!(
            event,
            ProviderRuntimeEvent::SessionStateChanged {
                status: SessionStatus::Error { .. },
                ..
            }
        )));
        assert!(!emitted.iter().any(|event| matches!(
            event,
            ProviderRuntimeEvent::SessionStateChanged {
                status: SessionStatus::Closed,
                ..
            }
        )));
        assert!(peer.state.lock().await.pending.is_empty());
        peer.release.send_replace(true);
        peer.next_request("DELETE", &format!("/api/session/s/inbox/{input_id}"))
            .await;
        tokio::time::timeout(
            NATIVE_SHUTDOWN_GRACE + std::time::Duration::from_secs(2),
            session.shutdown(),
        )
        .await
        .unwrap()
        .unwrap();
        let state = peer.state.lock().await;
        assert_eq!(state.admitted, vec![input_id.clone()]);
        assert_eq!(state.cancelled, vec![input_id]);
        assert!(state.pending.is_empty());
        assert_eq!(state.wakes, 0);
        assert!(!state.active);
        assert!(state.waits > 0);
        drop(state);
        let emitted: Vec<_> = std::iter::from_fn(|| events.try_recv().ok()).collect();
        assert!(matches!(
            emitted.last(),
            Some(ProviderRuntimeEvent::SessionStateChanged {
                status: SessionStatus::Closed,
                ..
            })
        ));
        peer.stop().await;
    }

    /// Hold the first matching response headers while serving teardown on a
    /// separate connection; this models an unresponsive admission endpoint.
    async fn stalled_first_response(
        path: &'static str,
    ) -> (
        String,
        tokio::sync::mpsc::UnboundedReceiver<String>,
        tokio::sync::watch::Sender<bool>,
        tokio::task::JoinHandle<()>,
    ) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let (seen, received) = tokio::sync::mpsc::unbounded_channel();
        let (release, released) = tokio::sync::watch::channel(false);
        let held = Arc::new(AtomicBool::new(false));
        let server = tokio::spawn(async move {
            let mut connections = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let (mut socket, _) = accepted.unwrap();
                        let seen = seen.clone();
                        let held = held.clone();
                        let mut released = released.clone();
                        connections.spawn(async move {
                            let mut request = Vec::new();
                            let mut chunk = [0_u8; 2048];
                            loop {
                                let read = socket.read(&mut chunk).await.unwrap();
                                if read == 0 { return; }
                                request.extend_from_slice(&chunk[..read]);
                                if let Some(end) = request.windows(4).position(|part| part == b"\r\n\r\n") {
                                    let headers = String::from_utf8_lossy(&request[..end]);
                                    let length = headers.lines().filter_map(|line| line.split_once(':'))
                                        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                                        .map(|(_, value)| value.trim().parse::<usize>().unwrap()).unwrap_or(0);
                                    if request.len() >= end + 4 + length { break; }
                                }
                            }
                            let target = String::from_utf8_lossy(&request).split_whitespace().nth(1).unwrap().to_string();
                            let _ = seen.send(target.clone());
                            if target == path && !held.swap(true, Ordering::Relaxed) {
                                while !*released.borrow_and_update() {
                                    if released.changed().await.is_err() { return; }
                                }
                            }
                            let _ = socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
                        });
                    }
                    _ = connections.join_next(), if !connections.is_empty() => {}
                }
            }
        });
        (format!("http://{address}"), received, release, server)
    }

    #[tokio::test]
    async fn shutdown_cancels_native_and_legacy_prompt_admission_without_response_headers() {
        for (api, path) in [
            (super::super::api::OpenCodeApi::V2, "/api/session/s/prompt"),
            (
                super::super::api::OpenCodeApi::Legacy,
                "/session/s/prompt_async",
            ),
        ] {
            let (base, mut requests, release, server) = stalled_first_response(path).await;
            let (session, _, mut events) = mock_session_api(base, "pw".into(), "s", api).await;
            let sending = tokio::spawn({
                let session = session.clone();
                async move { session.enqueue_or_send(delivery_input("held prompt")).await }
            });
            assert_eq!(requests.recv().await.unwrap(), path);
            tokio::time::timeout(std::time::Duration::from_secs(1), session.shutdown())
                .await
                .unwrap();
            assert!(matches!(
                sending.await.unwrap(),
                Err(ProviderError::SessionClosed { .. })
            ));
            let interrupt = if api == super::super::api::OpenCodeApi::V2 {
                "/api/session/s/interrupt?resume=false"
            } else {
                "/session/s/abort"
            };
            assert_eq!(requests.recv().await.unwrap(), interrupt);
            assert!(!session.turn_active().await);
            let emitted: Vec<_> = std::iter::from_fn(|| events.try_recv().ok()).collect();
            assert!(matches!(
                emitted.last(),
                Some(ProviderRuntimeEvent::SessionStateChanged {
                    status: SessionStatus::Closed,
                    ..
                })
            ));
            release.send_replace(true);
            tokio::task::yield_now().await;
            assert!(matches!(
                session.enqueue_or_send(delivery_input("late prompt")).await,
                Err(ProviderError::SessionClosed { .. })
            ));
            assert!(requests.try_recv().is_err());
            assert!(
                events.try_recv().is_err(),
                "the late ACK cannot publish Running after Closed"
            );
            server.abort();
            let _ = server.await;
        }
    }

    #[tokio::test]
    async fn shutdown_cancels_pending_model_selection_before_its_ack() {
        let (base, mut requests, release, server) =
            stalled_first_response("/api/session/s/model").await;
        let (session, _, _) =
            mock_session_api(base, "pw".into(), "s", super::super::api::OpenCodeApi::V2).await;
        let selecting = tokio::spawn({
            let session = session.clone();
            async move { session.set_model("p/m".into()).await }
        });
        assert_eq!(requests.recv().await.unwrap(), "/api/session/s/model");
        tokio::time::timeout(std::time::Duration::from_secs(1), session.shutdown())
            .await
            .unwrap();
        assert!(matches!(
            selecting.await.unwrap(),
            Err(ProviderError::SessionClosed { .. })
        ));
        assert_eq!(
            requests.recv().await.unwrap(),
            "/api/session/s/interrupt?resume=false"
        );
        release.send_replace(true);
        tokio::task::yield_now().await;
        assert_eq!(
            session.current_model.lock().await.as_deref(),
            Some("openai/gpt-5")
        );
        assert!(requests.try_recv().is_err());
        server.abort();
        let _ = server.await;
    }

    #[tokio::test]
    async fn shutdown_cancels_queued_promotion_waiting_on_interrupt_headers() {
        let (base, mut requests, release, server) =
            stalled_first_response("/api/session/s/interrupt?resume=false").await;
        let (session, _, mut events) =
            mock_session_api(base, "pw".into(), "s", super::super::api::OpenCodeApi::V2).await;
        session.event_ctx.lock().await.turn_active = true;
        let queued = session
            .enqueue_or_send(delivery_input("queued prompt"))
            .await
            .unwrap()
            .queued_id
            .unwrap();
        let promoting = tokio::spawn({
            let session = session.clone();
            let queued = queued.clone();
            async move { session.send_queued_now(&queued, false).await }
        });
        assert_eq!(
            requests.recv().await.unwrap(),
            "/api/session/s/interrupt?resume=false"
        );
        tokio::time::timeout(std::time::Duration::from_secs(1), session.shutdown())
            .await
            .unwrap();
        assert!(matches!(
            promoting.await.unwrap(),
            Err(ProviderError::SessionClosed { .. })
        ));
        assert_eq!(
            requests.recv().await.unwrap(),
            "/api/session/s/interrupt?resume=false"
        );
        release.send_replace(true);
        tokio::task::yield_now().await;
        assert!(session.queued.lock().await.is_empty());
        assert!(
            requests.try_recv().is_err(),
            "promotion cannot dispatch the prompt after closure"
        );
        let emitted: Vec<_> = std::iter::from_fn(|| events.try_recv().ok()).collect();
        assert!(!emitted
            .iter()
            .any(|event| matches!(event, ProviderRuntimeEvent::QueuedTurnDispatched { .. })));
        assert!(emitted.iter().any(|event| matches!(event, ProviderRuntimeEvent::QueuedTurnCancelled { queued_id, .. } if queued_id == &queued)));
        server.abort();
        let _ = server.await;
    }

    #[tokio::test]
    async fn native_shutdown_rejects_prompt_after_pending_model_ack() {
        let mut server = Server::new_async().await;
        let (seen, mut received) = tokio::sync::mpsc::unbounded_channel();
        let (release, gate) = std::sync::mpsc::channel();
        let gate = std::sync::Mutex::new(gate);
        let model = server
            .mock("POST", "/api/session/s/model")
            .match_request(move |_| {
                seen.send(()).unwrap();
                gate.lock()
                    .unwrap()
                    .recv_timeout(std::time::Duration::from_secs(5))
                    .unwrap();
                true
            })
            .with_status(204)
            .create_async()
            .await;
        let prompt = server
            .mock("POST", "/api/session/s/prompt")
            .with_status(200)
            .expect(0)
            .create_async()
            .await;
        let interrupt = server
            .mock("POST", "/api/session/s/interrupt?resume=false")
            .with_status(200)
            .create_async()
            .await;
        let (session, _, mut events) = mock_session_api(
            server.url(),
            "pw".into(),
            "s",
            super::super::api::OpenCodeApi::V2,
        )
        .await;
        let sending = tokio::spawn({
            let session = session.clone();
            async move {
                let mut input = delivery_input("hi");
                input.model_override = Some("p/m".into());
                session.enqueue_or_send(input).await
            }
        });
        received.recv().await.unwrap();
        let closing = tokio::spawn({
            let session = session.clone();
            async move { session.shutdown().await }
        });
        while !session.is_dead() {
            tokio::task::yield_now().await;
        }
        release.send(()).unwrap();
        assert!(matches!(
            sending.await.unwrap(),
            Err(ProviderError::SessionClosed { .. })
        ));
        closing.await.unwrap();
        model.assert_async().await;
        prompt.assert_async().await;
        interrupt.assert_async().await;
        let events: Vec<_> = std::iter::from_fn(|| events.try_recv().ok()).collect();
        assert!(events.iter().any(|event| matches!(
            event,
            ProviderRuntimeEvent::SessionStateChanged {
                status: SessionStatus::Closed,
                ..
            }
        )));
        assert!(!events.iter().any(|event| matches!(
            event,
            ProviderRuntimeEvent::SessionStateChanged {
                status: SessionStatus::Running { .. },
                ..
            }
        )));
    }

    #[tokio::test]
    async fn native_background_resume_keeps_prompt_queued_after_pending_model_ack() {
        let mut server = Server::new_async().await;
        let (seen, mut received) = tokio::sync::mpsc::unbounded_channel();
        let (release, gate) = std::sync::mpsc::channel();
        let gate = std::sync::Mutex::new(gate);
        let model = server
            .mock("POST", "/api/session/s/model")
            .match_request(move |_| {
                seen.send(()).unwrap();
                gate.lock()
                    .unwrap()
                    .recv_timeout(std::time::Duration::from_secs(5))
                    .unwrap();
                true
            })
            .with_status(204)
            .create_async()
            .await;
        let prompt = server
            .mock("POST", "/api/session/s/prompt")
            .match_body(mockito::Matcher::Json(
                serde_json::json!({"text":"waiting input","delivery":"queue","resume":true}),
            ))
            .with_status(200)
            .create_async()
            .await;
        let interrupt = server
            .mock("POST", "/api/session/s/interrupt?resume=false")
            .with_status(200)
            .create_async()
            .await;
        let (session, tx, mut events) = mock_session_api(
            server.url(),
            "pw".into(),
            "s",
            super::super::api::OpenCodeApi::V2,
        )
        .await;
        session.event_ctx.lock().await.turn_active = true;
        let mut input = delivery_input("waiting input");
        input.model_override = Some("p/m".into());
        let queued = session
            .enqueue_or_send(input)
            .await
            .unwrap()
            .queued_id
            .unwrap();
        session.event_ctx.lock().await.turn_active = false;
        let draining = tokio::spawn({
            let session = session.clone();
            async move { session.drain_queue().await }
        });
        received.recv().await.unwrap();
        let peer = SsePeer {
            api: super::super::api::OpenCodeApi::V2,
            ready: tokio::sync::watch::channel(true).0,
            directory: None,
            session_id: "s".into(),
            event_ctx: session.event_ctx.clone(),
            router: session.router.clone(),
            dead: session.dead.clone(),
            usage: session.usage.clone(),
        };
        let event = |id: &str, kind: &str| {
            serde_json::from_value(serde_json::json!({
                "id":id,"type":kind,"data":{"sessionID":"s"}
            }))
            .unwrap()
        };
        super::super::sse::handle_native(
            event("callback-start", "session.execution.started"),
            &peer,
            &tx,
        )
        .await;
        let callback_turn = session.event_ctx.lock().await.turn_id.clone();
        super::super::sse::handle_native(
            serde_json::from_value(serde_json::json!({
                "id":"callback-content","type":"session.text.ended",
                "data":{"sessionID":"s","assistantMessageID":"callback-message","ordinal":0,"text":"child result"}
            })).unwrap(),
            &peer, &tx,
        ).await;
        release.send(()).unwrap();
        draining.await.unwrap();
        assert_eq!(session.event_ctx.lock().await.turn_id, callback_turn);
        assert!(session.turn_active().await);
        assert_eq!(session.queued.lock().await.front().unwrap().0, queued);
        assert_eq!(session.queued.lock().await.len(), 1);
        assert!(
            !prompt.matched_async().await,
            "the waiting prompt cannot join the callback execution"
        );
        let before: Vec<_> = std::iter::from_fn(|| events.try_recv().ok()).collect();
        assert!(!before
            .iter()
            .any(|event| matches!(event, ProviderRuntimeEvent::QueuedTurnDispatched { .. })));
        assert!(before.iter().any(|event| matches!(event,
            ProviderRuntimeEvent::ItemCompleted { turn_id, item: crate::agent_provider::CompletedItem::AssistantText { text }, .. }
                if turn_id == &callback_turn && text == "child result"
        )));
        super::super::sse::handle_native(
            event("callback-end", "session.execution.succeeded"),
            &peer,
            &tx,
        )
        .await;
        let next_turn = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            let mut callback_completed = false;
            loop {
                match events.recv().await.unwrap() {
                    ProviderRuntimeEvent::TurnCompleted { turn_id, .. } => {
                        assert_eq!(turn_id, callback_turn);
                        callback_completed = true;
                    }
                    ProviderRuntimeEvent::QueuedTurnDispatched {
                        queued_id, turn_id, ..
                    } => {
                        assert!(callback_completed);
                        assert_eq!(queued_id, queued);
                        break turn_id;
                    }
                    _ => {}
                }
            }
        })
        .await
        .unwrap();
        assert_ne!(next_turn, callback_turn);
        assert!(session.queued.lock().await.is_empty());
        model.assert_async().await;
        prompt.assert_async().await;
        let _ = session.shutdown().await;
        interrupt.assert_async().await;
    }

    #[tokio::test]
    async fn native_interrupt_and_child_question_use_exact_owner_and_detach_does_not_delete() {
        let mut server = Server::new_async().await;
        let abort = server
            .mock("POST", "/api/session/s/interrupt?resume=false")
            .with_status(200)
            .with_body(r#"{"interrupted":true}"#)
            .expect(2)
            .create_async()
            .await;
        let delete = server
            .mock("DELETE", "/api/session/s")
            .expect(0)
            .create_async()
            .await;
        let reply = server
            .mock("POST", "/api/session/child/form/f/reply")
            .match_body(mockito::Matcher::Json(
                serde_json::json!({"answer":{"q0":"A"}}),
            ))
            .with_status(204)
            .create_async()
            .await;
        let (session, _, _) = mock_session_api(
            server.url(),
            "pw".into(),
            "s",
            super::super::api::OpenCodeApi::V2,
        )
        .await;
        session.router.lock().await.record_form(serde_json::json!({"id":"f","sessionID":"child","metadata":{"kind":"question"},"fields":[{"key":"q0","type":"string","description":"Pick","custom":true,"options":[{"value":"A","label":"A"}]}]}));
        session
            .respond_to_request(
                RequestId("f".into()),
                ApprovalDecision::Allow {
                    updated_input: Some(serde_json::json!({"answers":{"Pick":"A"}})),
                    updated_permissions: None,
                },
            )
            .await
            .unwrap();
        reply.assert_async().await;
        session.interrupt().await.unwrap();
        let _ = session.shutdown().await;
        abort.assert_async().await;
        delete.assert_async().await;
    }

    #[tokio::test]
    async fn send_turn_posts_prompt_async_and_emits_running_state() {
        let mut server = Server::new_async().await;
        let mock = server
            .mock("POST", "/session/sess_1/prompt_async")
            .match_header("authorization", mockito::Matcher::Any)
            .with_status(204)
            .create_async()
            .await;
        let (session, _tx, mut rx) = mock_session(server.url(), "pw".into(), "sess_1").await;

        let turn_id = session
            .send_turn("hello".into(), Vec::<ImageInput>::new(), None, None)
            .await
            .expect("send_turn must succeed");

        mock.assert_async().await;
        assert!(turn_id.0.starts_with("turn_"));
        // First event should be the SessionStateChanged → Running.
        let event = rx.try_recv().expect("running event published");
        match event {
            ProviderRuntimeEvent::SessionStateChanged {
                status: SessionStatus::Running { active_turn },
                ..
            } => {
                assert_eq!(active_turn, turn_id);
            }
            other => panic!("wrong event: {other:?}"),
        }
    }

    #[tokio::test]
    async fn send_turn_threads_effort_override_into_the_prompt_variant() {
        // A per-turn effort override reaches OpenCode as the prompt
        // `variant` (its reasoning-effort selector) — the regression
        // guard for the previously shown-but-ignored picker.
        let mut server = Server::new_async().await;
        let mock = server
            .mock("POST", "/session/sess_1/prompt_async")
            .match_header("authorization", mockito::Matcher::Any)
            .match_body(mockito::Matcher::PartialJsonString(
                r#"{"variant":"high"}"#.into(),
            ))
            .with_status(204)
            .create_async()
            .await;
        let (session, _tx, _rx) = mock_session(server.url(), "pw".into(), "sess_1").await;

        session
            .send_turn("hi".into(), vec![], None, Some("high".into()))
            .await
            .expect("send_turn must succeed");

        // The mock only matches if the body carried `variant: "high"`.
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn send_turn_surfaces_http_error_with_status_code() {
        let mut server = Server::new_async().await;
        let mock = server
            .mock("POST", "/session/sess_1/prompt_async")
            .with_status(400)
            .with_body(r#"{"error":"bad model"}"#)
            .create_async()
            .await;
        let (session, _tx, _rx) = mock_session(server.url(), "pw".into(), "sess_1").await;

        let err = session
            .send_turn("hi".into(), vec![], None, None)
            .await
            .expect_err("400 must surface");
        mock.assert_async().await;
        match err {
            ProviderError::RpcError { message } => {
                assert!(message.starts_with("prompt_async_http_status_400"));
            }
            other => panic!("wrong error: {other:?}"),
        }
    }

    #[tokio::test]
    async fn send_turn_validation_error_for_invalid_model_id() {
        let server = Server::new_async().await;
        let (session, _tx, _rx) = mock_session(server.url(), "pw".into(), "sess_1").await;
        // Override with an id missing the `provider/` prefix.
        let err = session
            .send_turn("hi".into(), vec![], Some("invalid".into()), None)
            .await
            .expect_err("must reject");
        match err {
            ProviderError::ValidationError { message } => {
                assert!(message.starts_with("invalid_model_id"));
            }
            other => panic!("wrong error: {other:?}"),
        }
    }

    #[tokio::test]
    async fn interrupt_posts_abort_endpoint() {
        let mut server = Server::new_async().await;
        let mock = server
            .mock("POST", "/session/sess_1/abort")
            .with_status(200)
            .with_body("true")
            .create_async()
            .await;
        let (session, _tx, _rx) = mock_session(server.url(), "pw".into(), "sess_1").await;
        session.interrupt().await.expect("abort succeeds");
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn respond_to_request_posts_permissions_endpoint() {
        let mut server = Server::new_async().await;
        let mock = server
            .mock("POST", "/session/sess_1/permissions/perm_1")
            .match_body(r#"{"response":"once"}"#)
            .with_status(200)
            .with_body("true")
            .create_async()
            .await;
        let (session, _tx, _rx) = mock_session(server.url(), "pw".into(), "sess_1").await;
        session
            .respond_to_request(
                RequestId("perm_1".into()),
                ApprovalDecision::Allow {
                    updated_input: None,
                    updated_permissions: None,
                },
            )
            .await
            .expect("respond succeeds");
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn respond_to_request_targets_child_session_for_subagent_permission() {
        // A subagent's approval must POST to the CHILD session id the
        // SSE listener recorded for that permission, not the root.
        let mut server = Server::new_async().await;
        let mock = server
            .mock("POST", "/session/ses_child/permissions/per_1")
            .match_body(r#"{"response":"once"}"#)
            .with_status(200)
            .with_body("true")
            .create_async()
            .await;
        let (session, _tx, _rx) = mock_session(server.url(), "pw".into(), "sess_1").await;
        // Simulate the SSE listener having recorded the child permission.
        session
            .router
            .lock()
            .await
            .record_permission("per_1".into(), "ses_child".into());
        session
            .respond_to_request(
                RequestId("per_1".into()),
                ApprovalDecision::Allow {
                    updated_input: None,
                    updated_permissions: None,
                },
            )
            .await
            .expect("respond succeeds");
        // The mock only matches the child-session URL.
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn shutdown_detaches_without_deleting_durable_history_and_emits_closed_state() {
        let mut server = Server::new_async().await;
        let mock = server
            .mock("POST", "/session/sess_1/abort")
            .with_status(200)
            .with_body("true")
            .create_async()
            .await;
        let (session, _tx, mut rx) = mock_session(server.url(), "pw".into(), "sess_1").await;
        let _ = session.shutdown().await;
        mock.assert_async().await;
        let event = rx.try_recv().expect("closed event published");
        match event {
            ProviderRuntimeEvent::SessionStateChanged {
                status: SessionStatus::Closed,
                ..
            } => {}
            other => panic!("wrong event: {other:?}"),
        }
    }

    #[test]
    fn resume_session_id_from_cursor_extracts_resume_key() {
        assert_eq!(
            resume_session_id_from_cursor(&serde_json::json!({ "resume": "ses_abc" })),
            Some("ses_abc".to_string())
        );
        // Missing / empty / non-string → None so the caller starts fresh.
        assert_eq!(resume_session_id_from_cursor(&serde_json::json!({})), None);
        assert_eq!(
            resume_session_id_from_cursor(&serde_json::json!({ "resume": "" })),
            None
        );
        assert_eq!(
            resume_session_id_from_cursor(&serde_json::json!({ "resume": 42 })),
            None
        );
    }

    /// End-to-end dead-run recovery (issue #154): the shared server dies,
    /// the manager's liveness probe respawns it, and `OpenCodeSession::start`
    /// readopts the persisted server-side session id on the NEW server —
    /// recovering the conversation context without an app restart. Drives
    /// the real `ensure_running` spawn path via the fake `opencode` binary
    /// from the manager test fixtures (banners `$FAKE_OPENCODE_URL`), with
    /// mock HTTP servers standing in for the child's API surface.
    #[cfg(unix)]
    #[tokio::test]
    #[serial_test::serial(opencode_path)]
    async fn dead_server_respawn_then_start_readopts_persisted_session() {
        use super::super::manager::{write_fake_opencode_binary, DisposableHttpServer};

        // Backend A is a disposable responder (NOT mockito — its pooled
        // servers keep the port listening after drop, so they can't
        // simulate a dead server); backend B is mockito so the readopt
        // probe can be asserted on.
        let backend_a = DisposableHttpServer::start().await;
        let backend_a_url = backend_a.url.clone();
        let tmp = tempfile::tempdir().expect("tempdir");
        write_fake_opencode_binary(tmp.path());
        let original_path = std::env::var("PATH").unwrap_or_default();
        // SAFETY: serial_test serialises this against every other
        // opencode test that touches PATH / FAKE_OPENCODE_URL.
        unsafe {
            std::env::set_var("PATH", tmp.path());
            std::env::set_var("FAKE_OPENCODE_URL", &backend_a_url);
        }

        let manager = Arc::new(OpenCodeServerManager::new());
        let first = manager.ensure_running().await;

        // Bind the replacement backend BEFORE killing A so the OS can't
        // reuse A's freed ephemeral port for B — that would make the
        // probe of the dead A URL hit live B and skip the respawn this
        // test exists to exercise. The replacement knows the persisted
        // session id: OpenCode keeps sessions on disk, so a respawned
        // server can address them. Then the server "dies" (laptop sleep
        // / crash) and its port starts refusing connections.
        let mut backend_b = Server::new_async().await;
        let readopt = backend_b
            .mock("GET", "/session/ses_keep/message?limit=1")
            .with_status(200)
            .with_body("[]")
            .create_async()
            .await;
        backend_a.kill().await;
        unsafe {
            std::env::set_var("FAKE_OPENCODE_URL", backend_b.url());
        }

        // A session rebuild with the persisted resume id: ensure_running
        // must detect the dead cache, respawn onto backend B, and the
        // readopt probe must succeed there — no `POST /session` fired.
        let (tx, _rx) = broadcast::channel(64);
        let started = OpenCodeSession::start(
            manager.clone(),
            None,
            ThreadId("t-respawn".into()),
            None,
            None,
            Some("ses_keep".into()),
            tx,
            None,
            None,
        )
        .await;

        // Restore the environment BEFORE asserting so a failure can't
        // leak the mutated PATH into later serial tests.
        unsafe {
            std::env::set_var("PATH", original_path);
            std::env::remove_var("FAKE_OPENCODE_URL");
        }
        manager.stop().await;

        first.expect("initial ensure_running must succeed");
        let session = started.expect("rebuild after server death must succeed");
        assert_eq!(
            session.provider_session_id.0, "ses_keep",
            "rebuilt session must readopt the persisted session id"
        );
        readopt.assert_async().await;
        let _ = session.shutdown().await;
    }

    #[tokio::test]
    async fn session_is_addressable_true_on_success_false_on_404() {
        let mut server = Server::new_async().await;
        let ok = server
            .mock("GET", "/session/ses_live/message?limit=1")
            .with_status(200)
            .with_body("[]")
            .create_async()
            .await;
        let gone = server
            .mock("GET", "/session/ses_gone/message?limit=1")
            .with_status(404)
            .create_async()
            .await;
        let http = reqwest::Client::new();
        assert!(
            session_is_addressable(&http, &server.url(), "pw", "ses_live").await,
            "a known session must be addressable"
        );
        assert!(
            !session_is_addressable(&http, &server.url(), "pw", "ses_gone").await,
            "a stale session id must not be addressable"
        );
        ok.assert_async().await;
        gone.assert_async().await;
    }

    #[tokio::test]
    async fn set_model_updates_cached_value_for_next_turn() {
        let mut server = Server::new_async().await;
        let mock = server
            .mock("POST", "/session/sess_1/prompt_async")
            .match_body(mockito::Matcher::PartialJsonString(
                r#"{"model":{"providerID":"anthropic","modelID":"claude-sonnet-4-6"}}"#.into(),
            ))
            .with_status(204)
            .create_async()
            .await;
        let (session, _tx, _rx) = mock_session(server.url(), "pw".into(), "sess_1").await;
        session
            .set_model("anthropic/claude-sonnet-4-6".into())
            .await
            .unwrap();
        session
            .send_turn("hi".into(), vec![], None, None)
            .await
            .expect("send_turn ok");
        mock.assert_async().await;
    }

    #[tokio::test]
    async fn set_model_invalidates_the_previous_model_s_context_window() {
        // End-to-end wiring proof: the session holds the same usage
        // handle the SSE listener writes, so a model swap re-publishes
        // the meter without the old model's denominator instead of
        // waiting for a probe that may never come.
        use super::super::translate::opencode_event_to_runtime_with;
        let mut server = Server::new_async().await;
        // The background re-probe must not resolve a window during the
        // test; an unmocked /config/providers 501s, leaving it unknown.
        let (session, _tx, mut rx) = mock_session(server.url(), "pw".into(), "sess_1").await;
        let windowed = EventContext {
            context_window_tokens: Some(1_000_000),
            ..session.event_ctx.lock().await.clone()
        };
        {
            let mut usage = session.usage.lock().await;
            let seeded = opencode_event_to_runtime_with(
                serde_json::from_value(serde_json::json!({
                    "type": "message.updated",
                    "properties": {"info": {
                        "id": "msg_1",
                        "sessionID": "sess_1",
                        "role": "assistant",
                        "tokens": {"input": 300_000, "output": 0, "reasoning": 0,
                                   "cache": {"read": 0, "write": 0}}
                    }}
                }))
                .unwrap(),
                &windowed,
                None,
                &mut usage,
            );
            // The message also produces a `UsageRecorded` ledger row, so
            // match on the meter event rather than on a one-element slice.
            let meter = seeded
                .iter()
                .find_map(|e| match e {
                    ProviderRuntimeEvent::ContextUsageUpdated { usage, .. } => Some(usage),
                    _ => None,
                })
                .expect("the parent message seeds the meter");
            assert_eq!(meter.max_tokens, Some(1_000_000));
        }
        while rx.try_recv().is_ok() {}

        session
            .set_model("anthropic/claude-sonnet-4-6".into())
            .await
            .unwrap();

        assert!(
            session
                .event_ctx
                .lock()
                .await
                .context_window_tokens
                .is_none(),
            "the routing-context copy is cleared for the SSE listener"
        );
        match rx.try_recv().expect("a corrected snapshot is published") {
            ProviderRuntimeEvent::ContextUsageUpdated { usage, .. } => {
                assert!(
                    usage.max_tokens.is_none(),
                    "the sticky window went with it — the meter degrades to a \
                     bare token count rather than dividing by the old model's window"
                );
                assert_eq!(usage.used_tokens, 300_000);
            }
            other => panic!("expected ContextUsageUpdated, got {other:?}"),
        }
        server.reset();
    }

    // ── Context-window lookup ──

    fn catalogue() -> Vec<OpenCodeProviderEntry> {
        let mut models = std::collections::BTreeMap::new();
        models.insert(
            "gpt-5".to_string(),
            super::super::client::OpenCodeModel {
                id: "gpt-5".into(),
                name: "GPT-5".into(),
                description: None,
                variants: vec![],
                context_window: Some(200_000),
                supports_images: false,
                is_free: false,
                cost: None,
            },
        );
        models.insert(
            "mystery".to_string(),
            super::super::client::OpenCodeModel {
                id: "mystery".into(),
                name: "Mystery".into(),
                description: None,
                variants: vec![],
                context_window: None,
                supports_images: false,
                is_free: false,
                cost: None,
            },
        );
        vec![OpenCodeProviderEntry {
            id: "openai".into(),
            name: "OpenAI".into(),
            connected: true,
            models,
        }]
    }

    #[test]
    fn lookup_context_window_resolves_a_known_model_slug() {
        assert_eq!(
            lookup_context_window(&catalogue(), "openai/gpt-5"),
            Some(200_000)
        );
    }

    #[test]
    fn lookup_context_window_is_none_for_unknown_or_silent_entries() {
        // Unknown provider, unknown model, a model the upstream reports
        // no window for, and a malformed slug all yield `None` — the
        // meter degrades rather than guessing a denominator.
        let cat = catalogue();
        assert!(lookup_context_window(&cat, "anthropic/claude").is_none());
        assert!(lookup_context_window(&cat, "openai/nope").is_none());
        assert!(lookup_context_window(&cat, "openai/mystery").is_none());
        assert!(lookup_context_window(&cat, "no-slash").is_none());
    }
    async fn await_native_http(session: &OpenCodeSession) {
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            while session
                .native_requests
                .lock()
                .await
                .iter()
                .any(|request| !request.is_finished())
            {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("native HTTP must return");
    }

    async fn await_native_completion(
        rx: &mut broadcast::Receiver<ProviderRuntimeEvent>,
    ) -> crate::agent_provider::TurnStatus {
        tokio::time::timeout(std::time::Duration::from_secs(2), async {
            loop {
                if let ProviderRuntimeEvent::TurnCompleted { status, .. } = rx.recv().await.unwrap()
                {
                    return status;
                }
            }
        })
        .await
        .expect("native command must settle")
    }

    #[tokio::test]
    async fn native_command_routes_arguments_and_images_to_command_endpoint() {
        let mut server = Server::new_async().await;
        let command = server.mock("POST", "/session/sess_1/command")
            .match_header("authorization", mockito::Matcher::Any)
            .match_body(mockito::Matcher::PartialJson(serde_json::json!({
                "command": "test", "arguments": "first\nsecond", "model": "openai/gpt-5", "variant": "high",
                "parts": [{ "type": "file", "mime": "image/png", "url": "data:image/png;base64,AQID" }]
            })))
            .with_status(200).with_body("{}").create_async().await;
        let (session, _, mut rx) = mock_session(server.url(), "pw".into(), "sess_1").await;
        session
            .send_turn(
                "/test first\nsecond".into(),
                vec![ImageInput {
                    data: vec![1, 2, 3],
                    media_type: "image/png".into(),
                }],
                None,
                Some("high".into()),
            )
            .await
            .unwrap();
        await_native_http(&session).await;
        assert!(
            session.turn_active().await,
            "HTTP success must wait for final SSE output and idle"
        );
        while let Ok(event) = rx.try_recv() {
            assert!(!matches!(event, ProviderRuntimeEvent::TurnCompleted { .. }));
        }
        command.assert_async().await;
    }

    #[tokio::test]
    async fn native_compact_uses_selected_model_and_rejects_extra_content() {
        let mut server = Server::new_async().await;
        let compact = server
            .mock("POST", "/session/sess_1/summarize")
            .match_body(mockito::Matcher::Json(
                serde_json::json!({"providerID": "openai", "modelID": "gpt-5"}),
            ))
            .with_status(200)
            .with_body("true")
            .create_async()
            .await;
        let (session, _, mut rx) = mock_session(server.url(), "pw".into(), "sess_1").await;
        assert!(session
            .send_turn("/compact more".into(), vec![], None, None)
            .await
            .is_err());
        assert!(!session.turn_active().await);
        session
            .send_turn("/compact".into(), vec![], None, None)
            .await
            .unwrap();
        await_native_http(&session).await;
        assert!(session.turn_active().await);
        while let Ok(event) = rx.try_recv() {
            assert!(!matches!(event, ProviderRuntimeEvent::TurnCompleted { .. }));
        }
        compact.assert_async().await;
    }

    #[tokio::test]
    async fn native_command_failure_does_not_leave_session_running() {
        let mut server = Server::new_async().await;
        let command = server
            .mock("POST", "/session/sess_1/command")
            .with_status(400)
            .with_body("Unknown command")
            .create_async()
            .await;
        let (session, _, mut rx) = mock_session(server.url(), "pw".into(), "sess_1").await;
        session
            .send_turn("/unknown".into(), vec![], None, None)
            .await
            .unwrap();
        assert!(
            matches!(await_native_completion(&mut rx).await, crate::agent_provider::TurnStatus::Error { message, .. } if message.contains("Unknown command"))
        );
        assert!(!session.turn_active().await);
        assert!(matches!(
            rx.try_recv(),
            Ok(ProviderRuntimeEvent::SessionStateChanged {
                status: SessionStatus::Ready,
                ..
            })
        ));
        command.assert_async().await;
    }

    #[tokio::test]
    async fn native_command_omits_unselected_model_and_effort() {
        let mut server = Server::new_async().await;
        let command = server
            .mock("POST", "/session/sess_1/command")
            .match_body(mockito::Matcher::Json(serde_json::json!({
                "command": "init", "arguments": "", "parts": [],
            })))
            .with_status(200)
            .with_body("{}")
            .create_async()
            .await;
        let (session, _, _) = mock_session(server.url(), "pw".into(), "sess_1").await;
        *session.current_model.lock().await = None;
        session
            .send_turn("/init".into(), vec![], None, None)
            .await
            .unwrap();
        await_native_http(&session).await;
        command.assert_async().await;
    }
}
