//! HTTP intake for webhook automations, merged into the Remote access
//! listener (`web_remote::server::router`).
//!
//! `POST /hooks/automations/:webhook_id/:token` needs no paired-browser
//! session: the URL token, plus the optional signature, is the credential.
//! Everything past the HTTP shape is `webhook::accept_delivery`. The route
//! answers as soon as the run is recorded and never waits for the agent.

use axum::{
    body::Bytes,
    extract::{DefaultBodyLimit, Path, State},
    Extension,
    http::{header, HeaderMap, HeaderValue, Method, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use serde_json::json;
use tauri::{AppHandle, Runtime};
use tokio::sync::{watch, Semaphore};

use super::webhook::{DeliveryRejection, WebhookRequest, MAX_WEBHOOK_BODY_BYTES};

/// The shutdown flag of the listener that serves a request, attached by
/// `web_remote::start_server`. `true` once that listener is stopped or
/// replaced; graceful shutdown still lets in-flight requests finish, and
/// those must not start runs.
#[derive(Clone)]
pub struct ListenerLife(pub watch::Receiver<bool>);

impl ListenerLife {
    fn live(&self) -> bool {
        !*self.0.borrow()
    }
}

/// Deliveries validated at once. Acceptance locks the database and may read
/// the secrets key file, so it runs on the blocking pool, and this cap keeps
/// a burst of requests from taking that pool over.
static ACCEPT_PERMITS: Semaphore = Semaphore::const_new(4);

/// Path prefix of webhook URLs; the full path is
/// `{WEBHOOK_PATH_PREFIX}/<webhook_id>/<token>`.
pub const WEBHOOK_PATH_PREFIX: &str = "/hooks/automations";

pub fn router<R: Runtime>() -> Router<AppHandle<R>> {
    Router::new()
        .route(
            &format!("{WEBHOOK_PATH_PREFIX}/:webhook_id/:token"),
            post(receive::<R>),
        )
        // Larger bodies are refused with 413 before they are buffered.
        .layer(DefaultBodyLimit::max(MAX_WEBHOOK_BODY_BYTES))
}

async fn receive<R: Runtime>(
    State(app): State<AppHandle<R>>,
    life: Option<Extension<ListenerLife>>,
    Path((webhook_id, token)): Path<(String, String)>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    // Without a live listener (stopped, rebound, or a router built without
    // one) nothing may run.
    let Some(Extension(life)) = life.filter(|Extension(life)| life.live()) else {
        return listener_gone();
    };
    let request = WebhookRequest::new(
        method.as_str(),
        uri.path(),
        uri.query().unwrap_or_default(),
        headers
            .iter()
            .filter_map(|(name, value)| Some((name.as_str(), value.to_str().ok()?))),
        body.to_vec(),
    );
    let Ok(permit) = ACCEPT_PERMITS.acquire().await else {
        return listener_gone();
    };
    // The permit moves into the blocking task, so a sender that disconnects
    // cannot free it while the work is still running.
    let dispatched = tauri::async_runtime::spawn_blocking(move || {
        let _permit = permit;
        // Re-check after waiting for a permit: the listener may have been
        // stopped meanwhile.
        if !life.live() {
            return None;
        }
        Some(super::executor::dispatch_webhook(&app, &webhook_id, &token, &request))
    })
    .await;
    let dispatched = match dispatched {
        Ok(Some(dispatched)) => dispatched,
        Ok(None) => return listener_gone(),
        Err(_) => {
            return respond(
                StatusCode::INTERNAL_SERVER_ERROR,
                json!({ "error": "Could not record the run" }),
            )
        }
    };
    match dispatched {
        Ok(accepted) => respond(
            StatusCode::ACCEPTED,
            json!({ "run_id": accepted.run_id, "missing": accepted.missing }),
        ),
        Err(rejection) => {
            if let DeliveryRejection::Storage(detail) = &rejection {
                eprintln!("[codemux::automations] webhook run not recorded: {detail}");
            }
            respond(
                rejection_status(&rejection),
                json!({ "error": rejection.to_string() }),
            )
        }
    }
}

fn listener_gone() -> Response {
    respond(StatusCode::SERVICE_UNAVAILABLE, json!({ "error": "Remote access is off" }))
}

/// HTTP status for each rejection. An unknown webhook and a wrong token
/// both read as 404, so a caller can't probe which ids exist.
fn rejection_status(rejection: &DeliveryRejection) -> StatusCode {
    match rejection {
        DeliveryRejection::NotFound => StatusCode::NOT_FOUND,
        DeliveryRejection::Disabled
        | DeliveryRejection::Busy
        | DeliveryRejection::Duplicate => StatusCode::CONFLICT,
        DeliveryRejection::PayloadTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
        DeliveryRejection::BadSignature => StatusCode::UNAUTHORIZED,
        DeliveryRejection::SignatureUnavailable => StatusCode::SERVICE_UNAVAILABLE,
        DeliveryRejection::EmptyPrompt | DeliveryRejection::PromptTooLarge => {
            StatusCode::UNPROCESSABLE_ENTITY
        }
        DeliveryRejection::Storage(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

fn respond(status: StatusCode, body: serde_json::Value) -> Response {
    let mut response = (status, Json(body)).into_response();
    let headers = response.headers_mut();
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    headers.insert("referrer-policy", HeaderValue::from_static("no-referrer"));
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::automation_triggers::AutomationTrigger;
    use crate::database::{AutomationInput, DatabaseStore};
    use axum::body::Body;
    use axum::http::Request;
    use tauri::Manager;
    use tower::ServiceExt;

    struct Harness {
        app: tauri::App<tauri::test::MockRuntime>,
        shutdown: watch::Sender<bool>,
    }

    fn app_with_webhook() -> (Harness, String, String, i64) {
        let app = tauri::test::mock_app();
        let db = crate::database::init_test_database();
        let automation = db
            .insert_automation(&AutomationInput {
                name: "Hook".into(),
                prompt: "Handle {{body.action}}".into(),
                agent: "claude".into(),
                schedule: String::new(),
                timezone: "UTC".into(),
                host_id: None,
                project_path: None,
                project_remote: None,
                retention_limit: 10,
                trigger_type: Some(AutomationTrigger::Webhook),
            })
            .unwrap();
        // No project, so an accepted run fails fast at repo resolution and
        // never launches an agent.
        app.manage(db);
        (
            Harness { app, shutdown: watch::channel(false).0 },
            automation.webhook_id.unwrap(),
            automation.webhook_token.unwrap(),
            automation.id,
        )
    }

    async fn post_to(
        harness: &Harness,
        path: &str,
        body: Vec<u8>,
    ) -> (StatusCode, serde_json::Value) {
        let response = router::<tauri::test::MockRuntime>()
            .layer(Extension(ListenerLife(harness.shutdown.subscribe())))
            .with_state(harness.app.handle().clone())
            .oneshot(
                Request::post(path)
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null))
    }

    #[tokio::test]
    async fn wrong_tokens_and_unknown_ids_are_404() {
        let (app, id, token, _) = app_with_webhook();
        let (status, _) = post_to(&app, &format!("{WEBHOOK_PATH_PREFIX}/{id}/nope"), b"{}".to_vec()).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        let (status, _) =
            post_to(&app, &format!("{WEBHOOK_PATH_PREFIX}/missing/{token}"), b"{}".to_vec()).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn a_valid_delivery_is_accepted_and_recorded() {
        let (app, id, token, automation_id) = app_with_webhook();
        let (status, body) = post_to(
            &app,
            &format!("{WEBHOOK_PATH_PREFIX}/{id}/{token}"),
            br#"{"action":"opened"}"#.to_vec(),
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED);
        assert_eq!(body["missing"], serde_json::json!([]));
        let runs = app.app.state::<DatabaseStore>().list_automation_runs(automation_id, 10);
        assert_eq!(runs.len(), 1);
        assert_eq!(body["run_id"], runs[0].id);
        assert_eq!(runs[0].trigger_type, AutomationTrigger::Webhook);
    }

    #[tokio::test]
    async fn a_paused_automation_answers_conflict_without_a_run() {
        let (app, id, token, automation_id) = app_with_webhook();
        let db = app.app.state::<DatabaseStore>();
        db.set_automation_enabled(automation_id, false).unwrap();
        let (status, body) =
            post_to(&app, &format!("{WEBHOOK_PATH_PREFIX}/{id}/{token}"), b"{}".to_vec()).await;
        assert_eq!(status, StatusCode::CONFLICT);
        assert_eq!(body["error"], "This automation is paused");
        assert!(db.list_automation_runs(automation_id, 10).is_empty());
    }

    #[tokio::test]
    async fn nothing_runs_once_remote_access_is_off() {
        let (app, id, token, automation_id) = app_with_webhook();
        // The listener that serves the request has been stopped or replaced.
        app.shutdown.send_replace(true);
        let (status, _) =
            post_to(&app, &format!("{WEBHOOK_PATH_PREFIX}/{id}/{token}"), b"{}".to_vec()).await;
        assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
        assert!(app.app.state::<DatabaseStore>().list_automation_runs(automation_id, 10).is_empty());
    }

    #[tokio::test]
    async fn oversized_bodies_are_refused_before_dispatch() {
        let (app, id, token, automation_id) = app_with_webhook();
        let (status, _) = post_to(
            &app,
            &format!("{WEBHOOK_PATH_PREFIX}/{id}/{token}"),
            vec![b'x'; MAX_WEBHOOK_BODY_BYTES + 1],
        )
        .await;
        assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
        let db = app.app.state::<DatabaseStore>();
        assert!(db.list_automation_runs(automation_id, 10).is_empty());
    }

    #[test]
    fn every_rejection_has_a_client_or_server_status() {
        for rejection in [
            DeliveryRejection::NotFound,
            DeliveryRejection::Disabled,
            DeliveryRejection::PayloadTooLarge,
            DeliveryRejection::BadSignature,
            DeliveryRejection::SignatureUnavailable,
            DeliveryRejection::EmptyPrompt,
            DeliveryRejection::PromptTooLarge,
            DeliveryRejection::Busy,
            DeliveryRejection::Duplicate,
            DeliveryRejection::Storage("x".into()),
        ] {
            let status = rejection_status(&rejection);
            assert!(status.is_client_error() || status.is_server_error(), "{rejection:?}");
        }
    }
}
