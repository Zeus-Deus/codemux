//! Invoke dispatch for the web-remote server.
//!
//! ## Strategy (Rust dispatch ladder, Option 1)
//!
//! A browser's `invoke(cmd, args)` frame is turned into a real
//! [`tauri::webview::InvokeRequest`] and driven through the main
//! window's webview via `WebviewWindow::on_message` — the same entry
//! point the desktop's own IPC uses. This reuses argument
//! deserialization, `State`/`AppHandle`/`Window` extraction, ACL
//! resolution, and error formatting for every one of the app's ~300
//! commands with zero per-command wiring. The `on_message` responder
//! forwards the [`InvokeResponse`] back over a oneshot; the WS task
//! awaits it and emits the id-matched `ok`/`err` frame. One tokio task
//! per invoke means responses may return out of order — exactly what
//! the protocol allows.
//!
//! ## Channels (the one thing on_message can't do for us)
//!
//! A `tauri::ipc::Channel` deserialized inside a synthesized invoke
//! would, by default, post its frames to the *desktop* webview's JS —
//! the wrong client. Tauri exposes a **channel interceptor**
//! (registered on the `Builder` in `lib.rs`) that fires for every
//! channel send with `(callback_id, index, body)`. Before dispatching,
//! we walk the invoke args and rewrite each `__CHANNEL__:<clientId>`
//! marker to a fresh **server-side** channel id, registering a route
//! `server_id → (this WS, clientId)` in [`ChannelRouter`]. When the
//! command later sends on that channel, the interceptor calls
//! [`ChannelRouter::route`], which serialises the body to the owning
//! WS as a `chan`/binary frame and returns `true` to suppress the
//! desktop delivery. This is uniform across every channel-taking
//! command (`attach_pty_output`, `attach_agent_chat_output`, …) and
//! needs no knowledge of their signatures — so it is unaffected by
//! concurrent changes to those commands' fan-out internals.
//!
//! ## Binary PTY framing
//!
//! One wrinkle: a `Channel<Vec<u8>>` is serialised by Tauri *before* the
//! interceptor sees it, so PTY bytes arrive as
//! `InvokeResponseBody::Json("[27,91,…]")` — a JSON number array roughly
//! 3.4x the size of the bytes it describes. The interceptor has no type
//! information to recover from, so the dispatcher supplies it: when it
//! rewrites a command's channel markers it already knows `cmd`, and it
//! tags the route as a raw byte stream for the commands in
//! [`is_raw_byte_stream`]. [`ChannelRouter::route`] then parses those
//! bodies back to `Vec<u8>` and emits the compact `[0x01]` binary frame
//! instead, falling back to the JSON `chan` frame if the body turns out
//! not to be a byte array. The desktop delivery path is untouched — this
//! is purely how the web transport encodes the same bytes.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use axum::extract::ws::Message;
use serde_json::{json, Value};
use tauri::ipc::{CallbackFn, InvokeBody, InvokeResponse, InvokeResponseBody};
use tauri::webview::InvokeRequest;
use tauri::{AppHandle, Manager, Runtime};

use super::server::OutboundTx;

/// The `@tauri-apps/api` `Channel` serialisation marker.
const CHANNEL_PREFIX: &str = "__CHANNEL__:";

/// Commands whose channel carries a genuine raw byte stream, so their
/// JSON-number-array bodies are worth re-encoding onto the compact `[0x01]`
/// binary frame (see the module note on binary PTY framing).
///
/// Only `attach_pty_output` qualifies: it is the app's single
/// `Channel<Vec<u8>>` command. The other channel-taking command,
/// `attach_agent_chat_output`, sends a structured `AgentChatEventPayload`
/// object — JSON is its natural encoding, not an expansion of bytes.
fn is_raw_byte_stream(cmd: &str) -> bool {
    matches!(cmd, "attach_pty_output")
}

/// Where the interceptor sends a given server-side channel's frames.
struct ChannelRoute {
    /// The callback id the browser's shim expects on its frames.
    client_cb: u32,
    /// Writer for the owning WS connection.
    out: OutboundTx,
    /// Owning connection — used to drop all of a socket's routes on close.
    conn_id: u64,
    /// Whether the owning command is one of [`is_raw_byte_stream`], i.e. its
    /// JSON bodies are byte arrays to be re-encoded as binary frames.
    raw_bytes: bool,
}

/// Maps server-allocated channel ids to their destination WS. Shared
/// between the dispatcher (which registers routes) and the channel
/// interceptor (which consumes them). One instance lives for the whole
/// app; the interceptor closure in `lib.rs` holds a clone.
#[derive(Default)]
pub struct ChannelRouter {
    routes: Mutex<HashMap<u32, ChannelRoute>>,
    next: AtomicU32,
    /// Live route count, kept in sync with `routes`. Lets the interceptor
    /// short-circuit without taking the lock when no web-remote channels
    /// are open — the desktop-only hot path (e.g. local PTY output) then
    /// pays nothing beyond one relaxed atomic load.
    active: AtomicUsize,
}

impl ChannelRouter {
    /// Allocate a fresh server-side channel id bound to a browser
    /// callback id on a specific connection. `raw_bytes` tags the route for
    /// binary re-encoding (see [`is_raw_byte_stream`]).
    fn alloc(&self, client_cb: u32, conn_id: u64, out: OutboundTx, raw_bytes: bool) -> u32 {
        // Start high to keep server ids clustered away from the low ids a
        // freshly booted desktop webview tends to mint first. (Collisions
        // are still only possible against a *currently live* server id and
        // are astronomically unlikely — see the module deviation note.)
        let server_cb = 0x4000_0000u32.wrapping_add(self.next.fetch_add(1, Ordering::SeqCst));
        let mut routes = self.routes.lock().unwrap();
        routes.insert(
            server_cb,
            ChannelRoute {
                client_cb,
                out,
                conn_id,
                raw_bytes,
            },
        );
        self.active.store(routes.len(), Ordering::Relaxed);
        server_cb
    }

    /// Interceptor entry point. Returns `true` when `server_cb` is one of
    /// ours (frame consumed and forwarded to its WS), `false` otherwise
    /// (a genuine desktop-webview channel — let Tauri deliver it).
    pub fn route(&self, server_cb: u32, idx: usize, body: &InvokeResponseBody) -> bool {
        // Fast path: no web channels open → never ours, don't touch the lock.
        if self.active.load(Ordering::Relaxed) == 0 {
            return false;
        }
        let routes = self.routes.lock().unwrap();
        let Some(route) = routes.get(&server_cb) else {
            return false;
        };
        let msg = match body {
            // Raw channel bodies (e.g. a command that sends
            // `tauri::ipc::Response` bytes) → compact binary frame.
            InvokeResponseBody::Raw(bytes) => {
                Message::Binary(binary_channel_frame(route.client_cb, idx, bytes))
            }
            // JSON channel bodies. For a route tagged as a raw byte stream the
            // JSON is a number array Tauri produced from a `Channel<Vec<u8>>`,
            // so decode it back to bytes and ship the binary frame; anything
            // else (and any body that isn't actually a byte array) keeps the
            // JSON `chan` frame.
            InvokeResponseBody::Json(s) => route
                .raw_bytes
                .then(|| serde_json::from_str::<Vec<u8>>(s).ok())
                .flatten()
                .map(|bytes| Message::Binary(binary_channel_frame(route.client_cb, idx, &bytes)))
                .unwrap_or_else(|| json_channel_frame(route.client_cb, idx, s)),
        };
        let _ = route.out.send(msg);
        true
    }

    /// Drop every route owned by a connection. Called when its WS closes —
    /// "channels die with their WS connection".
    pub fn remove_conn(&self, conn_id: u64) {
        let mut routes = self.routes.lock().unwrap();
        routes.retain(|_, r| r.conn_id != conn_id);
        self.active.store(routes.len(), Ordering::Relaxed);
    }
}

/// The compact binary channel frame:
///   `[0x01][u32 BE callbackId][u64 BE idx][payload]`
/// Shared by both paths that emit it so the raw and re-encoded-JSON bodies are
/// framed identically (same cb/idx encoding, `idx` forwarded verbatim).
fn binary_channel_frame(client_cb: u32, idx: usize, payload: &[u8]) -> Vec<u8> {
    let mut framed = Vec::with_capacity(1 + 4 + 8 + payload.len());
    framed.push(0x01u8);
    framed.extend_from_slice(&client_cb.to_be_bytes());
    framed.extend_from_slice(&(idx as u64).to_be_bytes());
    framed.extend_from_slice(payload);
    framed
}

/// `{"t":"chan","ch":<callbackId>,"idx":<n>,"data":<json>}` from a JSON body.
fn json_channel_frame(client_cb: u32, idx: usize, body: &str) -> Message {
    let data: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let frame = json!({
        "t": "chan",
        "ch": client_cb,
        "idx": idx,
        "data": data,
    });
    Message::Text(frame.to_string())
}

/// Recursively rewrite every `__CHANNEL__:<id>` string in `value` to a
/// server-side id and register its route. Handles top-level channel args
/// and channels nested inside object/array args alike. `raw_bytes` comes from
/// [`is_raw_byte_stream`] for the command being dispatched and tags every route
/// it opens.
fn rewrite_channel_markers(
    value: &mut Value,
    router: &Arc<ChannelRouter>,
    conn_id: u64,
    out: &OutboundTx,
    raw_bytes: bool,
) {
    match value {
        Value::String(s) => {
            if let Some(rest) = s.strip_prefix(CHANNEL_PREFIX) {
                if let Ok(client_cb) = rest.parse::<u32>() {
                    let server_cb = router.alloc(client_cb, conn_id, out.clone(), raw_bytes);
                    *s = format!("{CHANNEL_PREFIX}{server_cb}");
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                rewrite_channel_markers(item, router, conn_id, out, raw_bytes);
            }
        }
        Value::Object(map) => {
            for (_, v) in map.iter_mut() {
                rewrite_channel_markers(v, router, conn_id, out, raw_bytes);
            }
        }
        _ => {}
    }
}

/// Drive one invoke to completion and emit its response frame on `out`.
pub async fn dispatch_invoke<R: Runtime>(
    app: &AppHandle<R>,
    router: &Arc<ChannelRouter>,
    conn_id: u64,
    out: &OutboundTx,
    id: u64,
    cmd: String,
    mut args: Value,
) {
    // Route any channels this command opens back to *this* browser, tagging
    // them with whether this command's channel carries raw bytes.
    rewrite_channel_markers(&mut args, router, conn_id, out, is_raw_byte_stream(&cmd));

    let webview = match app.get_webview_window("main") {
        Some(w) => w,
        None => {
            let _ = out.send(err_text(id, "web-remote: main window unavailable"));
            return;
        }
    };
    // Reuse the main window's own origin so ACL resolves identically to a
    // desktop-initiated invoke of the same command.
    let url = match webview.url() {
        Ok(u) => u,
        Err(_) => {
            let _ = out.send(err_text(id, "web-remote: main window url unavailable"));
            return;
        }
    };

    let request = InvokeRequest {
        cmd,
        callback: CallbackFn(0),
        error: CallbackFn(1),
        url,
        body: InvokeBody::Json(args),
        headers: Default::default(),
        invoke_key: app.invoke_key().to_string(),
    };

    let (tx, rx) = tokio::sync::oneshot::channel::<InvokeResponse>();
    // Drive on_message off the async workers: a synchronous command runs
    // inline on this blocking thread, an async command is handed to
    // Tauri's runtime and resolves the responder later. Either way the
    // oneshot carries the result back.
    let _ = tokio::task::spawn_blocking(move || {
        webview.on_message(
            request,
            Box::new(move |_webview, _cmd, response, _callback, _error| {
                let _ = tx.send(response);
            }),
        );
    });

    let frame = match rx.await {
        Ok(InvokeResponse::Ok(body)) => ok_frame(id, body),
        Ok(InvokeResponse::Err(err)) => err_value(id, err.0),
        Err(_) => err_text(id, "web-remote: command dropped without responding"),
    };
    let _ = out.send(frame);
}

/// `{"t":"ok","id":…,"data":…}` from a resolved [`InvokeResponseBody`].
fn ok_frame(id: u64, body: InvokeResponseBody) -> Message {
    let data = match body {
        InvokeResponseBody::Json(s) => serde_json::from_str(&s).unwrap_or(Value::Null),
        // No app command returns a raw invoke body today; forward the bytes
        // as a JSON array so the shim can still reconstruct them if one ever
        // does. (Channel payloads — the real raw path — never come through
        // here; they go via the interceptor above.)
        InvokeResponseBody::Raw(bytes) => Value::Array(bytes.into_iter().map(|b| json!(b)).collect()),
    };
    Message::Text(json!({ "t": "ok", "id": id, "data": data }).to_string())
}

/// `{"t":"err","id":…,"error":<value>}` from a command rejection.
fn err_value(id: u64, error: Value) -> Message {
    Message::Text(json!({ "t": "err", "id": id, "error": error }).to_string())
}

/// `{"t":"err","id":…,"error":"<message>"}` for transport-level failures.
fn err_text(id: u64, message: &str) -> Message {
    Message::Text(json!({ "t": "err", "id": id, "error": message }).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    fn drain_text(rx: &mut mpsc::UnboundedReceiver<Message>) -> Value {
        match rx.try_recv().expect("a frame was sent") {
            Message::Text(s) => serde_json::from_str(&s).unwrap(),
            other => panic!("expected text frame, got {other:?}"),
        }
    }

    /// Register one route the way the dispatcher does and hand back the
    /// server-side id the interceptor would be called with.
    fn register_route(
        router: &Arc<ChannelRouter>,
        out: &OutboundTx,
        client_cb: u32,
        raw_bytes: bool,
    ) -> u32 {
        let mut args = json!({ "channel": format!("{CHANNEL_PREFIX}{client_cb}") });
        rewrite_channel_markers(&mut args, router, 1, out, raw_bytes);
        args["channel"]
            .as_str()
            .unwrap()
            .strip_prefix(CHANNEL_PREFIX)
            .unwrap()
            .parse()
            .unwrap()
    }

    #[test]
    fn rewrite_allocates_and_registers_top_level_channel() {
        let router = Arc::new(ChannelRouter::default());
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut args = json!({ "channel": "__CHANNEL__:7", "sessionId": "s1" });
        rewrite_channel_markers(&mut args, &router, 1, &tx, false);

        // The marker was rewritten to a server id (not the original 7).
        let rewritten = args["channel"].as_str().unwrap();
        assert!(rewritten.starts_with(CHANNEL_PREFIX));
        assert_ne!(rewritten, "__CHANNEL__:7");
        // Non-channel args are untouched.
        assert_eq!(args["sessionId"], json!("s1"));
        assert_eq!(router.routes.lock().unwrap().len(), 1);
    }

    #[test]
    fn rewrite_handles_nested_channels() {
        let router = Arc::new(ChannelRouter::default());
        let (tx, _rx) = mpsc::unbounded_channel();
        let mut args = json!({ "opts": { "cb": "__CHANNEL__:3" }, "list": ["__CHANNEL__:4"] });
        rewrite_channel_markers(&mut args, &router, 9, &tx, false);
        assert_eq!(router.routes.lock().unwrap().len(), 2);
    }

    #[test]
    fn route_json_body_emits_chan_frame_with_client_id_and_idx() {
        let router = Arc::new(ChannelRouter::default());
        let (tx, mut rx) = mpsc::unbounded_channel();
        let server_cb = register_route(&router, &tx, 42, false);

        // Simulate a channel send (as the interceptor would).
        let body = InvokeResponseBody::Json("[27,91,65]".to_string());
        assert!(router.route(server_cb, 5, &body));

        let frame = drain_text(&mut rx);
        assert_eq!(frame["t"], json!("chan"));
        assert_eq!(frame["ch"], json!(42), "client callback id preserved");
        assert_eq!(frame["idx"], json!(5));
        assert_eq!(frame["data"], json!([27, 91, 65]));
    }

    #[test]
    fn route_raw_body_emits_binary_frame() {
        let router = Arc::new(ChannelRouter::default());
        let (tx, mut rx) = mpsc::unbounded_channel();
        let server_cb = register_route(&router, &tx, 1, false);

        let body = InvokeResponseBody::Raw(vec![0xde, 0xad]);
        assert!(router.route(server_cb, 2, &body));

        match rx.try_recv().expect("a frame") {
            Message::Binary(bytes) => {
                assert_eq!(bytes[0], 0x01);
                assert_eq!(&bytes[1..5], &1u32.to_be_bytes()); // client cb id
                assert_eq!(&bytes[5..13], &2u64.to_be_bytes()); // idx
                assert_eq!(&bytes[13..], &[0xde, 0xad]);
            }
            other => panic!("expected binary frame, got {other:?}"),
        }
    }

    #[test]
    fn only_pty_output_is_tagged_as_a_raw_byte_stream() {
        // `attach_pty_output` is the app's single `Channel<Vec<u8>>` command.
        assert!(is_raw_byte_stream("attach_pty_output"));
        // The other channel-taking command sends a structured payload, so its
        // JSON body must keep the `chan` frame.
        assert!(!is_raw_byte_stream("attach_agent_chat_output"));
        assert!(!is_raw_byte_stream("get_app_state"));
        assert!(!is_raw_byte_stream(""));
    }

    #[test]
    fn tagged_route_reencodes_json_byte_array_as_a_binary_frame() {
        // The PTY path: Tauri serialised `Channel<Vec<u8>>` bytes to a number
        // array before the interceptor saw them, so the router decodes it back
        // and emits the same compact frame the raw path does.
        let router = Arc::new(ChannelRouter::default());
        let (tx, mut rx) = mpsc::unbounded_channel();
        let server_cb = register_route(&router, &tx, 42, true);

        let body = InvokeResponseBody::Json("[27,91,65]".to_string());
        assert!(router.route(server_cb, 5, &body));

        match rx.try_recv().expect("a frame") {
            Message::Binary(bytes) => {
                assert_eq!(bytes[0], 0x01);
                assert_eq!(&bytes[1..5], &42u32.to_be_bytes(), "client cb id");
                assert_eq!(&bytes[5..13], &5u64.to_be_bytes(), "idx forwarded verbatim");
                assert_eq!(&bytes[13..], &[27, 91, 65]);
                assert_eq!(bytes.len(), 1 + 4 + 8 + 3);
            }
            other => panic!("expected binary frame, got {other:?}"),
        }
    }

    #[test]
    fn untagged_route_keeps_the_json_chan_frame_for_the_same_body() {
        // Byte-identical behaviour for every command that isn't a raw byte
        // stream — the tag is the only thing that changes the encoding.
        let router = Arc::new(ChannelRouter::default());
        let (tx, mut rx) = mpsc::unbounded_channel();
        let server_cb = register_route(&router, &tx, 42, false);

        let body = InvokeResponseBody::Json("[27,91,65]".to_string());
        assert!(router.route(server_cb, 5, &body));

        let frame = drain_text(&mut rx);
        assert_eq!(frame["t"], json!("chan"));
        assert_eq!(frame["data"], json!([27, 91, 65]));
    }

    #[test]
    fn tagged_route_falls_back_to_chan_frame_for_non_byte_array_json() {
        // Defensive: if a tagged command's channel body shape ever changes (an
        // object, or numbers outside 0..=255), the JSON frame still goes out.
        let router = Arc::new(ChannelRouter::default());
        let (tx, mut rx) = mpsc::unbounded_channel();
        let server_cb = register_route(&router, &tx, 9, true);

        for body in [
            InvokeResponseBody::Json(r#"{"kind":"exit"}"#.to_string()),
            InvokeResponseBody::Json("[27,300]".to_string()),
            InvokeResponseBody::Json("not json at all".to_string()),
        ] {
            assert!(router.route(server_cb, 3, &body));
            let frame = drain_text(&mut rx);
            assert_eq!(frame["t"], json!("chan"));
            assert_eq!(frame["ch"], json!(9));
            assert_eq!(frame["idx"], json!(3), "idx preserved on the fallback too");
        }
    }

    #[test]
    fn route_unknown_id_is_ignored() {
        let router = Arc::new(ChannelRouter::default());
        let body = InvokeResponseBody::Json("null".to_string());
        assert!(!router.route(999, 0, &body), "unknown id must not be consumed");
    }

    #[test]
    fn remove_conn_drops_only_that_connections_routes() {
        let router = Arc::new(ChannelRouter::default());
        let (tx_a, _ra) = mpsc::unbounded_channel();
        let (tx_b, _rb) = mpsc::unbounded_channel();
        router.alloc(1, 100, tx_a, false);
        router.alloc(2, 200, tx_b, false);
        assert_eq!(router.routes.lock().unwrap().len(), 2);
        router.remove_conn(100);
        let routes = router.routes.lock().unwrap();
        assert_eq!(routes.len(), 1);
        assert!(routes.values().all(|r| r.conn_id == 200));
    }
}
