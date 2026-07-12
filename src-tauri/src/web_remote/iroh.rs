//! iroh transport for web remote access — the desktop half of the
//! "from-anywhere" tier.
//!
//! ## What this is
//!
//! The shipped web-remote server binds an axum `/ws` socket that a browser on
//! the same LAN or the user's own mesh VPN drives. That transport can't reach a
//! NAT'd desktop from a random network. This module adds a **parallel**
//! transport: the desktop becomes an [`iroh`] endpoint (a pure-Rust QUIC node),
//! so a browser can dial it by its stable [`iroh::EndpointId`] (the device's
//! `node_id`) over an E2E-encrypted QUIC stream — hole-punched directly when
//! possible, or forwarded as *ciphertext* through a relay of last resort (the
//! relay operator never sees plaintext). See
//! `docs/plans/web-remote-account-mode.md` (Design 1 §1.2, Stage C).
//!
//! It is strictly **additive and default-off**: nothing here runs unless the
//! new `web_remote.config.relay_mode_enabled` flag is on. The axum `/ws`
//! transport is unchanged and remains the default.
//!
//! ## The seam: same protocol, different pipe
//!
//! An iroh bi-directional stream carries the **exact same** `/ws` frame
//! contract (`invoke`/`ok`/`err`/`listen`/`event`/`chan` + binary PTY frames).
//! iroh replaces only the *transport*; every decoded control frame is fed into
//! [`super::server::handle_text_frame`] — the same dispatcher the WebSocket
//! reader loop uses — and outbound events / channel frames are the same
//! [`axum::extract::ws::Message`]s the [`super::dispatch::ChannelRouter`] and
//! [`super::events::EventHub`] already produce, re-encoded onto the wire. The
//! connection registers in the **shared** [`super::server::ConnectionRegistry`],
//! so revocation and `close_all` sever iroh sessions exactly as they do WS ones.
//!
//! ## Wire codec (kind-tagged, length-delimited)
//!
//! QUIC streams are boundary-less byte streams, so each logical frame is framed
//! as `[u8 kind][u32 BE len][payload]`:
//!   - `kind = 0` ([`KIND_TEXT`]): a UTF-8 JSON control frame (identical JSON to
//!     the `/ws` text frames, plus the handshake frames below).
//!   - `kind = 1` ([`KIND_BINARY`]): a raw binary payload — the same
//!     `[0x01][u32 BE ch][u64 BE idx][payload]` PTY frame the WS binary path
//!     carries. (The PTY frame is the *payload* of a kind-1 codec frame; the two
//!     framings nest, they don't collide.)
//! [`FrameDecoder`] reassembles discrete frames from arbitrary read chunks.
//!
//! ## Handshake / session over iroh
//!
//! WS tickets exist only to keep the long-lived session token out of the `/ws`
//! **URL** (and thus out of proxy/server logs). The iroh transport has no URL
//! and is E2E-encrypted end to end, so that concern is structurally absent —
//! the first frame presents the session token directly:
//!
//! ```text
//! C→S  kind 0  {"t":"hello","token":"<session_token>"}
//! S→C  kind 0  {"t":"welcome","session_id":"<id>"}          (on success)
//! S→C  kind 0  {"t":"unauthorized","reason":"<code>"}       (on rejection)
//! ```
//!
//! The token is authenticated by the **same** admission logic the HTTP session
//! gate uses ([`crate::web_remote::auth::authenticate`] → a non-revoked session,
//! then an approval check) — i.e. `require_session` minus the origin check,
//! which is moot on a cookieless, mutually-authenticated channel (the CSRF
//! concern it defends against cannot arise without ambient cookies; see the
//! design doc §1.3). Every subsequent frame rides that approved session.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use axum::extract::ws::Message;
use iroh::endpoint::{presets, Connection, RecvStream, SendStream};
use iroh::{Endpoint, SecretKey};
use serde_json::{json, Value};
use tauri::{AppHandle, Manager};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc, watch};

use super::Shared;
use crate::database::DatabaseStore;
use crate::web_remote::auth::{self, AuthedSession};

/// ALPN identifying this protocol on the QUIC handshake. Must match the browser
/// wasm client's ALPN. Versioned so a future incompatible framing can bump it.
pub const IROH_ALPN: &[u8] = b"codemux/web-remote/0";

/// File (under the app data dir) holding the machine-guarded iroh identity key.
const KEY_FILE: &str = "web-remote-iroh.key";

// ── Kind-tagged, length-delimited codec ─────────────────────────────

/// Control frame: a UTF-8 JSON payload (the `/ws` text-frame shapes).
pub const KIND_TEXT: u8 = 0;
/// Binary frame: a raw payload (the `[0x01]…` PTY framing).
pub const KIND_BINARY: u8 = 1;

/// Frame header: `[u8 kind][u32 BE len]`.
const HEADER_LEN: usize = 1 + 4;

/// Largest single frame accepted, matching axum's default 64 MiB WS message
/// cap. A declared length above this closes the connection rather than letting
/// a peer make the desktop allocate unbounded memory.
pub const MAX_FRAME_LEN: usize = 64 * 1024 * 1024;

/// One decoded wire frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub kind: u8,
    pub payload: Vec<u8>,
}

/// A fatal framing violation — the reader closes the connection on either.
#[derive(Debug, PartialEq, Eq)]
pub enum CodecError {
    /// A frame declared a length above [`MAX_FRAME_LEN`].
    FrameTooLarge(usize),
    /// A frame carried a kind byte that is neither [`KIND_TEXT`] nor
    /// [`KIND_BINARY`] — a desync or a hostile peer.
    UnknownKind(u8),
}

impl std::fmt::Display for CodecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CodecError::FrameTooLarge(n) => write!(f, "iroh frame too large: {n} bytes"),
            CodecError::UnknownKind(k) => write!(f, "iroh frame unknown kind: {k}"),
        }
    }
}

/// Encode one frame: `[kind][u32 BE len][payload]`.
pub fn encode_frame(kind: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_LEN + payload.len());
    out.push(kind);
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(payload);
    out
}

/// Incremental decoder that reassembles discrete [`Frame`]s from the
/// boundary-less QUIC byte stream. Feed it arbitrary read chunks with
/// [`push`](Self::push); pull complete frames with
/// [`next_frame`](Self::next_frame).
#[derive(Default)]
pub struct FrameDecoder {
    buf: Vec<u8>,
}

impl FrameDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append freshly read bytes.
    pub fn push(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
    }

    /// Pull the next complete frame, or `Ok(None)` if the buffer holds only a
    /// partial frame (the caller reads more and retries). A malformed header
    /// (unknown kind / oversized length) is a fatal [`CodecError`].
    pub fn next_frame(&mut self) -> Result<Option<Frame>, CodecError> {
        if self.buf.len() < HEADER_LEN {
            return Ok(None);
        }
        let kind = self.buf[0];
        if kind != KIND_TEXT && kind != KIND_BINARY {
            return Err(CodecError::UnknownKind(kind));
        }
        let len = u32::from_be_bytes([self.buf[1], self.buf[2], self.buf[3], self.buf[4]]) as usize;
        if len > MAX_FRAME_LEN {
            return Err(CodecError::FrameTooLarge(len));
        }
        if self.buf.len() < HEADER_LEN + len {
            return Ok(None);
        }
        let payload = self.buf[HEADER_LEN..HEADER_LEN + len].to_vec();
        self.buf.drain(..HEADER_LEN + len);
        Ok(Some(Frame { kind, payload }))
    }
}

/// Write one framed message to a QUIC send stream (or any async writer).
async fn write_frame<W: AsyncWrite + Unpin>(
    send: &mut W,
    kind: u8,
    payload: &[u8],
) -> std::io::Result<()> {
    let framed = encode_frame(kind, payload);
    send.write_all(&framed).await?;
    send.flush().await
}

/// Read the next complete frame from an async reader, driving the decoder and
/// refilling `readbuf` from the stream until a frame is available. `Ok(None)`
/// means the stream ended cleanly (EOF, possibly mid-frame — treated as close).
async fn read_frame<R: AsyncRead + Unpin>(
    recv: &mut R,
    decoder: &mut FrameDecoder,
    readbuf: &mut [u8],
) -> std::io::Result<Option<Frame>> {
    loop {
        match decoder.next_frame() {
            Ok(Some(frame)) => return Ok(Some(frame)),
            Ok(None) => {}
            Err(e) => {
                return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))
            }
        }
        let n = recv.read(readbuf).await?;
        if n == 0 {
            return Ok(None);
        }
        decoder.push(&readbuf[..n]);
    }
}

/// How an outbound [`Message`] maps onto the iroh wire. Keeps the writer loop's
/// dispatch pure and unit-testable.
enum MessageFrame<'a> {
    /// Encode as a codec frame of this kind + payload.
    Wire(u8, &'a [u8]),
    /// A close signal — stop the writer and finish the stream.
    Close,
    /// No wire representation (WS ping/pong; QUIC has its own keepalive).
    Ignore,
}

fn message_to_frame(msg: &Message) -> MessageFrame<'_> {
    match msg {
        Message::Text(s) => MessageFrame::Wire(KIND_TEXT, s.as_bytes()),
        Message::Binary(b) => MessageFrame::Wire(KIND_BINARY, b.as_slice()),
        Message::Close(_) => MessageFrame::Close,
        Message::Ping(_) | Message::Pong(_) => MessageFrame::Ignore,
    }
}

/// Drain a connection's outbound [`Message`] queue onto its QUIC send stream,
/// re-encoding each through the codec. The single owner of `send` — every
/// producer (invoke responses, channel router, event hub, the registry's Close)
/// feeds `out_rx`, exactly as the WS writer task works.
async fn writer_loop(mut send: SendStream, mut out_rx: mpsc::UnboundedReceiver<Message>) {
    while let Some(msg) = out_rx.recv().await {
        match message_to_frame(&msg) {
            MessageFrame::Wire(kind, payload) => {
                if write_frame(&mut send, kind, payload).await.is_err() {
                    break;
                }
            }
            MessageFrame::Close => break,
            MessageFrame::Ignore => {}
        }
    }
    // Half-close the stream so the peer sees a clean end.
    let _ = send.finish();
}

// ── Handshake / session admission ───────────────────────────────────

/// Why an iroh handshake was refused. Maps to the `unauthorized.reason` code
/// the client sees.
#[derive(Debug, PartialEq, Eq)]
pub enum HandshakeReject {
    /// The first frame was not a well-formed `{"t":"hello","token":…}` control
    /// frame.
    Malformed,
    /// The token resolved to no live (non-revoked) session.
    Unauthorized,
    /// The session exists but is still pending desktop approval.
    Pending,
}

impl HandshakeReject {
    fn code(&self) -> &'static str {
        match self {
            HandshakeReject::Malformed => "malformed_handshake",
            HandshakeReject::Unauthorized => "invalid_session",
            HandshakeReject::Pending => "pending_approval",
        }
    }
}

/// Admit an iroh handshake frame: parse `{"t":"hello","token":…}`, then resolve
/// the token to an **approved, non-revoked** session with the very same logic
/// the HTTP session gate uses ([`auth::authenticate`] + the approval check). The
/// origin check the HTTP gate also runs is deliberately omitted — it defends
/// against a cross-site page riding an ambient cookie, which cannot happen on
/// this cookieless, mutually-authenticated transport.
pub fn authorize_handshake(
    db: &DatabaseStore,
    frame: &Frame,
) -> Result<AuthedSession, HandshakeReject> {
    if frame.kind != KIND_TEXT {
        return Err(HandshakeReject::Malformed);
    }
    let value: Value =
        serde_json::from_slice(&frame.payload).map_err(|_| HandshakeReject::Malformed)?;
    if value.get("t").and_then(|v| v.as_str()) != Some("hello") {
        return Err(HandshakeReject::Malformed);
    }
    let token = value
        .get("token")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim();
    if token.is_empty() {
        return Err(HandshakeReject::Malformed);
    }
    let session = auth::authenticate(db, token).ok_or(HandshakeReject::Unauthorized)?;
    if !session.approved {
        return Err(HandshakeReject::Pending);
    }
    Ok(session)
}

fn reject_frame_bytes(reject: &HandshakeReject) -> Vec<u8> {
    json!({ "t": "unauthorized", "reason": reject.code() })
        .to_string()
        .into_bytes()
}

fn welcome_frame_bytes(session_id: &str) -> Vec<u8> {
    json!({ "t": "welcome", "session_id": session_id })
        .to_string()
        .into_bytes()
}

// ── Stable device identity (persisted iroh secret key) ──────────────

fn key_file_path() -> PathBuf {
    let data_dir = dirs::data_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".local/share"));
    data_dir.join(crate::APP_DIR_NAME).join(KEY_FILE)
}

/// Persist `key`'s 32 raw bytes, machine-guarded (the same AES-256-GCM
/// machine-id envelope the encrypted auth token uses) and `0600` on Unix, so
/// the device's iroh identity — and thus its `node_id` — is stable across
/// restarts and never sits in plaintext at rest.
fn persist_secret_key(path: &Path, key: &SecretKey) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let sealed = crate::auth::encrypt_data(&key.to_bytes())?;
    std::fs::write(path, &sealed).map_err(|e| format!("write iroh key: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

/// Load the persisted iroh identity, or generate + persist a fresh one. A
/// corrupt/short blob is treated as absent and regenerated (the endpoint still
/// comes up, with a new `node_id`).
fn load_or_create_secret_key() -> Result<SecretKey, String> {
    let path = key_file_path();
    if let Ok(sealed) = std::fs::read(&path) {
        if let Ok(bytes) = crate::auth::decrypt_data(&sealed) {
            if let Some(arr) = to_key_array(&bytes) {
                return Ok(SecretKey::from_bytes(&arr));
            }
        }
        eprintln!(
            "[codemux::web_remote] iroh identity key at {} unreadable; regenerating",
            path.display()
        );
    }
    let key = SecretKey::generate();
    persist_secret_key(&path, &key)?;
    Ok(key)
}

fn to_key_array(bytes: &[u8]) -> Option<[u8; 32]> {
    if bytes.len() != 32 {
        return None;
    }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(bytes);
    Some(arr)
}

/// The device's stable `node_id` (endpoint id) computed from the persisted
/// identity key **without** binding an endpoint. `None` when relay mode has
/// never been enabled (no key persisted yet).
pub fn persisted_node_id() -> Option<String> {
    let sealed = std::fs::read(key_file_path()).ok()?;
    let bytes = crate::auth::decrypt_data(&sealed).ok()?;
    let arr = to_key_array(&bytes)?;
    Some(SecretKey::from_bytes(&arr).public().to_string())
}

// ── Endpoint lifecycle manager ──────────────────────────────────────

/// A bound iroh endpoint plus its accept task. The task rides the app's tokio
/// runtime via `tauri::async_runtime::spawn`, so its handle is Tauri's.
struct RunningIroh {
    endpoint: Endpoint,
    accept: tauri::async_runtime::JoinHandle<()>,
}

/// Owns the (optional) running iroh endpoint. Lives inside [`Shared`] so the
/// iroh transport's lifecycle rides alongside the axum server's.
#[derive(Default)]
pub struct IrohManager {
    inner: Mutex<Option<RunningIroh>>,
    /// `node_id` of the running endpoint (cached so the command can answer
    /// without touching the endpoint).
    node_id: Mutex<Option<String>>,
}

impl IrohManager {
    pub fn is_running(&self) -> bool {
        self.inner.lock().unwrap().is_some()
    }

    /// The device's iroh `node_id`: the running endpoint's if up, else the
    /// persisted identity's, else `None` (relay mode never enabled).
    pub fn node_id(&self) -> Option<String> {
        if let Some(id) = self.node_id.lock().unwrap().clone() {
            return Some(id);
        }
        persisted_node_id()
    }

    fn install(&self, running: RunningIroh, node_id: String) {
        *self.node_id.lock().unwrap() = Some(node_id);
        *self.inner.lock().unwrap() = Some(running);
    }

    fn take(&self) -> Option<RunningIroh> {
        self.inner.lock().unwrap().take()
    }
}

/// Bind the desktop iroh endpoint and start accepting connections. Idempotent.
/// Runs on the app's existing tokio runtime — no second runtime is spawned.
pub(crate) async fn start(app: &AppHandle, shared: &Arc<Shared>) -> Result<(), String> {
    if shared.iroh.is_running() {
        return Ok(());
    }
    let key = load_or_create_secret_key()?;
    let endpoint = Endpoint::builder(presets::N0)
        .secret_key(key)
        .alpns(vec![IROH_ALPN.to_vec()])
        .bind()
        .await
        .map_err(|e| format!("iroh endpoint bind failed: {e}"))?;

    let node_id = endpoint.id().to_string();
    eprintln!("[codemux::web_remote] iroh relay transport enabled: node_id={node_id}");

    // Log the home relay once one is established (proof the relay accepted us).
    // Non-blocking: enabling must not wait on the relay handshake.
    {
        let ep = endpoint.clone();
        tauri::async_runtime::spawn(async move {
            ep.online().await;
            let relay = ep
                .addr()
                .relay_urls()
                .next()
                .map(|u| u.to_string())
                .unwrap_or_else(|| "<none>".to_string());
            eprintln!(
                "[codemux::web_remote] iroh home relay for node {}: {relay}",
                ep.id()
            );
        });
    }

    let accept = {
        let app = app.clone();
        let shared = shared.clone();
        let ep = endpoint.clone();
        tauri::async_runtime::spawn(accept_loop(app, shared, ep))
    };

    shared.iroh.install(RunningIroh { endpoint, accept }, node_id);
    Ok(())
}

/// Stop the iroh endpoint (abort its accept loop and close it gracefully). Live
/// iroh sessions are severed separately via the shared [`ConnectionRegistry`]
/// (`close_all`), exactly like the WS transport — this just stops accepting new
/// connections and releases the endpoint. Safe to call when not running.
///
/// [`ConnectionRegistry`]: super::server::ConnectionRegistry
pub(crate) fn stop(shared: &Arc<Shared>) {
    if let Some(running) = shared.iroh.take() {
        running.accept.abort();
        let endpoint = running.endpoint;
        tauri::async_runtime::spawn(async move {
            endpoint.close().await;
        });
    }
}

// ── Accept + connection bridge ──────────────────────────────────────

/// Accept incoming iroh connections until the endpoint closes.
async fn accept_loop(app: AppHandle, shared: Arc<Shared>, endpoint: Endpoint) {
    while let Some(incoming) = endpoint.accept().await {
        let app = app.clone();
        let shared = shared.clone();
        tauri::async_runtime::spawn(async move {
            match incoming.await {
                Ok(conn) => handle_connection(app, shared, conn).await,
                Err(e) => eprintln!("[codemux::web_remote] iroh incoming connection failed: {e}"),
            }
        });
    }
}

/// One accepted iroh connection. A connection may carry several bi-streams;
/// each bi-stream is an independent web-remote "socket" (its own handshake, its
/// own registry entry), mirroring how each WS upgrade is one socket.
async fn handle_connection(app: AppHandle, shared: Arc<Shared>, conn: Connection) {
    loop {
        match conn.accept_bi().await {
            Ok((send, recv)) => {
                let app = app.clone();
                let shared = shared.clone();
                tauri::async_runtime::spawn(handle_bi_stream(app, shared, send, recv));
            }
            // The connection closed (or errored) — stop accepting its streams.
            Err(_) => break,
        }
    }
}

/// Bridge one iroh bi-stream into the shared web-remote connection handling:
/// handshake → register → writer + ordered lane → reader loop → teardown. This
/// is the iroh sibling of [`super::server`]'s `handle_socket`; the frame
/// dispatch, channel routing, event fan-out, and registry teardown are all the
/// same shared code.
async fn handle_bi_stream(
    app: AppHandle,
    shared: Arc<Shared>,
    mut send: SendStream,
    mut recv: RecvStream,
) {
    let mut decoder = FrameDecoder::new();
    let mut readbuf = vec![0u8; 16 * 1024];

    // 1. Handshake: the first frame must present a valid, approved session.
    let handshake = match read_frame(&mut recv, &mut decoder, &mut readbuf).await {
        Ok(Some(f)) => f,
        _ => return, // stream ended / codec error before a handshake arrived
    };
    let session = {
        let db = app.state::<DatabaseStore>();
        authorize_handshake(&db, &handshake)
    };
    let session_id = match session {
        Ok(s) => s.id,
        Err(reject) => {
            let _ = write_frame(&mut send, KIND_TEXT, &reject_frame_bytes(&reject)).await;
            let _ = send.finish();
            return;
        }
    };

    // 2. Register in the SHARED registry the WS transport + revocation use, so
    //    close_session / close_all sever this iroh session too.
    let (out_tx, out_rx) = mpsc::unbounded_channel::<Message>();
    let (close_tx, mut close_rx) = watch::channel(false);
    let conn_id = shared
        .connections
        .register(&session_id, out_tx.clone(), close_tx.clone());
    super::emit_state_changed(&app);

    // 3. Acknowledge, then hand the send half to the writer task.
    let _ = write_frame(&mut send, KIND_TEXT, &welcome_frame_bytes(&session_id)).await;
    let writer = tauri::async_runtime::spawn(writer_loop(send, out_rx));

    // 4. Ordered dispatch lane (terminal input keeps client-send order), exactly
    //    as the WS reader does — `handle_text_frame` decides what routes here.
    let (ordered_tx, mut ordered_rx) = mpsc::unbounded_channel::<(u64, String, Value)>();
    let ordered_task = {
        let app = app.clone();
        let router = shared.channels.clone();
        let out = out_tx.clone();
        tauri::async_runtime::spawn(async move {
            while let Some((id, cmd, args)) = ordered_rx.recv().await {
                super::dispatch::dispatch_invoke(&app, &router, conn_id, &out, id, cmd, args).await;
            }
        })
    };

    // 5. Reader loop: dispatch client frames until close / EOF / revocation.
    loop {
        tokio::select! {
            changed = close_rx.changed() => {
                if changed.is_err() || *close_rx.borrow() {
                    break;
                }
            }
            frame = read_frame(&mut recv, &mut decoder, &mut readbuf) => {
                match frame {
                    Ok(Some(f)) if f.kind == KIND_TEXT => {
                        let txt = String::from_utf8_lossy(&f.payload);
                        super::server::handle_text_frame(
                            &app, &shared, conn_id, &out_tx, &ordered_tx, &txt,
                        );
                    }
                    // Inbound binary frames: the client never sends these today
                    // (PTY input rides a `write_to_pty` invoke), so ignore them
                    // just as the WS reader ignores inbound `Binary` messages.
                    Ok(Some(_)) => {}
                    Ok(None) | Err(_) => break,
                }
            }
        }
    }

    // 6. Teardown — channels + event subscriptions die with the stream, same as
    //    the WS lifecycle.
    shared.connections.unregister(conn_id);
    shared.channels.remove_conn(conn_id);
    shared.events.remove_conn(&app, conn_id);
    ordered_task.abort();
    let _ = close_tx.send(true);
    drop(out_tx);
    writer.abort();
    super::emit_state_changed(&app);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::init_test_database;
    use crate::web_remote::auth::{random_token, sha256_hex};

    // ── Codec: encode / decode / framing / partial reads / kind dispatch ──

    #[test]
    fn encode_frame_layout_is_kind_len_payload() {
        let framed = encode_frame(KIND_TEXT, b"hello");
        assert_eq!(framed[0], KIND_TEXT);
        assert_eq!(&framed[1..5], &5u32.to_be_bytes());
        assert_eq!(&framed[5..], b"hello");

        let empty = encode_frame(KIND_BINARY, b"");
        assert_eq!(empty[0], KIND_BINARY);
        assert_eq!(&empty[1..5], &0u32.to_be_bytes());
        assert_eq!(empty.len(), HEADER_LEN);
    }

    #[test]
    fn decoder_reassembles_two_concatenated_frames() {
        let mut dec = FrameDecoder::new();
        dec.push(&encode_frame(KIND_TEXT, br#"{"t":"invoke"}"#));
        dec.push(&encode_frame(KIND_BINARY, &[0x01, 0xAA, 0xBB]));

        let a = dec.next_frame().unwrap().unwrap();
        assert_eq!(a.kind, KIND_TEXT);
        assert_eq!(a.payload, br#"{"t":"invoke"}"#);
        let b = dec.next_frame().unwrap().unwrap();
        assert_eq!(b.kind, KIND_BINARY);
        assert_eq!(b.payload, vec![0x01, 0xAA, 0xBB]);
        assert!(dec.next_frame().unwrap().is_none(), "buffer drained");
    }

    #[test]
    fn decoder_handles_byte_at_a_time_partial_reads() {
        // Feeding the wire one byte at a time must never yield a frame early and
        // must yield exactly the whole frame once the last byte lands — the
        // property that makes reassembly across QUIC packet boundaries correct.
        let payload = br#"{"t":"ok","id":1,"data":null}"#;
        let framed = encode_frame(KIND_TEXT, payload);
        let mut dec = FrameDecoder::new();
        for (i, byte) in framed.iter().enumerate() {
            dec.push(&[*byte]);
            if i + 1 < framed.len() {
                assert!(
                    dec.next_frame().unwrap().is_none(),
                    "no frame before the final byte (had {} of {})",
                    i + 1,
                    framed.len()
                );
            }
        }
        let frame = dec.next_frame().unwrap().unwrap();
        assert_eq!(frame.kind, KIND_TEXT);
        assert_eq!(frame.payload, payload);
    }

    #[test]
    fn decoder_splits_frame_arriving_in_two_chunks() {
        let framed = encode_frame(KIND_BINARY, &[1, 2, 3, 4, 5, 6, 7, 8]);
        let (head, tail) = framed.split_at(4);
        let mut dec = FrameDecoder::new();
        dec.push(head);
        assert!(dec.next_frame().unwrap().is_none(), "header+partial not enough");
        dec.push(tail);
        let frame = dec.next_frame().unwrap().unwrap();
        assert_eq!(frame.payload, vec![1, 2, 3, 4, 5, 6, 7, 8]);
    }

    #[test]
    fn decoder_rejects_unknown_kind() {
        let mut dec = FrameDecoder::new();
        dec.push(&[0x09, 0, 0, 0, 1, 0xFF]); // kind 9 is neither text nor binary
        assert_eq!(dec.next_frame(), Err(CodecError::UnknownKind(0x09)));
    }

    #[test]
    fn decoder_rejects_oversized_frame() {
        let mut dec = FrameDecoder::new();
        let too_big = (MAX_FRAME_LEN as u32) + 1;
        let mut header = vec![KIND_TEXT];
        header.extend_from_slice(&too_big.to_be_bytes());
        dec.push(&header);
        assert_eq!(
            dec.next_frame(),
            Err(CodecError::FrameTooLarge(too_big as usize))
        );
    }

    #[test]
    fn message_to_frame_maps_kinds_and_control() {
        match message_to_frame(&Message::Text("hi".into())) {
            MessageFrame::Wire(KIND_TEXT, p) => assert_eq!(p, b"hi"),
            _ => panic!("text must map to a kind-0 wire frame"),
        }
        match message_to_frame(&Message::Binary(vec![0x01, 0x02])) {
            MessageFrame::Wire(KIND_BINARY, p) => assert_eq!(p, &[0x01, 0x02]),
            _ => panic!("binary must map to a kind-1 wire frame"),
        }
        assert!(matches!(
            message_to_frame(&Message::Close(None)),
            MessageFrame::Close
        ));
        assert!(matches!(
            message_to_frame(&Message::Ping(vec![])),
            MessageFrame::Ignore
        ));
        assert!(matches!(
            message_to_frame(&Message::Pong(vec![])),
            MessageFrame::Ignore
        ));
    }

    #[tokio::test]
    async fn write_then_read_roundtrips_both_kinds_byte_exact() {
        // The codec over a real async pipe (tokio duplex): a control frame and a
        // byte-exact PTY frame survive write + read unchanged. The writer runs
        // concurrently and the duplex buffer is deliberately tiny (32 bytes) with
        // an 8-byte read window, so both frames are forced to stream in fragments
        // — exercising `FrameDecoder` reassembly across chunk boundaries.
        let (mut a, mut b) = tokio::io::duplex(32);
        let control = br#"{"t":"invoke","id":7,"cmd":"ping"}"#.to_vec();
        let pty = pty_frame(9, 42, b"hello-terminal\x1b[0m");

        let writer = {
            let control = control.clone();
            let pty = pty.clone();
            tokio::spawn(async move {
                write_frame(&mut a, KIND_TEXT, &control).await.unwrap();
                write_frame(&mut a, KIND_BINARY, &pty).await.unwrap();
            })
        };

        let mut dec = FrameDecoder::new();
        let mut buf = vec![0u8; 8];
        let f1 = read_frame(&mut b, &mut dec, &mut buf).await.unwrap().unwrap();
        let f2 = read_frame(&mut b, &mut dec, &mut buf).await.unwrap().unwrap();
        writer.await.unwrap();

        assert_eq!(f1.kind, KIND_TEXT);
        assert_eq!(f1.payload, control);
        assert_eq!(f2.kind, KIND_BINARY);
        assert_eq!(f2.payload, pty, "PTY frame is byte-exact through the codec");
    }

    // ── Stable device identity ────────────────────────────────────────

    #[test]
    fn secret_key_bytes_roundtrip_preserves_node_id() {
        // The device's node_id must be STABLE across restarts: persisting the
        // 32 secret-key bytes and reloading them has to reproduce the exact same
        // public key (= node_id). This is the invariant `persist_secret_key` /
        // `load_or_create_secret_key` rely on, checked without touching the disk.
        let key = SecretKey::generate();
        let node_id = key.public().to_string();

        let bytes = key.to_bytes();
        let arr = to_key_array(&bytes).expect("32 bytes");
        let reloaded = SecretKey::from_bytes(&arr);
        assert_eq!(
            reloaded.public().to_string(),
            node_id,
            "reloaded identity yields the same node_id"
        );

        // A wrong-length blob is rejected (treated as absent → regenerate).
        assert!(to_key_array(&[0u8; 31]).is_none());
        assert!(to_key_array(&[0u8; 33]).is_none());
    }

    // ── Handshake / session gate ──────────────────────────────────────

    fn hello(token: &str) -> Frame {
        Frame {
            kind: KIND_TEXT,
            payload: json!({ "t": "hello", "token": token }).to_string().into_bytes(),
        }
    }

    fn insert_session(db: &DatabaseStore, approved: bool) -> String {
        let token = random_token();
        db.web_remote_insert_session("iroh-sess", Some("Browser"), None, &sha256_hex(&token), approved)
            .unwrap();
        token
    }

    #[test]
    fn handshake_admits_valid_approved_session() {
        let db = init_test_database();
        let token = insert_session(&db, true);
        let session = authorize_handshake(&db, &hello(&token)).expect("valid token admitted");
        assert_eq!(session.id, "iroh-sess");
        assert!(session.approved);
    }

    #[test]
    fn handshake_rejects_unknown_token() {
        let db = init_test_database();
        insert_session(&db, true);
        assert_eq!(
            authorize_handshake(&db, &hello("not-a-real-token")).unwrap_err(),
            HandshakeReject::Unauthorized
        );
    }

    #[test]
    fn handshake_rejects_pending_session() {
        let db = init_test_database();
        let token = insert_session(&db, false); // not yet approved
        assert_eq!(
            authorize_handshake(&db, &hello(&token)).unwrap_err(),
            HandshakeReject::Pending
        );
    }

    #[test]
    fn handshake_rejects_revoked_session() {
        let db = init_test_database();
        let token = insert_session(&db, true);
        db.web_remote_revoke_session("iroh-sess").unwrap();
        assert_eq!(
            authorize_handshake(&db, &hello(&token)).unwrap_err(),
            HandshakeReject::Unauthorized
        );
    }

    #[test]
    fn handshake_rejects_malformed_and_wrong_kind() {
        let db = init_test_database();
        // Not JSON.
        assert_eq!(
            authorize_handshake(&db, &Frame { kind: KIND_TEXT, payload: b"nonsense".to_vec() })
                .unwrap_err(),
            HandshakeReject::Malformed
        );
        // Wrong `t`.
        let wrong_t = Frame {
            kind: KIND_TEXT,
            payload: json!({ "t": "invoke" }).to_string().into_bytes(),
        };
        assert_eq!(authorize_handshake(&db, &wrong_t).unwrap_err(), HandshakeReject::Malformed);
        // Empty token.
        assert_eq!(
            authorize_handshake(&db, &hello("")).unwrap_err(),
            HandshakeReject::Malformed
        );
        // A binary first frame can never be a handshake.
        let bin = Frame { kind: KIND_BINARY, payload: vec![0x01, 0x02] };
        assert_eq!(authorize_handshake(&db, &bin).unwrap_err(), HandshakeReject::Malformed);
    }

    // ── Real iroh transport: end-to-end over a direct loopback path ───
    //
    // Stands up the production-shape server bridge (handshake + session gate +
    // codec + shared ConnectionRegistry + the real writer_loop) on one iroh
    // endpoint and a native iroh CLIENT on another, connected DIRECTLY over
    // loopback (no public relay — the `Minimal` preset has none), then:
    //   - proves a valid session handshake is admitted and a bad one rejected,
    //   - round-trips a control frame AND a byte-exact PTY frame, and
    //   - proves revocation via the shared registry severs the live iroh stream.
    //
    // The only substitution vs. the production `handle_bi_stream` is the frame
    // handler: this echoes decoded frames back (the real handler routes them to
    // the webview-coupled invoke dispatch, which no headless test can exercise).
    // Every load-bearing transport primitive — `authorize_handshake`,
    // `read_frame`/`write_frame`, `writer_loop`, `FrameDecoder`, and the shared
    // `ConnectionRegistry` — is the exact production code.

    /// Build the PTY binary frame shape `[0x01][u32 BE ch][u64 BE idx][payload]`
    /// (the payload of a kind-1 codec frame).
    fn pty_frame(ch: u32, idx: u64, payload: &[u8]) -> Vec<u8> {
        let mut v = Vec::with_capacity(1 + 4 + 8 + payload.len());
        v.push(0x01);
        v.extend_from_slice(&ch.to_be_bytes());
        v.extend_from_slice(&idx.to_be_bytes());
        v.extend_from_slice(payload);
        v
    }

    /// The server-side bridge for the test: real handshake gate + registry +
    /// writer_loop, with an echo frame handler in place of invoke dispatch.
    async fn serve_echo_stream(
        shared: Arc<Shared>,
        db: Arc<DatabaseStore>,
        mut send: SendStream,
        mut recv: RecvStream,
    ) {
        let mut decoder = FrameDecoder::new();
        let mut readbuf = vec![0u8; 16 * 1024];

        let handshake = match read_frame(&mut recv, &mut decoder, &mut readbuf).await {
            Ok(Some(f)) => f,
            _ => return,
        };
        let session_id = match authorize_handshake(&db, &handshake) {
            Ok(s) => s.id,
            Err(reject) => {
                let _ = write_frame(&mut send, KIND_TEXT, &reject_frame_bytes(&reject)).await;
                let _ = send.finish();
                return;
            }
        };

        let (out_tx, out_rx) = mpsc::unbounded_channel::<Message>();
        let (close_tx, mut close_rx) = watch::channel(false);
        let conn_id = shared
            .connections
            .register(&session_id, out_tx.clone(), close_tx.clone());

        let _ = write_frame(&mut send, KIND_TEXT, &welcome_frame_bytes(&session_id)).await;
        let writer = tokio::spawn(writer_loop(send, out_rx));

        loop {
            tokio::select! {
                changed = close_rx.changed() => {
                    if changed.is_err() || *close_rx.borrow() { break; }
                }
                frame = read_frame(&mut recv, &mut decoder, &mut readbuf) => {
                    match frame {
                        // Echo every decoded frame back as the matching Message,
                        // exercising both codec directions + the writer_loop.
                        Ok(Some(f)) if f.kind == KIND_TEXT => {
                            let s = String::from_utf8_lossy(&f.payload).to_string();
                            if out_tx.send(Message::Text(s)).is_err() { break; }
                        }
                        Ok(Some(f)) => {
                            if out_tx.send(Message::Binary(f.payload)).is_err() { break; }
                        }
                        Ok(None) | Err(_) => break,
                    }
                }
            }
        }

        shared.connections.unregister(conn_id);
        let _ = close_tx.send(true);
        drop(out_tx);
        writer.abort();
    }

    /// A loopback direct address for the endpoint's bound UDP port.
    fn loopback_addr(ep: &Endpoint) -> std::net::SocketAddr {
        let bound = ep.bound_sockets();
        let s = bound.first().copied().expect("endpoint has a bound socket");
        match s {
            std::net::SocketAddr::V4(v4) => {
                std::net::SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, v4.port()))
            }
            std::net::SocketAddr::V6(v6) => {
                std::net::SocketAddr::from((std::net::Ipv6Addr::LOCALHOST, v6.port()))
            }
        }
    }

    #[tokio::test]
    async fn iroh_direct_roundtrip_handshake_codec_and_revocation() {
        // Shared state + DB the server bridge authenticates + registers against.
        let shared = Arc::new(Shared::default());
        let db = Arc::new(init_test_database());
        let token = {
            let t = random_token();
            db.web_remote_insert_session("iroh-live", Some("Browser"), None, &sha256_hex(&t), true)
                .unwrap();
            t
        };

        // Server ("desktop") endpoint — direct only (Minimal preset: no relay).
        let server_ep = Endpoint::builder(presets::Minimal)
            .alpns(vec![IROH_ALPN.to_vec()])
            .bind()
            .await
            .expect("server endpoint binds");
        let server_id = server_ep.id();
        let server_addr = loopback_addr(&server_ep);

        // Accept exactly one connection → one bi-stream → run the bridge.
        let accept = {
            let shared = shared.clone();
            let db = db.clone();
            tokio::spawn(async move {
                let incoming = server_ep.accept().await.expect("a connection");
                let conn = incoming.await.expect("connect completes");
                let (send, recv) = conn.accept_bi().await.expect("client opens a bi-stream");
                serve_echo_stream(shared, db, send, recv).await;
                // Linger until the client has read everything and closed, so no
                // teardown (endpoint/connection drop) races the client's reads.
                conn.closed().await;
                drop(server_ep);
            })
        };

        // Client ("browser") endpoint — dial the server DIRECTLY over loopback.
        let client_ep = Endpoint::builder(presets::Minimal)
            .bind()
            .await
            .expect("client endpoint binds");
        let dial = iroh::EndpointAddr::new(server_id).with_ip_addr(server_addr);
        let conn = client_ep
            .connect(dial, IROH_ALPN)
            .await
            .expect("direct connect succeeds");
        let (mut send, mut recv) = conn.open_bi().await.expect("open bi-stream");

        let mut dec = FrameDecoder::new();
        let mut buf = vec![0u8; 4096];

        // 1. Handshake with the valid session token → a welcome frame.
        write_frame(&mut send, KIND_TEXT, &hello(&token).payload)
            .await
            .unwrap();
        let welcome = read_frame(&mut recv, &mut dec, &mut buf)
            .await
            .unwrap()
            .expect("welcome frame");
        let welcome_json: Value = serde_json::from_slice(&welcome.payload).unwrap();
        assert_eq!(welcome_json["t"], "welcome");
        assert_eq!(welcome_json["session_id"], "iroh-live");

        // 2. Round-trip a control frame + a byte-exact PTY frame through the
        //    codec + the real bridge (server echoes them back).
        let control = br#"{"t":"invoke","id":1,"cmd":"ping","args":{}}"#.to_vec();
        let pty = pty_frame(7, 99, b"live-bytes\x1b[0m\x00\x01\x02");
        write_frame(&mut send, KIND_TEXT, &control).await.unwrap();
        write_frame(&mut send, KIND_BINARY, &pty).await.unwrap();

        let echo1 = read_frame(&mut recv, &mut dec, &mut buf).await.unwrap().unwrap();
        let echo2 = read_frame(&mut recv, &mut dec, &mut buf).await.unwrap().unwrap();
        assert_eq!(echo1.kind, KIND_TEXT);
        assert_eq!(echo1.payload, control, "control frame round-trips byte-exact");
        assert_eq!(echo2.kind, KIND_BINARY);
        assert_eq!(echo2.payload, pty, "PTY frame round-trips byte-exact");

        // The live iroh session is visible in the SHARED registry.
        assert!(shared.connections.session_live("iroh-live"));

        // 3. Revocation through the shared registry severs the iroh stream: the
        //    registry frames a Close onto the connection's outbound queue, the
        //    writer finishes the stream, and the client reads EOF.
        assert_eq!(shared.connections.close_session("iroh-live"), 1);
        // The client's next read yields no more frames — a clean EOF if the
        // writer drained the Close first, or a transport error if the stream was
        // torn down; either way the session can no longer deliver frames.
        let after = read_frame(&mut recv, &mut dec, &mut buf).await;
        assert!(
            matches!(after, Ok(None) | Err(_)),
            "revocation must sever the iroh stream, got {after:?}"
        );

        // Close from the client so the server's `conn.closed()` linger resolves.
        conn.close(0u32.into(), b"client-done");
        accept.await.unwrap();
    }

    #[tokio::test]
    async fn iroh_direct_rejects_connection_without_a_valid_session() {
        // A native client that presents no valid session is refused: it receives
        // an `unauthorized` control frame and never a `welcome`, and no entry is
        // left in the shared registry.
        let shared = Arc::new(Shared::default());
        let db = Arc::new(init_test_database()); // empty — no sessions exist

        let server_ep = Endpoint::builder(presets::Minimal)
            .alpns(vec![IROH_ALPN.to_vec()])
            .bind()
            .await
            .expect("server endpoint binds");
        let server_id = server_ep.id();
        let server_addr = loopback_addr(&server_ep);

        let accept = {
            let shared = shared.clone();
            let db = db.clone();
            tokio::spawn(async move {
                let incoming = server_ep.accept().await.expect("a connection");
                let conn = incoming.await.expect("connect completes");
                let (send, recv) = conn.accept_bi().await.expect("client opens a bi-stream");
                serve_echo_stream(shared, db, send, recv).await;
                // Keep the connection alive until the client has read the
                // rejection and closed, so the frame is delivered before teardown.
                conn.closed().await;
                drop(server_ep);
            })
        };

        let client_ep = Endpoint::builder(presets::Minimal)
            .bind()
            .await
            .expect("client endpoint binds");
        let dial = iroh::EndpointAddr::new(server_id).with_ip_addr(server_addr);
        let conn = client_ep.connect(dial, IROH_ALPN).await.expect("connect");
        let (mut send, mut recv) = conn.open_bi().await.expect("open bi-stream");

        let mut dec = FrameDecoder::new();
        let mut buf = vec![0u8; 4096];

        // Present a bogus token.
        write_frame(&mut send, KIND_TEXT, &hello("bogus-token").payload)
            .await
            .unwrap();
        let reply = read_frame(&mut recv, &mut dec, &mut buf)
            .await
            .unwrap()
            .expect("a rejection frame");
        let reply_json: Value = serde_json::from_slice(&reply.payload).unwrap();
        assert_eq!(reply_json["t"], "unauthorized");
        assert_eq!(reply_json["reason"], "invalid_session");

        // Stream is closed after the rejection; no session registered.
        let after = read_frame(&mut recv, &mut dec, &mut buf).await;
        assert!(
            matches!(after, Ok(None) | Err(_)),
            "rejected handshake ends the stream, got {after:?}"
        );
        assert_eq!(shared.connections.active_count(), 0);

        conn.close(0u32.into(), b"client-done");
        accept.await.unwrap();
    }
}
