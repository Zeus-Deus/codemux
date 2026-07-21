//! Browser-pane proxy for the web-remote server.
//!
//! The embedded browser pane is a screenshot-driven Chromium session served by
//! a per-workspace `agent-browser` daemon bound to `127.0.0.1:<port>` (ports
//! 9223–9299; see `docs/features/browser.md` and
//! `src-tauri/src/agent_browser.rs`). The desktop pane talks to that daemon
//! directly: a WebSocket for the screencast + input events, and HTTP
//! `/api/status` / `/api/command` for liveness probes and evals
//! (`src/components/browser/{BrowserPane,stream-protocol}.ts`).
//!
//! A remote browser can't reach those loopback daemons, so this module bridges
//! them through the authenticated web-remote origin:
//!
//! - `GET /proxy/browser/:port/ws` upgrades the client socket and pipes it,
//!   frame-for-frame (binary-safe, backpressure-aware), to `ws://127.0.0.1:<port>`.
//! - `ANY /proxy/browser/:port/api/*rest` forwards the daemon's HTTP endpoints.
//!
//! ## The single-segment HTTP constraint
//!
//! The daemon reads each HTTP request with ONE peek and only sees the first
//! TCP segment (~1.4 KB); a request that spans segments looks like it has an
//! empty body (`docs/features/browser.md`). The HTTP forwarder therefore
//! rebuilds a MINIMAL request — request line, `Host`, and (only with a body)
//! `Content-Type` / `Content-Length`, plus `Connection: close` — and writes it
//! in a single `write_all`. Every inbound header (cookies, the session bearer,
//! `User-Agent`, `Accept*`, and all hop-by-hop headers) is dropped, both to
//! respect the size budget and to never leak the paired session's credentials
//! to the daemon.
//!
//! ## Port validation
//!
//! `:port` is validated strictly against the agent-browser range before any
//! connection is attempted; anything else is rejected. That keeps the proxy
//! from being turned into a general-purpose request forwarder against
//! arbitrary loopback ports.

use std::time::Duration;

use axum::{
    body::Bytes,
    extract::{
        ws::{Message as AxumMessage, WebSocket, WebSocketUpgrade},
        Path, State,
    },
    http::{header, HeaderMap, Method, StatusCode, Uri},
    response::{IntoResponse, Response},
};
use futures_util::{SinkExt, StreamExt};
use tauri::{AppHandle, Runtime};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio_tungstenite::tungstenite::Message as TungMessage;

/// Lowest agent-browser stream port. Sourced from the public constant so the
/// proxy range tracks the allocator's floor automatically.
const BROWSER_PORT_MIN: u16 = crate::agent_browser::DEFAULT_STREAM_PORT; // 9223
/// Highest agent-browser stream port. Mirrors the (private)
/// `agent_browser::MAX_STREAM_PORT`; kept in sync by this constant + the
/// `port_range_low_bound_tracks_agent_browser` test's floor assertion.
const BROWSER_PORT_MAX: u16 = 9299;

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

// ── WebSocket proxy ─────────────────────────────────────────────────

/// `GET /proxy/browser/:port/ws` — upgrade and bridge to the daemon socket.
pub async fn ws_proxy<R: Runtime>(
    State(app): State<AppHandle<R>>,
    Path(port): Path<u16>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    if let Err(resp) = super::server::require_session(&app, &headers) {
        return resp;
    }
    if !port_in_range(port) {
        return (StatusCode::BAD_REQUEST, "port out of range").into_response();
    }
    ws.on_upgrade(move |socket| bridge_ws(socket, port))
}

/// Bidirectionally pipe an upgraded client socket to the loopback daemon at
/// `port`. Both directions await each send, so a slow peer stalls the reader
/// on the other side (TCP backpressure) instead of buffering unboundedly.
async fn bridge_ws(client: WebSocket, port: u16) {
    let url = format!("ws://127.0.0.1:{port}");
    let daemon = match tokio_tungstenite::connect_async(&url).await {
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

/// `ANY /proxy/browser/:port/api/*rest` — forward the daemon's HTTP endpoints
/// (`/api/status`, `/api/command`) with a minimal, credential-free request.
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
    // Reject a percent-decoded CR/LF (or other control char) in the path before
    // it reaches the raw request line — no header/request smuggling to the
    // daemon. (`uri.query()` stays percent-encoded, so it needs no such guard.)
    if has_control_char(&rest) {
        return (StatusCode::BAD_REQUEST, "invalid path").into_response();
    }
    match forward(port, &method, &rest, uri.query(), &headers, &body).await {
        Ok(resp) => resp,
        Err(()) => (StatusCode::BAD_GATEWAY, "daemon unreachable").into_response(),
    }
}

/// Open a raw TCP connection to the daemon, write a minimal single-segment
/// request, and translate the response back into an axum `Response`.
async fn forward(
    port: u16,
    method: &Method,
    rest: &str,
    query: Option<&str>,
    headers: &HeaderMap,
    body: &[u8],
) -> Result<Response, ()> {
    let target = match query {
        Some(q) if !q.is_empty() => format!("/api/{rest}?{q}"),
        _ => format!("/api/{rest}"),
    };

    // Minimal request head. Deliberately NO cookies/authorization/user-agent/
    // accept — see the module note on the single-segment constraint and on
    // never leaking the paired session's credentials to the daemon.
    let mut head = format!(
        "{} {} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\n",
        method.as_str(),
        target,
        port
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

    let mut stream = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::net::TcpStream::connect(("127.0.0.1", port)),
    )
    .await
    .map_err(|_| ())?
    .map_err(|_| ())?;

    // One write_all → the whole request lands in a single loopback segment,
    // satisfying the daemon's single-peek read.
    stream.write_all(&wire).await.map_err(|_| ())?;
    let _ = stream.flush().await;

    let raw = tokio::time::timeout(Duration::from_secs(30), read_response(&mut stream))
        .await
        .map_err(|_| ())?
        .map_err(|_| ())?;

    let parsed = parse_http_response(&raw).ok_or(())?;
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
/// Without a `Content-Length` — chunked or close-delimited — it reads to EOF,
/// which the outer `Connection: close` guarantees terminates. A 30s timeout in
/// the caller backstops any pathological server.
async fn read_response<R: AsyncRead + Unpin>(stream: &mut R) -> Result<Vec<u8>, ()> {
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

    // 2. Prefer an explicit Content-Length; fall back to close-delimited.
    let content_length = {
        let head = String::from_utf8_lossy(&buf[..header_end]);
        head.split("\r\n").skip(1).find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.trim()
                .eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())
                .flatten()
        })
    };

    match content_length {
        Some(len) => {
            while buf.len() < body_start + len {
                let n = stream.read(&mut chunk).await.map_err(|_| ())?;
                if n == 0 {
                    break; // short read; return what we have
                }
                buf.extend_from_slice(&chunk[..n]);
            }
            let end = (body_start + len).min(buf.len());
            buf.truncate(end);
        }
        None => loop {
            let n = stream.read(&mut chunk).await.map_err(|_| ())?;
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
            if buf.len() > (16 << 20) {
                break; // 16 MiB safety cap for an unframed body
            }
        },
    }
    Ok(buf)
}

struct ParsedResponse {
    status: StatusCode,
    content_type: Option<String>,
    body: Vec<u8>,
}

/// Parse a raw HTTP/1.1 response into status + content-type + decoded body.
/// Handles the `Transfer-Encoding: chunked` case; otherwise the body is taken
/// verbatim (Content-Length or connection-close delimited).
fn parse_http_response(raw: &[u8]) -> Option<ParsedResponse> {
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

    let body = if chunked {
        dechunk(body_raw)
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
fn dechunk(mut data: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let Some(nl) = find_subsequence(data, b"\r\n") else {
            break;
        };
        // The size line may carry a `;ext` suffix; only the hex prefix counts.
        let size_line = std::str::from_utf8(&data[..nl]).unwrap_or("");
        let size_hex = size_line.split(';').next().unwrap_or("").trim();
        let Ok(size) = usize::from_str_radix(size_hex, 16) else {
            break;
        };
        data = &data[nl + 2..];
        if size == 0 {
            break; // final chunk
        }
        if data.len() < size {
            out.extend_from_slice(data);
            break;
        }
        out.extend_from_slice(&data[..size]);
        data = &data[size..];
        // Skip the CRLF that terminates each chunk's data.
        if data.starts_with(b"\r\n") {
            data = &data[2..];
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let parsed = parse_http_response(raw).expect("parses");
        assert_eq!(parsed.status, StatusCode::OK);
        assert_eq!(parsed.content_type.as_deref(), Some("application/json"));
        assert_eq!(parsed.body, b"{\"ok\":true}");
    }

    #[test]
    fn parse_response_preserves_error_status() {
        let raw = b"HTTP/1.1 404 Not Found\r\nContent-Type: text/plain\r\n\r\nnope";
        let parsed = parse_http_response(raw).expect("parses");
        assert_eq!(parsed.status, StatusCode::NOT_FOUND);
        assert_eq!(parsed.body, b"nope");
    }

    #[test]
    fn parse_response_decodes_chunked_body() {
        let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n";
        let parsed = parse_http_response(raw).expect("parses");
        assert_eq!(parsed.status, StatusCode::OK);
        assert_eq!(parsed.body, b"hello world");
    }

    #[test]
    fn parse_response_rejects_garbage() {
        assert!(parse_http_response(b"not http at all").is_none());
    }

    // Feed `raw` through a duplex pipe (writer drops → EOF) and read it back.
    async fn read_via_pipe(raw: &'static [u8]) -> Result<Vec<u8>, ()> {
        use tokio::io::AsyncWriteExt;
        let (mut w, mut r) = tokio::io::duplex(32);
        tokio::spawn(async move {
            let _ = w.write_all(raw).await;
            // drop `w` → EOF on `r`
        });
        read_response(&mut r).await
    }

    #[tokio::test]
    async fn read_response_uses_content_length_and_stops() {
        // Trailing bytes after the declared body must NOT be returned (and the
        // read must not block waiting for a close that may never come).
        let raw = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhelloEXTRA";
        let out = read_via_pipe(raw).await.expect("reads");
        let parsed = parse_http_response(&out).expect("parses");
        assert_eq!(parsed.body, b"hello");
    }

    #[tokio::test]
    async fn read_response_falls_back_to_eof_when_unframed() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\r\nno length here";
        let out = read_via_pipe(raw).await.expect("reads");
        let parsed = parse_http_response(&out).expect("parses");
        assert_eq!(parsed.body, b"no length here");
    }

    #[tokio::test]
    async fn read_response_errors_on_truncated_headers() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Type: text/pla"; // no header terminator
        assert!(read_via_pipe(raw).await.is_err());
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
