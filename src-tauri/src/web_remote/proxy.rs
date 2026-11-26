//! Browser-pane proxy for the web-remote server.
//!
//! The embedded browser pane is a screenshot-driven Chromium session served by
//! a per-workspace `agent-browser` daemon bound to `127.0.0.1:<port>` (ports
//! 9223–9299; see `src-tauri/src/agent_browser.rs`). The desktop pane uses the
//! app-local capability bridge for screencast and input WebSockets, status
//! probes, and commands (`src-tauri/src/browser_stream_bridge.rs`).
//!
//! A remote browser can't reach those loopback daemons, so this module bridges
//! them through the authenticated web-remote origin:
//!
//! - `GET /proxy/browser/:port/ws` upgrades the client socket and pipes it,
//!   frame-for-frame (binary-safe, backpressure-aware), to `ws://127.0.0.1:<port>`.
//! - `GET /proxy/browser/:port/api/status` and
//!   `POST /proxy/browser/:port/api/command` forward the pane's HTTP requests.
//!
//! ## The single-segment HTTP constraint
//!
//! The daemon reads each HTTP request with ONE peek and only sees the first
//! TCP segment (~1.4 KB); a request that spans segments looks like it has an
//! empty body. The HTTP forwarder therefore
//! rebuilds a MINIMAL request — request line, loopback `Host` and matching
//! `Origin`, and (only with a body) `Content-Type` / `Content-Length`, plus
//! `Connection: close` — and writes it
//! in a single `write_all`. Every inbound header (cookies, the session bearer,
//! `User-Agent`, `Accept*`, and all hop-by-hop headers) is dropped, both to
//! respect the size budget and to never leak the paired session's credentials
//! to the daemon.
//!
//! ## Port validation
//!
//! `:port` must be inside the agent-browser range and currently owned by a
//! live browser daemon tracked by this app. That prevents a paired client
//! from using the proxy to reach unrelated loopback services.

use std::time::Duration;

use axum::{
    body::Bytes,
    extract::{
        ws::{Message as AxumMessage, WebSocket, WebSocketUpgrade},
        Path, State,
    },
    http::{header, HeaderMap, Method, StatusCode, Uri},
    response::{IntoResponse, Response},
    Extension,
};
use futures_util::{SinkExt, StreamExt};
use tauri::{AppHandle, Manager, Runtime};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::sync::watch;
use tokio_tungstenite::tungstenite::Message as TungMessage;

use crate::agent_browser::{AgentBrowserManager, BrowserStreamIdentity};

use super::{ListenerLife, WebRemoteState};

/// Lowest agent-browser stream port. Sourced from the public constant so the
/// proxy range tracks the allocator's floor automatically.
const BROWSER_PORT_MIN: u16 = crate::agent_browser::DEFAULT_STREAM_PORT; // 9223
/// Highest agent-browser stream port. Mirrors the (private)
/// `agent_browser::MAX_STREAM_PORT`; kept in sync by this constant + the
/// `port_range_low_bound_tracks_agent_browser` test's floor assertion.
const BROWSER_PORT_MAX: u16 = 9299;
const MAX_BROWSER_RESPONSE_BYTES: usize = 16 << 20;

/// Whether `port` is inside the agent-browser stream-port range.
fn port_in_range(port: u16) -> bool {
    (BROWSER_PORT_MIN..=BROWSER_PORT_MAX).contains(&port)
}

/// Whether `s` contains an ASCII control character (CR, LF, NUL, …). axum
/// percent-decodes the wildcard `rest` path segment, so a `%0d%0a` would
/// otherwise be written verbatim into the raw HTTP request line built in
/// [`forward`] — smuggling extra headers or a second request line into the
/// loopback daemon. A legitimate daemon path (`status`, `command`) never
/// carries a control character, so rejecting them closes the injection with
/// no false positives.
fn has_control_char(s: &str) -> bool {
    s.bytes().any(|b| b < 0x20 || b == 0x7f)
}

fn allowed_browser_api(method: &Method, rest: &str) -> bool {
    (method == Method::GET && rest == "status")
        || (method == Method::POST && rest == "command")
}

// ── WebSocket proxy ─────────────────────────────────────────────────

/// `GET /proxy/browser/:port/ws` — upgrade and bridge to the daemon socket.
pub(super) async fn ws_proxy<R: Runtime>(
    State(app): State<AppHandle<R>>,
    life: Option<Extension<ListenerLife>>,
    Path(port): Path<u16>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    let session = match super::server::require_session(&app, &headers) {
        Ok(session) => session,
        Err(resp) => return resp,
    };
    let Some(Extension(mut life)) = life.filter(|Extension(life)| life.live()) else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    if !port_in_range(port) {
        return (StatusCode::BAD_REQUEST, "port out of range").into_response();
    }
    let Some(identity) = app.state::<AgentBrowserManager>()
        .verified_stream_identity_for_port(port).await else {
        return StatusCode::FORBIDDEN.into_response();
    };
    ws.on_upgrade(move |socket| async move {
        let shared = app.state::<WebRemoteState>().shared();
        let (close, mut closed) = watch::channel(false);
        let connection = shared.connections.register_browser_proxy(&session.id, close);
        // Register before checking again so a revocation during the upgrade
        // is either visible here or can signal this bridge to close.
        if super::server::session_active(&app, &session.id)
            && app.state::<AgentBrowserManager>()
                .verified_stream_identity_for_port(port).await == Some(identity)
        {
            if let Some(mut authorization) = app.state::<AgentBrowserManager>()
                .subscribe_active_stream(identity).await {
                tokio::select! {
                    biased;
                    _ = authorization.changed() => {}
                    _ = closed.changed() => {}
                    _ = life.stopped() => {}
                    _ = bridge_ws(socket, identity) => {}
                }
            }
        }
        shared.connections.unregister(connection);
    })
}

/// Bidirectionally pipe an upgraded client socket to the loopback daemon at
/// `port`. Both directions await each send, so a slow peer stalls the reader
/// on the other side (TCP backpressure) instead of buffering unboundedly.
pub(crate) async fn bridge_ws(client: WebSocket, identity: BrowserStreamIdentity) {
    let daemon = match crate::agent_browser::connect_verified_stream_peer(identity).await {
        Ok(stream) => tokio_tungstenite::client_async(
            crate::agent_browser::authenticated_stream_request(identity), stream,
        ).await,
        Err(_) => {
            let mut client = client;
            let _ = client.send(AxumMessage::Close(None)).await;
            return;
        }
    };
    let daemon = match daemon {
        Ok((ws, _)) => ws,
        Err(_) => {
            // Daemon not up (pane still launching / already closed). Close the
            // client cleanly; the pane's reconnect loop retries.
            let mut client = client;
            let _ = client.send(AxumMessage::Close(None)).await;
            return;
        }
    };

    let (mut client_tx, mut client_rx) = client.split();
    let (mut daemon_tx, mut daemon_rx) = daemon.split();

    // Client → daemon.
    let c2d = async move {
        while let Some(Ok(msg)) = client_rx.next().await {
            let closing = matches!(msg, AxumMessage::Close(_));
            if let Some(out) = axum_to_tungstenite(msg) {
                if daemon_tx.send(out).await.is_err() {
                    break;
                }
            }
            if closing {
                break;
            }
        }
        let _ = daemon_tx.close().await;
    };

    // Daemon → client.
    let d2c = async move {
        while let Some(Ok(msg)) = daemon_rx.next().await {
            let closing = matches!(msg, TungMessage::Close(_));
            if let Some(out) = tungstenite_to_axum(msg) {
                if client_tx.send(out).await.is_err() {
                    break;
                }
            }
            if closing {
                break;
            }
        }
        let _ = client_tx.close().await;
    };

    // First direction to end (peer closed / errored) tears the other down when
    // its future is dropped, closing both halves.
    tokio::select! {
        _ = c2d => {}
        _ = d2c => {}
    }
}

/// Convert an axum client frame into the daemon-side frame type. Binary-safe:
/// screencast bytes and JSON input frames pass through unchanged.
fn axum_to_tungstenite(msg: AxumMessage) -> Option<TungMessage> {
    Some(match msg {
        AxumMessage::Text(t) => TungMessage::Text(t.into()),
        AxumMessage::Binary(b) => TungMessage::Binary(b.into()),
        AxumMessage::Ping(b) => TungMessage::Ping(b.into()),
        AxumMessage::Pong(b) => TungMessage::Pong(b.into()),
        AxumMessage::Close(_) => TungMessage::Close(None),
    })
}

/// Convert a daemon frame back into an axum client frame.
fn tungstenite_to_axum(msg: TungMessage) -> Option<AxumMessage> {
    Some(match msg {
        TungMessage::Text(t) => AxumMessage::Text(t.as_str().to_owned()),
        TungMessage::Binary(b) => AxumMessage::Binary(b.to_vec()),
        TungMessage::Ping(b) => AxumMessage::Ping(b.to_vec()),
        TungMessage::Pong(b) => AxumMessage::Pong(b.to_vec()),
        TungMessage::Close(_) => AxumMessage::Close(None),
        // Raw frames are an internal tungstenite detail; nothing to forward.
        TungMessage::Frame(_) => return None,
    })
}

// ── HTTP forward ────────────────────────────────────────────────────

/// Forward only the browser pane's status GET and command POST with a
/// minimal request carrying the verified session's listener credential.
pub async fn http_forward<R: Runtime>(
    State(app): State<AppHandle<R>>,
    Path((port, rest)): Path<(u16, String)>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Err(resp) = super::server::require_session(&app, &headers) {
        return resp;
    }
    if !port_in_range(port) {
        return (StatusCode::BAD_REQUEST, "port out of range").into_response();
    }
    let Some(identity) = app.state::<AgentBrowserManager>()
        .verified_stream_identity_for_port(port).await else {
        return StatusCode::FORBIDDEN.into_response();
    };
    // Reject a percent-decoded CR/LF (or other control char) in the path before
    // it reaches the raw request line — no header/request smuggling to the
    // daemon. (`uri.query()` stays percent-encoded, so it needs no such guard.)
    if has_control_char(&rest) {
        return (StatusCode::BAD_REQUEST, "invalid path").into_response();
    }
    if !allowed_browser_api(&method, &rest) {
        return StatusCode::NOT_FOUND.into_response();
    }
    match forward(identity, &method, &rest, uri.query(), &headers, &body).await {
        Ok(resp) => resp,
        Err(()) => (StatusCode::BAD_GATEWAY, "daemon unreachable").into_response(),
    }
}

/// Open a raw TCP connection to the daemon, write a minimal single-segment
/// request, and translate the response back into an axum `Response`.
pub(crate) async fn forward(
    identity: BrowserStreamIdentity,
    method: &Method,
    rest: &str,
    query: Option<&str>,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Response, ()> {
    let port = identity.port;
    let target = match query {
        Some(q) if !q.is_empty() => format!("/api/{rest}?{q}"),
        _ => format!("/api/{rest}"),
    };

    // The daemon requires a same-origin Origin for command POSTs. This
    // trusted, authenticated proxy supplies its own loopback origin instead
    // of forwarding the remote client's origin or credentials.
    let mut head = format!(
        "{} {} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nOrigin: http://127.0.0.1:{}\r\nX-Codemux-Browser-Token: {}\r\n",
        method.as_str(),
        target,
        port,
        port,
        identity.listener_token_header(),
    );
    if !body.is_empty() {
        if let Some(ct) = headers.get(header::CONTENT_TYPE).and_then(|v| v.to_str().ok()) {
            head.push_str("Content-Type: ");
            head.push_str(ct);
            head.push_str("\r\n");
        }
        head.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    head.push_str("Connection: close\r\n\r\n");

    let mut wire = head.into_bytes();
    wire.extend_from_slice(body);

    let mut stream = crate::agent_browser::connect_verified_stream_peer(identity)
        .await
        .map_err(|_| ())?;

    // One write_all → the whole request lands in a single loopback segment,
    // satisfying the daemon's single-peek read.
    stream.write_all(&wire).await.map_err(|_| ())?;
    let _ = stream.flush().await;

    let raw = tokio::time::timeout(Duration::from_secs(30), read_response(&mut stream, method))
        .await
        .map_err(|_| ())?
        .map_err(|_| ())?;

    let parsed = parse_http_response(&raw, method).ok_or(())?;
    let mut builder = axum::response::Response::builder().status(parsed.status);
    if let Some(ct) = parsed.content_type {
        builder = builder.header(header::CONTENT_TYPE, ct);
    }
    builder
        .body(axum::body::Body::from(parsed.body))
        .map_err(|_| ())
}

/// Read a full HTTP/1.1 response off `stream`.
///
/// Headers are read first; then, if the response carries a `Content-Length`,
/// exactly that many body bytes are read and reading STOPS (so a daemon that
/// ignores our `Connection: close` and keeps the socket open can't wedge us).
/// Without a `Content-Length` — chunked or close-delimited — it reads to EOF.
/// The caller requests `Connection: close`; its 30s timeout bounds a server
/// that ignores that request. Incomplete bodies are rejected, not forwarded.
async fn read_response<R: AsyncRead + Unpin>(stream: &mut R, method: &Method) -> Result<Vec<u8>, ()> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];

    // 1. Read until the header terminator.
    let header_end = loop {
        if let Some(pos) = find_subsequence(&buf, b"\r\n\r\n") {
            break pos;
        }
        let n = stream.read(&mut chunk).await.map_err(|_| ())?;
        if n == 0 {
            return Err(()); // closed before a complete header block
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.len() > (1 << 20) {
            return Err(()); // 1 MiB header cap — nothing legitimate is bigger
        }
    };
    let body_start = header_end + 4;
    let head = String::from_utf8_lossy(&buf[..header_end]);
    let code = head.split_whitespace().nth(1).ok_or(())?
        .parse::<u16>().map_err(|_| ())?;
    let status = StatusCode::from_u16(code).map_err(|_| ())?;
    // HEAD and 304 can carry representation length/encoding metadata without
    // sending that representation. Bodyless responses end at their headers.
    if !response_has_body(method, status) {
        buf.truncate(body_start);
        return Ok(buf);
    }

    // 2. Prefer an explicit Content-Length; fall back to close-delimited.
    let content_length = {
        let head = String::from_utf8_lossy(&buf[..header_end]);
        let mut length = None;
        for line in head.split("\r\n").skip(1) {
            let Some((name, value)) = line.split_once(':') else {
                continue;
            };
            if name.trim().eq_ignore_ascii_case("content-length") {
                for value in value.split(',') {
                    let parsed = value.trim().parse::<usize>().map_err(|_| ())?;
                    if length.is_some_and(|previous| previous != parsed) {
                        return Err(());
                    }
                    length = Some(parsed);
                }
            }
        }
        length
    };

    match content_length {
        Some(len) => {
            if len > MAX_BROWSER_RESPONSE_BYTES {
                return Err(());
            }
            let end = body_start.checked_add(len).ok_or(())?;
            while buf.len() < end {
                let n = stream.read(&mut chunk).await.map_err(|_| ())?;
                if n == 0 {
                    return Err(());
                }
                buf.extend_from_slice(&chunk[..n]);
            }
            buf.truncate(end);
        }
        None => loop {
            let n = stream.read(&mut chunk).await.map_err(|_| ())?;
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
            if buf.len() > MAX_BROWSER_RESPONSE_BYTES {
                return Err(()); // never return a silently truncated response
            }
        },
    }
    Ok(buf)
}

fn response_has_body(method: &Method, status: StatusCode) -> bool {
    method != Method::HEAD
        && !status.is_informational()
        && status != StatusCode::NO_CONTENT
        && status != StatusCode::NOT_MODIFIED
}

struct ParsedResponse {
    status: StatusCode,
    content_type: Option<String>,
    body: Vec<u8>,
}

/// Parse a raw HTTP/1.1 response into status + content-type + decoded body.
/// Handles the `Transfer-Encoding: chunked` case; otherwise the body is taken
/// verbatim (Content-Length or connection-close delimited).
fn parse_http_response(raw: &[u8], method: &Method) -> Option<ParsedResponse> {
    let sep = find_subsequence(raw, b"\r\n\r\n")?;
    let head = &raw[..sep];
    let body_raw = &raw[sep + 4..];

    let head_str = String::from_utf8_lossy(head);
    let mut lines = head_str.split("\r\n");
    let status_line = lines.next()?;
    // "HTTP/1.1 200 OK" → the second whitespace token is the numeric status.
    let code: u16 = status_line.split_whitespace().nth(1)?.parse().ok()?;
    let status = StatusCode::from_u16(code).ok()?;

    let mut content_type = None;
    let mut chunked = false;
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            let value = value.trim();
            match name.trim().to_ascii_lowercase().as_str() {
                "content-type" => content_type = Some(value.to_string()),
                "transfer-encoding" if value.to_ascii_lowercase().contains("chunked") => {
                    chunked = true;
                }
                _ => {}
            }
        }
    }

    let body = if !response_has_body(method, status) {
        Vec::new()
    } else if chunked {
        dechunk(body_raw)?
    } else {
        body_raw.to_vec()
    };
    Some(ParsedResponse {
        status,
        content_type,
        body,
    })
}

/// Position of the first occurrence of `needle` in `hay`.
fn find_subsequence(hay: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() {
        return None;
    }
    hay.windows(needle.len()).position(|w| w == needle)
}

/// Decode an HTTP/1.1 chunked-transfer body.
fn dechunk(mut data: &[u8]) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    loop {
        let nl = find_subsequence(data, b"\r\n")?;
        // The size line may carry a `;ext` suffix; only the hex prefix counts.
        let size_line = std::str::from_utf8(&data[..nl]).ok()?;
        let size_hex = size_line.split(';').next()?.trim();
        let size = usize::from_str_radix(size_hex, 16).ok()?;
        data = &data[nl + 2..];
        if size == 0 {
            // The final chunk is followed by an empty line or a complete
            // trailer section. A missing terminator means a truncated body.
            return (data.starts_with(b"\r\n")
                || find_subsequence(data, b"\r\n\r\n").is_some())
                .then_some(out);
        }
        let next = size.checked_add(2)?;
        if data.len() < next || &data[size..next] != b"\r\n" {
            return None;
        }
        out.extend_from_slice(&data[..size]);
        data = &data[next..];
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;
    use crate::web_remote::{auth, server};
    use tokio::net::{TcpListener, TcpStream};
    use tokio_tungstenite::{tungstenite, WebSocketStream};

    #[test]
    fn proxy_unrelated_process_fixture() {
        if std::env::var_os("CODEMUX_PROXY_UNRELATED_TEST_CHILD").is_some() {
            std::thread::sleep(Duration::from_secs(30));
        }
    }

    struct UnrelatedProcess(std::process::Child);

    impl Drop for UnrelatedProcess {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    fn unrelated_live_identity(port: u16) -> (UnrelatedProcess, BrowserStreamIdentity) {
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "web_remote::proxy::tests::proxy_unrelated_process_fixture"])
            .env("CODEMUX_PROXY_UNRELATED_TEST_CHILD", "1")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let started = Instant::now();
        loop {
            if let Some(identity) = crate::agent_browser::test_process_stream_identity(port, child.id()) {
                std::thread::sleep(Duration::from_millis(100));
                if child.try_wait().unwrap().is_none() {
                    return (UnrelatedProcess(child), identity);
                }
            }
            if child.try_wait().unwrap().is_some() || started.elapsed() >= Duration::from_secs(3) {
                let _ = child.kill();
                let _ = child.wait();
                panic!("unrelated process did not expose a live identity");
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn remote_proxy_exposes_only_the_browser_pane_api() {
        assert!(allowed_browser_api(&Method::GET, "status"));
        assert!(allowed_browser_api(&Method::POST, "command"));
        assert!(!allowed_browser_api(&Method::GET, "command"));
        assert!(!allowed_browser_api(&Method::POST, "status"));
        assert!(!allowed_browser_api(&Method::POST, "shutdown"));
        assert!(!allowed_browser_api(&Method::GET, "../status"));
    }

    async fn bind_test_daemon() -> TcpListener {
        // Keep the successful bind for the whole test. Only connect the proxy
        // to this listener, even if a real daemon already owns another port.
        for port in (BROWSER_PORT_MIN..=BROWSER_PORT_MAX).rev() {
            match TcpListener::bind(("127.0.0.1", port)).await {
                Ok(listener) => return listener,
                Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => continue,
                Err(e) => panic!("binding loopback test daemon on {port}: {e}"),
            }
        }
        panic!("no free loopback port in the browser daemon range");
    }

    #[tokio::test]
    async fn authenticated_http_proxy_supplies_daemon_origin_without_client_credentials() {
        let daemon = bind_test_daemon().await;
        let port = daemon.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (mut stream, _) = daemon.accept().await.unwrap();
            let mut buf = [0; 4096];
            let len = stream.read(&mut buf).await.unwrap();
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\n\r\n{}")
                .await
                .unwrap();
            String::from_utf8_lossy(&buf[..len]).into_owned()
        });

        let mut headers = HeaderMap::new();
        headers.insert(header::CONTENT_TYPE, "application/json".parse().unwrap());
        headers.insert(header::COOKIE, "session=private".parse().unwrap());
        headers.insert(header::AUTHORIZATION, "Bearer private".parse().unwrap());
        let response = forward(
            crate::agent_browser::current_process_stream_identity(port),
            &Method::POST,
            "command",
            None,
            &headers,
            br#"{"action":"stream_status"}"#,
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let request = server.await.unwrap();
        assert!(request.contains(&format!("Host: 127.0.0.1:{port}\r\n")));
        assert!(request.contains(&format!("Origin: http://127.0.0.1:{port}\r\n")));
        let token = crate::agent_browser::current_process_stream_identity(port).listener_token_header();
        assert!(request.contains(&format!("X-Codemux-Browser-Token: {token}\r\n")));
        assert!(!request.contains("session=private"));
        assert!(!request.contains("Bearer private"));
    }

    #[tokio::test]
    async fn http_proxy_does_not_send_to_a_different_established_peer() {
        let daemon = bind_test_daemon().await;
        let port = daemon.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (mut stream, _) = daemon.accept().await.unwrap();
            let mut buf = [0; 1];
            tokio::time::timeout(Duration::from_secs(2), stream.read(&mut buf))
                .await
                .expect("proxy must close a rejected peer")
                .unwrap()
        });
        let (unrelated, identity) = unrelated_live_identity(port);
        let response = forward(
            identity,
            &Method::POST,
            "command",
            None,
            &HeaderMap::new(),
            br#"{"action":"stream_status"}"#,
        )
        .await;
        let received = server.await.unwrap();
        drop(unrelated);
        assert!(response.is_err());
        assert_eq!(received, 0, "rejected peer received request bytes");
    }

    #[tokio::test]
    async fn websocket_peer_check_sends_no_handshake_to_unrelated_listener() {
        let daemon = bind_test_daemon().await;
        let port = daemon.local_addr().unwrap().port();
        let upstream = tokio::spawn(async move {
            let (mut stream, _) = daemon.accept().await.unwrap();
            let mut buf = [0; 1];
            tokio::time::timeout(Duration::from_secs(2), stream.read(&mut buf))
                .await
                .expect("rejected socket must close before a WebSocket handshake")
                .unwrap()
        });
        let (unrelated, identity) = unrelated_live_identity(port);
        async fn direct_bridge(
            State(identity): State<BrowserStreamIdentity>,
            ws: WebSocketUpgrade,
        ) -> Response {
            ws.on_upgrade(move |socket| bridge_ws(socket, identity))
        }
        let bridge_listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let bridge_address = bridge_listener.local_addr().unwrap();
        let bridge_server = tokio::spawn(async move {
            axum::serve(
                bridge_listener,
                axum::Router::new().route("/ws", axum::routing::get(direct_bridge)).with_state(identity),
            )
            .await
            .unwrap();
        });
        let url = format!("ws://{bridge_address}/ws");
        let (mut client, _) = tokio_tungstenite::client_async(
            &url,
            TcpStream::connect(bridge_address).await.unwrap(),
        )
        .await
        .unwrap();
        let received = upstream.await.unwrap();
        assert_peer_closed(&mut client).await;
        bridge_server.abort();
        let _ = bridge_server.await;
        drop(unrelated);
        assert_eq!(received, 0, "rejected peer received WebSocket handshake bytes");
    }

    #[tokio::test]
    async fn paired_client_cannot_proxy_to_an_unowned_browser_range_port() {
        let app = tauri::test::mock_app();
        let db = crate::database::DatabaseStore::new_in_memory();
        db.web_remote_insert_session("phone", None, None, &auth::sha256_hex("phone"), true)
            .unwrap();
        app.manage(db);
        app.manage(crate::agent_browser::AgentBrowserManager::new());

        let unrelated = bind_test_daemon().await;
        let port = unrelated.local_addr().unwrap().port();
        let mut headers = HeaderMap::new();
        headers.insert(header::AUTHORIZATION, "Bearer phone".parse().unwrap());
        let response = http_forward(
            State(app.handle().clone()),
            Path((port, "status".to_owned())),
            Method::GET,
            "/api/status".parse().unwrap(),
            headers,
            Bytes::new(),
        ).await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    type TestSocket = WebSocketStream<TcpStream>;

    async fn open_bridge(
        proxy: std::net::SocketAddr,
        daemon: &TcpListener,
        token: &str,
    ) -> (TestSocket, TestSocket) {
        use tungstenite::client::IntoClientRequest;

        let port = daemon.local_addr().unwrap().port();
        let mut request = format!("ws://{proxy}/proxy/browser/{port}/ws")
            .into_client_request()
            .unwrap();
        request.headers_mut().insert(
            header::COOKIE,
            format!("{}={token}", server::SESSION_COOKIE)
                .parse()
                .unwrap(),
        );
        let (client, _) =
            tokio_tungstenite::client_async(request, TcpStream::connect(proxy).await.unwrap())
                .await
                .expect("authorized browser upgrade");
        let (stream, _) = daemon.accept().await.unwrap();
        let upstream = tokio_tungstenite::accept_async(stream).await.unwrap();
        (client, upstream)
    }

    async fn forward_frame(from: &mut TestSocket, to: &mut TestSocket, text: &str) {
        let frame = TungMessage::text(text);
        from.send(frame.clone()).await.unwrap();
        assert_eq!(to.next().await.unwrap().unwrap(), frame);
    }

    async fn assert_peer_closed(socket: &mut TestSocket) {
        use std::io::ErrorKind;
        use tungstenite::{error::ProtocolError, Error};

        match socket.next().await {
            None | Some(Ok(TungMessage::Close(_))) => {}
            // Cancelling the bridge drops its TCP sockets without a close handshake.
            Some(Err(Error::Protocol(ProtocolError::ResetWithoutClosingHandshake))) => {}
            Some(Err(Error::Io(e)))
                if matches!(
                    e.kind(),
                    ErrorKind::ConnectionReset
                        | ErrorKind::ConnectionAborted
                        | ErrorKind::UnexpectedEof
                ) => {}
            other => panic!("expected the bridge to close the socket, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn active_bridges_close_on_session_revocation_and_listener_shutdown() {
        tokio::time::timeout(Duration::from_secs(10), async {
            let app = tauri::test::mock_app();
            let db = crate::database::DatabaseStore::new_in_memory();
            for id in ["phone", "tablet"] {
                db.web_remote_insert_session(id, None, None, &auth::sha256_hex(id), true)
                    .unwrap();
            }
            app.manage(db);
            app.manage(WebRemoteState::default());
            app.manage(crate::agent_browser::AgentBrowserManager::new());
            let shared = app.state::<WebRemoteState>().shared();

            let daemon = bind_test_daemon().await;
            app.state::<crate::agent_browser::AgentBrowserManager>()
                .allow_test_stream_port(daemon.local_addr().unwrap().port()).await;
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let proxy = listener.local_addr().unwrap();
            let (shutdown, rx) = watch::channel(false);
            let mut life = ListenerLife(rx);
            let router = server::router(app.handle().clone()).layer(Extension(life.clone()));
            let server = tokio::spawn(async move {
                axum::serve(listener, router)
                    .with_graceful_shutdown(async move { life.stopped().await })
                    .await
                    .unwrap();
            });

            let (mut phone, mut phone_daemon) = open_bridge(proxy, &daemon, "phone").await;
            let (mut tablet, mut tablet_daemon) = open_bridge(proxy, &daemon, "tablet").await;
            forward_frame(&mut phone, &mut phone_daemon, "phone input").await;
            forward_frame(&mut tablet, &mut tablet_daemon, "tablet input").await;

            crate::web_remote::web_remote_revoke_session(app.handle().clone(), "phone".into())
                .unwrap();
            assert_peer_closed(&mut phone).await;
            assert_peer_closed(&mut phone_daemon).await;
            assert_eq!(
                shared.connections.close_session("phone"),
                0,
                "revoked bridge still registered"
            );
            forward_frame(&mut tablet_daemon, &mut tablet, "tablet still connected").await;

            shutdown.send(true).unwrap();
            assert_peer_closed(&mut tablet).await;
            assert_peer_closed(&mut tablet_daemon).await;
            assert_eq!(
                shared.connections.close_all(),
                0,
                "stopped listener left a registration"
            );
            server.await.unwrap();
        })
        .await
        .expect("loopback bridges must respond to revocation and listener shutdown");
    }

    #[tokio::test]
    async fn active_browser_bridge_closes_when_approval_becomes_pending() {
        use tungstenite::client::IntoClientRequest;

        tokio::time::timeout(Duration::from_secs(10), async {
            let app = tauri::test::mock_app();
            let db = crate::database::DatabaseStore::new_in_memory();
            db.web_remote_insert_session("phone", None, None, &auth::sha256_hex("phone"), true)
                .unwrap();
            app.manage(db);
            app.manage(WebRemoteState::default());
            let manager = crate::agent_browser::AgentBrowserManager::new();
            app.manage(manager.clone());

            let daemon = bind_test_daemon().await;
            let port = daemon.local_addr().unwrap().port();
            manager.allow_test_stream_session("browser-pending", port).await;
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let proxy = listener.local_addr().unwrap();
            let (shutdown, rx) = watch::channel(false);
            let mut life = ListenerLife(rx);
            let router = server::router(app.handle().clone()).layer(Extension(life.clone()));
            let server = tokio::spawn(async move {
                axum::serve(listener, router)
                    .with_graceful_shutdown(async move { life.stopped().await })
                    .await
                    .unwrap();
            });

            let (mut client, mut upstream) = open_bridge(proxy, &daemon, "phone").await;
            forward_frame(&mut client, &mut upstream, "before approval").await;
            manager.park_test_stream_confirmation("browser-pending").await;
            assert_peer_closed(&mut client).await;
            assert_peer_closed(&mut upstream).await;

            let mut request = format!("ws://{proxy}/proxy/browser/{port}/ws")
                .into_client_request().unwrap();
            request.headers_mut().insert(header::COOKIE,
                format!("{}=phone", server::SESSION_COOKIE).parse().unwrap());
            let denied = tokio_tungstenite::client_async(request, TcpStream::connect(proxy).await.unwrap())
                .await.unwrap_err();
            assert!(matches!(denied, tungstenite::Error::Http(response) if response.status() == StatusCode::FORBIDDEN));

            shutdown.send(true).unwrap();
            server.await.unwrap();
        }).await.expect("approval must revoke existing and new browser bridges");
    }

    #[tokio::test]
    async fn listener_shutdown_before_upgrade_is_still_observed() {
        let (shutdown, receiver) = watch::channel(false);
        let mut life = ListenerLife(receiver);
        assert!(life.live());

        shutdown.send(true).unwrap();
        assert!(!life.live());
        tokio::time::timeout(Duration::from_secs(1), life.stopped())
            .await
            .expect("an upgrade on the old listener must observe its shutdown");
    }

    #[test]
    fn port_range_accepts_in_range_and_rejects_out() {
        // Endpoints and interior of the agent-browser range are accepted.
        assert!(port_in_range(9223));
        assert!(port_in_range(9260));
        assert!(port_in_range(9299));
        // Anything outside is rejected — no SSRF to arbitrary loopback ports.
        assert!(!port_in_range(9222));
        assert!(!port_in_range(9300));
        assert!(!port_in_range(0));
        assert!(!port_in_range(80));
        assert!(!port_in_range(4377)); // the web-remote server's own port
    }

    #[test]
    fn rest_path_control_chars_are_rejected() {
        // A percent-decoded CRLF in the wildcard path must be rejected so it
        // can't be smuggled into the raw request line sent to the daemon.
        assert!(has_control_char("status\r\nX-Injected: evil"));
        assert!(has_control_char("a\nb"));
        assert!(has_control_char("a\rb"));
        assert!(has_control_char("nul\0byte"));
        // Legitimate daemon paths carry no control characters.
        assert!(!has_control_char("status"));
        assert!(!has_control_char("command"));
        assert!(!has_control_char("eval/deep/path"));
    }

    #[test]
    fn port_range_low_bound_tracks_agent_browser() {
        assert_eq!(BROWSER_PORT_MIN, crate::agent_browser::DEFAULT_STREAM_PORT);
        assert!(BROWSER_PORT_MAX > BROWSER_PORT_MIN);
    }

    #[test]
    fn parse_response_extracts_status_type_and_body() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 11\r\nConnection: close\r\n\r\n{\"ok\":true}";
        let parsed = parse_http_response(raw, &Method::GET).expect("parses");
        assert_eq!(parsed.status, StatusCode::OK);
        assert_eq!(parsed.content_type.as_deref(), Some("application/json"));
        assert_eq!(parsed.body, b"{\"ok\":true}");
    }

    #[test]
    fn parse_response_preserves_error_status() {
        let raw = b"HTTP/1.1 404 Not Found\r\nContent-Type: text/plain\r\n\r\nnope";
        let parsed = parse_http_response(raw, &Method::GET).expect("parses");
        assert_eq!(parsed.status, StatusCode::NOT_FOUND);
        assert_eq!(parsed.body, b"nope");
    }

    #[test]
    fn parse_response_decodes_chunked_body() {
        let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n";
        let parsed = parse_http_response(raw, &Method::GET).expect("parses");
        assert_eq!(parsed.status, StatusCode::OK);
        assert_eq!(parsed.body, b"hello world");
    }

    #[test]
    fn parse_response_decodes_chunk_extensions_and_trailers() {
        let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5;source=test\r\nhello\r\n0\r\nX-Trace: complete\r\n\r\n";
        assert_eq!(parse_http_response(raw, &Method::GET).unwrap().body, b"hello");
    }

    #[test]
    fn parse_response_rejects_incomplete_or_malformed_chunks() {
        for body in [
            b"5\r\nhel".as_slice(),
            b"5\r\nhello\r\n",
            b"5\r\nhelloXX0\r\n\r\n",
            b"not-hex\r\nhello\r\n0\r\n\r\n",
            b"0\r\n",
        ] {
            let mut raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec();
            raw.extend_from_slice(body);
            assert!(
                parse_http_response(&raw, &Method::GET).is_none(),
                "accepted malformed chunk body {body:?}"
            );
        }
    }

    #[test]
    fn parse_response_rejects_garbage() {
        assert!(parse_http_response(b"not http at all", &Method::GET).is_none());
    }

    // Feed `raw` through a duplex pipe (writer drops → EOF) and read it back.
    async fn read_via_pipe(raw: &'static [u8]) -> Result<Vec<u8>, ()> {
        read_via_pipe_with_method(raw, &Method::GET).await
    }

    async fn read_via_pipe_with_method(raw: &'static [u8], method: &Method) -> Result<Vec<u8>, ()> {
        use tokio::io::AsyncWriteExt;
        let (mut w, mut r) = tokio::io::duplex(32);
        tokio::spawn(async move {
            let _ = w.write_all(raw).await;
            // drop `w` → EOF on `r`
        });
        read_response(&mut r, method).await
    }

    #[tokio::test]
    async fn read_response_uses_content_length_and_stops() {
        // Trailing bytes after the declared body must NOT be returned (and the
        // read must not block waiting for a close that may never come).
        let raw = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhelloEXTRA";
        let out = read_via_pipe(raw).await.expect("reads");
        let parsed = parse_http_response(&out, &Method::GET).expect("parses");
        assert_eq!(parsed.body, b"hello");
    }

    #[tokio::test]
    async fn read_response_falls_back_to_eof_when_unframed() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\r\nno length here";
        let out = read_via_pipe(raw).await.expect("reads");
        let parsed = parse_http_response(&out, &Method::GET).expect("parses");
        assert_eq!(parsed.body, b"no length here");
    }

    #[tokio::test]
    async fn read_response_errors_on_truncated_headers() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Type: text/pla"; // no header terminator
        assert!(read_via_pipe(raw).await.is_err());
    }

    #[tokio::test]
    async fn read_response_preserves_head_content_length_metadata() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Length: 123\r\n\r\n";
        let response = read_via_pipe_with_method(raw, &Method::HEAD).await.unwrap();
        let parsed = parse_http_response(&response, &Method::HEAD).unwrap();
        assert_eq!(parsed.status, StatusCode::OK);
        assert!(parsed.body.is_empty());
    }

    #[tokio::test]
    async fn read_response_preserves_not_modified_content_length_metadata() {
        let raw = b"HTTP/1.1 304 Not Modified\r\nContent-Length: 123\r\n\r\n";
        let response = read_via_pipe(raw).await.unwrap();
        let parsed = parse_http_response(&response, &Method::GET).unwrap();
        assert_eq!(parsed.status, StatusCode::NOT_MODIFIED);
        assert!(parsed.body.is_empty());
    }

    #[tokio::test]
    async fn read_response_preserves_bodyless_transfer_encoding_metadata() {
        for (raw, method) in [
            (b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n".as_slice(), Method::HEAD),
            (b"HTTP/1.1 304 Not Modified\r\nTransfer-Encoding: chunked\r\n\r\n", Method::GET),
        ] {
            let response = read_via_pipe_with_method(raw, &method).await.unwrap();
            assert!(parse_http_response(&response, &method).unwrap().body.is_empty());
        }
    }

    #[tokio::test]
    async fn read_response_finishes_no_content_without_waiting_for_close() {
        let (mut writer, mut reader) = tokio::io::duplex(128);
        writer.write_all(b"HTTP/1.1 204 No Content\r\n\r\n").await.unwrap();
        let response = tokio::time::timeout(
            Duration::from_secs(1),
            read_response(&mut reader, &Method::GET),
        ).await.expect("204 ends at its headers, even on a persistent connection").unwrap();
        let parsed = parse_http_response(&response, &Method::GET).unwrap();
        assert_eq!(parsed.status, StatusCode::NO_CONTENT);
        assert!(parsed.body.is_empty());
        drop(writer);
    }

    #[tokio::test]
    async fn read_response_rejects_truncated_content_length_body() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nshort";
        assert!(read_via_pipe(raw).await.is_err());
    }

    #[tokio::test]
    async fn read_response_rejects_overflowing_content_length() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Length: 18446744073709551615\r\n\r\n";
        assert!(read_via_pipe(raw).await.is_err());
    }

    #[tokio::test]
    async fn read_response_rejects_declared_body_above_safety_cap() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Length: 16777217\r\n\r\n";
        assert!(read_via_pipe(raw).await.is_err());
    }

    #[tokio::test]
    async fn read_response_rejects_invalid_content_length() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Length: invalid\r\n\r\nhello";
        assert!(read_via_pipe(raw).await.is_err());
    }

    #[tokio::test]
    async fn read_response_rejects_conflicting_content_lengths() {
        for raw in [
            b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nContent-Length: 10\r\n\r\nhello".as_slice(),
            b"HTTP/1.1 200 OK\r\nContent-Length: 5, 10\r\n\r\nhello",
        ] {
            assert!(read_via_pipe(raw).await.is_err());
        }
    }

    #[tokio::test]
    async fn read_response_accepts_repeated_identical_content_lengths() {
        for raw in [
            b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\nContent-Length: 5\r\n\r\nhello".as_slice(),
            b"HTTP/1.1 200 OK\r\nContent-Length: 5, 5\r\n\r\nhello",
        ] {
            let response = read_via_pipe(raw).await.unwrap();
            assert_eq!(parse_http_response(&response, &Method::GET).unwrap().body, b"hello");
        }
    }

    #[tokio::test]
    async fn read_response_rejects_unframed_body_beyond_existing_safety_cap() {
        let (mut writer, mut reader) = tokio::io::duplex(8192);
        let producer = tokio::spawn(async move {
            let _ = writer.write_all(b"HTTP/1.1 200 OK\r\n\r\n").await;
            let _ = writer.write_all(&vec![b'x'; (16 << 20) + 1]).await;
        });
        let result = read_response(&mut reader, &Method::GET).await;
        drop(reader);
        producer.await.unwrap();
        assert!(result.is_err());
    }

    #[test]
    fn ws_frame_conversions_are_binary_safe() {
        // Binary (screencast) bytes survive the axum → daemon hop unchanged.
        match axum_to_tungstenite(AxumMessage::Binary(vec![0, 1, 2, 255])).unwrap() {
            TungMessage::Binary(b) => assert_eq!(b.as_ref(), &[0, 1, 2, 255]),
            other => panic!("expected binary, got {other:?}"),
        }
        // Text (JSON input) survives the daemon → axum hop unchanged.
        match tungstenite_to_axum(TungMessage::Text("{\"t\":\"resize\"}".into())).unwrap() {
            AxumMessage::Text(s) => assert_eq!(s, "{\"t\":\"resize\"}"),
            other => panic!("expected text, got {other:?}"),
        }
        // Close maps to Close in both directions.
        assert!(matches!(
            axum_to_tungstenite(AxumMessage::Close(None)),
            Some(TungMessage::Close(None))
        ));
        assert!(matches!(
            tungstenite_to_axum(TungMessage::Close(None)),
            Some(AxumMessage::Close(None))
        ));
    }
}
