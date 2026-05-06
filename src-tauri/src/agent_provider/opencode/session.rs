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
use super::sse::{spawn_sse_listener, SsePeer};
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
    server_handle: OpenCodeServerHandle,
    http: reqwest::Client,
    /// Routing context shared with the SSE listener — the listener
    /// reads it on each event so updates land before the next
    /// `prompt_async` triggers a new burst of SSE traffic.
    event_ctx: Arc<Mutex<EventContext>>,
    sse_handle: Mutex<Option<JoinHandle<()>>>,
    event_tx: broadcast::Sender<ProviderRuntimeEvent>,
}

impl OpenCodeSession {
    /// Bring up a session. Spawns the HTTP server (if absent), creates
    /// the OpenCode-side session, kicks off the SSE listener, and
    /// emits `SessionConfigured` on the provider's broadcast channel.
    pub async fn start(
        manager: Arc<OpenCodeServerManager>,
        thread_id: ThreadId,
        initial_model: Option<String>,
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

        let create_url = format!("{}/session", server_handle.base_url.trim_end_matches('/'));
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
        let provider_session_id = ProviderSessionId(session_resp.id.clone());

        let initial_turn = TurnId(format!("turn_{}", Uuid::new_v4()));
        let event_ctx = Arc::new(Mutex::new(EventContext {
            thread_id: thread_id.clone(),
            turn_id: initial_turn,
            provider_session_id: provider_session_id.clone(),
        }));

        let peer = SsePeer {
            session_id: session_resp.id.clone(),
            event_ctx: event_ctx.clone(),
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
        let _ = event_tx.send(ProviderRuntimeEvent::SessionStateChanged {
            thread_id: thread_id.clone(),
            status: SessionStatus::Ready,
        });

        Ok(Arc::new(Self {
            thread_id,
            provider_session_id,
            current_model: Mutex::new(initial_model),
            server_handle,
            http,
            event_ctx,
            sse_handle: Mutex::new(Some(sse_handle)),
            event_tx,
        }))
    }

    /// Send a user turn. Mints a new Codemux turn id, threads it into
    /// the SSE routing context, then `POST`s `prompt_async`.
    pub async fn send_turn(
        &self,
        text: String,
        images: Vec<ImageInput>,
        model_override: Option<String>,
    ) -> Result<TurnId, ProviderError> {
        let turn_id = TurnId(format!("turn_{}", Uuid::new_v4()));
        {
            let mut ctx = self.event_ctx.lock().await;
            ctx.turn_id = turn_id.clone();
        }

        let model = match model_override {
            Some(m) => Some(m),
            None => self.current_model.lock().await.clone(),
        };
        let body = build_prompt_async_request(text, model.as_deref(), &images)
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
            .await
            .map_err(|err| ProviderError::RpcError {
                message: format!("prompt_async_send_failed: {err}"),
            })?;
        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            return Err(ProviderError::RpcError {
                message: format!(
                    "prompt_async_http_status_{}: {body}",
                    status.as_u16()
                ),
            });
        }
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
        let url = format!(
            "{}/session/{}/permissions/{}",
            self.server_handle.base_url.trim_end_matches('/'),
            urlencoding::encode(&self.provider_session_id.0),
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
        }));
        let session = Arc::new(OpenCodeSession {
            thread_id: ThreadId("t1".into()),
            provider_session_id,
            current_model: Mutex::new(Some("openai/gpt-5".into())),
            server_handle: handle,
            http,
            event_ctx,
            sse_handle: Mutex::new(None),
            event_tx: tx.clone(),
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
            .send_turn("hello".into(), Vec::<ImageInput>::new(), None)
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
            .send_turn("hi".into(), vec![], None)
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
            .send_turn("hi".into(), vec![], Some("invalid".into()))
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
            .send_turn("hi".into(), vec![], None)
            .await
            .expect("send_turn ok");
        mock.assert_async().await;
    }
}
