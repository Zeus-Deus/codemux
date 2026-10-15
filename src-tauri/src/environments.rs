//! Environments: this backend's identity, and the native client half of
//! connections to the user's other Codemux backends.
//!
//! An environment is one running Codemux backend that owns its workspaces.
//! Clients connect to every environment the account shares and merge them
//! (see `docs/plans/environments.md`). The frontend owns the connection logic:
//! it reuses the browser's `DeviceRegistry` and `IrohWebSocket`, which perform
//! the account handshake and speak the `/ws` frame protocol. This module only
//! supplies what a webview cannot do itself:
//!
//! - [`environment_info`]: the descriptor of this backend.
//! - [`environment_account_request`]: the account device endpoints, called with
//!   the desktop's bearer token, which never leaves Rust. Only listing devices
//!   and minting a connect grant are allowed.
//! - [`environment_dial`] / [`environment_stream_write`] /
//!   [`environment_stream_close`]: a raw iroh bi-stream. A native endpoint can
//!   hole-punch a direct path; the browser's wasm client can only relay.
//!
//! None of these commands are in the web-remote policy except
//! `environment_info`, so a paired browser can never dial out through a desktop.

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use iroh::endpoint::{presets, Connection, SendStream};
use iroh::{Endpoint, EndpointAddr, EndpointId, RelayUrl};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::ipc::{Channel, InvokeBody, InvokeResponseBody, Request};
use tauri::{AppHandle, Manager, Runtime, State};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};

use crate::database::DatabaseStore;
use crate::web_remote::iroh::IROH_ALPN;
use crate::web_remote::WebRemoteState;

/// How long a dial may take before the frontend's transport retries.
const DIAL_TIMEOUT: Duration = Duration::from_secs(20);
/// Read buffer for one chunk forwarded to the webview.
const READ_CHUNK: usize = 64 * 1024;
/// Header carrying the stream id on the raw-body write command.
const STREAM_HEADER: &str = "x-stream-id";

/// This backend's identity, as other clients see it in their catalog.
#[derive(Debug, Clone, Serialize)]
pub struct EnvironmentDescriptor {
    /// Stable per-install id; the account registry's `deviceId`.
    pub id: String,
    pub name: String,
    pub platform: String,
    pub app_version: String,
    /// Whether this machine accepts connections from the account's devices.
    pub sharing: bool,
}

#[tauri::command]
pub fn environment_info<R: Runtime>(app: AppHandle<R>) -> EnvironmentDescriptor {
    let db = app.state::<DatabaseStore>();
    let sharing = crate::web_remote::relay_wanted(
        &app.state::<WebRemoteState>().shared().config.lock().unwrap(),
    );
    EnvironmentDescriptor {
        id: crate::web_remote::registration::stable_device_id(&db),
        name: crate::web_remote::registration::device_name(),
        platform: std::env::consts::OS.to_string(),
        app_version: app.package_info().version.to_string(),
        sharing,
    }
}

// ── Account device endpoints ─────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct AccountRequest {
    pub method: String,
    pub path: String,
    #[serde(default)]
    pub body: Option<Value>,
}

/// The HTTP status and JSON body, so the frontend's `DeviceRegistry` maps
/// failures exactly as it does for a browser.
#[derive(Debug, Serialize)]
pub struct AccountResponse {
    pub status: u16,
    pub body: Value,
}

/// Only the two calls a client needs to reach an environment.
fn account_request_allowed(method: &str, path: &str) -> bool {
    match method {
        "GET" => path == "/api/devices",
        "POST" => path
            .strip_prefix("/api/devices/")
            .and_then(|rest| rest.strip_suffix("/connect"))
            .is_some_and(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit())),
        _ => false,
    }
}

#[tauri::command]
pub async fn environment_account_request(
    db: State<'_, DatabaseStore>,
    request: AccountRequest,
) -> Result<AccountResponse, String> {
    if !account_request_allowed(&request.method, &request.path) {
        return Err("environments: account request not allowed".into());
    }
    let token = crate::auth::load_token(&db)
        .filter(|(_, expires_at)| !crate::auth::is_token_expired(expires_at))
        .map(|(token, _)| token);
    let Some(token) = token else {
        return Ok(AccountResponse {
            status: 401,
            body: json!({ "error": "signed_out" }),
        });
    };
    let url = format!("{}{}", crate::auth::api_base_url(), request.path);
    let client = reqwest::Client::new();
    let builder = match request.method.as_str() {
        "GET" => client.get(&url),
        _ => client.post(&url).json(&request.body.unwrap_or_else(|| json!({}))),
    };
    let response = builder
        .bearer_auth(token)
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| format!("environments: account request failed: {e}"))?;
    let status = response.status().as_u16();
    let body = response.json::<Value>().await.unwrap_or(Value::Null);
    Ok(AccountResponse { status, body })
}

// ── Native iroh byte streams ─────────────────────────────────────────

struct OpenStream {
    send: tokio::sync::Mutex<SendStream>,
    connection: Connection,
}

/// Managed state: one client endpoint, bound on first dial, and the open
/// streams by id.
#[derive(Default)]
pub struct EnvironmentStreams {
    endpoint: tokio::sync::OnceCell<Endpoint>,
    next_id: AtomicU64,
    streams: Mutex<HashMap<u64, Arc<OpenStream>>>,
}

impl EnvironmentStreams {
    async fn endpoint(&self) -> Result<&Endpoint, String> {
        self.endpoint
            .get_or_try_init(|| async {
                Endpoint::builder(presets::N0)
                    .bind()
                    .await
                    .map_err(|e| format!("environments: iroh client bind failed: {e}"))
            })
            .await
    }

    fn remove(&self, id: u64) -> Option<Arc<OpenStream>> {
        self.streams.lock().unwrap().remove(&id)
    }
}

/// Parse the dial target the frontend resolved from a connect grant.
fn dial_addr(node_id: &str, relay_url: Option<&str>) -> Result<EndpointAddr, String> {
    let id = EndpointId::from_str(node_id.trim())
        .map_err(|e| format!("environments: invalid node id: {e}"))?;
    let mut addr = EndpointAddr::new(id);
    if let Some(url) = relay_url.map(str::trim).filter(|url| !url.is_empty()) {
        let url = RelayUrl::from_str(url)
            .map_err(|e| format!("environments: invalid relay url: {e}"))?;
        addr = addr.with_relay_url(url);
    }
    Ok(addr)
}

/// Open a bi-stream to an environment. Bytes it sends arrive on `on_data` as
/// raw chunks; a final JSON `{"t":"end"}` (with `error` on failure) marks the
/// end of the stream. Returns the stream id for writes and close.
#[tauri::command]
pub async fn environment_dial(
    streams: State<'_, Arc<EnvironmentStreams>>,
    node_id: String,
    relay_url: Option<String>,
    on_data: Channel<InvokeResponseBody>,
) -> Result<u64, String> {
    let addr = dial_addr(&node_id, relay_url.as_deref())?;
    let endpoint = streams.endpoint().await?;
    let connection = tokio::time::timeout(DIAL_TIMEOUT, endpoint.connect(addr, IROH_ALPN))
        .await
        .map_err(|_| "environments: dial timed out".to_string())?
        .map_err(|e| format!("environments: dial failed: {e}"))?;
    let (send, mut recv) = connection
        .open_bi()
        .await
        .map_err(|e| format!("environments: open stream failed: {e}"))?;

    let id = streams.next_id.fetch_add(1, Ordering::Relaxed) + 1;
    let open = Arc::new(OpenStream {
        send: tokio::sync::Mutex::new(send),
        connection,
    });
    streams.streams.lock().unwrap().insert(id, open.clone());

    let registry = streams.inner().clone();
    tauri::async_runtime::spawn(async move {
        forward(&mut recv, |body| on_data.send(body).is_ok()).await;
        if let Some(open) = registry.remove(id) {
            open.connection.close(0u32.into(), b"done");
        }
    });
    Ok(id)
}

/// Deliver every chunk read from `recv` as raw bytes, then one JSON end marker
/// (`error` is set when the read failed). `deliver` returns `false` once the
/// webview channel is gone, which stops the read: nobody is listening.
async fn forward<S: AsyncRead + Unpin>(recv: &mut S, mut deliver: impl FnMut(InvokeResponseBody) -> bool) {
    let mut buf = vec![0u8; READ_CHUNK];
    let error = loop {
        match recv.read(&mut buf).await {
            Ok(0) => break None,
            Ok(n) => {
                if !deliver(InvokeResponseBody::Raw(buf[..n].to_vec())) {
                    return;
                }
            }
            Err(e) => break Some(e.to_string()),
        }
    };
    deliver(InvokeResponseBody::Json(json!({ "t": "end", "error": error }).to_string()));
}

/// Write the raw request body to a stream. The id rides the `x-stream-id`
/// header so the bytes need no JSON encoding.
#[tauri::command]
pub async fn environment_stream_write(
    streams: State<'_, Arc<EnvironmentStreams>>,
    request: Request<'_>,
) -> Result<(), String> {
    let id = request
        .headers()
        .get(STREAM_HEADER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok())
        .ok_or_else(|| "environments: missing stream id".to_string())?;
    let InvokeBody::Raw(bytes) = request.body() else {
        return Err("environments: expected a raw body".into());
    };
    let bytes = bytes.clone();
    let open = streams
        .streams
        .lock()
        .unwrap()
        .get(&id)
        .cloned()
        .ok_or_else(|| "environments: stream closed".to_string())?;
    let mut send = open.send.lock().await;
    send.write_all(&bytes)
        .await
        .map_err(|e| format!("environments: write failed: {e}"))?;
    send.flush()
        .await
        .map_err(|e| format!("environments: write failed: {e}"))
}

#[tauri::command]
pub fn environment_stream_close(streams: State<'_, Arc<EnvironmentStreams>>, stream_id: u64) {
    if let Some(open) = streams.remove(stream_id) {
        open.connection.close(0u32.into(), b"closed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_requests_are_limited_to_listing_and_connecting() {
        assert!(account_request_allowed("GET", "/api/devices"));
        assert!(account_request_allowed("POST", "/api/devices/42/connect"));

        assert!(!account_request_allowed("POST", "/api/devices"));
        assert!(!account_request_allowed("DELETE", "/api/devices/42"));
        assert!(!account_request_allowed("GET", "/api/devices/42/connect"));
        assert!(!account_request_allowed("POST", "/api/devices//connect"));
        assert!(!account_request_allowed("POST", "/api/devices/4a/connect"));
        assert!(!account_request_allowed("POST", "/api/devices/1/../../user/connect"));
        assert!(!account_request_allowed("GET", "/api/workspaces"));
        assert!(!account_request_allowed("GET", "/api/devices?x=1"));
    }

    #[test]
    fn dial_addr_validates_node_and_relay() {
        let key = iroh::SecretKey::generate().public().to_string();
        let addr = dial_addr(&key, Some("https://relay.example.org./")).unwrap();
        assert_eq!(addr.id.to_string(), key);
        assert_eq!(addr.relay_urls().count(), 1);
        assert_eq!(dial_addr(&key, Some("  ")).unwrap().relay_urls().count(), 0);
        assert!(dial_addr("not-a-key", None).is_err());
        assert!(dial_addr(&key, Some("not a url")).is_err());
    }

    fn collect(bodies: &[InvokeResponseBody]) -> (Vec<u8>, Vec<Value>) {
        let mut bytes = Vec::new();
        let mut json = Vec::new();
        for body in bodies {
            match body {
                InvokeResponseBody::Raw(chunk) => bytes.extend_from_slice(chunk),
                InvokeResponseBody::Json(text) => json.push(serde_json::from_str(text).unwrap()),
            }
        }
        (bytes, json)
    }

    #[tokio::test]
    async fn forward_delivers_bytes_then_a_clean_end() {
        let mut bodies = Vec::new();
        let mut source: &[u8] = b"\x00\x00\x00\x00\x02{}";
        forward(&mut source, |body| {
            bodies.push(body);
            true
        })
        .await;
        let (bytes, json) = collect(&bodies);
        assert_eq!(bytes, b"\x00\x00\x00\x00\x02{}");
        assert_eq!(json, vec![json!({ "t": "end", "error": null })]);
    }

    #[tokio::test]
    async fn forward_reports_read_errors_in_the_end_marker() {
        struct Broken;
        impl AsyncRead for Broken {
            fn poll_read(
                self: std::pin::Pin<&mut Self>,
                _: &mut std::task::Context<'_>,
                _: &mut tokio::io::ReadBuf<'_>,
            ) -> std::task::Poll<std::io::Result<()>> {
                std::task::Poll::Ready(Err(std::io::Error::other("connection lost")))
            }
        }
        let mut bodies = Vec::new();
        forward(&mut Broken, |body| {
            bodies.push(body);
            true
        })
        .await;
        let (_, json) = collect(&bodies);
        assert_eq!(json, vec![json!({ "t": "end", "error": "connection lost" })]);
    }

    #[tokio::test]
    async fn forward_stops_when_the_webview_channel_is_gone() {
        let mut calls = 0;
        let mut source: &[u8] = b"data";
        forward(&mut source, |_| {
            calls += 1;
            false
        })
        .await;
        assert_eq!(calls, 1, "no end marker is sent to a closed channel");
    }
}
