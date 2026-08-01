//! Headless end-to-end proof of the web-remote bandwidth optimisations
//! (issue #215): negotiated WebSocket frame compression, binary PTY framing,
//! and HTTP gzip.
//!
//! Boots the same `codemux_lib::build_headless_app()` the `codemux serve`
//! daemon runs, binds the web-remote server on loopback, pairs, and then drives
//! the three paths over real HTTP/WebSocket against 127.0.0.1:
//!
//!   1. a WS connection WITHOUT `compress=deflate` → text frames only, exactly
//!      as before this change (the backwards-compatibility guarantee),
//!   2. a WS connection WITH `compress=deflate` → `0x02`-tagged wrappers that
//!      inflate, through one streaming raw-deflate context, back to the same
//!      JSON the plain connection got,
//!   3. PTY output arriving as `[0x01]` binary channel frames instead of JSON
//!      number arrays — over the plain connection, and again over the
//!      compressed one where the same frames ride `0x03` wrappers,
//!   4. `GET /api/snapshot` with `Accept-Encoding: gzip` → a gzip body with
//!      `Vary: Accept-Encoding` that gunzips to the identity response,
//!   5. `/api/assets`: already-compressed media streams untouched with its
//!      `Content-Length`, while a compressible text asset is gzipped.
//!
//! The wire contract under test is documented in
//! `docs/features/web-remote-access.md` § "WS protocol contract".
//!
//! CRITICAL ISOLATION (identical to `serve_web_remote_roundtrip.rs`): a real
//! Codemux instance may be running on this machine, so `isolate_env` repoints
//! every state-dir resolver at a throwaway tempdir and defuses the boot side
//! effects BEFORE the app boots.
//!
//! Unix-only: `tauri::test` needs WebView2Loader.dll at process startup on
//! Windows (same gate as the other `serve_*` tests).

#![cfg(unix)]

use std::io::Read;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message;

/// The client callback id the test's `attach_pty_output` channel marker uses.
const PTY_CHANNEL_CB: u32 = 77;
/// The same, for the session attached over the compressed connection.
const ZIPPED_PTY_CHANNEL_CB: u32 = 91;

/// Point every state-dir resolver at `tmp` and defuse boot side effects a
/// headless test should not perform. Must run BEFORE `build_headless_app()`.
/// Mirrors `serve_web_remote_roundtrip.rs::isolate_env` verbatim.
fn isolate_env(tmp: &std::path::Path) {
    for (key, sub) in [
        ("HOME", ""),
        ("XDG_CONFIG_HOME", "config"),
        ("XDG_DATA_HOME", "data"),
        ("XDG_STATE_HOME", "state"),
        ("XDG_CACHE_HOME", "cache"),
        ("XDG_RUNTIME_DIR", "run"),
    ] {
        let dir = if sub.is_empty() {
            tmp.to_path_buf()
        } else {
            tmp.join(sub)
        };
        std::fs::create_dir_all(&dir).expect("create isolated state dir");
        std::env::set_var(key, &dir);
    }
    std::env::set_var("CODEMUX_DISABLE_PTY_DAEMON", "1");
    std::env::set_var("CODEMUX_API_URL", "http://127.0.0.1:1");
    std::env::remove_var("DISPLAY");
    std::env::remove_var("WAYLAND_DISPLAY");
}

/// Ask the OS for a currently-free loopback TCP port.
fn free_loopback_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .expect("bind ephemeral port")
        .local_addr()
        .expect("local addr")
        .port()
}

/// Poll `/api/health` until the listener accepts.
fn wait_healthy(client: &reqwest::blocking::Client, base: &str) {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(resp) = client.get(format!("{base}/api/health")).send() {
            if resp.status().is_success() {
                return;
            }
        }
        if std::time::Instant::now() > deadline {
            panic!("web-remote server at {base} never became healthy");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The client half of the compressed-frame contract: ONE streaming raw-deflate
/// context per connection, fed each `0x02`/`0x03` wrapper in arrival order. This
/// is the Rust mirror of what the browser shim does with fflate's synchronous
/// `Inflate` (`FrameInflater` in `src/remote/transport.ts`), and it only
/// produces correct output if the server really did keep one context and end
/// every message at a sync-flush boundary.
struct Inflater {
    inflate: flate2::Decompress,
}

impl Inflater {
    fn new() -> Self {
        Self {
            inflate: flate2::Decompress::new(false),
        }
    }

    /// Unwrap one compressed frame, returning `(tag, original_bytes)`.
    fn feed(&mut self, frame: &[u8]) -> (u8, Vec<u8>) {
        assert!(
            frame.len() >= 5,
            "a compressed frame is at least tag(1) + len(4) bytes, got {}",
            frame.len()
        );
        let tag = frame[0];
        assert!(
            tag == 0x02 || tag == 0x03,
            "compressed frames are tagged 0x02 (text) or 0x03 (binary), got {tag:#04x}"
        );
        let declared = u32::from_be_bytes(frame[1..5].try_into().unwrap()) as usize;
        let body = &frame[5..];
        let mut out = Vec::with_capacity(declared);
        let mut consumed = 0usize;
        while out.len() < declared {
            if out.len() == out.capacity() {
                out.reserve(declared.max(64));
            }
            let before_in = self.inflate.total_in();
            let before_out = self.inflate.total_out();
            self.inflate
                .decompress_vec(&body[consumed..], &mut out, flate2::FlushDecompress::Sync)
                .expect("compressed frame should inflate");
            consumed += (self.inflate.total_in() - before_in) as usize;
            // If the frame declares more bytes than it actually inflates to, the
            // loop would spin forever on an empty input slice. Stop on the first
            // no-progress iteration and let the length assertion below fail
            // loudly instead.
            if self.inflate.total_in() == before_in && self.inflate.total_out() == before_out {
                break;
            }
        }
        assert_eq!(
            out.len(),
            declared,
            "inflated length must match the declared u32 BE header"
        );
        (tag, out)
    }
}

/// How many `0x03` (wrapped binary channel) frames [`unwrap_compressed`] has
/// seen. PTY bursts routinely clear the 64-byte floor, so this is expected to be
/// non-zero by the end of the test — the assertion is what proves `0x03` was
/// exercised on a real wire rather than only in the unit tests.
static WRAPPED_CHANNEL_FRAMES: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

/// One logical S→C frame, with any compression wrapper already removed.
enum Frame {
    /// A JSON text frame (possibly delivered inside a `0x02` wrapper).
    Json(Value),
    /// A `[0x01][u32 BE cb][u64 BE idx][payload]` channel frame (possibly
    /// delivered inside a `0x03` wrapper).
    Channel { cb: u32, idx: u64, payload: Vec<u8> },
    /// Ping/Pong/Close — nothing this test asserts on.
    Other,
}

/// Decode a `[0x01][u32 BE cb][u64 BE idx][payload]` channel frame.
fn parse_channel_frame(bytes: &[u8]) -> Frame {
    assert_eq!(bytes.first(), Some(&0x01), "channel frames carry the 0x01 tag");
    assert!(
        bytes.len() >= 13,
        "a channel frame is at least tag(1) + cb(4) + idx(8) bytes, got {}",
        bytes.len()
    );
    Frame::Channel {
        cb: u32::from_be_bytes(bytes[1..5].try_into().unwrap()),
        idx: u64::from_be_bytes(bytes[5..13].try_into().unwrap()),
        payload: bytes[13..].to_vec(),
    }
}

/// Unwrap one message from the COMPRESSED connection, feeding every `0x02` /
/// `0x03` wrapper through that connection's single shared inflate context — in
/// arrival order, which is the contract under test. Also pins the 64-byte floor
/// in both directions: anything at or above it must have arrived wrapped, and
/// anything that arrived unwrapped must be below it.
fn unwrap_compressed(inflater: &mut Inflater, msg: Message) -> Frame {
    let json = |bytes: &[u8]| -> Frame {
        Frame::Json(serde_json::from_slice(bytes).expect("frame bytes should be JSON"))
    };
    match msg {
        Message::Text(text) => {
            assert!(
                text.len() < 64,
                "a {}-byte text frame is above the compression floor and must have \
                 arrived as a 0x02 wrapper",
                text.len()
            );
            json(text.as_bytes())
        }
        Message::Binary(bytes) => match bytes.first().copied() {
            Some(0x02) => {
                let (_, original) = inflater.feed(&bytes);
                json(&original)
            }
            Some(0x03) => {
                let (_, original) = inflater.feed(&bytes);
                assert!(
                    original.len() >= 64,
                    "only frames at or above the 64-byte floor are wrapped, got {}B",
                    original.len()
                );
                WRAPPED_CHANNEL_FRAMES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                parse_channel_frame(&original)
            }
            Some(0x01) => {
                assert!(
                    bytes.len() < 64,
                    "a {}-byte channel frame is above the compression floor and must \
                     have arrived as a 0x03 wrapper",
                    bytes.len()
                );
                parse_channel_frame(&bytes)
            }
            other => panic!("unexpected binary frame tag {other:?}"),
        },
        _ => Frame::Other,
    }
}

type Socket = tokio_tungstenite::WebSocketStream<
    tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
>;

/// Send `{"t":"invoke",…}` on a socket.
async fn send_invoke(socket: &mut Socket, id: u64, cmd: &str, args: Value) {
    socket
        .send(Message::Text(
            json!({ "t": "invoke", "id": id, "cmd": cmd, "args": args })
                .to_string()
                .into(),
        ))
        .await
        .unwrap_or_else(|e| panic!("send {cmd} invoke frame: {e}"));
}

/// Read the next frame, failing the test rather than hanging.
async fn next_frame(socket: &mut Socket, deadline: tokio::time::Instant) -> Message {
    let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
    tokio::time::timeout(remaining, socket.next())
        .await
        .expect("a frame should arrive before the timeout")
        .expect("WS stream should stay open")
        .expect("WS frame should read cleanly")
}

#[test]
fn web_remote_compresses_ws_frames_binary_frames_pty_and_gzips_http() {
    let tmp = tempfile::tempdir().expect("tempdir");
    isolate_env(tmp.path());

    let app = codemux_lib::build_headless_app().expect("headless serve app should build");
    let handle = app.handle().clone();

    // ── Enable on loopback on a free ephemeral port (bind-probe retry). ──
    let mut bound_port = 0u16;
    for _ in 0..12 {
        let port = free_loopback_port();
        match tauri::async_runtime::block_on(codemux_lib::web_remote::control_enable(
            &handle,
            Some("loopback".to_string()),
            Some(port),
        )) {
            Ok(result) => {
                bound_port = result.status.port;
                break;
            }
            Err(_) => continue,
        }
    }
    assert!(bound_port != 0, "should have bound on a free loopback port");

    let base = format!("http://127.0.0.1:{bound_port}");
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .expect("http client");
    wait_healthy(&client, &base);

    // ── Pair once; both sockets ride the same session. ──
    let token = codemux_lib::web_remote::control_pair(&handle, None)
        .expect("control_pair should mint a token")
        .token;
    let pair_body: Value = client
        .post(format!("{base}/api/pair"))
        .json(&json!({ "token": token }))
        .send()
        .expect("pair request")
        .json()
        .expect("pair response json");
    let session_token = pair_body["session_token"]
        .as_str()
        .expect("pair response carries a session_token")
        .to_string();

    let ws_ticket = || -> String {
        let body: Value = client
            .post(format!("{base}/api/ws-ticket"))
            .header("Authorization", format!("Bearer {session_token}"))
            .send()
            .expect("ws-ticket request")
            .json()
            .expect("ws-ticket response json");
        body["ticket"]
            .as_str()
            .expect("ws-ticket response carries a ticket")
            .to_string()
    };
    let plain_url = format!("ws://127.0.0.1:{bound_port}/ws?ticket={}", ws_ticket());
    let compressed_url = format!(
        "ws://127.0.0.1:{bound_port}/ws?ticket={}&compress=deflate",
        ws_ticket()
    );

    tauri::async_runtime::block_on(async move {
        // ── 1. Plain connection: text frames only, byte-identical to before. ──
        let (mut plain, _) = tokio_tungstenite::connect_async(&plain_url)
            .await
            .expect("plain WS upgrade should succeed");
        send_invoke(&mut plain, 1, "get_app_state", json!({})).await;

        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);

        let plain_state = loop {
            match next_frame(&mut plain, deadline).await {
                Message::Text(text) => {
                    let v: Value = serde_json::from_str(text.as_str()).expect("valid JSON frame");
                    if v["id"] == json!(1) {
                        assert_eq!(v["t"], json!("ok"), "get_app_state should resolve: {v}");
                        break v["data"].clone();
                    }
                }
                Message::Binary(bytes) => panic!(
                    "a connection that did not negotiate compression must never get \
                     a compressed frame (tag {:?})",
                    bytes.first()
                ),
                _ => {}
            }
        };
        assert!(
            plain_state.get("workspaces").is_some(),
            "plain connection should carry the app-state snapshot"
        );

        // ── 2. Compressed connection: the same reply as a 0x02 wrapper. ──
        let (mut zipped, _) = tokio_tungstenite::connect_async(&compressed_url)
            .await
            .expect("compressed WS upgrade should succeed");
        send_invoke(&mut zipped, 1, "get_app_state", json!({})).await;

        // Reaching past this loop is itself the proof that a `0x02` wrapper
        // arrived: it only breaks out of the binary arm.
        let mut inflater = Inflater::new();
        let compressed_state = loop {
            match next_frame(&mut zipped, deadline).await {
                Message::Binary(bytes) => {
                    let (tag, original) = inflater.feed(&bytes);
                    assert_eq!(tag, 0x02, "an app-state reply is a wrapped text frame");
                    assert!(
                        bytes.len() < original.len(),
                        "the app-state snapshot should shrink ({}B → {}B)",
                        original.len(),
                        bytes.len()
                    );
                    let v: Value =
                        serde_json::from_slice(&original).expect("inflated bytes are the JSON frame");
                    if v["id"] == json!(1) {
                        assert_eq!(v["t"], json!("ok"), "get_app_state should resolve: {v}");
                        break v["data"].clone();
                    }
                }
                // Small frames (under the 64-byte floor) still ride as text.
                Message::Text(text) => {
                    let v: Value = serde_json::from_str(text.as_str()).expect("valid JSON frame");
                    assert_ne!(
                        v["id"],
                        json!(1),
                        "the app-state reply is far above the compression floor: {v}"
                    );
                }
                _ => {}
            }
        };

        // The inflated payload is the same command's reply on the same app
        // instance. Compare the structure rather than the whole value: nothing
        // mutated the state between the two invokes, but a live snapshot is not
        // contractually stable, so pin the field set plus the workspace tree.
        let keys = |v: &Value| {
            let mut k: Vec<String> = v
                .as_object()
                .expect("app-state data is an object")
                .keys()
                .cloned()
                .collect();
            k.sort();
            k
        };
        assert_eq!(
            keys(&compressed_state),
            keys(&plain_state),
            "the inflated reply must be the same shape as the plain one"
        );
        assert_eq!(
            compressed_state["workspaces"], plain_state["workspaces"],
            "the inflated reply must carry the same workspace tree"
        );

        // ── 3. PTY output arrives as [0x01] binary channel frames. ──
        // A `Channel<Vec<u8>>` used to reach the browser as a JSON number array
        // (~3.4x the bytes); it is now re-encoded onto the compact binary frame.
        send_invoke(&mut plain, 2, "create_terminal_session", json!({})).await;
        let session_id = loop {
            match next_frame(&mut plain, deadline).await {
                Message::Text(text) => {
                    let v: Value = serde_json::from_str(text.as_str()).expect("valid JSON frame");
                    if v["id"] == json!(2) {
                        assert_eq!(
                            v["t"],
                            json!("ok"),
                            "create_terminal_session should resolve headlessly: {v}"
                        );
                        break v["data"].as_str().expect("session id string").to_string();
                    }
                }
                other => panic!("unexpected frame while creating a session: {other:?}"),
            }
        };

        send_invoke(
            &mut plain,
            3,
            "attach_pty_output",
            json!({ "channel": format!("__CHANNEL__:{PTY_CHANNEL_CB}"), "sessionId": session_id }),
        )
        .await;
        send_invoke(
            &mut plain,
            4,
            "write_to_pty",
            json!({ "data": "echo cmux-marker-215\n", "sessionId": session_id }),
        )
        .await;

        // Collect binary channel frames until the echoed marker shows up.
        let mut pty_bytes: Vec<u8> = Vec::new();
        let mut last_idx: Option<u64> = None;
        let mut frames = 0usize;
        loop {
            match next_frame(&mut plain, deadline).await {
                Message::Binary(bytes) => {
                    assert_eq!(bytes[0], 0x01, "PTY output rides the binary channel frame");
                    assert_eq!(
                        u32::from_be_bytes(bytes[1..5].try_into().unwrap()),
                        PTY_CHANNEL_CB,
                        "frames carry the browser's own callback id"
                    );
                    let idx = u64::from_be_bytes(bytes[5..13].try_into().unwrap());
                    if let Some(prev) = last_idx {
                        assert!(
                            idx > prev,
                            "channel idx must advance monotonically ({prev} → {idx})"
                        );
                    }
                    last_idx = Some(idx);
                    frames += 1;
                    pty_bytes.extend_from_slice(&bytes[13..]);
                    if String::from_utf8_lossy(&pty_bytes).contains("cmux-marker-215") {
                        break;
                    }
                }
                Message::Text(text) => {
                    let v: Value = serde_json::from_str(text.as_str()).expect("valid JSON frame");
                    assert_ne!(
                        v["t"],
                        json!("chan"),
                        "PTY bytes must not fall back to the JSON chan frame: {v}"
                    );
                    if v["id"] == json!(3) || v["id"] == json!(4) {
                        assert_eq!(v["t"], json!("ok"), "PTY setup invoke failed: {v}");
                    }
                }
                _ => {}
            }
        }
        assert!(frames > 0, "at least one binary PTY frame should arrive");

        // ── 3b. The same PTY path on the COMPRESSED connection: `0x03`
        // wrappers riding the connection's ONE ongoing deflate stream (the
        // `inflater` from step 2, deliberately reused — a fresh context here
        // would fail, which is the point). Small frames still ride as plain
        // `0x01`. This is the only place `0x03` is exercised on a real wire.
        send_invoke(&mut zipped, 2, "create_terminal_session", json!({})).await;
        let zipped_session = loop {
            match unwrap_compressed(&mut inflater, next_frame(&mut zipped, deadline).await) {
                Frame::Json(v) if v["id"] == json!(2) => {
                    assert_eq!(
                        v["t"],
                        json!("ok"),
                        "create_terminal_session should resolve headlessly: {v}"
                    );
                    break v["data"].as_str().expect("session id string").to_string();
                }
                _ => {}
            }
        };

        send_invoke(
            &mut zipped,
            3,
            "attach_pty_output",
            json!({
                "channel": format!("__CHANNEL__:{ZIPPED_PTY_CHANNEL_CB}"),
                "sessionId": zipped_session,
            }),
        )
        .await;
        send_invoke(
            &mut zipped,
            4,
            "write_to_pty",
            json!({ "data": "echo cmux-marker-215-zipped\n", "sessionId": zipped_session }),
        )
        .await;

        let mut zipped_pty: Vec<u8> = Vec::new();
        let mut zipped_last_idx: Option<u64> = None;
        let mut zipped_frames = 0usize;
        loop {
            match unwrap_compressed(&mut inflater, next_frame(&mut zipped, deadline).await) {
                Frame::Channel { cb, idx, payload } => {
                    assert_eq!(
                        cb, ZIPPED_PTY_CHANNEL_CB,
                        "frames carry the browser's own callback id"
                    );
                    if let Some(prev) = zipped_last_idx {
                        assert!(
                            idx > prev,
                            "channel idx must advance monotonically ({prev} → {idx})"
                        );
                    }
                    zipped_last_idx = Some(idx);
                    zipped_frames += 1;
                    zipped_pty.extend_from_slice(&payload);
                    if String::from_utf8_lossy(&zipped_pty).contains("cmux-marker-215-zipped") {
                        break;
                    }
                }
                Frame::Json(v) => {
                    assert_ne!(
                        v["t"],
                        json!("chan"),
                        "PTY bytes must not fall back to the JSON chan frame: {v}"
                    );
                    if v["id"] == json!(3) || v["id"] == json!(4) {
                        assert_eq!(v["t"], json!("ok"), "PTY setup invoke failed: {v}");
                    }
                }
                Frame::Other => {}
            }
        }
        assert!(
            zipped_frames > 0,
            "at least one PTY frame should arrive on the compressed connection"
        );
        assert!(
            WRAPPED_CHANNEL_FRAMES.load(std::sync::atomic::Ordering::Relaxed) > 0,
            "an echoed command's PTY burst clears the 64-byte floor, so at least one \
             channel frame must have arrived as a 0x03 wrapper"
        );
    });

    // ── 4. HTTP gzip on /api/snapshot. ──
    let authed = |accept_gzip: bool| -> reqwest::blocking::Response {
        let mut req = client
            .get(format!("{base}/api/snapshot"))
            .header("Authorization", format!("Bearer {session_token}"));
        if accept_gzip {
            req = req.header("Accept-Encoding", "gzip");
        }
        req.send().expect("snapshot request")
    };

    let identity = authed(false);
    assert!(identity.status().is_success());
    assert!(
        identity.headers().get("content-encoding").is_none(),
        "a client that does not accept gzip gets identity bytes"
    );
    let identity_body = identity.bytes().expect("identity body").to_vec();

    let gzipped = authed(true);
    assert!(gzipped.status().is_success());
    assert_eq!(
        gzipped
            .headers()
            .get("content-encoding")
            .and_then(|v| v.to_str().ok()),
        Some("gzip"),
        "an Accept-Encoding: gzip client should get a gzip body"
    );
    let vary = gzipped
        .headers()
        .get("vary")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_ascii_lowercase();
    assert!(
        vary.contains("accept-encoding"),
        "compressible routes must advertise Vary: Accept-Encoding, got {vary:?}"
    );
    let gzipped_body = gzipped.bytes().expect("gzip body").to_vec();
    assert!(
        gzipped_body.len() < identity_body.len(),
        "the gzip snapshot ({}B) should be smaller than identity ({}B)",
        gzipped_body.len(),
        identity_body.len()
    );

    let mut gunzipped = Vec::new();
    flate2::read::GzDecoder::new(&gzipped_body[..])
        .read_to_end(&mut gunzipped)
        .expect("gunzip the snapshot body");
    // Both responses are the same live snapshot of an untouched app, so the
    // envelope shape must match; compare the parsed values' key sets plus the
    // versioned envelope field the API contract pins.
    let identity_json: Value = serde_json::from_slice(&identity_body).expect("identity json");
    let gunzipped_json: Value = serde_json::from_slice(&gunzipped).expect("gunzipped json");
    assert_eq!(gunzipped_json["api_version"], identity_json["api_version"]);
    assert_eq!(
        gunzipped_json["app_state"]["workspaces"],
        identity_json["app_state"]["workspaces"],
        "the gunzipped body must be the same snapshot as the identity body"
    );

    // ── 5. /api/assets: media streams untouched, text is gzipped. ──
    let fetch_asset = |path: &std::path::Path| -> reqwest::blocking::Response {
        client
            .get(format!("{base}/api/assets"))
            .query(&[("path", path.to_string_lossy().to_string())])
            .header("Authorization", format!("Bearer {session_token}"))
            .header("Accept-Encoding", "gzip")
            .send()
            .expect("asset request")
    };

    // Already-compressed media: gzipping it would waste CPU and force a chunked
    // body, so tower-http's default predicate excludes `image/*` and the route
    // keeps streaming with the explicit Content-Length it set.
    let png = tmp.path().join("marker.png");
    std::fs::write(&png, vec![0x89u8; 4096]).expect("write a fake png");
    let asset = fetch_asset(&png);
    assert!(asset.status().is_success(), "asset should serve");
    assert_eq!(
        asset.headers().get("content-type").unwrap(),
        "image/png",
        "the asset route guesses the media type from the extension"
    );
    assert!(
        asset.headers().get("content-encoding").is_none(),
        "image/* must not be re-compressed"
    );
    assert_eq!(
        asset
            .headers()
            .get("content-length")
            .and_then(|v| v.to_str().ok()),
        Some("4096"),
        "an uncompressed asset keeps its Content-Length and stays streamable"
    );

    // A compressible text asset takes the other branch: the predicate says yes,
    // so the body ships gzipped and chunked with Content-Length dropped. Worth
    // pinning explicitly — the doc's "assets stream unchanged" only ever
    // applied to the predicate-excluded media types.
    let md = tmp.path().join("notes.md");
    let md_body = "# Codemux\n\nrepeatable line of markdown text\n".repeat(64);
    std::fs::write(&md, &md_body).expect("write a markdown asset");
    let text_asset = fetch_asset(&md);
    assert!(text_asset.status().is_success(), "markdown asset should serve");
    assert_eq!(
        text_asset
            .headers()
            .get("content-encoding")
            .and_then(|v| v.to_str().ok()),
        Some("gzip"),
        "a compressible text asset is gzipped like any other compressible body"
    );
    assert!(
        text_asset.headers().get("content-length").is_none(),
        "a gzipped asset trades its Content-Length for a chunked body"
    );
    let mut md_gunzipped = String::new();
    flate2::read::GzDecoder::new(&text_asset.bytes().expect("md body")[..])
        .read_to_string(&mut md_gunzipped)
        .expect("gunzip the markdown asset");
    assert_eq!(
        md_gunzipped, md_body,
        "the gzipped asset must gunzip to the exact file contents"
    );

    // Keep the backend alive for the whole test.
    drop(app);
}
