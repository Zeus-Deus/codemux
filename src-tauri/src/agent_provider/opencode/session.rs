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
//! 5. [`shutdown`](OpenCodeSession::shutdown) — abort the SSE task,
//!    `DELETE /session/{id}`, emit `Closed` state.

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

use super::manager::{OpenCodeServerHandle, OpenCodeServerManager};
use super::protocol::{
    PermissionRespondRequest, SessionCreateRequest, SessionResponse,
};
use super::sse::{spawn_sse_listener, SsePeer, SseRouter};
use super::translate::{
    approval_decision_to_permission_reply, build_prompt_async_request, EventContext,
};

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
    sse_handle: Mutex<Option<JoinHandle<()>>>,
    event_tx: broadcast::Sender<ProviderRuntimeEvent>,
    /// Set by the SSE listener when it gives up on an unreachable server.
    /// [`is_dead`](OpenCodeSession::is_dead) reports it so the provider's
    /// `has_session` treats the corpse as absent and the next send rebuilds
    /// a fresh session via `ensure_live_session`.
    dead: Arc<AtomicBool>,
}

impl OpenCodeSession {
    /// Bring up a session. Spawns the HTTP server (if absent), creates
    /// the OpenCode-side session, kicks off the SSE listener, and
    /// emits `SessionConfigured` on the provider's broadcast channel.
    pub async fn start(
        manager: Arc<OpenCodeServerManager>,
        thread_id: ThreadId,
        initial_model: Option<String>,
        initial_variant: Option<String>,
        resume_session_id: Option<String>,
        event_tx: broadcast::Sender<ProviderRuntimeEvent>,
    ) -> Result<Arc<Self>, ProviderError> {
        let server_handle = manager.ensure_running().await.map_err(|err| {
            if err == "opencode_not_installed" {
                ProviderError::NotInstalled {
                    provider: ProviderKind::OpenCode,
                    hint: "Install OpenCode (https://opencode.ai/) and ensure `opencode` is on PATH.".into(),
                }
            } else {
                ProviderError::ProcessError {
                    message: "failed to start OpenCode server".into(),
                    source: Some(err),
                }
            }
        })?;

        let http = reqwest::Client::builder()
            .build()
            .map_err(|err| ProviderError::ProcessError {
                message: "failed to build HTTP client".into(),
                source: Some(err.to_string()),
            })?;

        // Best-effort resume: readopt the persisted server-side session id when
        // it still exists on the OpenCode server (it persists sessions on disk,
        // so a rebuilt session — even after a server restart — can keep its
        // conversation context). We probe `GET /session/{id}/message?limit=1`:
        // a success means the session is addressable, so we skip `POST /session`
        // and reuse the id; a 404 / error means it is stale, so we fall through
        // to creating a fresh session (the visible transcript still hydrates
        // from the DB, so the user keeps their history either way).
        let base = server_handle.base_url.trim_end_matches('/').to_string();
        let resumed_id = match &resume_session_id {
            Some(id) if session_is_addressable(&http, &base, &server_handle.server_password, id).await => {
                Some(id.clone())
            }
            _ => None,
        };
        let session_id = match resumed_id {
            Some(id) => id,
            None => {
                let create_url = format!("{base}/session");
                let body = SessionCreateRequest::default();
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
                let session_resp: SessionResponse = response.json().await.map_err(|err| {
                    ProviderError::RpcError {
                        message: format!("session_create_decode_failed: {err}"),
                    }
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
        }));

        // Shared liveness flag: the SSE listener flips it when it exhausts
        // its reconnect budget (server gone), which makes `has_session`
        // report the session dead so the next send auto-resumes through
        // `ensure_live_session` rather than POSTing to a corpse.
        let dead = Arc::new(AtomicBool::new(false));
        let router = Arc::new(Mutex::new(SseRouter::new(session_id.clone())));
        let peer = SsePeer {
            session_id: session_id.clone(),
            event_ctx: event_ctx.clone(),
            router: router.clone(),
            dead: dead.clone(),
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
        let _ = event_tx.send(ProviderRuntimeEvent::SessionStateChanged {
            thread_id: thread_id.clone(),
            status: SessionStatus::Ready,
        });

        Ok(Arc::new(Self {
            thread_id,
            provider_session_id,
            current_model: Mutex::new(initial_model),
            current_variant: Mutex::new(initial_variant),
            server_handle,
            http,
            event_ctx,
            router,
            sse_handle: Mutex::new(Some(sse_handle)),
            event_tx,
            dead,
        }))
    }

    /// Whether the SSE listener has declared this session's server
    /// unreachable (reconnect budget exhausted). A dead session is
    /// treated as absent by the provider so the next send auto-resumes.
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
        let turn_id = TurnId(format!("turn_{}", Uuid::new_v4()));
        // Swap the turn id into the routing context up front so SSE events for
        // this turn tag correctly — but DO NOT arm `turn_active` yet. Arming is
        // deferred until the POST succeeds: a failed `prompt_async` means no
        // turn ever started, so a later SSE give-up must not synthesize a
        // `child_exited` completion for a phantom turn.
        {
            let mut ctx = self.event_ctx.lock().await;
            ctx.turn_id = turn_id.clone();
        }

        let model = match model_override {
            Some(m) => Some(m),
            None => self.current_model.lock().await.clone(),
        };
        // Per-turn effort wins; otherwise fall back to the session's
        // variant. OpenCode's `variant` selects the model's reasoning
        // effort on each `prompt_async`.
        let variant = match effort_override {
            Some(v) => Some(v),
            None => self.current_variant.lock().await.clone(),
        };
        let body =
            build_prompt_async_request(text, model.as_deref(), variant.as_deref(), &images)
                .map_err(|err| ProviderError::ValidationError { message: err })?;
        let url = format!(
            "{}/session/{}/prompt_async",
            self.server_handle.base_url.trim_end_matches('/'),
            urlencoding::encode(&self.provider_session_id.0)
        );
        let response = self
            .http
            .post(&url)
            .basic_auth("opencode", Some(&self.server_handle.server_password))
            .json(&body)
            .send()
            .await;
        let response = match response {
            Ok(r) => r,
            Err(err) => {
                // POST never landed — make sure the give-up path stays disarmed
                // (defensive against a stale prior arm) and surface the error.
                self.event_ctx.lock().await.turn_active = false;
                return Err(ProviderError::RpcError {
                    message: format!("prompt_async_send_failed: {err}"),
                });
            }
        };
        let status = response.status();
        if !status.is_success() {
            self.event_ctx.lock().await.turn_active = false;
            let body = response.text().await.unwrap_or_default();
            return Err(ProviderError::RpcError {
                message: format!(
                    "prompt_async_http_status_{}: {body}",
                    status.as_u16()
                ),
            });
        }
        // POST accepted — NOW arm the give-up path: a turn is genuinely in
        // flight, so if the SSE listener exhausts its reconnect budget before
        // this turn settles it must synthesize a `child_exited` `TurnCompleted`.
        self.event_ctx.lock().await.turn_active = true;
        let _ = self.event_tx.send(ProviderRuntimeEvent::SessionStateChanged {
            thread_id: self.thread_id.clone(),
            status: SessionStatus::Running {
                active_turn: turn_id.clone(),
            },
        });
        Ok(turn_id)
    }

    /// Abort the active turn. OpenCode's `/abort` endpoint is
    /// idempotent — calling it on an idle session returns 200 with
    /// `false`, which we treat as success.
    pub async fn interrupt(&self) -> Result<(), ProviderError> {
        let url = format!(
            "{}/session/{}/abort",
            self.server_handle.base_url.trim_end_matches('/'),
            urlencoding::encode(&self.provider_session_id.0)
        );
        let response = self
            .http
            .post(&url)
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
    pub async fn set_model(&self, model: String) {
        let mut current = self.current_model.lock().await;
        *current = Some(model);
    }

    /// Tear down the session. Aborts the SSE task, deletes the
    /// OpenCode-side session, and emits the closed state.
    pub async fn shutdown(&self) {
        if let Some(handle) = self.sse_handle.lock().await.take() {
            handle.abort();
        }
        let url = format!(
            "{}/session/{}",
            self.server_handle.base_url.trim_end_matches('/'),
            urlencoding::encode(&self.provider_session_id.0)
        );
        let _ = self
            .http
            .delete(&url)
            .basic_auth("opencode", Some(&self.server_handle.server_password))
            .send()
            .await;
        let _ = self.event_tx.send(ProviderRuntimeEvent::SessionStateChanged {
            thread_id: self.thread_id.clone(),
            status: SessionStatus::Closed,
        });
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_provider::types::{ImageInput, ThreadId};
    use mockito::Server;

    /// Helper: spawn a manager whose `ensure_running` returns a
    /// pre-built handle pointing at the supplied mock URL. Bypasses
    /// the real `opencode serve` process so unit tests don't depend
    /// on the binary.
    async fn mock_session(
        base_url: String,
        password: String,
        session_id: &str,
    ) -> (
        Arc<OpenCodeSession>,
        broadcast::Sender<ProviderRuntimeEvent>,
        broadcast::Receiver<ProviderRuntimeEvent>,
    ) {
        let (tx, rx) = broadcast::channel(64);
        let handle = OpenCodeServerHandle {
            base_url,
            server_password: password,
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
            sse_handle: Mutex::new(None),
            event_tx: tx.clone(),
            dead: Arc::new(AtomicBool::new(false)),
        });
        (session, tx, rx)
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
        let (session, _tx, mut rx) =
            mock_session(server.url(), "pw".into(), "sess_1").await;

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
        let (session, _tx, _rx) =
            mock_session(server.url(), "pw".into(), "sess_1").await;

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
        let (session, _tx, _rx) =
            mock_session(server.url(), "pw".into(), "sess_1").await;

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
        let (session, _tx, _rx) =
            mock_session(server.url(), "pw".into(), "sess_1").await;
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
        let (session, _tx, _rx) =
            mock_session(server.url(), "pw".into(), "sess_1").await;
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
        let (session, _tx, _rx) =
            mock_session(server.url(), "pw".into(), "sess_1").await;
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
        let (session, _tx, _rx) =
            mock_session(server.url(), "pw".into(), "sess_1").await;
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
    async fn shutdown_deletes_session_and_emits_closed_state() {
        let mut server = Server::new_async().await;
        let mock = server
            .mock("DELETE", "/session/sess_1")
            .with_status(200)
            .with_body("true")
            .create_async()
            .await;
        let (session, _tx, mut rx) =
            mock_session(server.url(), "pw".into(), "sess_1").await;
        session.shutdown().await;
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
            ThreadId("t-respawn".into()),
            None,
            None,
            Some("ses_keep".into()),
            tx,
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
        session.shutdown().await;
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
        let (session, _tx, _rx) =
            mock_session(server.url(), "pw".into(), "sess_1").await;
        session
            .set_model("anthropic/claude-sonnet-4-6".into())
            .await;
        session
            .send_turn("hi".into(), vec![], None, None)
            .await
            .expect("send_turn ok");
        mock.assert_async().await;
    }
}
