//! App-local bridge for agent-browser's origin-restricted stream endpoint.
//! Each URL carries a random capability tied to one verified daemon lifetime.

use std::{collections::HashMap, future::Future, sync::Arc};

use axum::{
    body::Bytes,
    extract::{
        ws::{WebSocket, WebSocketUpgrade},
        DefaultBodyLimit, Path, State,
    },
    http::{header, HeaderMap, HeaderValue, Method, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Router,
};
use serde::Serialize;
use tauri::{AppHandle, Manager, Runtime};
use tokio::sync::{Mutex, RwLock};

use crate::agent_browser::{AgentBrowserManager, BrowserStreamIdentity};

#[derive(Clone)]
struct BridgeTarget {
    browser_id: String,
    identity: BrowserStreamIdentity,
}

struct RouterState<R: Runtime> {
    app: AppHandle<R>,
    targets: Arc<RwLock<HashMap<String, BridgeTarget>>>,
    port: u16,
}

impl<R: Runtime> Clone for RouterState<R> {
    fn clone(&self) -> Self {
        Self {
            app: self.app.clone(),
            targets: self.targets.clone(),
            port: self.port,
        }
    }
}

#[derive(Default)]
pub struct BrowserStreamBridge {
    server_port: Mutex<Option<u16>>,
    targets: Arc<RwLock<HashMap<String, BridgeTarget>>>,
}

#[derive(Serialize)]
pub struct BrowserStreamBridgeEndpoints {
    pub ws_url: String,
    pub http_base: String,
}

impl BrowserStreamBridge {
    pub async fn endpoints<R: Runtime>(
        &self,
        app: AppHandle<R>,
        manager: &AgentBrowserManager,
        browser_id: String,
    ) -> Result<BrowserStreamBridgeEndpoints, String> {
        let identity = manager.verified_stream_identity(&browser_id).await?;
        let port = {
            let mut server_port = self.server_port.lock().await;
            match *server_port {
                Some(port) => port,
                None => {
                    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
                        .await
                        .map_err(|error| format!("Cannot start browser stream bridge: {error}"))?;
                    let port = listener
                        .local_addr()
                        .map_err(|error| format!("Cannot inspect browser stream bridge: {error}"))?
                        .port();
                    let state = RouterState {
                        app,
                        targets: self.targets.clone(),
                        port,
                    };
                    let router = Router::new()
                        .route("/:token/ws", get(bridge_ws::<R>))
                        .route(
                            "/:token/api/status",
                            get(bridge_status::<R>).options(bridge_options::<R>),
                        )
                        .route(
                            "/:token/api/command",
                            post(bridge_command::<R>).options(bridge_options::<R>),
                        )
                        .layer(DefaultBodyLimit::max(16 * 1024))
                        .with_state(state);
                    tokio::spawn(async move {
                        if let Err(error) = axum::serve(listener, router).await {
                            eprintln!("[codemux::browser] stream bridge stopped: {error}");
                        }
                    });
                    *server_port = Some(port);
                    port
                }
            }
        };

        // Reuse one capability across overlapping panes for the same daemon,
        // but invalidate it when that daemon's generation changes.
        let token = publish_current_token(&self.targets, &browser_id, identity, || {
            manager.verified_stream_identity(&browser_id)
        })
        .await?;
        let base = format!("127.0.0.1:{port}/{token}");
        Ok(BrowserStreamBridgeEndpoints {
            ws_url: format!("ws://{base}/ws"),
            http_base: format!("http://{base}"),
        })
    }
}

async fn publish_current_token<F, Fut>(
    targets: &Arc<RwLock<HashMap<String, BridgeTarget>>>,
    browser_id: &str,
    captured: BrowserStreamIdentity,
    verify: F,
) -> Result<String, String>
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<BrowserStreamIdentity, String>>,
{
    // Verification and publication share the token lock. An older endpoint
    // request must not revoke a replacement daemon's capability after it
    // finishes waiting for bridge startup.
    let mut targets = targets.write().await;
    if verify().await? != captured {
        return Err("Browser session changed before its stream URL was issued".into());
    }
    Ok(issue_token(&mut targets, browser_id.to_owned(), captured))
}

fn issue_token(
    targets: &mut HashMap<String, BridgeTarget>,
    browser_id: String,
    identity: BrowserStreamIdentity,
) -> String {
    if let Some(existing) = targets.iter().find_map(|(token, target)| {
        (target.browser_id == browser_id && target.identity == identity).then(|| token.clone())
    }) {
        return existing;
    }
    targets.retain(|_, target| target.browser_id != browser_id);
    let token = uuid::Uuid::new_v4().simple().to_string();
    targets.insert(
        token.clone(),
        BridgeTarget {
            browser_id,
            identity,
        },
    );
    token
}

fn allowed_desktop_origin(headers: &HeaderMap) -> Option<&HeaderValue> {
    let origin = headers.get(header::ORIGIN)?;
    let value = origin.to_str().ok()?;
    let app_origin = matches!(
        value,
        "tauri://localhost" | "http://tauri.localhost" | "https://tauri.localhost"
    );
    let dev_origin = cfg!(debug_assertions)
        && matches!(value, "http://localhost:1420" | "http://127.0.0.1:1420");
    (app_origin || dev_origin).then_some(origin)
}

fn host_matches(headers: &HeaderMap, port: u16) -> bool {
    headers
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value == format!("127.0.0.1:{port}"))
}

fn with_cors(mut response: Response, origin: &HeaderValue) -> Response {
    let headers = response.headers_mut();
    headers.insert(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin.clone());
    headers.insert(header::VARY, HeaderValue::from_static("Origin"));
    response
}

async fn verified_target<R: Runtime>(
    state: &RouterState<R>,
    token: &str,
    headers: &HeaderMap,
) -> Option<BridgeTarget> {
    if !host_matches(headers, state.port) {
        return None;
    }
    let target = state.targets.read().await.get(token).cloned()?;
    let manager = state.app.state::<AgentBrowserManager>();
    let current = manager
        .verified_stream_identity(&target.browser_id)
        .await
        .ok()?;
    (current == target.identity).then_some(target)
}

async fn bridge_ws<R: Runtime>(
    State(state): State<RouterState<R>>,
    Path(token): Path<String>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    if allowed_desktop_origin(&headers).is_none() {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Some(target) = verified_target(&state, &token, &headers).await else {
        return StatusCode::FORBIDDEN.into_response();
    };
    ws.on_upgrade(move |socket: WebSocket| async move {
        // A daemon can exit or the capability can rotate between the HTTP
        // handshake and the upgrade callback. Never connect that socket to a
        // new occupant of the same port using a stale authorization decision.
        if verified_target(&state, &token, &headers)
            .await
            .is_some_and(|current| current.identity == target.identity)
        {
            if let Some(mut authorization) = state.app.state::<AgentBrowserManager>()
                .subscribe_active_stream(target.identity).await {
                tokio::select! {
                    biased;
                    _ = authorization.changed() => {}
                    _ = crate::web_remote::proxy::bridge_ws(socket, target.identity) => {}
                }
            }
        }
    })
}

async fn bridge_status<R: Runtime>(
    State(state): State<RouterState<R>>,
    Path(token): Path<String>,
    headers: HeaderMap,
) -> Response {
    bridge_http(&state, &token, &headers, &Method::GET, "status", &[]).await
}

async fn bridge_command<R: Runtime>(
    State(state): State<RouterState<R>>,
    Path(token): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    bridge_http(&state, &token, &headers, &Method::POST, "command", &body).await
}

async fn bridge_http<R: Runtime>(
    state: &RouterState<R>,
    token: &str,
    headers: &HeaderMap,
    method: &Method,
    rest: &str,
    body: &[u8],
) -> Response {
    let Some(origin) = allowed_desktop_origin(headers) else {
        return StatusCode::FORBIDDEN.into_response();
    };
    let Some(target) = verified_target(state, token, headers).await else {
        return StatusCode::FORBIDDEN.into_response();
    };
    let mut daemon_headers = HeaderMap::new();
    if method == Method::POST {
        daemon_headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/json"),
        );
    }
    match crate::web_remote::proxy::forward(
        target.identity,
        method,
        rest,
        None,
        &daemon_headers,
        body,
    )
    .await
    {
        Ok(response) => with_cors(response, origin),
        Err(()) => StatusCode::BAD_GATEWAY.into_response(),
    }
}

async fn bridge_options<R: Runtime>(
    State(state): State<RouterState<R>>,
    Path(token): Path<String>,
    headers: HeaderMap,
) -> Response {
    let Some(origin) = allowed_desktop_origin(&headers) else {
        return StatusCode::FORBIDDEN.into_response();
    };
    if verified_target(&state, &token, &headers).await.is_none() {
        return StatusCode::FORBIDDEN.into_response();
    }
    let mut response = with_cors(StatusCode::NO_CONTENT.into_response(), origin);
    let headers = response.headers_mut();
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static("GET, POST, OPTIONS"),
    );
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_HEADERS,
        HeaderValue::from_static("Content-Type"),
    );
    headers.insert(
        header::ACCESS_CONTROL_MAX_AGE,
        HeaderValue::from_static("600"),
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn active_desktop_bridge_closes_when_approval_becomes_pending() {
        use futures_util::{SinkExt, StreamExt};
        use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message};

        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            let app = tauri::test::mock_app();
            let manager = AgentBrowserManager::new();
            app.manage(manager.clone());
            let daemon = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let port = daemon.local_addr().unwrap().port();
            manager.allow_test_stream_session("browser-pending", port).await;
            let bridge = BrowserStreamBridge::default();
            let endpoints = bridge.endpoints(app.handle().clone(), &manager, "browser-pending".into()).await.unwrap();

            let upstream_task = tokio::spawn(async move {
                let (stream, _) = daemon.accept().await.unwrap();
                tokio_tungstenite::accept_async(stream).await.unwrap()
            });
            let mut request = endpoints.ws_url.clone().into_client_request().unwrap();
            request.headers_mut().insert(header::ORIGIN, HeaderValue::from_static("tauri://localhost"));
            let url = url::Url::parse(&endpoints.ws_url).unwrap();
            let stream = tokio::net::TcpStream::connect(("127.0.0.1", url.port().unwrap())).await.unwrap();
            let (mut client, _) = tokio_tungstenite::client_async(request.clone(), stream).await.unwrap();
            let mut upstream = upstream_task.await.unwrap();
            client.send(Message::text("before approval")).await.unwrap();
            assert_eq!(upstream.next().await.unwrap().unwrap(), Message::text("before approval"));

            manager.park_test_stream_confirmation("browser-pending").await;
            let client_end = client.next().await;
            let upstream_end = upstream.next().await;
            assert!(matches!(&client_end, None | Some(Ok(Message::Close(_))) | Some(Err(_))),
                "desktop socket stayed open after approval: {client_end:?}");
            assert!(matches!(&upstream_end, None | Some(Ok(Message::Close(_))) | Some(Err(_))),
                "daemon socket stayed open after approval: {upstream_end:?}");

            let stream = tokio::net::TcpStream::connect(("127.0.0.1", url.port().unwrap())).await.unwrap();
            let denied = tokio_tungstenite::client_async(request, stream).await.unwrap_err();
            assert!(matches!(denied, tokio_tungstenite::tungstenite::Error::Http(response)
                if response.status() == StatusCode::FORBIDDEN));
        }).await.expect("approval must revoke existing and new desktop bridges");
    }

    #[test]
    fn desktop_bridge_only_allows_known_origins_and_loopback_host() {
        let mut headers = HeaderMap::new();
        headers.insert(header::HOST, HeaderValue::from_static("127.0.0.1:52123"));
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("tauri://localhost"),
        );
        assert!(host_matches(&headers, 52123));
        assert!(allowed_desktop_origin(&headers).is_some());
        headers.insert(
            header::ORIGIN,
            HeaderValue::from_static("https://evil.example"),
        );
        assert!(allowed_desktop_origin(&headers).is_none());
        headers.insert(header::HOST, HeaderValue::from_static("evil.example:52123"));
        assert!(!host_matches(&headers, 52123));
    }

    #[test]
    fn capability_survives_a_remount_but_rotates_for_a_new_daemon() {
        let mut targets = HashMap::new();
        let identity = BrowserStreamIdentity {
            port: 9223,
            pid: 1,
            start_time: 10,
            generation: 1,
        };
        let token = issue_token(&mut targets, "browser-a".into(), identity);
        assert_eq!(
            issue_token(&mut targets, "browser-a".into(), identity),
            token
        );
        let newer = BrowserStreamIdentity {
            generation: 2,
            ..identity
        };
        let replacement = issue_token(&mut targets, "browser-a".into(), newer);
        assert_ne!(replacement, token);
        assert!(!targets.contains_key(&token));
    }

    #[tokio::test]
    async fn delayed_old_request_cannot_revoke_replacement_capability() {
        let targets = Arc::new(RwLock::new(HashMap::new()));
        let old = BrowserStreamIdentity {
            port: 9223,
            pid: 1,
            start_time: 10,
            generation: 1,
        };
        let replacement = BrowserStreamIdentity {
            generation: 2,
            ..old
        };
        let old_token = publish_current_token(&targets, "browser-a", old, || async { Ok(old) })
            .await
            .unwrap();
        let (release_old, older_request_ready) = tokio::sync::oneshot::channel::<()>();
        let older_targets = targets.clone();
        let older_request = tokio::spawn(async move {
            older_request_ready.await.unwrap();
            publish_current_token(&older_targets, "browser-a", old, || async {
                Ok(replacement)
            })
            .await
        });
        let replacement_token = publish_current_token(&targets, "browser-a", replacement, || async {
            Ok(replacement)
        })
        .await
        .unwrap();
        assert_ne!(replacement_token, old_token);

        // The first request captured `old` before the replacement, then
        // reached token publication after the replacement request did.
        release_old.send(()).unwrap();
        let late = older_request.await.unwrap();
        assert!(late.is_err());
        let targets = targets.read().await;
        assert_eq!(targets.len(), 1);
        assert_eq!(targets[&replacement_token].identity, replacement);
    }
}
