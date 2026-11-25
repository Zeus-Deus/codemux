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
//! relay operator never sees plaintext).
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
//! so revocation and `close_all` sever iroh sessions exactly as they do WS ones
//! (tagged as the relay transport, so turning relay off severs only these).
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

#[cfg(unix)]
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use axum::extract::ws::Message;
use iroh::endpoint::{presets, Connection, RecvStream, SendStream};
use iroh::{Endpoint, SecretKey};
use serde::Deserialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Manager, Runtime};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc, watch};

use super::server::OutboundRx;
use super::Shared;
use crate::database::DatabaseStore;
use crate::web_remote::auth::{self, AuthedSession};

/// ALPN identifying this protocol on the QUIC handshake. Must match the browser
/// wasm client's ALPN. Versioned so a future incompatible framing can bump it.
pub const IROH_ALPN: &[u8] = b"codemux/web-remote/0";

/// File (under a private app-data subdirectory) holding the machine-guarded iroh identity key.
const KEY_FILE: &str = "web-remote-iroh.key";
const KEY_DIR: &str = "iroh-identity";

// ── Kind-tagged, length-delimited codec ─────────────────────────────

/// Control frame: a UTF-8 JSON payload (the `/ws` text-frame shapes).
pub const KIND_TEXT: u8 = 0;
/// Binary frame: a raw payload (the `[0x01]…` PTY framing).
pub const KIND_BINARY: u8 = 1;
// Kinds 2 and 3 are RESERVED for compressed text / binary frames, mirroring the
// `0x02`/`0x03` wrappers the WebSocket transport already uses
// ([`super::compress`]). This transport does not emit them yet: QUIC gives it no
// permessage-deflate either, so it will want the same scheme, and pinning the
// numbering now keeps the two transports' tags aligned.

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
async fn writer_loop(mut send: SendStream, mut out_rx: OutboundRx) {
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

// ── Account (from-anywhere / relay) handshake ───────────────────────
//
// The token handshake above authenticates a session that already exists (minted
// over the LAN via `/api/pair` or `/api/pair-account`). The **account** (relay)
// handshake is the from-anywhere path: the browser has no pre-existing device
// session — it holds a short-lived **connection grant** the control plane signed
// after verifying it owns this account and this device. The grant IS the
// credential; the desktop verifies it out-of-band and, on success, mints a fresh
// `web_remote_sessions` row so everything downstream (`/ws` frames, revocation)
// is identical to a paired session.
//
//   C→S  kind 0  {"t":"hello-account","grant":"<grant>","nonce":"<browserNonce>"}
//   S→C  kind 0  {"t":"welcome","session_id":"<id>"}         (on accept)
//   S→C  kind 0  {"t":"unauthorized","reason":"<code>"}      (on reject)

/// A parsed `hello-account` first frame.
struct AccountHello {
    grant: String,
    /// The per-dial nonce the browser bound at `/api/devices/:id/connect` time;
    /// the same value is embedded in the grant, so a valid grant echoes it back
    /// through the verify endpoint and the two must agree.
    nonce: String,
}

/// Why an account (grant) handshake was refused. Maps to the
/// `unauthorized.reason` code the browser sees, distinct from [`HandshakeReject`]
/// so the codes name the account-path failure precisely.
#[derive(Debug, PartialEq, Eq)]
pub enum AccountHandshakeReject {
    /// The first frame was not a well-formed `hello-account` control frame.
    Malformed,
    /// The control plane did not vouch for the grant (bad signature, expired,
    /// or the verify call itself failed) — `valid:false` or a network error.
    Unverified,
    /// The grant is for a different device's `node_id` than this desktop's.
    WrongNode,
    /// The grant's account `uid` is not the desktop's signed-in user (or the
    /// desktop is signed out, so there is no account to match).
    WrongAccount,
    /// The grant verified but its bound nonce disagrees with the one the browser
    /// presented in the frame — a replay / grant-substitution signal.
    NonceMismatch,
    /// The verified session is still pending desktop approval.
    Pending,
    /// Minting the session row failed.
    Internal(String),
}

impl AccountHandshakeReject {
    fn code(&self) -> &'static str {
        match self {
            AccountHandshakeReject::Malformed => "malformed_handshake",
            AccountHandshakeReject::Unverified => "grant_invalid",
            AccountHandshakeReject::WrongNode => "wrong_device",
            AccountHandshakeReject::WrongAccount => "account_mismatch",
            AccountHandshakeReject::NonceMismatch => "nonce_mismatch",
            AccountHandshakeReject::Pending => "pending_approval",
            AccountHandshakeReject::Internal(_) => "internal_error",
        }
    }
}

fn account_reject_frame_bytes(reject: &AccountHandshakeReject) -> Vec<u8> {
    json!({ "t": "unauthorized", "reason": reject.code() })
        .to_string()
        .into_bytes()
}

/// Whether a first frame is a `hello-account` frame (the account/relay path).
/// A frame that is not valid JSON, or whose `t` is anything else, is treated as
/// the token path — [`authorize_handshake`] then decides admit-or-reject, so a
/// malformed token frame keeps its existing `invalid_session`/`malformed`
/// handling rather than being misrouted here.
fn is_account_handshake(frame: &Frame) -> bool {
    if frame.kind != KIND_TEXT {
        return false;
    }
    serde_json::from_slice::<Value>(&frame.payload)
        .ok()
        .and_then(|v| v.get("t").and_then(|t| t.as_str()).map(str::to_string))
        .as_deref()
        == Some("hello-account")
}

/// Parse a `hello-account` first frame into its grant + nonce. `None` if the
/// frame is malformed or the grant is empty.
fn parse_account_hello(frame: &Frame) -> Option<AccountHello> {
    if frame.kind != KIND_TEXT {
        return None;
    }
    let value: Value = serde_json::from_slice(&frame.payload).ok()?;
    if value.get("t").and_then(|v| v.as_str()) != Some("hello-account") {
        return None;
    }
    let grant = value.get("grant").and_then(|v| v.as_str())?.trim().to_string();
    if grant.is_empty() {
        return None;
    }
    let nonce = value
        .get("nonce")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    Some(AccountHello { grant, nonce })
}

/// The `POST /api/devices/grant/verify` response shape. The endpoint returns
/// `200 {valid:true, userId, deviceId, nodeId, nonce}` on a good, unexpired,
/// correctly-signed grant, and `200 {valid:false, reason}` otherwise — it never
/// 500s on a bad grant, so any non-2xx here is a transport/availability fault.
#[derive(Debug, Deserialize)]
struct GrantVerifyResponse {
    valid: bool,
    #[serde(rename = "userId", default)]
    user_id: Option<String>,
    #[serde(rename = "deviceId", default)]
    device_id: Option<String>,
    #[serde(rename = "nodeId", default)]
    node_id: Option<String>,
    #[serde(default)]
    nonce: Option<String>,
}

/// The identity a verified grant asserts, once the accept matrix passes.
#[derive(Debug, PartialEq, Eq)]
struct GrantIdentity {
    user_id: String,
    #[allow(dead_code)]
    device_id: Option<String>,
}

/// Call the control plane's grant-verify endpoint. This is a machine-to-machine
/// check — the grant is the sole credential, so no user bearer is sent. Reuses
/// the same `CODEMUX_API_URL` base every other API call reads.
async fn verify_grant(base: &str, grant: &str) -> Result<GrantVerifyResponse, String> {
    let url = format!("{base}/api/devices/grant/verify");
    let resp = reqwest::Client::new()
        .post(&url)
        .json(&json!({ "grant": grant }))
        .send()
        .await
        .map_err(|e| format!("grant verify request failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("grant verify returned {}", resp.status()));
    }
    resp.json::<GrantVerifyResponse>()
        .await
        .map_err(|e| format!("grant verify parse failed: {e}"))
}

/// Pure accept/reject matrix for a verified grant, given this desktop's own
/// `node_id`, its signed-in account `uid`, and the nonce the browser presented.
/// Admits ONLY when the control plane vouched (`valid`), the grant targets THIS
/// device's node, the grant's account matches the desktop's own signed-in user,
/// and the presented nonce agrees with the grant's bound nonce. Pure so the
/// whole matrix is unit-testable without the network.
fn evaluate_grant(
    resp: &GrantVerifyResponse,
    own_node_id: &str,
    own_user_id: Option<&str>,
    presented_nonce: &str,
) -> Result<GrantIdentity, AccountHandshakeReject> {
    if !resp.valid {
        return Err(AccountHandshakeReject::Unverified);
    }
    // The grant must target THIS device's iroh node, not some other device the
    // same account owns.
    if resp.node_id.as_deref().unwrap_or_default() != own_node_id {
        return Err(AccountHandshakeReject::WrongNode);
    }
    // The grant's account must be the desktop's own signed-in account. Signed
    // out (own_user_id None) → nothing to match → reject.
    let uid = resp.user_id.as_deref().unwrap_or_default();
    match own_user_id {
        Some(own) if !own.is_empty() && own == uid => {}
        _ => return Err(AccountHandshakeReject::WrongAccount),
    }
    // Nonce binding: a valid grant carries the browser's per-dial nonce, so the
    // verify response echoes it. It must equal what the browser presented in the
    // frame — a mismatch means a grant minted for a different dial was replayed.
    // Only enforced when the response actually echoes a nonce (never rejects a
    // legitimate connection, which by construction always matches).
    if let Some(bound) = resp.nonce.as_deref() {
        if !bound.is_empty() && bound != presented_nonce {
            return Err(AccountHandshakeReject::NonceMismatch);
        }
    }
    Ok(GrantIdentity {
        user_id: uid.to_string(),
        device_id: resp.device_id.clone(),
    })
}

/// Reuse an existing non-revoked account session for `uid` so repeated relay
/// dials (e.g. retries while a session is pending approval) don't pile up
/// duplicate `web_remote_sessions` rows. Returns `(session_id, approved)`.
fn existing_account_session(db: &DatabaseStore, uid: &str) -> Option<(String, bool)> {
    db.web_remote_list_sessions()
        .into_iter()
        .find(|s| s.source == "account" && s.account_user_id.as_deref() == Some(uid))
        .map(|s| (s.id, s.approved))
}

/// Admit an account (grant) handshake: verify the grant with the control plane,
/// run the accept matrix against this desktop's own identity, then mint (or
/// reuse) an account-tagged session row. Returns the `session_id` to bind the
/// stream to, or a reject reason. Approval composes exactly as the token path:
/// an account session defaults to pending unless `trust_account_browsers` is on,
/// and a pending session refuses the stream (the desktop approves it in the
/// device list, then the browser reconnects).
///
/// Takes `db` + `own_node_id` explicitly (rather than reaching through an
/// `AppHandle`) so the full network + mint path is unit-testable with a mocked
/// verify endpoint and an in-memory DB.
async fn authorize_account_handshake(
    db: &DatabaseStore,
    shared: &Arc<Shared>,
    own_node_id: &str,
    hello: &AccountHello,
) -> Result<String, AccountHandshakeReject> {
    // Snapshot the approval policy up front so the non-Send config mutex guard
    // is never held across the verify await.
    let approve_policy = {
        let cfg = shared.config.lock().unwrap();
        super::account_session_approved(&cfg)
    };
    let own_user_id = crate::auth::load_cached_user(db).map(|u| u.id);

    let base = crate::auth::api_base_url();
    let resp = verify_grant(&base, &hello.grant)
        .await
        .map_err(|_| AccountHandshakeReject::Unverified)?;

    let identity = evaluate_grant(&resp, own_node_id, own_user_id.as_deref(), &hello.nonce)?;

    // Mint once per account (reuse an existing row on retry), tagged
    // `source="account"` + the verified account user id. The plaintext token is
    // discarded — the iroh stream binds directly to `session_id`, so no bearer
    // is ever presented on this transport.
    let (session_id, approved) = match existing_account_session(db, &identity.user_id) {
        Some(existing) => existing,
        None => {
            let id = uuid::Uuid::new_v4().to_string();
            let token = super::auth::random_token();
            let token_hash = super::auth::sha256_hex(&token);
            db.web_remote_insert_account_session(
                &id,
                None,
                None,
                &token_hash,
                approve_policy,
                &identity.user_id,
            )
            .map_err(AccountHandshakeReject::Internal)?;
            (id, approve_policy)
        }
    };

    if !approved {
        return Err(AccountHandshakeReject::Pending);
    }
    Ok(session_id)
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
    data_dir.join(crate::APP_DIR_NAME).join(KEY_DIR).join(KEY_FILE)
}

fn legacy_key_file_path(path: &Path) -> Option<PathBuf> {
    let private_dir = path.parent()?;
    if private_dir.file_name()? != KEY_DIR {
        return None;
    }
    Some(private_dir.parent()?.join(KEY_FILE))
}

#[cfg(unix)]
#[derive(Clone, Copy)]
enum KeyAclScope {
    Ancestor,
    PrivateDirectory,
    KeyFile,
}

#[cfg(unix)]
#[derive(Clone, Copy, PartialEq, Eq)]
enum KeyDirAccess {
    PrivateCreate,
    PrivateReadOnly,
    AncestorCreate,
    AncestorReadOnly,
}

#[cfg(unix)]
struct KeyParent {
    directory: std::fs::File,
    created_child: bool,
}

#[cfg(any(target_os = "macos", all(unix, test)))]
const ACL_FIRST_ENTRY: libc::c_int = 0;
#[cfg(any(target_os = "macos", all(unix, test)))]
const ACL_NEXT_ENTRY: libc::c_int = -1;
#[cfg(any(target_os = "macos", all(unix, test)))]
const ACL_EXTENDED_ALLOW: libc::c_int = 1;
#[cfg(any(target_os = "macos", all(unix, test)))]
const ACL_EXTENDED_DENY: libc::c_int = 2;
#[cfg(any(target_os = "macos", all(unix, test)))]
const ACL_GENERIC_ALL: u64 = 1 << 21;
#[cfg(any(target_os = "macos", all(unix, test)))]
const ACL_GENERIC_EXECUTE: u64 = 1 << 22;
#[cfg(any(target_os = "macos", all(unix, test)))]
const ACL_GENERIC_WRITE: u64 = 1 << 23;
#[cfg(any(target_os = "macos", all(unix, test)))]
const ACL_GENERIC_READ: u64 = 1 << 24;

#[cfg(any(target_os = "macos", all(unix, test)))]
fn acl_lookup_has_entry(
    found: libc::c_int,
    entry_id: libc::c_int,
    error: Option<i32>,
    context: &str,
) -> Result<bool, String> {
    if found == 0 {
        return Ok(true);
    }
    if found == -1 && entry_id == ACL_NEXT_ENTRY && error == Some(libc::EINVAL) {
        return Ok(false);
    }
    Err(format!(
        "inspect {context} ACL entry: {}",
        error.map(std::io::Error::from_raw_os_error).map_or_else(
            || format!("unexpected return code {found}"),
            |e| e.to_string()
        )
    ))
}

#[cfg(any(target_os = "macos", all(unix, test)))]
fn check_key_acl_entries(
    context: &str,
    scope: KeyAclScope,
    mut next: impl FnMut() -> Result<Option<(libc::c_int, u64)>, String>,
) -> Result<(), String> {
    // From macOS <sys/acl.h>. Directory mutation includes deletion of the
    // directory itself, not just creation or deletion of its children.
    const MUTATE: u64 =
        (1 << 2) | (1 << 4) | (1 << 5) | (1 << 6) | (1 << 8) | (1 << 10) | (1 << 12) | (1 << 13);
    const READ_DATA: u64 = 1 << 1;
    const SEARCH: u64 = 1 << 3;
    const GENERIC_RIGHTS: u64 =
        ACL_GENERIC_ALL | ACL_GENERIC_EXECUTE | ACL_GENERIC_WRITE | ACL_GENERIC_READ;
    const SPECIFIC_RIGHTS: u64 = ((1 << 14) - (1 << 1)) | (1 << 20);
    let forbidden = match scope {
        KeyAclScope::Ancestor => MUTATE,
        KeyAclScope::PrivateDirectory => MUTATE | READ_DATA | SEARCH,
        KeyAclScope::KeyFile => MUTATE | READ_DATA,
    };

    while let Some((tag, mask)) = next()? {
        if tag == ACL_EXTENDED_DENY {
            continue;
        }
        if tag != ACL_EXTENDED_ALLOW {
            return Err(format!("{context} has an unsupported ACL entry"));
        }
        if mask & !(SPECIFIC_RIGHTS | GENERIC_RIGHTS) != 0 {
            return Err(format!("{context} ACL has unsupported rights"));
        }
        // Darwin expands these generic rights when evaluating an ACL entry.
        // Use the same specific rights before deciding whether a grant is safe.
        let mut rights = mask;
        if mask & (ACL_GENERIC_ALL | ACL_GENERIC_READ) != 0 {
            rights |= READ_DATA | (1 << 7) | (1 << 9) | (1 << 11);
        }
        if mask & (ACL_GENERIC_ALL | ACL_GENERIC_WRITE) != 0 {
            rights |= (1 << 2) | (1 << 4) | (1 << 5) | (1 << 6)
                | (1 << 8) | (1 << 10) | (1 << 12);
        }
        if mask & (ACL_GENERIC_ALL | ACL_GENERIC_EXECUTE) != 0 {
            rights |= SEARCH;
        }
        if rights & forbidden != 0 {
            return Err(format!("{context} ACL grants unsafe access"));
        }
    }
    Ok(())
}

/// macOS ACL allow entries can grant access despite private POSIX mode bits.
/// Inspect the opened inode, never a pathname that can be replaced afterward.
#[cfg(target_os = "macos")]
fn check_key_acl(file: &std::fs::File, context: &str, scope: KeyAclScope) -> Result<(), String> {
    use std::ffi::c_void;
    use std::os::fd::AsRawFd;

    unsafe extern "C" {
        fn acl_get_fd_np(fd: libc::c_int, acl_type: libc::c_int) -> *mut c_void;
        fn acl_get_entry(
            acl: *mut c_void,
            entry_id: libc::c_int,
            entry: *mut *mut c_void,
        ) -> libc::c_int;
        fn acl_get_tag_type(entry: *mut c_void, tag: *mut libc::c_int) -> libc::c_int;
        fn acl_get_permset_mask_np(entry: *mut c_void, mask: *mut u64) -> libc::c_int;
        fn acl_free(object: *mut c_void) -> libc::c_int;
    }

    const ACL_TYPE_EXTENDED: libc::c_int = 0x100;

    let acl = unsafe { acl_get_fd_np(file.as_raw_fd(), ACL_TYPE_EXTENDED) };
    if acl.is_null() {
        let error = std::io::Error::last_os_error();
        return if error.raw_os_error() == Some(libc::ENOENT) {
            Ok(()) // No extended ACL on this inode.
        } else {
            Err(format!("inspect {context} ACL: {error}"))
        };
    }
    let result = (|| {
        let mut entry = std::ptr::null_mut();
        let mut entry_id = ACL_FIRST_ENTRY;
        check_key_acl_entries(context, scope, || {
            let found = unsafe { acl_get_entry(acl, entry_id, &mut entry) };
            let error = (found < 0)
                .then(|| std::io::Error::last_os_error().raw_os_error())
                .flatten();
            if !acl_lookup_has_entry(found, entry_id, error, context)? {
                return Ok(None);
            }
            entry_id = ACL_NEXT_ENTRY;
            let mut tag = 0;
            if unsafe { acl_get_tag_type(entry, &mut tag) } < 0 {
                return Err(format!(
                    "inspect {context} ACL tag: {}",
                    std::io::Error::last_os_error()
                ));
            }
            let mut mask = 0u64;
            if unsafe { acl_get_permset_mask_np(entry, &mut mask) } < 0 {
                return Err(format!(
                    "inspect {context} ACL rights: {}",
                    std::io::Error::last_os_error()
                ));
            }
            Ok(Some((tag, mask)))
        })
    })();
    if unsafe { acl_free(acl) } != 0 {
        return Err(format!(
            "release {context} ACL: {}",
            std::io::Error::last_os_error()
        ));
    }
    result
}

#[cfg(all(unix, not(target_os = "macos")))]
fn check_key_acl(_file: &std::fs::File, _context: &str, _scope: KeyAclScope) -> Result<(), String> {
    Ok(())
}

#[cfg(target_os = "macos")]
fn check_symlink_key_acl(
    directory: &std::fs::File,
    name: &std::ffi::CStr,
    expected: &libc::stat,
) -> Result<(), String> {
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::fs::MetadataExt;

    // O_SYMLINK pins the link itself. A pathname-based ACL lookup could
    // inspect a replacement link after fstatat had validated the original.
    let fd = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            libc::O_SYMLINK | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(format!("open iroh key relocation for ACL: {}", std::io::Error::last_os_error()));
    }
    let link = unsafe { std::fs::File::from_raw_fd(fd) };
    let meta = link.metadata().map_err(|e| format!("inspect iroh key relocation: {e}"))?;
    if !meta.file_type().is_symlink()
        || meta.dev() != expected.st_dev as u64
        || meta.ino() != expected.st_ino as u64
    {
        return Err("iroh key relocation changed during ACL validation".to_string());
    }
    check_key_acl(&link, "iroh key relocation", KeyAclScope::Ancestor)
}

/// Resolve each path entry from a pinned parent directory. An app-data
/// relocation is allowed when its link and target are under directories that
/// another user cannot replace or modify.
#[cfg(unix)]
fn open_private_key_dir(parent: &Path) -> Result<std::fs::File, String> {
    open_key_dir(parent, KeyDirAccess::PrivateCreate)
}

/// An open directory entry is not durable until its containing directory is
/// synced. Reopen the validated handle for directory I/O; O_PATH/O_SEARCH
/// traversal handles themselves cannot be fsynced.
#[cfg(unix)]
fn sync_pinned_key_parent(parent: &std::fs::File) -> Result<(), String> {
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::fs::MetadataExt;

    let dot = std::ffi::CString::new(".").expect("static directory name has no NUL");
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            dot.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(format!(
            "open iroh key parent for sync: {}",
            std::io::Error::last_os_error()
        ));
    }
    let readable = unsafe { std::fs::File::from_raw_fd(fd) };
    let expected = parent
        .metadata()
        .map_err(|e| format!("inspect iroh key parent: {e}"))?;
    let actual = readable
        .metadata()
        .map_err(|e| format!("inspect iroh key sync handle: {e}"))?;
    if actual.dev() != expected.dev() || actual.ino() != expected.ino() {
        return Err("iroh key parent changed before sync".to_string());
    }
    readable
        .sync_all()
        .map_err(|e| format!("sync iroh key parent directory: {e}"))
}

#[cfg(unix)]
fn ancestor_entry_may_be_new(parent: &std::fs::File, created_child: bool) -> Result<bool, String> {
    use std::os::fd::AsRawFd;

    if created_child {
        return Ok(true);
    }
    let dot = std::ffi::CString::new(".").expect("static directory name has no NUL");
    let accessible = unsafe {
        libc::faccessat(
            parent.as_raw_fd(),
            dot.as_ptr(),
            libc::W_OK | libc::X_OK,
            libc::AT_EACCESS,
        )
    };
    if accessible == 0 {
        return Ok(true);
    }
    let error = std::io::Error::last_os_error();
    ancestor_write_access_result(Err(error))
}

#[cfg(unix)]
fn ancestor_write_access_result(result: std::io::Result<()>) -> Result<bool, String> {
    let Err(error) = result else {
        return Ok(true);
    };
    // EROFS also means this process could not have created the ancestor's
    // entry, even if the directory mode itself grants write permission.
    if matches!(error.raw_os_error(), Some(libc::EACCES | libc::EPERM | libc::EROFS)) {
        return Ok(false);
    }
    Err(format!("check iroh key ancestor write access: {error}"))
}

/// Flush every parent that this process could have changed, including an
/// entry left visible by an interrupted earlier attempt. A non-writable
/// established ancestor cannot contain an entry created by this process.
#[cfg(unix)]
fn sync_new_key_ancestry(root: &std::fs::File, parents: &[KeyParent]) -> Result<(), String> {
    sync_pinned_key_parent(root)?;
    for parent in parents.iter().rev() {
        if !ancestor_entry_may_be_new(&parent.directory, parent.created_child)? {
            break;
        }
        sync_pinned_key_parent(&parent.directory)?;
    }
    Ok(())
}

#[cfg(unix)]
fn open_key_dir(parent: &Path, access: KeyDirAccess) -> Result<std::fs::File, String> {
    open_key_dir_with_parents(parent, access).map(|(directory, _)| directory)
}

#[cfg(unix)]
fn open_key_dir_with_parents(
    parent: &Path,
    access: KeyDirAccess,
) -> Result<(std::fs::File, Vec<KeyParent>), String> {
    use std::collections::VecDeque;
    use std::ffi::{CString, OsString};
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
    use std::path::Component;

    // Traversal needs search permission, not read permission. The final
    // directory is reopened below with a usable descriptor for key I/O.
    #[cfg(target_os = "linux")]
    const SEARCH_ONLY: libc::c_int = libc::O_PATH;
    #[cfg(target_os = "macos")]
    const SEARCH_ONLY: libc::c_int = libc::O_SEARCH;
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    const SEARCH_ONLY: libc::c_int = libc::O_RDONLY;

    #[derive(Debug)]
    enum Part {
        Root,
        Parent,
        Name(OsString),
    }

    fn parts(path: &Path) -> Result<VecDeque<Part>, String> {
        path.components()
            .map(|part| match part {
                Component::RootDir => Ok(Part::Root),
                Component::CurDir => Ok(Part::Name(OsString::from("."))),
                Component::ParentDir => Ok(Part::Parent),
                Component::Normal(name) => Ok(Part::Name(name.to_os_string())),
                _ => Err("unsupported iroh key directory path".to_string()),
            })
            .collect()
    }

    if !parent.is_absolute() {
        return Err("iroh key directory must be absolute".to_string());
    }
    let root = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open("/")
        .map_err(|e| format!("open filesystem root for iroh key: {e}"))?;
    let mut directories = vec![root];
    let mut created_children = vec![false];
    let mut remaining = parts(parent)?;
    let mut links = 0;
    let uid = unsafe { libc::geteuid() };

    while let Some(part) = remaining.pop_front() {
        let name = match part {
            Part::Root => {
                directories.truncate(1);
                created_children.truncate(1);
                continue;
            }
            Part::Parent => {
                if directories.len() > 1 {
                    directories.pop();
                    created_children.pop();
                }
                continue;
            }
            Part::Name(name) if name == "." => continue,
            Part::Name(name) => name,
        };
        let directory = directories.last().expect("root directory remains open");
        let parent_meta = directory
            .metadata()
            .map_err(|e| format!("inspect iroh key ancestor: {e}"))?;
        check_key_acl(directory, "iroh key ancestor", KeyAclScope::Ancestor)?;
        if parent_meta.uid() != uid && parent_meta.uid() != 0 {
            return Err("iroh key ancestor is not owner controlled".to_string());
        }
        let mode = parent_meta.permissions().mode();
        let writable_by_others = mode & 0o022 != 0;
        if writable_by_others && mode & (libc::S_ISVTX as u32) == 0 {
            return Err("iroh key ancestor allows other users to replace entries".to_string());
        }

        let name = CString::new(name.as_bytes())
            .map_err(|_| "iroh key directory contains a NUL byte".to_string())?;
        let mut entry = unsafe { std::mem::zeroed::<libc::stat>() };
        let mut result = unsafe {
            libc::fstatat(
                directory.as_raw_fd(),
                name.as_ptr(),
                &mut entry,
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        if result < 0
            && matches!(
                access,
                KeyDirAccess::PrivateCreate | KeyDirAccess::AncestorCreate
            )
            && std::io::Error::last_os_error().kind() == std::io::ErrorKind::NotFound
        {
            result = unsafe { libc::mkdirat(directory.as_raw_fd(), name.as_ptr(), 0o700) };
            if result < 0
                && std::io::Error::last_os_error().kind() != std::io::ErrorKind::AlreadyExists
            {
                return Err(format!(
                    "create iroh key directory: {}",
                    std::io::Error::last_os_error()
                ));
            }
            if result == 0 {
                *created_children
                    .last_mut()
                    .expect("parent directory remains open") = true;
            }
            result = unsafe {
                libc::fstatat(
                    directory.as_raw_fd(),
                    name.as_ptr(),
                    &mut entry,
                    libc::AT_SYMLINK_NOFOLLOW,
                )
            };
        }
        if result < 0 {
            return Err(format!("inspect iroh key directory entry: {}", std::io::Error::last_os_error()));
        }
        if entry.st_uid != uid && entry.st_uid != 0 {
            return Err("iroh key directory entry is not owner controlled".to_string());
        }
        if writable_by_others && entry.st_uid != uid && entry.st_uid != parent_meta.uid() {
            return Err("iroh key directory entry can be replaced by another user".to_string());
        }

        if entry.st_mode & libc::S_IFMT == libc::S_IFLNK {
            #[cfg(target_os = "macos")]
            check_symlink_key_acl(directory, &name, &entry)?;
            links += 1;
            if links > 40 {
                return Err("too many iroh key directory relocations".to_string());
            }
            let mut target = vec![0u8; 256];
            let target = loop {
                let length = unsafe {
                    libc::readlinkat(
                        directory.as_raw_fd(),
                        name.as_ptr(),
                        target.as_mut_ptr().cast(),
                        target.len(),
                    )
                };
                if length < 0 {
                    return Err(format!("read iroh key relocation: {}", std::io::Error::last_os_error()));
                }
                if (length as usize) < target.len() {
                    target.truncate(length as usize);
                    break PathBuf::from(OsString::from_vec(target));
                }
                if target.len() >= 1 << 20 {
                    return Err("iroh key relocation is too long".to_string());
                }
                target.resize(target.len() * 2, 0);
            };
            let mut target_parts = parts(&target)?;
            target_parts.append(&mut remaining);
            remaining = target_parts;
            continue;
        }
        if entry.st_mode & libc::S_IFMT != libc::S_IFDIR {
            return Err("iroh key directory entry is not a directory".to_string());
        }
        let fd = unsafe {
            libc::openat(
                directory.as_raw_fd(),
                name.as_ptr(),
                SEARCH_ONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(format!("open iroh key directory entry: {}", std::io::Error::last_os_error()));
        }
        let opened = unsafe { std::fs::File::from_raw_fd(fd) };
        let opened_meta = opened
            .metadata()
            .map_err(|e| format!("inspect opened iroh key directory: {e}"))?;
        if opened_meta.dev() != entry.st_dev as u64 || opened_meta.ino() != entry.st_ino as u64 {
            return Err("iroh key directory entry changed during validation".to_string());
        }
        check_key_acl(&opened, "iroh key directory entry", KeyAclScope::Ancestor)?;
        directories.push(opened);
        created_children.push(false);
    }
    let search_handle = directories.pop().expect("root directory remains open");
    created_children.pop();
    let search_meta = search_handle
        .metadata()
        .map_err(|e| format!("inspect iroh key directory: {e}"))?;
    // Legacy lookup needs only search permission on the app-data root. Keep
    // the validated descriptor: reopening it for read would reject an owned
    // 0300 directory even though its private key is accessible by name.
    if matches!(
        access,
        KeyDirAccess::AncestorCreate | KeyDirAccess::AncestorReadOnly
    ) {
        if !search_meta.is_dir()
            || search_meta.uid() != uid
            || search_meta.permissions().mode() & 0o022 != 0
        {
            return Err("iroh key directory permissions remain unsafe".to_string());
        }
        check_key_acl(&search_handle, "iroh key directory", KeyAclScope::Ancestor)?;
        let parents = directories
            .into_iter()
            .zip(created_children)
            .map(|(directory, created_child)| KeyParent {
                directory,
                created_child,
            })
            .collect();
        return Ok((search_handle, parents));
    }
    let dot = std::ffi::CString::new(".").expect("static directory name has no NUL");
    let fd = unsafe {
        libc::openat(
            search_handle.as_raw_fd(),
            dot.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(format!("open iroh key directory for I/O: {}", std::io::Error::last_os_error()));
    }
    let directory = unsafe { std::fs::File::from_raw_fd(fd) };
    let meta = directory
        .metadata()
        .map_err(|e| format!("inspect usable iroh key directory: {e}"))?;
    if meta.dev() != search_meta.dev() || meta.ino() != search_meta.ino() {
        return Err("iroh key directory changed while opening for I/O".to_string());
    }
    // The descriptor, not the pathname, identifies the directory we tighten.
    if !meta.is_dir() || meta.uid() != uid {
        return Err("iroh key directory is not an owned directory".to_string());
    }
    let scope = if access == KeyDirAccess::AncestorReadOnly {
        KeyAclScope::Ancestor
    } else {
        KeyAclScope::PrivateDirectory
    };
    check_key_acl(&directory, "iroh key directory", scope)?;
    if access == KeyDirAccess::PrivateCreate && meta.permissions().mode() & 0o777 != 0o700 {
        directory
            .set_permissions(std::fs::Permissions::from_mode(0o700))
            .map_err(|e| format!("secure iroh key directory: {e}"))?;
    }
    // Some filesystems acknowledge chmod without changing the effective
    // mode. Never read or publish key material until the pinned directory
    // reports the private mode we require.
    let secured = directory
        .metadata()
        .map_err(|e| format!("recheck iroh key directory permissions: {e}"))?;
    let mode = secured.permissions().mode();
    let safe_mode = if access == KeyDirAccess::AncestorReadOnly {
        mode & 0o022 == 0
    } else {
        mode & 0o777 == 0o700
    };
    if !secured.is_dir() || secured.uid() != uid || !safe_mode {
        return Err("iroh key directory permissions remain unsafe".to_string());
    }
    check_key_acl(&directory, "iroh key directory", scope)?;
    let parents = directories
        .into_iter()
        .zip(created_children)
        .map(|(directory, created_child)| KeyParent {
            directory,
            created_child,
        })
        .collect();
    Ok((directory, parents))
}

/// Open the fixed private child relative to the validated app-data root.
/// App-data root relocations remain supported, but this child cannot redirect
/// identity reads or writes to a different root.
#[cfg(unix)]
fn open_private_child_dir(
    root: &std::fs::File,
    access: KeyDirAccess,
) -> Result<Option<std::fs::File>, String> {
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let name = std::ffi::CString::new(KEY_DIR).expect("static directory name has no NUL");
    let mut entry: libc::stat = unsafe { std::mem::zeroed() };
    let mut result = unsafe {
        libc::fstatat(root.as_raw_fd(), name.as_ptr(), &mut entry, libc::AT_SYMLINK_NOFOLLOW)
    };
    if result < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::NotFound {
        if access == KeyDirAccess::PrivateReadOnly {
            return Ok(None);
        }
        result = unsafe { libc::mkdirat(root.as_raw_fd(), name.as_ptr(), 0o700) };
        if result < 0 && std::io::Error::last_os_error().kind() != std::io::ErrorKind::AlreadyExists {
            return Err(format!("create iroh key directory: {}", std::io::Error::last_os_error()));
        }
        result = unsafe {
            libc::fstatat(root.as_raw_fd(), name.as_ptr(), &mut entry, libc::AT_SYMLINK_NOFOLLOW)
        };
    }
    if result < 0 {
        return Err(format!("inspect iroh key directory entry: {}", std::io::Error::last_os_error()));
    }
    if entry.st_mode & libc::S_IFMT != libc::S_IFDIR {
        return Err("iroh key directory entry is not a directory".to_string());
    }
    let fd = unsafe {
        libc::openat(
            root.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(format!("open iroh key directory for I/O: {}", std::io::Error::last_os_error()));
    }
    let directory = unsafe { std::fs::File::from_raw_fd(fd) };
    let meta = directory.metadata().map_err(|e| format!("inspect iroh key directory: {e}"))?;
    if !meta.is_dir()
        || meta.dev() != entry.st_dev as u64
        || meta.ino() != entry.st_ino as u64
        || meta.uid() != unsafe { libc::geteuid() }
    {
        return Err("iroh key directory changed during validation".to_string());
    }
    check_key_acl(&directory, "iroh key directory", KeyAclScope::PrivateDirectory)?;
    if access == KeyDirAccess::PrivateCreate && meta.permissions().mode() & 0o777 != 0o700 {
        directory
            .set_permissions(std::fs::Permissions::from_mode(0o700))
            .map_err(|e| format!("secure iroh key directory: {e}"))?;
    }
    let secured = directory.metadata().map_err(|e| format!("recheck iroh key directory permissions: {e}"))?;
    if !secured.is_dir()
        || secured.uid() != unsafe { libc::geteuid() }
        || secured.permissions().mode() & 0o777 != 0o700
    {
        return Err("iroh key directory permissions remain unsafe".to_string());
    }
    check_key_acl(&directory, "iroh key directory", KeyAclScope::PrivateDirectory)?;
    Ok(Some(directory))
}

/// Open only the key entry under the validated directory. Startup may tighten
/// an old mode; status reads only accept a file already private.
#[cfg(unix)]
fn open_private_key_file(directory: &std::fs::File, tighten: bool) -> Result<Option<std::fs::File>, String> {
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let name = std::ffi::CString::new(KEY_FILE).expect("static key filename has no NUL");
    let fd = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            name.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK,
            0,
        )
    };
    if fd < 0 {
        let error = std::io::Error::last_os_error();
        if error.kind() == std::io::ErrorKind::NotFound {
            return Ok(None);
        }
        if error.raw_os_error() == Some(libc::ELOOP) {
            return Err("iroh key path is not a regular file".to_string());
        }
        return Err(format!("open iroh key file: {error}"));
    }
    let file = unsafe { std::fs::File::from_raw_fd(fd) };
    let meta = file
        .metadata()
        .map_err(|e| format!("inspect iroh key file: {e}"))?;
    if !meta.is_file() || meta.uid() != unsafe { libc::geteuid() } || meta.nlink() != 1 {
        return Err("iroh key path is not a singly linked owned regular file".to_string());
    }
    check_key_acl(&file, "iroh key file", KeyAclScope::KeyFile)?;
    if tighten && meta.permissions().mode() & 0o777 != 0o600 {
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("secure iroh key file: {e}"))?;
    }
    let secured = file
        .metadata()
        .map_err(|e| format!("recheck iroh key file permissions: {e}"))?;
    if !secured.is_file()
        || secured.uid() != unsafe { libc::geteuid() }
        || secured.nlink() != 1
        || secured.permissions().mode() & 0o777 != 0o600
    {
        return Err("iroh key file permissions remain unsafe".to_string());
    }
    check_key_acl(&file, "iroh key file", KeyAclScope::KeyFile)?;
    Ok(Some(file))
}

#[cfg(unix)]
fn read_private_key_in(directory: &std::fs::File, tighten: bool) -> Result<Option<Vec<u8>>, String> {
    let Some(mut file) = open_private_key_file(directory, tighten)? else {
        return Ok(None);
    };
    let mut sealed = Vec::new();
    file.read_to_end(&mut sealed)
        .map_err(|e| format!("read iroh key file: {e}"))?;
    Ok(Some(sealed))
}

#[cfg(unix)]
fn persist_secret_key_in(directory: &std::fs::File, key: &SecretKey) -> Result<(), String> {
    persist_secret_key_in_with_sync(directory, key, &std::fs::File::sync_all)
}

#[cfg(unix)]
fn persist_secret_key_in_with_sync(
    directory: &std::fs::File,
    key: &SecretKey,
    directory_sync: &dyn Fn(&std::fs::File) -> std::io::Result<()>,
) -> Result<(), String> {
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    // Refuse hardlinked or symlinked legacy entries before replacing them.
    drop(open_private_key_file(directory, true)?);
    let sealed = crate::auth::encrypt_data(&key.to_bytes())?;
    let key_name = std::ffi::CString::new(KEY_FILE).expect("static key filename has no NUL");
    let temp_name = std::ffi::CString::new(format!(".{KEY_FILE}.{}.tmp", uuid::Uuid::new_v4()))
        .expect("generated temp filename has no NUL");
    let fd = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            temp_name.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            0o600,
        )
    };
    if fd < 0 {
        return Err(format!("create iroh key temp file: {}", std::io::Error::last_os_error()));
    }
    let mut file = unsafe { std::fs::File::from_raw_fd(fd) };
    let result = (|| {
        file.set_permissions(std::fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("secure iroh key temp file: {e}"))?;
        let secured = file
            .metadata()
            .map_err(|e| format!("recheck iroh key temp file permissions: {e}"))?;
        if !secured.is_file()
            || secured.uid() != unsafe { libc::geteuid() }
            || secured.nlink() != 1
            || secured.permissions().mode() & 0o777 != 0o600
        {
            return Err("iroh key temp file permissions remain unsafe".to_string());
        }
        check_key_acl(&file, "iroh key temp file", KeyAclScope::KeyFile)?;
        file.write_all(&sealed)
            .map_err(|e| format!("write iroh key temp file: {e}"))?;
        file.sync_all()
            .map_err(|e| format!("sync iroh key temp file: {e}"))?;
        let renamed = unsafe {
            libc::renameat(
                directory.as_raw_fd(),
                temp_name.as_ptr(),
                directory.as_raw_fd(),
                key_name.as_ptr(),
            )
        };
        if renamed < 0 {
            return Err(format!(
                "publish iroh key file: {}",
                std::io::Error::last_os_error()
            ));
        }
        directory_sync(directory).map_err(|e| format!("sync iroh key directory: {e}"))?;
        Ok(())
    })();
    if result.is_err() {
        unsafe { libc::unlinkat(directory.as_raw_fd(), temp_name.as_ptr(), 0) };
    }
    result
}

#[cfg(test)]
#[cfg(unix)]
fn persist_secret_key(path: &Path, key: &SecretKey) -> Result<(), String> {
    let parent = path.parent().ok_or("iroh key path has no parent")?;
    let directory = open_private_key_dir(parent)?;
    persist_secret_key_in(&directory, key)
}

#[cfg(windows)]
mod windows_key_security {
    use std::ffi::{OsStr, OsString};
    use std::io::{Read, Write};
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use std::path::{Component, Path, PathBuf, Prefix};
    use std::ptr::{null, null_mut};

    use windows_sys::Wdk::Foundation::OBJECT_ATTRIBUTES;
    use windows_sys::Wdk::Storage::FileSystem::{
        FileRenameInformation, NtCreateFile, NtSetInformationFile, FILE_CREATE,
        FILE_DIRECTORY_FILE, FILE_NON_DIRECTORY_FILE, FILE_OPEN, FILE_OPEN_IF,
        FILE_OPEN_REPARSE_POINT, FILE_RENAME_INFORMATION, FILE_RENAME_INFORMATION_0,
        FILE_SYNCHRONOUS_IO_NONALERT,
    };
    use windows_sys::Win32::Foundation::{
        LocalFree, GENERIC_ALL, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE,
        STATUS_ACCESS_DENIED, STATUS_OBJECT_NAME_NOT_FOUND, STATUS_OBJECT_PATH_NOT_FOUND,
        STATUS_INVALID_PARAMETER, STATUS_SHARING_VIOLATION, UNICODE_STRING,
    };
    use windows_sys::Win32::Security::Authorization::{
        ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
        GetSecurityInfo, SetSecurityInfo, SDDL_REVISION_1, SE_FILE_OBJECT,
    };
    use windows_sys::Win32::Security::{
        EqualSid, GetAce, GetSecurityDescriptorControl, GetSecurityDescriptorDacl,
        GetTokenInformation, IsValidSid, IsWellKnownSid, TokenUser, ACL, ACCESS_ALLOWED_ACE,
        DACL_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION,
        PSECURITY_DESCRIPTOR, PSID, SE_DACL_PROTECTED, TOKEN_QUERY,
        TOKEN_USER, WinBuiltinAdministratorsSid, WinLocalSystemSid, INHERIT_ONLY_ACE,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FileDispositionInfo, GetFileInformationByHandle, SetFileInformationByHandle,
        BY_HANDLE_FILE_INFORMATION, DELETE, FILE_ADD_FILE, FILE_ADD_SUBDIRECTORY, FILE_ATTRIBUTE_DIRECTORY,
        FILE_ATTRIBUTE_NORMAL, FILE_ATTRIBUTE_REPARSE_POINT, FILE_DELETE_CHILD,
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_GENERIC_READ,
        FILE_GENERIC_WRITE, FILE_READ_ATTRIBUTES, FILE_READ_EA, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        FILE_TRAVERSE, FILE_WRITE_ATTRIBUTES, FILE_WRITE_EA, FILE_DISPOSITION_INFO,
        OPEN_EXISTING, READ_CONTROL, SYNCHRONIZE, WRITE_DAC, WRITE_OWNER,
    };
    use windows_sys::Win32::System::IO::{DeviceIoControl, IO_STATUS_BLOCK, IO_STATUS_BLOCK_0};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    fn wide(value: &OsStr) -> Vec<u16> {
        value.encode_wide().chain(Some(0)).collect()
    }

    struct PrivateDacl {
        descriptor: PSECURITY_DESCRIPTOR,
        dacl: *mut ACL,
        // TOKEN_USER's Sid points into this aligned allocation.
        user_info: Vec<usize>,
    }

    impl PrivateDacl {
        fn new(directory: bool) -> Result<Self, String> {
            Self::with_extra_ace(directory, "")
        }

        fn with_extra_ace(directory: bool, extra_ace: &str) -> Result<Self, String> {
            let mut token: HANDLE = null_mut();
            if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
                return Err(format!("open user token for iroh key: {}", std::io::Error::last_os_error()));
            }
            let token = unsafe { OwnedHandle::from_raw_handle(token) };
            let mut length = 0u32;
            unsafe { GetTokenInformation(token.as_raw_handle(), TokenUser, null_mut(), 0, &mut length) };
            if length == 0 {
                return Err(format!("size user SID for iroh key: {}", std::io::Error::last_os_error()));
            }
            let words = (length as usize).div_ceil(std::mem::size_of::<usize>());
            let mut user_info = vec![0usize; words];
            if unsafe {
                GetTokenInformation(
                    token.as_raw_handle(), TokenUser, user_info.as_mut_ptr().cast(),
                    (words * std::mem::size_of::<usize>()) as u32, &mut length,
                )
            } == 0 {
                return Err(format!("read user SID for iroh key: {}", std::io::Error::last_os_error()));
            }
            let sid = unsafe { (*user_info.as_ptr().cast::<TOKEN_USER>()).User.Sid };
            if sid.is_null() || unsafe { IsValidSid(sid) } == 0 {
                return Err("user SID for iroh key is invalid".to_string());
            }
            let mut sid_text = null_mut();
            if unsafe { ConvertSidToStringSidW(sid, &mut sid_text) } == 0 {
                return Err(format!("format user SID for iroh key: {}", std::io::Error::last_os_error()));
            }
            let sid_string = (|| {
                let mut length = 0usize;
                while length < 256 && unsafe { *sid_text.add(length) } != 0 {
                    length += 1;
                }
                if length == 256 {
                    return Err("user SID for iroh key is too long".to_string());
                }
                String::from_utf16(unsafe { std::slice::from_raw_parts(sid_text, length) })
                    .map_err(|e| format!("decode user SID for iroh key: {e}"))
            })();
            unsafe { LocalFree(sid_text.cast()) };
            let sid_string = sid_string?;
            let flags = if directory { "OICI" } else { "" };
            let sddl = format!(
                "D:P(A;{flags};FA;;;{sid_string})(A;{flags};FA;;;SY)(A;{flags};FA;;;BA){extra_ace}"
            );
            let mut descriptor = null_mut();
            if unsafe {
                ConvertStringSecurityDescriptorToSecurityDescriptorW(
                    wide(OsStr::new(sddl.as_str())).as_ptr(), SDDL_REVISION_1, &mut descriptor, null_mut(),
                )
            } == 0 {
                return Err(format!("build private iroh key ACL: {}", std::io::Error::last_os_error()));
            }
            let mut present = 0;
            let mut defaulted = 0;
            let mut dacl = null_mut();
            if unsafe { GetSecurityDescriptorDacl(descriptor, &mut present, &mut dacl, &mut defaulted) } == 0
                || present == 0 || dacl.is_null()
            {
                unsafe { LocalFree(descriptor) };
                return Err("private iroh key ACL has no DACL".to_string());
            }
            Ok(Self { descriptor, dacl, user_info })
        }

        fn user_sid(&self) -> PSID {
            unsafe { (*self.user_info.as_ptr().cast::<TOKEN_USER>()).User.Sid }
        }

        fn owner_allowed(&self, owner: PSID) -> bool {
            !owner.is_null() && unsafe {
                EqualSid(owner, self.user_sid()) != 0
                    || IsWellKnownSid(owner, WinBuiltinAdministratorsSid) != 0
                    || IsWellKnownSid(owner, WinLocalSystemSid) != 0
            } || trusted_installer_owner(owner)
        }
    }

    fn trusted_installer_owner(owner: PSID) -> bool {
        // Windows-managed ancestors can be owned by the Modules Installer.
        // Treat only that exact service SID as a trusted OS owner.
        const SID: &str = "S-1-5-80-956008885-3418522649-1831038044-1853292631-2271478464";
        let mut text = null_mut();
        if owner.is_null() || unsafe { ConvertSidToStringSidW(owner, &mut text) } == 0 {
            return false;
        }
        let mut length = 0usize;
        while length < 128 && unsafe { *text.add(length) } != 0 {
            length += 1;
        }
        let trusted = length < 128
            && String::from_utf16(unsafe { std::slice::from_raw_parts(text, length) })
                .is_ok_and(|value| value == SID);
        unsafe { LocalFree(text.cast()) };
        trusted
    }

    impl Drop for PrivateDacl {
        fn drop(&mut self) {
            unsafe { LocalFree(self.descriptor) };
        }
    }

    fn same_aces(actual: *mut ACL, expected: *mut ACL) -> bool {
        if actual.is_null() || expected.is_null() || unsafe { (*actual).AceCount != (*expected).AceCount } {
            return false;
        }
        for index in 0..unsafe { (*expected).AceCount } as u32 {
            let mut actual_ace = null_mut();
            let mut expected_ace = null_mut();
            if unsafe { GetAce(actual, index, &mut actual_ace) } == 0
                || unsafe { GetAce(expected, index, &mut expected_ace) } == 0
            {
                return false;
            }
            let actual_header = unsafe { &*(actual_ace.cast::<windows_sys::Win32::Security::ACE_HEADER>()) };
            let expected_header = unsafe { &*(expected_ace.cast::<windows_sys::Win32::Security::ACE_HEADER>()) };
            if actual_header.AceType != 0 || expected_header.AceType != 0
                || actual_header.AceSize < std::mem::size_of::<ACCESS_ALLOWED_ACE>() as u16
                || expected_header.AceSize < std::mem::size_of::<ACCESS_ALLOWED_ACE>() as u16
            {
                return false;
            }
            let actual_ace = unsafe { &*(actual_ace.cast::<ACCESS_ALLOWED_ACE>()) };
            let expected_ace = unsafe { &*(expected_ace.cast::<ACCESS_ALLOWED_ACE>()) };
            if actual_ace.Header.AceFlags != expected_ace.Header.AceFlags
                || actual_ace.Header.AceSize != expected_ace.Header.AceSize
                || actual_ace.Mask != expected_ace.Mask
                || unsafe {
                    EqualSid(
                        (&actual_ace.SidStart as *const u32).cast_mut().cast(),
                        (&expected_ace.SidStart as *const u32).cast_mut().cast(),
                    )
                } == 0
            {
                return false;
            }
        }
        true
    }

    fn validate_handle(file: &std::fs::File, directory: bool, security: &PrivateDacl, tighten: bool) -> Result<(), String> {
        let handle = file.as_raw_handle();
        let mut info = unsafe { std::mem::zeroed::<BY_HANDLE_FILE_INFORMATION>() };
        if unsafe { GetFileInformationByHandle(handle, &mut info) } == 0 {
            return Err(format!("inspect iroh key handle: {}", std::io::Error::last_os_error()));
        }
        if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0
            || (info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0) != directory
            || (!directory && info.nNumberOfLinks != 1)
        {
            return Err("iroh key storage is a reparse point, wrong type, or hardlink".to_string());
        }
        let mut owner = null_mut();
        let mut descriptor = null_mut();
        let status = unsafe {
            GetSecurityInfo(handle, SE_FILE_OBJECT, OWNER_SECURITY_INFORMATION,
                &mut owner, null_mut(), null_mut(), null_mut(), &mut descriptor)
        };
        if status != 0 {
            return Err(format!("inspect iroh key owner: {}", std::io::Error::from_raw_os_error(status as i32)));
        }
        let owner_allowed = security.owner_allowed(owner);
        unsafe { LocalFree(descriptor) };
        if !owner_allowed {
            return Err("iroh key storage is owned by another user".to_string());
        }
        if tighten {
            let status = unsafe {
                SetSecurityInfo(handle, SE_FILE_OBJECT,
                    DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                    null_mut(), null_mut(), security.dacl, null())
            };
            if status != 0 {
                return Err(format!("secure iroh key ACL: {}", std::io::Error::from_raw_os_error(status as i32)));
            }
        }
        let mut owner = null_mut();
        let mut dacl = null_mut();
        let mut descriptor = null_mut();
        let result = (|| {
            let status = unsafe {
                GetSecurityInfo(handle, SE_FILE_OBJECT,
                    OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                    &mut owner, null_mut(), &mut dacl, null_mut(), &mut descriptor)
            };
            if status != 0 {
                return Err(format!("recheck iroh key ACL: {}", std::io::Error::from_raw_os_error(status as i32)));
            }
            let mut control = 0u16;
            let mut revision = 0u32;
            if descriptor.is_null()
                || unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) } == 0
                || control & SE_DACL_PROTECTED == 0
                || !security.owner_allowed(owner)
                || !same_aces(dacl, security.dacl)
            {
                return Err("iroh key ACL remains unsafe".to_string());
            }
            Ok(())
        })();
        if !descriptor.is_null() {
            unsafe { LocalFree(descriptor) };
        }
        result
    }

    const OBJ_CASE_INSENSITIVE: u32 = 0x40;

    // A single name is resolved against a live directory handle. In particular,
    // neither an ancestor nor the private directory is looked up by path again.
    fn open_relative(
        parent: &std::fs::File,
        name: &OsStr,
        access: u32,
        share: u32,
        disposition: u32,
        options: u32,
        security: Option<&PrivateDacl>,
    ) -> Result<std::fs::File, i32> {
        open_relative_with_reparse(parent, name, access, share, disposition, options, security, false)
    }

    fn open_relative_with_reparse(
        parent: &std::fs::File,
        name: &OsStr,
        access: u32,
        share: u32,
        disposition: u32,
        options: u32,
        security: Option<&PrivateDacl>,
        follow_reparse: bool,
    ) -> Result<std::fs::File, i32> {
        let mut name: Vec<u16> = name.encode_wide().collect();
        if name.is_empty() || name == [b'.' as u16] || name == [b'.' as u16, b'.' as u16]
            || name.iter().any(|value| matches!(*value, 0 | 47 | 58 | 92))
        {
            return Err(STATUS_INVALID_PARAMETER);
        }
        let bytes = u16::try_from(name.len().checked_mul(2).ok_or(STATUS_INVALID_PARAMETER)?)
            .map_err(|_| STATUS_INVALID_PARAMETER)?;
        let unicode = UNICODE_STRING { Length: bytes, MaximumLength: bytes, Buffer: name.as_mut_ptr() };
        let attributes = OBJECT_ATTRIBUTES {
            Length: std::mem::size_of::<OBJECT_ATTRIBUTES>() as u32,
            RootDirectory: parent.as_raw_handle(),
            ObjectName: &unicode,
            Attributes: OBJ_CASE_INSENSITIVE,
            SecurityDescriptor: security.map_or(null(), |security| security.descriptor.cast()),
            SecurityQualityOfService: null(),
        };
        let mut status_block = IO_STATUS_BLOCK { Anonymous: IO_STATUS_BLOCK_0 { Status: 0 }, Information: 0 };
        let mut handle: HANDLE = null_mut();
        let status = unsafe {
            NtCreateFile(
                &mut handle, access | SYNCHRONIZE, &attributes, &mut status_block,
                null(), FILE_ATTRIBUTE_NORMAL, share, disposition,
                options | FILE_SYNCHRONOUS_IO_NONALERT
                    | if follow_reparse { 0 } else { FILE_OPEN_REPARSE_POINT },
                null(), 0,
            )
        };
        if status < 0 {
            return Err(status);
        }
        Ok(unsafe { std::fs::File::from_raw_handle(handle) })
    }

    fn missing(status: i32) -> bool {
        matches!(status, STATUS_OBJECT_NAME_NOT_FOUND | STATUS_OBJECT_PATH_NOT_FOUND)
    }

    fn open_existing(
        parent: &std::fs::File,
        name: &OsStr,
        directory: bool,
        security: &PrivateDacl,
        tighten: bool,
    ) -> Result<Option<std::fs::File>, String> {
        let access = FILE_READ_ATTRIBUTES | READ_CONTROL | if tighten { WRITE_DAC } else { 0 }
            | if directory { FILE_TRAVERSE } else { FILE_GENERIC_READ };
        let share = FILE_SHARE_READ | FILE_SHARE_WRITE
            | if directory { 0 } else { FILE_SHARE_DELETE };
        let options = if directory { FILE_DIRECTORY_FILE } else { FILE_NON_DIRECTORY_FILE };
        let file = match open_relative(parent, name, access, share, FILE_OPEN, options, None) {
            Ok(file) => file,
            Err(status) if missing(status) => return Ok(None),
            Err(status) => return Err(format!("open iroh key storage: NTSTATUS {status:#010x}")),
        };
        validate_handle(&file, directory, security, tighten)?;
        Ok(Some(file))
    }

    fn directory_attributes(file: &std::fs::File) -> Result<u32, String> {
        let mut info = unsafe { std::mem::zeroed::<BY_HANDLE_FILE_INFORMATION>() };
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), &mut info) } == 0 {
            return Err(format!("inspect iroh app directory: {}", std::io::Error::last_os_error()));
        }
        if info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0 {
            return Err("iroh app directory is not a directory".to_string());
        }
        Ok(info.dwFileAttributes)
    }

    fn validate_app_directory(
        file: &std::fs::File,
        security: &PrivateDacl,
        app_root: bool,
        allow_reparse: bool,
    ) -> Result<(), String> {
        const MUTATE: u32 = GENERIC_ALL | GENERIC_WRITE | FILE_ADD_FILE | FILE_ADD_SUBDIRECTORY
            | FILE_DELETE_CHILD | FILE_WRITE_EA | FILE_WRITE_ATTRIBUTES | DELETE | WRITE_DAC | WRITE_OWNER;
        const REPLACE_CHILD: u32 = GENERIC_ALL | FILE_DELETE_CHILD | DELETE | WRITE_DAC | WRITE_OWNER;
        let attributes = directory_attributes(file)?;
        if !allow_reparse && attributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
            return Err("iroh app directory is not a normal directory".to_string());
        }
        let mut owner = null_mut();
        let mut dacl = null_mut();
        let mut descriptor = null_mut();
        let result = (|| {
            let status = unsafe {
                GetSecurityInfo(file.as_raw_handle(), SE_FILE_OBJECT,
                    OWNER_SECURITY_INFORMATION | DACL_SECURITY_INFORMATION,
                    &mut owner, null_mut(), &mut dacl, null_mut(), &mut descriptor)
            };
            if status != 0 {
                return Err(format!("inspect iroh app directory ACL: {}", std::io::Error::from_raw_os_error(status as i32)));
            }
            if !security.owner_allowed(owner) || dacl.is_null() {
                return Err("iroh app directory owner or ACL is unsafe".to_string());
            }
            for index in 0..unsafe { (*dacl).AceCount } as u32 {
                let mut ace = null_mut();
                if unsafe { GetAce(dacl, index, &mut ace) } == 0 {
                    return Err(format!("inspect iroh app directory ACL entry: {}", std::io::Error::last_os_error()));
                }
                let header = unsafe { &*(ace.cast::<windows_sys::Win32::Security::ACE_HEADER>()) };
                if header.AceFlags & INHERIT_ONLY_ACE as u8 != 0 || header.AceType == 1 {
                    continue;
                }
                if header.AceType != 0 || header.AceSize < std::mem::size_of::<ACCESS_ALLOWED_ACE>() as u16 {
                    return Err("iroh app directory ACL has an unsupported entry".to_string());
                }
                let grant = unsafe { &*(ace.cast::<ACCESS_ALLOWED_ACE>()) };
                let sid: PSID = (&grant.SidStart as *const u32).cast_mut().cast();
                let forbidden = if app_root { MUTATE } else { REPLACE_CHILD };
                if unsafe { IsValidSid(sid) } == 0
                    || (!security.owner_allowed(sid) && grant.Mask & forbidden != 0)
                {
                    return Err("iroh app directory grants another user write access".to_string());
                }
            }
            Ok(())
        })();
        if !descriptor.is_null() {
            unsafe { LocalFree(descriptor) };
        }
        result
    }

    fn volume_guid_prefix(name: &OsStr) -> bool {
        let name = name.to_string_lossy().to_ascii_lowercase();
        let Some(guid) = name.strip_prefix("volume{").and_then(|name| name.strip_suffix('}')) else {
            return false;
        };
        guid.len() == 36 && guid.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) { byte == b'-' } else { byte.is_ascii_hexdigit() }
        })
    }

    fn supported_prefix(prefix: Prefix<'_>) -> bool {
        match prefix {
            Prefix::Disk(_) | Prefix::VerbatimDisk(_) | Prefix::UNC(_, _) | Prefix::VerbatimUNC(_, _) => true,
            Prefix::Verbatim(name) => volume_guid_prefix(name),
            _ => false,
        }
    }

    fn parse_reparse_target(buffer: &[u8], base: &Path) -> Result<PathBuf, String> {
        // The print name is display text. Only the substitute name determines
        // where Windows follows the link.
        const IO_REPARSE_TAG_MOUNT_POINT: u32 = 0xa000_0003;
        const IO_REPARSE_TAG_SYMLINK: u32 = 0xa000_000c;
        if buffer.len() < 16 {
            return Err("iroh app directory reparse response is short".to_string());
        }
        let u16_at = |offset: usize| u16::from_le_bytes([buffer[offset], buffer[offset + 1]]) as usize;
        let tag = u32::from_le_bytes(buffer[..4].try_into().unwrap());
        let data_len = u16_at(4);
        if buffer.len() != 8 + data_len {
            return Err("iroh app directory reparse response has an invalid length".to_string());
        }
        let (path_start, relative): (usize, bool) = match tag {
            IO_REPARSE_TAG_MOUNT_POINT if data_len >= 8 => (16, false),
            IO_REPARSE_TAG_SYMLINK if data_len >= 12 && buffer.len() >= 20 => {
                let flags = u32::from_le_bytes(buffer[16..20].try_into().unwrap());
                if flags & !1 != 0 {
                    return Err("iroh app directory symlink has unsupported flags".to_string());
                }
                (20, flags == 1)
            }
            _ => return Err("iroh app directory has an unsupported reparse tag".to_string()),
        };
        let offset = u16_at(8);
        let length = u16_at(10);
        let end = path_start.checked_add(offset).and_then(|start| start.checked_add(length))
            .ok_or("iroh app directory reparse target is too long")?;
        if offset % 2 != 0 || length == 0 || length % 2 != 0 || end > buffer.len() {
            return Err("iroh app directory reparse target is malformed".to_string());
        }
        let name: Vec<u16> = buffer[path_start + offset..end]
            .chunks_exact(2)
            .map(|unit| u16::from_le_bytes([unit[0], unit[1]]))
            .collect();
        if name.contains(&0) {
            return Err("iroh app directory reparse target contains a NUL".to_string());
        }
        if relative {
            let relative = PathBuf::from(OsString::from_wide(&name));
            let mut target = base.to_path_buf();
            for component in relative.components() {
                match component {
                    Component::CurDir => {}
                    Component::ParentDir if target.pop() => {}
                    Component::Normal(name) => target.push(name),
                    _ => return Err("iroh app directory symlink target escapes its volume".to_string()),
                }
            }
            return Ok(target);
        }
        if !name.starts_with(&[92, 63, 63, 92]) {
            return Err("iroh app directory reparse target is not absolute".to_string());
        }
        let mut path = vec![92, 92, 63, 92]; // Convert native \??\ to Win32 \\?\.
        path.extend_from_slice(&name[4..]);
        let target = PathBuf::from(OsString::from_wide(&path));
        let mut components = target.components();
        let (Some(Component::Prefix(prefix)), Some(Component::RootDir)) =
            (components.next(), components.next())
        else {
            return Err("iroh app directory reparse target is not absolute".to_string());
        };
        if !supported_prefix(prefix.kind()) {
            return Err("iroh app directory reparse target uses an unsupported namespace".to_string());
        }
        Ok(target)
    }

    fn reparse_target(link: &std::fs::File, base: &Path) -> Result<PathBuf, String> {
        // FSCTL_GET_REPARSE_POINT returns an 8-byte header followed by tag-
        // specific metadata and a UTF-16 path buffer.
        const FSCTL_GET_REPARSE_POINT: u32 = 0x0009_00a8;
        const MAX_REPARSE_DATA: usize = 16_384;
        let mut buffer = vec![0u8; MAX_REPARSE_DATA];
        let mut returned = 0u32;
        if unsafe {
            DeviceIoControl(link.as_raw_handle(), FSCTL_GET_REPARSE_POINT,
                null(), 0, buffer.as_mut_ptr().cast(), buffer.len() as u32,
                &mut returned, null_mut())
        } == 0 {
            return Err(format!("read iroh app directory reparse point: {}", std::io::Error::last_os_error()));
        }
        let returned = returned as usize;
        if returned > buffer.len() {
            return Err("iroh app directory reparse response is too long".to_string());
        }
        parse_reparse_target(&buffer[..returned], base)
    }

    struct PinnedDirectory {
        file: std::fs::File,
        resolved_path: PathBuf,
    }

    fn open_app_root(root: &Path, create: bool, security: &PrivateDacl) -> Result<std::fs::File, String> {
        Ok(open_app_root_inner(root, create, security, true, 0)?.file)
    }

    fn open_app_root_inner(
        root: &Path,
        create: bool,
        security: &PrivateDacl,
        final_is_app_root: bool,
        hops: u8,
    ) -> Result<PinnedDirectory, String> {
        if hops > 32 {
            return Err("iroh app directory reparse chain is too long".to_string());
        }
        let mut components = root.components();
        let (Some(Component::Prefix(prefix)), Some(Component::RootDir)) =
            (components.next(), components.next())
        else {
            return Err("iroh app directory must have an absolute Windows path".to_string());
        };
        if !supported_prefix(prefix.kind()) {
            return Err("iroh app directory uses an unsupported Windows path prefix".to_string());
        }
        // As with existing UNC and mapped-drive app-data paths, these ACL
        // checks rely on the backing filesystem or server enforcing Windows
        // security descriptors. A network-storage policy must cover both.
        let mut anchor_path = PathBuf::from(prefix.as_os_str());
        anchor_path.push("\\");
        let mut names = Vec::<OsString>::new();
        for component in components {
            match component {
                Component::Normal(name) => names.push(name.to_os_string()),
                _ => return Err("iroh app directory contains an unsafe path component".to_string()),
            }
        }
        if names.is_empty() && hops == 0 {
            return Err("iroh app directory cannot be a volume root".to_string());
        }
        let handle = unsafe {
            CreateFileW(wide(anchor_path.as_os_str()).as_ptr(),
                FILE_READ_ATTRIBUTES | READ_CONTROL | FILE_TRAVERSE,
                FILE_SHARE_READ | FILE_SHARE_WRITE, null(), OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT, null_mut())
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(format!("open iroh volume root: {}", std::io::Error::last_os_error()));
        }
        let anchor = PinnedDirectory {
            file: unsafe { std::fs::File::from_raw_handle(handle) },
            resolved_path: anchor_path,
        };
        validate_app_directory(&anchor.file, security, names.is_empty() && final_is_app_root, false)?;
        if names.is_empty() {
            return Ok(anchor);
        }
        // Keep every parent open without delete sharing until its child is
        // validated. A path component cannot be swapped during the walk.
        let mut held = vec![anchor];
        let mut links = Vec::new();
        for (index, name) in names.iter().enumerate() {
            let parent = held.last().unwrap();
            let file = open_relative(
                &parent.file, name,
                FILE_READ_ATTRIBUTES | FILE_READ_EA | READ_CONTROL | FILE_TRAVERSE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                if create { FILE_OPEN_IF } else { FILE_OPEN },
                FILE_DIRECTORY_FILE,
                if create { Some(security) } else { None },
            ).map_err(|status| format!("open iroh app directory component: NTSTATUS {status:#010x}"))?;
            let is_reparse = directory_attributes(&file)? & FILE_ATTRIBUTE_REPARSE_POINT != 0;
            if is_reparse {
                // A reparse point can change its target through a write to
                // the link itself. Require a strict ACL before reading it.
                validate_app_directory(&file, security, true, true)?;
                let target_path = reparse_target(&file, &parent.resolved_path)?;
                let target = open_app_root_inner(
                    &target_path, false, security,
                    final_is_app_root && index + 1 == names.len(), hops + 1,
                )?;
                let followed = open_relative_with_reparse(
                    &parent.file, name, FILE_READ_ATTRIBUTES | READ_CONTROL | FILE_TRAVERSE,
                    FILE_SHARE_READ | FILE_SHARE_WRITE, FILE_OPEN,
                    FILE_DIRECTORY_FILE, None, true,
                ).map_err(|status| format!("follow iroh app directory reparse point: NTSTATUS {status:#010x}"))?;
                let walked_id = crate::windows_file_id::file_id(&target.file)
                    .map_err(|e| format!("identify iroh reparse target: {e}"))?;
                let followed_id = crate::windows_file_id::file_id(&followed)
                    .map_err(|e| format!("identify followed iroh reparse point: {e}"))?;
                if walked_id != followed_id {
                    return Err("iroh app directory reparse target changed during validation".to_string());
                }
                links.push(file);
                held.push(target);
            } else {
                validate_app_directory(
                    &file, security, final_is_app_root && index + 1 == names.len(), false,
                )?;
                let mut resolved_path = parent.resolved_path.clone();
                resolved_path.push(name);
                held.push(PinnedDirectory { file, resolved_path });
            }
        }
        Ok(held.pop().unwrap())
    }

    fn read_key(parent: &std::fs::File, name: &OsStr, tighten: bool) -> Result<Option<Vec<u8>>, String> {
        let security = PrivateDacl::new(false)?;
        let Some(mut file) = open_existing(parent, name, false, &security, tighten)? else {
            return Ok(None);
        };
        let mut sealed = Vec::new();
        file.read_to_end(&mut sealed).map_err(|e| format!("read iroh key file: {e}"))?;
        Ok(Some(sealed))
    }

    fn discard(file: &std::fs::File) {
        let info = FILE_DISPOSITION_INFO { DeleteFile: 1 };
        unsafe {
            SetFileInformationByHandle(file.as_raw_handle(), FileDispositionInfo,
                std::ptr::addr_of!(info).cast(), std::mem::size_of::<FILE_DISPOSITION_INFO>() as u32);
        }
    }

    fn publish(file: &std::fs::File, parent: &std::fs::File, name: &OsStr) -> Result<(), String> {
        let wide: Vec<u16> = name.encode_wide().collect();
        let size = std::mem::size_of::<FILE_RENAME_INFORMATION>() + wide.len() * 2;
        let mut buffer = vec![0u64; size.div_ceil(8)];
        let info = buffer.as_mut_ptr().cast::<FILE_RENAME_INFORMATION>();
        unsafe {
            (*info).Anonymous = FILE_RENAME_INFORMATION_0 { ReplaceIfExists: 1 };
            (*info).RootDirectory = parent.as_raw_handle();
            (*info).FileNameLength = (wide.len() * 2) as u32;
            std::ptr::copy_nonoverlapping(
                wide.as_ptr(), std::ptr::addr_of_mut!((*info).FileName).cast::<u16>(), wide.len());
        }
        let mut attempts = 0;
        loop {
            let mut status_block = IO_STATUS_BLOCK { Anonymous: IO_STATUS_BLOCK_0 { Status: 0 }, Information: 0 };
            let status = unsafe {
                NtSetInformationFile(file.as_raw_handle(), &mut status_block, info.cast(), size as u32,
                    FileRenameInformation)
            };
            attempts += 1;
            if status >= 0 {
                return Ok(());
            }
            if !matches!(status, STATUS_ACCESS_DENIED | STATUS_SHARING_VIOLATION) || attempts == 50 {
                return Err(format!("publish iroh key file: NTSTATUS {status:#010x}"));
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    pub(super) struct KeyStore {
        root: std::fs::File,
        private: Option<std::fs::File>,
        name: OsString,
    }

    impl KeyStore {
        pub(super) fn open(path: &Path, create: bool) -> Result<Self, String> {
            let parent = path.parent().ok_or("iroh key path has no parent")?;
            let root = parent.parent().ok_or("iroh key directory has no app root")?;
            let name = path.file_name().ok_or("iroh key path has no filename")?.to_os_string();
            let private_name = parent.file_name().ok_or("iroh key directory has no name")?;
            let directory_security = PrivateDacl::new(true)?;
            let root = open_app_root(root, create, &directory_security)?;
            let private = if create {
                let file = open_relative(&root, private_name,
                    FILE_READ_ATTRIBUTES | READ_CONTROL | WRITE_DAC | FILE_TRAVERSE,
                    FILE_SHARE_READ | FILE_SHARE_WRITE, FILE_OPEN_IF, FILE_DIRECTORY_FILE,
                    Some(&directory_security),
                ).map_err(|status| format!("open private iroh key directory: NTSTATUS {status:#010x}"))?;
                validate_handle(&file, true, &directory_security, true)?;
                Some(file)
            } else {
                open_existing(&root, private_name, true, &directory_security, false)?
            };
            Ok(Self { root, private, name })
        }

        pub(super) fn read_private(&self, tighten: bool) -> Result<Option<Vec<u8>>, String> {
            match self.private.as_ref() {
                Some(parent) => read_key(parent, &self.name, tighten),
                None => Ok(None),
            }
        }

        pub(super) fn read_legacy(&self, tighten: bool) -> Result<Option<Vec<u8>>, String> {
            read_key(&self.root, &self.name, tighten)
        }

        pub(super) fn persist(&self, key: &super::SecretKey) -> Result<(), String> {
            let parent = self.private.as_ref().ok_or("private iroh key directory is missing")?;
            let security = PrivateDacl::new(false)?;
            drop(open_existing(parent, &self.name, false, &security, true)?);
            let sealed = crate::auth::encrypt_data(&key.to_bytes())?;
            let temp_name = OsString::from(format!(".{}.{}.tmp", super::KEY_FILE, uuid::Uuid::new_v4()));
            let mut file = open_relative(parent, &temp_name,
                FILE_GENERIC_WRITE | FILE_READ_ATTRIBUTES | READ_CONTROL | WRITE_DAC | DELETE,
                0, FILE_CREATE, FILE_NON_DIRECTORY_FILE, Some(&security),
            ).map_err(|status| format!("create private iroh key temp file: NTSTATUS {status:#010x}"))?;
            let mut published = false;
            let result = (|| {
                validate_handle(&file, false, &security, true)?;
                file.write_all(&sealed).map_err(|e| format!("write iroh key temp file: {e}"))?;
                file.sync_all().map_err(|e| format!("sync iroh key temp file: {e}"))?;
                publish(&file, parent, &self.name)?;
                published = true;
                file.sync_all().map_err(|e| format!("sync published iroh key file: {e}"))?;
                Ok(())
            })();
            if result.is_err() && !published {
                discard(&file);
            }
            result
        }
    }

    #[cfg(test)]
    pub(super) fn inherited_grant_fixture(parent: &Path, right: &str) -> Result<(), String> {
        let security = PrivateDacl::with_extra_ace(true, &format!("(A;OICI;{right};;;BU)"))?;
        let handle = unsafe {
            CreateFileW(wide(parent.as_os_str()).as_ptr(), FILE_READ_ATTRIBUTES | READ_CONTROL | WRITE_DAC,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, null(), OPEN_EXISTING,
                FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT, null_mut())
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(format!("open iroh ACL test directory: {}", std::io::Error::last_os_error()));
        }
        let file = unsafe { std::fs::File::from_raw_handle(handle) };
        let status = unsafe {
            SetSecurityInfo(file.as_raw_handle(), SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                null_mut(), null_mut(), security.dacl, null())
        };
        if status != 0 {
            return Err(format!("set iroh ACL test grant: {}", std::io::Error::from_raw_os_error(status as i32)));
        }
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn key_ace_count(path: &Path, directory: bool) -> Result<u16, String> {
        let handle = unsafe {
            CreateFileW(wide(path.as_os_str()).as_ptr(), READ_CONTROL,
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, null(), OPEN_EXISTING,
                FILE_FLAG_OPEN_REPARSE_POINT | if directory { FILE_FLAG_BACKUP_SEMANTICS } else { 0 },
                null_mut())
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(format!("open iroh ACL test file: {}", std::io::Error::last_os_error()));
        }
        let file = unsafe { std::fs::File::from_raw_handle(handle) };
        let mut dacl = null_mut();
        let mut descriptor = null_mut();
        let status = unsafe {
            GetSecurityInfo(file.as_raw_handle(), SE_FILE_OBJECT, DACL_SECURITY_INFORMATION,
                null_mut(), null_mut(), &mut dacl, null_mut(), &mut descriptor)
        };
        if status != 0 {
            return Err(format!("read iroh ACL test file: {}", std::io::Error::from_raw_os_error(status as i32)));
        }
        let count = if dacl.is_null() { 0 } else { unsafe { (*dacl).AceCount } };
        unsafe { LocalFree(descriptor) };
        Ok(count)
    }

    #[cfg(test)]
    mod reparse_tests {
        use super::*;

        const MOUNT: u32 = 0xa000_0003;
        const SYMLINK: u32 = 0xa000_000c;

        fn reparse_data(tag: u32, flags: u32, substitute: &str) -> Vec<u8> {
            let units: Vec<u16> = substitute.encode_utf16().collect();
            let metadata = if tag == SYMLINK { 12 } else { 8 };
            let mut bytes = vec![0u8; 8 + metadata + units.len() * 2];
            let data_len = u16::try_from(bytes.len() - 8).unwrap();
            bytes[..4].copy_from_slice(&tag.to_le_bytes());
            bytes[4..6].copy_from_slice(&data_len.to_le_bytes());
            bytes[10..12].copy_from_slice(&u16::try_from(units.len() * 2).unwrap().to_le_bytes());
            if tag == SYMLINK {
                bytes[16..20].copy_from_slice(&flags.to_le_bytes());
            }
            for (chunk, unit) in bytes[8 + metadata..].chunks_exact_mut(2).zip(units) {
                chunk.copy_from_slice(&unit.to_le_bytes());
            }
            bytes
        }

        #[test]
        fn accepts_directory_symlinks_and_volume_guid_mounts() {
            let base = Path::new(r"\\?\D:\real\app");
            let absolute = reparse_data(SYMLINK, 0, r"\??\C:\relocated");
            assert_eq!(parse_reparse_target(&absolute, base).unwrap(), PathBuf::from(r"\\?\C:\relocated"));

            let relative = reparse_data(SYMLINK, 1, r"..\relocated");
            assert_eq!(parse_reparse_target(&relative, base).unwrap(), PathBuf::from(r"\\?\D:\real\relocated"));

            let volume = r"\??\Volume{01234567-89ab-cdef-0123-456789abcdef}\relocated";
            let mounted = reparse_data(MOUNT, 0, volume);
            assert_eq!(
                parse_reparse_target(&mounted, base).unwrap(),
                PathBuf::from(r"\\?\Volume{01234567-89ab-cdef-0123-456789abcdef}\relocated"),
            );
        }

        #[test]
        fn rejects_unsafe_reparse_namespaces_and_relative_escape() {
            let base = Path::new(r"\\?\C:\");
            assert!(parse_reparse_target(&reparse_data(SYMLINK, 1, r"..\outside"), base).is_err());
            assert!(parse_reparse_target(&reparse_data(SYMLINK, 2, r"target"), base).is_err());
            assert!(parse_reparse_target(
                &reparse_data(MOUNT, 0, r"\??\GLOBALROOT\Device\HarddiskVolume1"), base,
            ).is_err());
            let mut truncated = reparse_data(SYMLINK, 0, r"\??\C:\safe");
            truncated[10..12].copy_from_slice(&u16::MAX.to_le_bytes());
            assert!(parse_reparse_target(&truncated, base).is_err());
        }

        #[test]
        fn walks_volume_guid_namespace_to_the_same_directory() {
            use windows_sys::Win32::Storage::FileSystem::GetVolumeNameForVolumeMountPointW;

            let temp = tempfile::tempdir().unwrap();
            let mut components = temp.path().components();
            let prefix = match components.next() {
                Some(Component::Prefix(prefix)) => prefix,
                _ => panic!("temporary directory has no Windows volume"),
            };
            assert!(matches!(components.next(), Some(Component::RootDir)));
            let mut drive_root = PathBuf::from(prefix.as_os_str());
            drive_root.push("\\");
            let mut volume_name = vec![0u16; 256];
            let result = unsafe {
                GetVolumeNameForVolumeMountPointW(
                    wide(drive_root.as_os_str()).as_ptr(), volume_name.as_mut_ptr(),
                    volume_name.len() as u32,
                )
            };
            assert_ne!(result, 0, "volume GUID lookup: {}", std::io::Error::last_os_error());
            let end = volume_name.iter().position(|unit| *unit == 0).unwrap();
            let volume_root = PathBuf::from(OsString::from_wide(&volume_name[..end]));
            let by_guid = volume_root.join(temp.path().strip_prefix(&drive_root).unwrap());
            let security = PrivateDacl::new(true).unwrap();
            let original = open_app_root_inner(temp.path(), false, &security, false, 0).unwrap();
            let mounted = open_app_root_inner(&by_guid, false, &security, false, 1).unwrap();
            assert_eq!(
                crate::windows_file_id::file_id(&original.file).unwrap(),
                crate::windows_file_id::file_id(&mounted.file).unwrap(),
            );
        }
    }
}

#[cfg(all(windows, test))]
fn persist_secret_key(path: &Path, key: &SecretKey) -> Result<(), String> {
    windows_key_security::KeyStore::open(path, true)?.persist(key)
}

#[cfg(all(not(unix), not(windows)))]
fn persist_secret_key(path: &Path, key: &SecretKey) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create iroh key directory: {e}"))?;
    }
    secure_existing_key_file(path)?;
    let sealed = crate::auth::encrypt_data(&key.to_bytes())?;
    std::fs::write(path, &sealed).map_err(|e| format!("write iroh key: {e}"))
}

#[cfg(all(not(unix), not(windows)))]
fn secure_existing_key_file(path: &Path) -> Result<(), String> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_file() => Ok(()),
        Ok(_) => Err("iroh key path is not a regular file".to_string()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("inspect iroh key file: {e}")),
    }
}

/// Copy an existing root-level key only after validating its inode. The app
/// data directory is an ancestor here: its unrelated permissions stay intact.
#[cfg(unix)]
fn migrate_legacy_secret_key(
    path: &Path,
    legacy_dir: &std::fs::File,
    directory: &std::fs::File,
) -> Result<Option<SecretKey>, String> {
    if legacy_key_file_path(path).is_none() {
        return Ok(None);
    }
    let Some(mut file) = open_private_key_file(&legacy_dir, true)? else {
        return Ok(None);
    };
    let mut sealed = Vec::new();
    file.read_to_end(&mut sealed)
        .map_err(|e| format!("read legacy iroh key: {e}"))?;
    let Some(key) = decode_secret_key(&sealed) else {
        log::warn!("[codemux::web_remote] legacy iroh identity key is unreadable; regenerating");
        return Ok(None);
    };
    persist_secret_key_in(directory, &key)?;
    // Keep the 0600 legacy copy for crash recovery. A check-then-unlink would
    // race another owner-controlled rename of the legacy directory entry.
    Ok(Some(key))
}

#[cfg(windows)]
fn migrate_legacy_secret_key(path: &Path, store: &windows_key_security::KeyStore) -> Result<Option<SecretKey>, String> {
    if legacy_key_file_path(path).is_none() {
        return Ok(None);
    }
    let Some(sealed) = store.read_legacy(true)? else {
        return Ok(None);
    };
    let Some(key) = decode_secret_key(&sealed) else {
        log::warn!("[codemux::web_remote] legacy iroh identity key is unreadable; regenerating");
        return Ok(None);
    };
    store.persist(&key)?;
    // The original remains for crash recovery only after its DACL is private.
    Ok(Some(key))
}

#[cfg(all(not(unix), not(windows)))]
fn migrate_legacy_secret_key(path: &Path) -> Result<Option<SecretKey>, String> {
    let Some(legacy) = legacy_key_file_path(path) else {
        return Ok(None);
    };
    secure_existing_key_file(&legacy)?;
    let sealed = match std::fs::read(&legacy) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("read legacy iroh key: {error}")),
    };
    let Some(key) = decode_secret_key(&sealed) else {
        log::warn!("[codemux::web_remote] legacy iroh identity key is unreadable; regenerating");
        return Ok(None);
    };
    persist_secret_key(path, &key)?;
    // Windows cannot confirm that a newly created directory entry survived a
    // crash before the old name is removed. Keep this private recovery copy.
    Ok(Some(key))
}

/// Load the persisted iroh identity, or generate + persist a fresh one. A
/// corrupt/short blob is treated as absent and regenerated (the endpoint still
/// comes up, with a new `node_id`).
fn load_or_create_secret_key() -> Result<SecretKey, String> {
    let path = key_file_path();
    load_or_create_secret_key_at(&path)
}

fn load_or_create_secret_key_at(path: &Path) -> Result<SecretKey, String> {
    #[cfg(unix)]
    {
        load_or_create_secret_key_at_with_sync(path, &std::fs::File::sync_all)
    }
    #[cfg(not(unix))]
    {
        load_or_create_secret_key_at_with_sync(path)
    }
}

fn load_or_create_secret_key_at_with_sync(
    path: &Path,
    #[cfg(unix)] directory_sync: &dyn Fn(&std::fs::File) -> std::io::Result<()>,
) -> Result<SecretKey, String> {
    #[cfg(not(windows))]
    let parent = path.parent().ok_or("iroh key path has no parent")?;
    #[cfg(unix)]
    let (legacy_root, root_parents) = if let Some(legacy) = legacy_key_file_path(path) {
        let (root, parents) = open_key_dir_with_parents(
            legacy.parent().ok_or("legacy iroh key has no parent")?,
            KeyDirAccess::AncestorCreate,
        )?;
        (Some(root), parents)
    } else {
        (None, Vec::new())
    };
    #[cfg(unix)]
    let directory = if let Some(root) = legacy_root.as_ref() {
        open_private_child_dir(root, KeyDirAccess::PrivateCreate)?
            .ok_or("iroh key directory was not created")?
    } else {
        open_private_key_dir(parent)?
    };
    #[cfg(unix)]
    let sealed = read_private_key_in(&directory, true)?;
    #[cfg(windows)]
    let store = windows_key_security::KeyStore::open(path, true)?;
    #[cfg(windows)]
    let sealed = store.read_private(true)?;
    #[cfg(all(not(unix), not(windows)))]
    let sealed = {
        std::fs::create_dir_all(parent).map_err(|e| format!("create iroh key directory: {e}"))?;
        secure_existing_key_file(path)?;
        match std::fs::read(path) {
            Ok(bytes) => Some(bytes),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(format!("read iroh key file: {e}")),
        }
    };
    if let Some(sealed) = sealed {
        if let Some(key) = decode_secret_key(&sealed) {
            // The prior attempt may have failed just after renameat. A visible
            // key is not durable until its containing directory is synced.
            #[cfg(unix)]
            directory_sync(&directory)
                .map_err(|e| format!("sync existing iroh key directory: {e}"))?;
            return Ok(key);
        }
        log::warn!(
            "[codemux::web_remote] iroh identity key at {} unreadable; regenerating",
            path.display()
        );
    }
    #[cfg(unix)]
    if let Some(root) = legacy_root.as_ref() {
        if let Some(key) = migrate_legacy_secret_key(path, root, &directory)? {
            return Ok(key);
        }
    }
    #[cfg(windows)]
    if let Some(key) = migrate_legacy_secret_key(path, &store)? {
        return Ok(key);
    }
    #[cfg(all(not(unix), not(windows)))]
    if let Some(key) = migrate_legacy_secret_key(path)? {
        return Ok(key);
    }
    // The key file and its private directory are synced by persist_secret_key_in.
    // A fresh identity has no legacy recovery copy, so also flush the private
    // directory's entry in the app-data root before advertising its node ID.
    #[cfg(unix)]
    if let Some(root) = legacy_root.as_ref() {
        sync_new_key_ancestry(root, &root_parents)?;
    }
    let key = SecretKey::generate();
    #[cfg(unix)]
    persist_secret_key_in_with_sync(&directory, &key, directory_sync)?;
    #[cfg(windows)]
    store.persist(&key)?;
    #[cfg(all(not(unix), not(windows)))]
    persist_secret_key(path, &key)?;
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

fn decode_secret_key(sealed: &[u8]) -> Option<SecretKey> {
    let bytes = crate::auth::decrypt_data(sealed).ok()?;
    Some(SecretKey::from_bytes(&to_key_array(&bytes)?))
}

/// The device's stable `node_id` (endpoint id) computed from the persisted
/// identity key **without** binding an endpoint. `None` when relay mode has
/// never been enabled (no key persisted yet).
pub fn persisted_node_id() -> Option<String> {
    persisted_node_id_at(&key_file_path())
}

fn persisted_node_id_at(path: &Path) -> Option<String> {
    #[cfg(unix)]
    let (current, legacy_root) = {
        let legacy = legacy_key_file_path(path)?;
        let root = open_key_dir(legacy.parent()?, KeyDirAccess::AncestorReadOnly).ok()?;
        let current = match open_private_child_dir(&root, KeyDirAccess::PrivateReadOnly) {
            Ok(Some(directory)) => read_private_key_in(&directory, false).ok()?,
            Ok(None) => None,
            Err(_) => return None,
        };
        (current, root)
    };
    #[cfg(windows)]
    let (current, store) = {
        let store = windows_key_security::KeyStore::open(path, false).ok()?;
        let current = store.read_private(false).ok()?;
        (current, store)
    };
    #[cfg(all(not(unix), not(windows)))]
    let current = {
        match std::fs::symlink_metadata(path) {
            Ok(_) => {
                secure_existing_key_file(path).ok()?;
                Some(std::fs::read(path).ok()?)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(_) => return None,
        }
    };
    if let Some(key) = current.as_deref().and_then(decode_secret_key) {
        return Some(key.public().to_string());
    }

    // A damaged new copy can be restored from a retained legacy key at
    // startup. Status uses the same fallback without changing either file.
    #[cfg(unix)]
    let sealed = read_private_key_in(&legacy_root, false).ok()??;
    #[cfg(windows)]
    let sealed = store.read_legacy(false).ok()??;
    #[cfg(all(not(unix), not(windows)))]
    let sealed = {
        let legacy = legacy_key_file_path(path)?;
        secure_existing_key_file(&legacy).ok()?;
        std::fs::read(legacy).ok()?
    };
    Some(decode_secret_key(&sealed)?.public().to_string())
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
    /// Serialises [`start`]: the enable path, the Settings toggle, and the
    /// registration supervisor can all try to bring the endpoint up at once,
    /// and two concurrent binds would leak the loser's endpoint.
    start_lock: tokio::sync::Mutex<()>,
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
pub(crate) async fn start<R: Runtime>(app: &AppHandle<R>, shared: &Arc<Shared>) -> Result<(), String> {
    let _start = shared.iroh.start_lock.lock().await;
    if shared.iroh.is_running() {
        return Ok(());
    }
    // A caller that queued behind `start_lock` may be stale: relay mode could
    // have been switched off meanwhile. Don't bind an endpoint nobody wants.
    if !super::relay_wanted(&shared.config.lock().unwrap()) {
        return Ok(());
    }
    let key = load_or_create_secret_key()?;
    let endpoint = Endpoint::builder(presets::N0)
        .secret_key(key)
        .alpns(vec![IROH_ALPN.to_vec()])
        .bind()
        .await
        .map_err(|e| format!("iroh endpoint bind failed: {e}"))?;

    // Relay (or remote access) may have been switched off while the bind was
    // in flight. Check and install under one config-lock hold: every disable
    // writes the config before `stop` takes the endpoint, so either that stop
    // sees this install or this check sees the disable. The accept loop starts
    // only once installed, so an unwanted endpoint never accepts anything.
    let node_id = endpoint.id().to_string();
    let installed = {
        let cfg = shared.config.lock().unwrap();
        let wanted = super::relay_wanted(&cfg);
        if wanted {
            let accept = tauri::async_runtime::spawn(accept_loop(
                app.clone(),
                shared.clone(),
                endpoint.clone(),
            ));
            let running = RunningIroh {
                endpoint: endpoint.clone(),
                accept,
            };
            shared.iroh.install(running, node_id.clone());
        }
        wanted
    };
    if !installed {
        endpoint.close().await;
        return Ok(());
    }
    log::info!("[codemux::web_remote] iroh relay transport enabled: node_id={node_id}");

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
            log::info!(
                "[codemux::web_remote] iroh home relay for node {}: {relay}",
                ep.id()
            );
        });
    }
    Ok(())
}

/// Stop the iroh endpoint (abort its accept loop and close it gracefully). Live
/// iroh sessions are severed separately via the shared [`ConnectionRegistry`]
/// (`close_transport(Relay)` / `close_all`), exactly like the WS transport — this
/// just stops accepting new connections and releases the endpoint. Safe to call
/// when not running.
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
async fn accept_loop<R: Runtime>(app: AppHandle<R>, shared: Arc<Shared>, endpoint: Endpoint) {
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
async fn handle_connection<R: Runtime>(app: AppHandle<R>, shared: Arc<Shared>, conn: Connection) {
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
async fn handle_bi_stream<R: Runtime>(
    app: AppHandle<R>,
    shared: Arc<Shared>,
    mut send: SendStream,
    mut recv: RecvStream,
) {
    let mut decoder = FrameDecoder::new();
    let mut readbuf = vec![0u8; 16 * 1024];

    // 1. Handshake: the first frame either presents an existing session token
    //    (LAN/Tailscale path) or an account connection grant (from-anywhere
    //    path). The `t` field routes it; each path resolves to a `session_id`
    //    the stream then binds to, or writes an `unauthorized` frame and closes.
    let handshake = match read_frame(&mut recv, &mut decoder, &mut readbuf).await {
        Ok(Some(f)) => f,
        _ => return, // stream ended / codec error before a handshake arrived
    };
    let session_id = if is_account_handshake(&handshake) {
        // Account (grant) path: verify the grant out-of-band, then mint/reuse
        // an account session. `own_node_id` must be this endpoint's — a grant
        // for another device's node is rejected in the accept matrix.
        let own_node_id = shared.iroh.node_id().unwrap_or_default();
        let hello = match parse_account_hello(&handshake) {
            Some(h) => h,
            None => {
                let reject = AccountHandshakeReject::Malformed;
                let _ =
                    write_frame(&mut send, KIND_TEXT, &account_reject_frame_bytes(&reject)).await;
                let _ = send.finish();
                return;
            }
        };
        let admit = {
            let db = app.state::<DatabaseStore>();
            authorize_account_handshake(&db, &shared, &own_node_id, &hello).await
        };
        match admit {
            Ok(id) => id,
            Err(reject) => {
                let _ =
                    write_frame(&mut send, KIND_TEXT, &account_reject_frame_bytes(&reject)).await;
                let _ = send.finish();
                return;
            }
        }
    } else {
        // Token path: resolve the presented session token to an approved,
        // non-revoked session (same admission logic as the HTTP session gate).
        let session = {
            let db = app.state::<DatabaseStore>();
            authorize_handshake(&db, &handshake)
        };
        match session {
            Ok(s) => s.id,
            Err(reject) => {
                let _ = write_frame(&mut send, KIND_TEXT, &reject_frame_bytes(&reject)).await;
                let _ = send.finish();
                return;
            }
        }
    };

    // 2. Register in the SHARED registry the WS transport + revocation use, so
    //    close_session / close_all sever this iroh session too.
    let (close_tx, mut close_rx) = watch::channel(false);
    let (out_tx, out_rx) = super::server::outbound_channel(close_tx.clone());
    let conn_id = shared
        .connections
        .register(&session_id, crate::web_remote::server::Transport::Relay, out_tx.clone(), close_tx.clone());
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

    #[cfg(unix)]
    #[test]
    fn iroh_identity_key_is_private_on_creation_and_existing_load() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("codemux");
        let path = dir.join(KEY_FILE);
        let key = SecretKey::generate();
        persist_secret_key(&path, &key).unwrap();
        assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);

        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let loaded = load_or_create_secret_key_at(&path).unwrap();
        assert_eq!(loaded.public(), key.public(), "loading must keep the device identity");
        assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn iroh_identity_key_acl_scans_past_deny_entries() {
        let mut entries = [(ACL_EXTENDED_DENY, 1 << 1), (ACL_EXTENDED_ALLOW, 1 << 1)].into_iter();
        let error = check_key_acl_entries("iroh key", KeyAclScope::KeyFile, || Ok(entries.next()))
            .unwrap_err();
        assert!(error.contains("grants unsafe access"), "{error}");

        let mut entries = [(ACL_EXTENDED_DENY, 1 << 1), (ACL_EXTENDED_ALLOW, 1 << 3)].into_iter();
        check_key_acl_entries("iroh key", KeyAclScope::KeyFile, || Ok(entries.next())).unwrap();
        assert!(entries.next().is_none());
    }

    #[cfg(unix)]
    #[test]
    fn iroh_identity_key_acl_lookup_only_accepts_next_entry_end() {
        assert!(acl_lookup_has_entry(0, ACL_FIRST_ENTRY, None, "iroh key").unwrap());
        assert!(acl_lookup_has_entry(0, ACL_NEXT_ENTRY, None, "iroh key").unwrap());
        assert!(!acl_lookup_has_entry(-1, ACL_NEXT_ENTRY, Some(libc::EINVAL), "iroh key").unwrap());
        assert!(acl_lookup_has_entry(-1, ACL_FIRST_ENTRY, Some(libc::EINVAL), "iroh key").is_err());
        assert!(acl_lookup_has_entry(-1, ACL_NEXT_ENTRY, Some(libc::EIO), "iroh key").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn iroh_identity_key_acl_expands_generic_grants() {
        fn check(scope: KeyAclScope, rights: u64) -> Result<(), String> {
            let mut entries = [(ACL_EXTENDED_ALLOW, rights)].into_iter();
            check_key_acl_entries("iroh key", scope, || Ok(entries.next()))
        }

        for (scope, generic, specific) in [
            (KeyAclScope::KeyFile, ACL_GENERIC_READ, 1 << 1),
            (KeyAclScope::Ancestor, ACL_GENERIC_WRITE, 1 << 2),
            (KeyAclScope::PrivateDirectory, ACL_GENERIC_EXECUTE, 1 << 3),
        ] {
            assert!(check(scope, specific).unwrap_err().contains("unsafe access"));
            assert!(check(scope, generic).unwrap_err().contains("unsafe access"));
        }
        assert!(check(KeyAclScope::KeyFile, ACL_GENERIC_ALL)
            .unwrap_err()
            .contains("unsafe access"));
        check(KeyAclScope::Ancestor, 1 << 1).unwrap();
        check(KeyAclScope::Ancestor, ACL_GENERIC_READ).unwrap();
        assert!(check(KeyAclScope::KeyFile, 1 << 31)
            .unwrap_err()
            .contains("unsupported rights"));
    }

    #[cfg(unix)]
    #[test]
    fn iroh_identity_key_works_through_search_only_ancestor() {
        use std::os::unix::fs::PermissionsExt;

        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let ancestor = temp.path().join("search-only");
        let dir = ancestor.join("codemux");
        std::fs::create_dir(&ancestor).unwrap();
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join(KEY_FILE);
        std::fs::set_permissions(&ancestor, std::fs::Permissions::from_mode(0o111)).unwrap();

        let result = load_or_create_secret_key_at(&path);
        std::fs::set_permissions(&ancestor, std::fs::Permissions::from_mode(0o700)).unwrap();
        let key = result.unwrap();
        assert_eq!(load_or_create_secret_key_at(&path).unwrap().public(), key.public());
        assert_eq!(std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777, 0o700);
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);

        // The app-data root itself may be writable and searchable without
        // being readable. Status and migration must still find the old key by
        // name, and migration must preserve its identity and root mode.
        let private_path = dir.join(KEY_DIR).join(KEY_FILE);
        let node_id = key.public().to_string();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o300)).unwrap();
        std::fs::set_permissions(&ancestor, std::fs::Permissions::from_mode(0o111)).unwrap();
        let status_before = persisted_node_id_at(&private_path);
        let migrated = load_or_create_secret_key_at(&private_path);
        let status_after = persisted_node_id_at(&private_path);
        let mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        std::fs::set_permissions(&ancestor, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(status_before.as_deref(), Some(node_id.as_str()));
        assert_eq!(migrated.unwrap().public(), key.public());
        assert_eq!(status_after.as_deref(), Some(node_id.as_str()));
        assert_eq!(mode, 0o300);
        assert!(path.exists(), "search-only parent keeps the private recovery copy");
        std::fs::write(&private_path, b"incomplete replacement").unwrap();
        assert_eq!(persisted_node_id_at(&private_path).as_deref(), Some(node_id.as_str()));
        assert_eq!(load_or_create_secret_key_at(&private_path).unwrap().public(), key.public());
    }

    #[cfg(unix)]
    #[test]
    fn iroh_identity_key_first_start_persists_through_new_app_root() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("codemux").join(KEY_DIR).join(KEY_FILE);
        assert!(!path.parent().unwrap().parent().unwrap().exists());

        let created = load_or_create_secret_key_at(&path).unwrap();
        assert!(path.exists());
        assert_eq!(
            load_or_create_secret_key_at(&path).unwrap().public(),
            created.public()
        );
        let node_id = created.public().to_string();
        assert_eq!(
            persisted_node_id_at(&path).as_deref(),
            Some(node_id.as_str())
        );
    }

    #[cfg(unix)]
    #[test]
    fn iroh_identity_key_first_start_below_established_search_only_ancestor() {
        use std::os::unix::fs::PermissionsExt;

        if unsafe { libc::geteuid() } == 0 {
            return;
        }

        let temp = tempfile::tempdir().unwrap();
        let search_only = temp.path().join("search-only");
        let readable = search_only.join("readable");
        std::fs::create_dir_all(&readable).unwrap();
        std::fs::File::open(&search_only).unwrap().sync_all().unwrap();
        std::fs::File::open(temp.path()).unwrap().sync_all().unwrap();
        let path = readable.join("codemux").join(KEY_DIR).join(KEY_FILE);
        std::fs::set_permissions(&search_only, std::fs::Permissions::from_mode(0o111)).unwrap();
        let result = load_or_create_secret_key_at(&path);
        std::fs::set_permissions(&search_only, std::fs::Permissions::from_mode(0o700)).unwrap();
        let key = result.unwrap();
        assert!(path.exists());
        assert_eq!(
            load_or_create_secret_key_at(&path).unwrap().public(),
            key.public()
        );
    }

    #[cfg(unix)]
    #[test]
    fn iroh_identity_key_first_start_below_foreign_group_writable_ancestor() {
        use std::os::unix::fs::PermissionsExt;

        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let ancestor = temp.path().join("search-only");
        let readable = ancestor.join("readable");
        std::fs::create_dir_all(&readable).unwrap();
        std::fs::File::open(&ancestor).unwrap().sync_all().unwrap();
        std::fs::File::open(temp.path()).unwrap().sync_all().unwrap();
        let path = readable.join("codemux").join(KEY_DIR).join(KEY_FILE);
        // The owner has search but no write access. The group write bit is
        // irrelevant to this process, just as for an unrelated root-owned group.
        std::fs::set_permissions(&ancestor, std::fs::Permissions::from_mode(0o1131)).unwrap();
        let result = load_or_create_secret_key_at(&path);
        std::fs::set_permissions(&ancestor, std::fs::Permissions::from_mode(0o700)).unwrap();
        let key = result.unwrap();
        assert!(path.exists());
        assert_eq!(load_or_create_secret_key_at(&path).unwrap().public(), key.public());
    }

    #[cfg(unix)]
    #[test]
    fn iroh_identity_key_read_only_ancestor_stops_sync_walk() {
        assert!(!ancestor_write_access_result(Err(std::io::Error::from_raw_os_error(libc::EROFS))).unwrap());
        assert!(ancestor_write_access_result(Ok(())).unwrap());
        assert!(ancestor_write_access_result(Err(std::io::Error::from_raw_os_error(libc::EIO))).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn iroh_identity_key_retry_syncs_key_left_by_failed_publication() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("codemux").join(KEY_DIR).join(KEY_FILE);
        let fail_sync =
            |_: &std::fs::File| Err(std::io::Error::other("injected directory sync failure"));

        let first = load_or_create_secret_key_at_with_sync(&path, &fail_sync).unwrap_err();
        assert!(first.contains("sync iroh key directory"), "{first}");
        let visible = decode_secret_key(&std::fs::read(&path).unwrap()).unwrap();

        let retry = load_or_create_secret_key_at_with_sync(&path, &fail_sync).unwrap_err();
        assert!(
            retry.contains("sync existing iroh key directory"),
            "{retry}"
        );
        let recovered = load_or_create_secret_key_at(&path).unwrap();
        assert_eq!(recovered.public(), visible.public());
    }

    #[cfg(unix)]
    #[test]
    fn iroh_identity_key_fresh_creation_needs_syncable_app_root() {
        use std::os::unix::fs::PermissionsExt;

        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("codemux");
        std::fs::create_dir(&root).unwrap();
        let path = root.join(KEY_DIR).join(KEY_FILE);
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o300)).unwrap();
        let result = load_or_create_secret_key_at(&path);
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        let error = result.unwrap_err();
        assert!(error.contains("for sync"), "{error}");
        assert!(!path.exists(), "no fresh identity is published without parent sync");

        let legacy_key = SecretKey::generate();
        let legacy = root.join(KEY_FILE);
        std::fs::write(&legacy, crate::auth::encrypt_data(&legacy_key.to_bytes()).unwrap()).unwrap();
        std::fs::set_permissions(&legacy, std::fs::Permissions::from_mode(0o600)).unwrap();
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o300)).unwrap();
        let migrated = load_or_create_secret_key_at(&path);
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(migrated.unwrap().public(), legacy_key.public());
    }

    #[cfg(unix)]
    #[test]
    fn iroh_identity_key_new_app_root_needs_syncable_parent() {
        use std::os::unix::fs::PermissionsExt;

        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let parent = temp.path().join("search-only");
        std::fs::create_dir(&parent).unwrap();
        let path = parent.join("codemux").join(KEY_DIR).join(KEY_FILE);
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o300)).unwrap();
        let result = load_or_create_secret_key_at(&path);
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o700)).unwrap();
        let error = result.unwrap_err();
        assert!(error.contains("for sync"), "{error}");
        assert!(!path.exists(), "no fresh identity is published without ancestor sync");

        // A retry must not treat the unsynced but still-present app-data
        // directory as durable merely because this invocation did not create it.
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o300)).unwrap();
        let retry = load_or_create_secret_key_at(&path);
        std::fs::set_permissions(&parent, std::fs::Permissions::from_mode(0o700)).unwrap();
        assert!(retry.unwrap_err().contains("for sync"));
        assert!(!path.exists());
    }

    #[cfg(unix)]
    #[test]
    fn iroh_identity_key_load_rejects_symlink() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("codemux");
        std::fs::create_dir(&dir).unwrap();
        let target = temp.path().join("other-file");
        std::fs::write(&target, b"not a key").unwrap();
        std::os::unix::fs::symlink(&target, dir.join(KEY_FILE)).unwrap();
        let err = load_or_create_secret_key_at(&dir.join(KEY_FILE)).unwrap_err();
        assert!(err.contains("not a regular file"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn iroh_identity_key_accepts_owned_directory_relocation_without_redirecting_io() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let first = temp.path().join("first");
        let second = temp.path().join("second");
        std::fs::create_dir(&first).unwrap();
        std::fs::create_dir(&second).unwrap();
        let relocated = temp.path().join("codemux");
        std::os::unix::fs::symlink(&first, &relocated).unwrap();
        let path = relocated.join(KEY_FILE);
        let original = SecretKey::generate();
        persist_secret_key(&path, &original).unwrap();
        assert_eq!(
            load_or_create_secret_key_at(&path).unwrap().public(),
            original.public()
        );
        assert_eq!(
            std::fs::metadata(&first).unwrap().permissions().mode() & 0o777,
            0o700
        );

        // The path changes after validation, but the open descriptor still
        // writes only to the original user-owned directory.
        let directory = open_private_key_dir(&relocated).unwrap();
        std::fs::remove_file(&relocated).unwrap();
        std::os::unix::fs::symlink(&second, &relocated).unwrap();
        let replacement = SecretKey::generate();
        persist_secret_key_in(&directory, &replacement).unwrap();
        assert_eq!(
            load_or_create_secret_key_at(&first.join(KEY_FILE)).unwrap().public(),
            replacement.public()
        );
        assert!(!second.join(KEY_FILE).exists());
    }

    #[cfg(unix)]
    #[test]
    fn iroh_identity_key_migrates_without_changing_relocated_app_directory() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let data = temp.path().join("data");
        let relocated = temp.path().join("relocated-app-data");
        std::fs::create_dir(&data).unwrap();
        std::fs::create_dir(&relocated).unwrap();
        std::fs::set_permissions(&relocated, std::fs::Permissions::from_mode(0o750)).unwrap();
        let app_dir = data.join("codemux");
        std::os::unix::fs::symlink(&relocated, &app_dir).unwrap();
        let unrelated = relocated.join("unrelated.txt");
        std::fs::write(&unrelated, b"keep").unwrap();
        std::fs::set_permissions(&unrelated, std::fs::Permissions::from_mode(0o640)).unwrap();

        let path = app_dir.join(KEY_DIR).join(KEY_FILE);
        assert!(persisted_node_id_at(&path).is_none());
        assert!(!relocated.join(KEY_DIR).exists());
        let original = SecretKey::generate();
        let node_id = original.public().to_string();
        let legacy = relocated.join(KEY_FILE);
        std::fs::write(&legacy, crate::auth::encrypt_data(&original.to_bytes()).unwrap()).unwrap();
        std::fs::set_permissions(&legacy, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(persisted_node_id_at(&path).as_deref(), Some(node_id.as_str()));
        assert!(!relocated.join(KEY_DIR).exists());

        let loaded = load_or_create_secret_key_at(&path).unwrap();
        assert_eq!(loaded.public(), original.public());
        assert_eq!(persisted_node_id_at(&path).as_deref(), Some(node_id.as_str()));
        assert!(legacy.exists(), "the private legacy key remains for crash recovery");
        assert_eq!(std::fs::metadata(&relocated).unwrap().permissions().mode() & 0o777, 0o750);
        assert_eq!(std::fs::metadata(&unrelated).unwrap().permissions().mode() & 0o777, 0o640);
        assert_eq!(std::fs::metadata(relocated.join(KEY_DIR)).unwrap().permissions().mode() & 0o777, 0o700);
        assert_eq!(std::fs::metadata(relocated.join(KEY_DIR).join(KEY_FILE)).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::read(&unrelated).unwrap(), b"keep");
    }

    #[cfg(unix)]
    #[test]
    fn iroh_identity_key_migration_uses_pinned_app_data_root_after_relocation() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let first = temp.path().join("first");
        let second = temp.path().join("second");
        std::fs::create_dir(&first).unwrap();
        std::fs::create_dir(&second).unwrap();
        let configured = temp.path().join("codemux");
        std::os::unix::fs::symlink(&first, &configured).unwrap();
        let path = configured.join(KEY_DIR).join(KEY_FILE);
        let first_key = SecretKey::generate();
        let second_key = SecretKey::generate();
        for (root, key) in [(&first, &first_key), (&second, &second_key)] {
            let legacy = root.join(KEY_FILE);
            std::fs::write(&legacy, crate::auth::encrypt_data(&key.to_bytes()).unwrap()).unwrap();
            std::fs::set_permissions(&legacy, std::fs::Permissions::from_mode(0o600)).unwrap();
        }

        let root = open_key_dir(&configured, KeyDirAccess::AncestorReadOnly).unwrap();
        let private = open_private_child_dir(&root, KeyDirAccess::PrivateCreate)
            .unwrap()
            .unwrap();
        std::fs::remove_file(&configured).unwrap();
        std::os::unix::fs::symlink(&second, &configured).unwrap();

        let migrated = migrate_legacy_secret_key(&path, &root, &private).unwrap().unwrap();
        assert_eq!(migrated.public(), first_key.public());
        assert_eq!(load_or_create_secret_key_at(&first.join(KEY_DIR).join(KEY_FILE)).unwrap().public(), first_key.public());
        assert!(!second.join(KEY_DIR).exists());
        assert_eq!(
            decode_secret_key(&std::fs::read(first.join(KEY_FILE)).unwrap()).unwrap().public(),
            first_key.public()
        );
        assert_eq!(
            decode_secret_key(&std::fs::read(second.join(KEY_FILE)).unwrap()).unwrap().public(),
            second_key.public()
        );
    }

    #[cfg(unix)]
    #[test]
    fn iroh_identity_key_rejects_private_child_relocation_without_selecting_foreign_legacy_key() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let configured = temp.path().join("codemux");
        let foreign = temp.path().join("foreign");
        std::fs::create_dir(&configured).unwrap();
        std::fs::create_dir(&foreign).unwrap();
        std::fs::create_dir(foreign.join(KEY_DIR)).unwrap();
        let own_key = SecretKey::generate();
        let foreign_key = SecretKey::generate();
        for (root, key) in [(&configured, &own_key), (&foreign, &foreign_key)] {
            let legacy = root.join(KEY_FILE);
            std::fs::write(&legacy, crate::auth::encrypt_data(&key.to_bytes()).unwrap()).unwrap();
            std::fs::set_permissions(&legacy, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let child = configured.join(KEY_DIR);
        std::os::unix::fs::symlink(foreign.join(KEY_DIR), &child).unwrap();
        let path = child.join(KEY_FILE);

        assert!(persisted_node_id_at(&path).is_none());
        let error = load_or_create_secret_key_at(&path).unwrap_err();
        assert!(error.contains("not a directory"), "{error}");
        assert!(!foreign.join(KEY_DIR).join(KEY_FILE).exists());
        assert_eq!(
            decode_secret_key(&std::fs::read(configured.join(KEY_FILE)).unwrap()).unwrap().public(),
            own_key.public()
        );
        assert_eq!(
            decode_secret_key(&std::fs::read(foreign.join(KEY_FILE)).unwrap()).unwrap().public(),
            foreign_key.public()
        );

        std::fs::remove_file(child).unwrap();
        let own_id = own_key.public().to_string();
        assert_eq!(persisted_node_id_at(&path).as_deref(), Some(own_id.as_str()));
        assert_eq!(load_or_create_secret_key_at(&path).unwrap().public(), own_key.public());
        assert!(!foreign.join(KEY_DIR).join(KEY_FILE).exists());
    }

    #[cfg(windows)]
    #[test]
    fn iroh_identity_key_migration_retains_private_legacy_recovery_copy() {
        let temp = tempfile::tempdir().unwrap();
        let app_dir = temp.path().join("codemux");
        std::fs::create_dir(&app_dir).unwrap();
        let legacy = app_dir.join(KEY_FILE);
        let path = app_dir.join(KEY_DIR).join(KEY_FILE);
        let original = SecretKey::generate();
        let node_id = original.public().to_string();
        std::fs::write(&legacy, crate::auth::encrypt_data(&original.to_bytes()).unwrap()).unwrap();

        assert_eq!(load_or_create_secret_key_at(&path).unwrap().public(), original.public());
        assert!(legacy.exists(), "the old key remains available for crash recovery");
        assert!(path.exists());

        // Even if the replacement is lost or damaged, startup must recover
        // the same node ID from the private legacy copy.
        std::fs::write(&path, b"incomplete replacement").unwrap();
        assert_eq!(persisted_node_id_at(&path).as_deref(), Some(node_id.as_str()));
        assert_eq!(load_or_create_secret_key_at(&path).unwrap().public(), original.public());
    }

    #[cfg(windows)]
    #[test]
    fn iroh_identity_key_publishes_temp_name_beyond_max_path() {
        use std::os::windows::ffi::OsStrExt;

        let temp = tempfile::tempdir().unwrap();
        let mut app_dir = temp.path().join("codemux");
        let path_length = |path: &Path| path.as_os_str().encode_wide().count();
        while path_length(&app_dir.join(KEY_DIR).join(KEY_FILE)) < 230 {
            app_dir.push("nested-directory");
        }
        let path = app_dir.join(KEY_DIR).join(KEY_FILE);
        let temporary = path.parent().unwrap().join(format!(".{KEY_FILE}.{}.tmp", uuid::Uuid::nil()));
        assert!(path_length(&path) < 260, "the destination should fit MAX_PATH");
        assert!(path_length(&temporary) > 260, "the temporary name should exceed MAX_PATH");

        let key = SecretKey::generate();
        persist_secret_key(&path, &key).unwrap();
        assert_eq!(load_or_create_secret_key_at(&path).unwrap().public(), key.public());
    }

    #[cfg(windows)]
    #[test]
    fn iroh_identity_key_migration_restricts_inherited_windows_read_grant() {
        let temp = tempfile::tempdir().unwrap();
        let app_dir = temp.path().join("codemux");
        std::fs::create_dir(&app_dir).unwrap();
        windows_key_security::inherited_grant_fixture(&app_dir, "GR").unwrap();
        let legacy = app_dir.join(KEY_FILE);
        let path = app_dir.join(KEY_DIR).join(KEY_FILE);
        let original = SecretKey::generate();
        let node_id = original.public().to_string();
        std::fs::write(&legacy, crate::auth::encrypt_data(&original.to_bytes()).unwrap()).unwrap();
        assert!(windows_key_security::key_ace_count(&legacy, false).unwrap() > 3);

        assert_eq!(load_or_create_secret_key_at(&path).unwrap().public(), original.public());
        assert_eq!(persisted_node_id_at(&path).as_deref(), Some(node_id.as_str()));
        assert!(legacy.exists(), "the now-private legacy key remains for crash recovery");
        assert_eq!(windows_key_security::key_ace_count(&legacy, false).unwrap(), 3);
        assert_eq!(windows_key_security::key_ace_count(path.parent().unwrap(), true).unwrap(), 3);
        assert_eq!(windows_key_security::key_ace_count(&path, false).unwrap(), 3);
    }

    #[cfg(windows)]
    #[test]
    fn iroh_identity_key_migrates_through_trusted_junction() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("relocated-app-data");
        std::fs::create_dir(&target).unwrap();
        let junction = temp.path().join("codemux");
        let made = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&junction)
            .arg(&target)
            .output()
            .unwrap();
        assert!(made.status.success(), "{}", String::from_utf8_lossy(&made.stderr));
        windows_key_security::inherited_grant_fixture(&junction, "GR").unwrap();
        windows_key_security::inherited_grant_fixture(&target, "GR").unwrap();

        let key = SecretKey::generate();
        let node_id = key.public().to_string();
        std::fs::write(target.join(KEY_FILE), crate::auth::encrypt_data(&key.to_bytes()).unwrap()).unwrap();
        let path = junction.join(KEY_DIR).join(KEY_FILE);
        assert_eq!(load_or_create_secret_key_at(&path).unwrap().public(), key.public());
        assert_eq!(persisted_node_id_at(&path).as_deref(), Some(node_id.as_str()));
        assert!(target.join(KEY_DIR).join(KEY_FILE).exists());
        assert!(target.join(KEY_FILE).exists(), "the legacy key remains for crash recovery");
    }

    #[cfg(windows)]
    #[test]
    fn iroh_identity_key_migrates_through_absolute_directory_symlink() {
        let temp = tempfile::tempdir().unwrap();
        let target = temp.path().join("relocated-app-data");
        std::fs::create_dir(&target).unwrap();
        let link = temp.path().join("codemux");
        match std::os::windows::fs::symlink_dir(&target, &link) {
            Ok(()) => {}
            Err(error) if error.raw_os_error() == Some(1314) => return, // Symlink privilege is unavailable.
            Err(error) => panic!("create directory symlink: {error}"),
        }
        windows_key_security::inherited_grant_fixture(&link, "GR").unwrap();
        windows_key_security::inherited_grant_fixture(&target, "GR").unwrap();

        let key = SecretKey::generate();
        let node_id = key.public().to_string();
        std::fs::write(target.join(KEY_FILE), crate::auth::encrypt_data(&key.to_bytes()).unwrap()).unwrap();
        let path = link.join(KEY_DIR).join(KEY_FILE);
        assert_eq!(load_or_create_secret_key_at(&path).unwrap().public(), key.public());
        assert_eq!(persisted_node_id_at(&path).as_deref(), Some(node_id.as_str()));
        assert!(target.join(KEY_DIR).join(KEY_FILE).exists());
    }

    #[cfg(windows)]
    #[test]
    fn iroh_identity_key_resolves_relative_symlink_after_junction() {
        let temp = tempfile::tempdir().unwrap();
        let real = temp.path().join("one").join("real");
        let target = temp.path().join("one").join("relocated");
        std::fs::create_dir_all(&real).unwrap();
        std::fs::create_dir(&target).unwrap();
        let alias = temp.path().join("alias");
        let made = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&alias)
            .arg(&real)
            .output()
            .unwrap();
        assert!(made.status.success(), "{}", String::from_utf8_lossy(&made.stderr));
        let link = real.join("codemux");
        match std::os::windows::fs::symlink_dir(r"..\relocated", &link) {
            Ok(()) => {}
            Err(error) if error.raw_os_error() == Some(1314) => return, // Symlink privilege is unavailable.
            Err(error) => panic!("create relative directory symlink: {error}"),
        }
        for directory in [&alias, &link, &target] {
            windows_key_security::inherited_grant_fixture(directory, "GR").unwrap();
        }

        let key = SecretKey::generate();
        let node_id = key.public().to_string();
        std::fs::write(target.join(KEY_FILE), crate::auth::encrypt_data(&key.to_bytes()).unwrap()).unwrap();
        let path = alias.join("codemux").join(KEY_DIR).join(KEY_FILE);
        assert_eq!(load_or_create_secret_key_at(&path).unwrap().public(), key.public());
        assert_eq!(persisted_node_id_at(&path).as_deref(), Some(node_id.as_str()));
        assert!(target.join(KEY_DIR).join(KEY_FILE).exists());
    }

    #[cfg(windows)]
    #[test]
    fn iroh_identity_key_rejects_junction_target_through_replaceable_ancestor() {
        let temp = tempfile::tempdir().unwrap();
        let shared = temp.path().join("shared");
        let target = shared.join("relocated-app-data");
        std::fs::create_dir_all(&target).unwrap();
        windows_key_security::inherited_grant_fixture(&shared, "0x00000040").unwrap();
        windows_key_security::inherited_grant_fixture(&target, "GR").unwrap();
        let junction = temp.path().join("codemux");
        let made = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&junction)
            .arg(&target)
            .output()
            .unwrap();
        assert!(made.status.success(), "{}", String::from_utf8_lossy(&made.stderr));
        windows_key_security::inherited_grant_fixture(&junction, "GR").unwrap();

        let path = junction.join(KEY_DIR).join(KEY_FILE);
        let error = load_or_create_secret_key_at(&path).unwrap_err();
        assert!(error.contains("grants another user write access"), "{error}");
        assert!(!target.join(KEY_DIR).exists());
    }

    #[cfg(windows)]
    #[test]
    fn iroh_identity_key_rejects_app_root_writable_by_other_users() {
        let temp = tempfile::tempdir().unwrap();
        let app_dir = temp.path().join("codemux");
        std::fs::create_dir(&app_dir).unwrap();
        windows_key_security::inherited_grant_fixture(&app_dir, "GW").unwrap();
        let path = app_dir.join(KEY_DIR).join(KEY_FILE);

        let error = load_or_create_secret_key_at(&path).unwrap_err();
        assert!(error.contains("grants another user write access"), "{error}");
        assert!(!path.exists());
    }

    #[cfg(windows)]
    #[test]
    fn iroh_identity_key_rejects_replaceable_app_root() {
        let temp = tempfile::tempdir().unwrap();
        let shared = temp.path().join("shared");
        std::fs::create_dir(&shared).unwrap();
        // SDDL's DC alias is a directory-service right, not FILE_DELETE_CHILD.
        windows_key_security::inherited_grant_fixture(&shared, "0x00000040").unwrap();
        let app_dir = shared.join("codemux");
        std::fs::create_dir(&app_dir).unwrap();
        windows_key_security::inherited_grant_fixture(&app_dir, "GR").unwrap();
        let path = app_dir.join(KEY_DIR).join(KEY_FILE);

        let error = load_or_create_secret_key_at(&path).unwrap_err();
        assert!(error.contains("grants another user write access"), "{error}");
        assert!(!path.exists());
    }

    #[cfg(windows)]
    #[test]
    fn iroh_identity_key_rejects_replaceable_earlier_ancestor() {
        let temp = tempfile::tempdir().unwrap();
        let outer = temp.path().join("outer");
        let inner = outer.join("inner");
        let app_dir = inner.join("codemux");
        std::fs::create_dir_all(&app_dir).unwrap();
        // The immediate parent and app root are safe. A different user can
        // replace the earlier entry through DELETE_CHILD on its parent.
        windows_key_security::inherited_grant_fixture(&outer, "0x00000040").unwrap();
        windows_key_security::inherited_grant_fixture(&inner, "GR").unwrap();
        windows_key_security::inherited_grant_fixture(&app_dir, "GR").unwrap();
        let path = app_dir.join(KEY_DIR).join(KEY_FILE);

        let error = load_or_create_secret_key_at(&path).unwrap_err();
        assert!(error.contains("grants another user write access"), "{error}");
        assert!(!path.exists());
    }

    #[cfg(unix)]
    #[test]
    fn iroh_status_lookup_never_tightens_unsafe_key_modes() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let app_dir = temp.path().join("codemux");
        std::fs::create_dir(&app_dir).unwrap();
        std::fs::set_permissions(&app_dir, std::fs::Permissions::from_mode(0o750)).unwrap();
        let path = app_dir.join(KEY_DIR).join(KEY_FILE);
        let original = SecretKey::generate();
        let legacy = app_dir.join(KEY_FILE);
        std::fs::write(&legacy, crate::auth::encrypt_data(&original.to_bytes()).unwrap()).unwrap();
        std::fs::set_permissions(&legacy, std::fs::Permissions::from_mode(0o640)).unwrap();

        assert!(persisted_node_id_at(&path).is_none());
        assert!(!app_dir.join(KEY_DIR).exists());
        assert_eq!(std::fs::metadata(&app_dir).unwrap().permissions().mode() & 0o777, 0o750);
        assert_eq!(std::fs::metadata(&legacy).unwrap().permissions().mode() & 0o777, 0o640);

        std::fs::create_dir(app_dir.join(KEY_DIR)).unwrap();
        std::fs::set_permissions(app_dir.join(KEY_DIR), std::fs::Permissions::from_mode(0o750)).unwrap();
        assert!(persisted_node_id_at(&path).is_none());
        assert_eq!(std::fs::metadata(app_dir.join(KEY_DIR)).unwrap().permissions().mode() & 0o777, 0o750);

        let loaded = load_or_create_secret_key_at(&path).unwrap();
        assert_eq!(loaded.public(), original.public());
        assert_eq!(std::fs::metadata(&app_dir).unwrap().permissions().mode() & 0o777, 0o750);
        assert_eq!(std::fs::metadata(app_dir.join(KEY_DIR)).unwrap().permissions().mode() & 0o777, 0o700);
        assert!(legacy.exists(), "the private legacy key remains for crash recovery");
    }

    #[cfg(unix)]
    #[test]
    fn iroh_identity_key_rejects_relocation_through_writable_ancestor() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let shared = temp.path().join("shared");
        let target = temp.path().join("target");
        std::fs::create_dir(&shared).unwrap();
        std::fs::create_dir(&target).unwrap();
        std::fs::set_permissions(temp.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o777)).unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::os::unix::fs::symlink(&target, shared.join("codemux")).unwrap();

        let error = load_or_create_secret_key_at(&shared.join("codemux").join(KEY_FILE))
            .unwrap_err();
        assert!(error.contains("replace entries"), "{error}");
        assert_eq!(
            std::fs::metadata(&target).unwrap().permissions().mode() & 0o777,
            0o755
        );
        assert!(!target.join(KEY_FILE).exists());
    }

    #[cfg(unix)]
    #[test]
    fn iroh_identity_key_allows_owned_entry_under_sticky_directory() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let shared = temp.path().join("shared");
        let target = temp.path().join("target");
        std::fs::create_dir(&shared).unwrap();
        std::fs::create_dir(&target).unwrap();
        std::fs::set_permissions(&shared, std::fs::Permissions::from_mode(0o1777)).unwrap();
        std::os::unix::fs::symlink(&target, shared.join("codemux")).unwrap();

        let path = shared.join("codemux").join(KEY_FILE);
        let key = load_or_create_secret_key_at(&path).unwrap();
        assert_eq!(load_or_create_secret_key_at(&path).unwrap().public(), key.public());
    }

    #[cfg(unix)]
    #[test]
    fn iroh_identity_key_rejects_hardlinked_legacy_file_without_changing_it() {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("codemux");
        std::fs::create_dir(&dir).unwrap();
        let target = temp.path().join("other-file");
        std::fs::write(&target, b"legacy content").unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o644)).unwrap();
        let path = dir.join(KEY_FILE);
        std::fs::hard_link(&target, &path).unwrap();

        let error = load_or_create_secret_key_at(&path).unwrap_err();
        assert!(error.contains("singly linked"), "{error}");
        let error = persist_secret_key(&path, &SecretKey::generate()).unwrap_err();
        assert!(error.contains("singly linked"), "{error}");
        assert_eq!(std::fs::read(&target).unwrap(), b"legacy content");
        assert_eq!(std::fs::metadata(&target).unwrap().permissions().mode() & 0o777, 0o644);
    }

    #[cfg(unix)]
    #[test]
    fn iroh_identity_key_replaces_invalid_file_without_truncating_old_inode() {
        use std::io::Read;
        use std::os::unix::fs::MetadataExt;

        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("codemux");
        std::fs::create_dir(&dir).unwrap();
        let path = dir.join(KEY_FILE);
        std::fs::write(&path, b"corrupt key").unwrap();
        let mut old = std::fs::File::open(&path).unwrap();
        let old_ino = old.metadata().unwrap().ino();

        let key = load_or_create_secret_key_at(&path).unwrap();
        assert_ne!(std::fs::metadata(&path).unwrap().ino(), old_ino);
        let mut unchanged = Vec::new();
        old.read_to_end(&mut unchanged).unwrap();
        assert_eq!(unchanged, b"corrupt key");
        assert_eq!(load_or_create_secret_key_at(&path).unwrap().public(), key.public());
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

    // ── Account (grant) handshake ─────────────────────────────────────

    const OWN_NODE: &str = "own-node-id-abc";
    const OWN_UID: &str = "user-owner-1";

    fn account_hello_frame(grant: &str, nonce: &str) -> Frame {
        Frame {
            kind: KIND_TEXT,
            payload: json!({ "t": "hello-account", "grant": grant, "nonce": nonce })
                .to_string()
                .into_bytes(),
        }
    }

    /// A grant-verify response that would ADMIT: valid, targets our node, our
    /// account, and echoes the given nonce. Tests mutate one field to exercise
    /// each reject branch.
    fn good_verify(nonce: &str) -> GrantVerifyResponse {
        GrantVerifyResponse {
            valid: true,
            user_id: Some(OWN_UID.to_string()),
            device_id: Some("dev-1".to_string()),
            node_id: Some(OWN_NODE.to_string()),
            nonce: Some(nonce.to_string()),
        }
    }

    #[test]
    fn is_account_handshake_routes_only_hello_account() {
        assert!(is_account_handshake(&account_hello_frame("g", "n")));
        // The token hello and anything else route to the token path.
        assert!(!is_account_handshake(&hello("tok")));
        assert!(!is_account_handshake(&Frame {
            kind: KIND_TEXT,
            payload: b"not json".to_vec(),
        }));
        // A binary first frame is never an account handshake.
        assert!(!is_account_handshake(&Frame { kind: KIND_BINARY, payload: vec![1, 2] }));
    }

    #[test]
    fn parse_account_hello_extracts_grant_and_nonce() {
        let parsed = parse_account_hello(&account_hello_frame("the-grant", "the-nonce")).unwrap();
        assert_eq!(parsed.grant, "the-grant");
        assert_eq!(parsed.nonce, "the-nonce");
        // Missing/empty grant is malformed.
        assert!(parse_account_hello(&account_hello_frame("", "n")).is_none());
        assert!(parse_account_hello(&Frame {
            kind: KIND_TEXT,
            payload: json!({ "t": "hello-account", "nonce": "n" }).to_string().into_bytes(),
        })
        .is_none());
    }

    #[test]
    fn evaluate_grant_admits_matching_valid_grant() {
        let id = evaluate_grant(&good_verify("nonce-1"), OWN_NODE, Some(OWN_UID), "nonce-1")
            .expect("a valid, matching grant is admitted");
        assert_eq!(id.user_id, OWN_UID);
        assert_eq!(id.device_id.as_deref(), Some("dev-1"));
    }

    #[test]
    fn evaluate_grant_rejects_invalid_grant() {
        // valid:false is how a bad signature OR an expired grant surfaces from
        // the verify endpoint — both collapse to `Unverified` here.
        let mut resp = good_verify("n");
        resp.valid = false;
        assert_eq!(
            evaluate_grant(&resp, OWN_NODE, Some(OWN_UID), "n").unwrap_err(),
            AccountHandshakeReject::Unverified
        );
    }

    #[test]
    fn evaluate_grant_rejects_grant_for_another_node() {
        let mut resp = good_verify("n");
        resp.node_id = Some("some-other-node".to_string());
        assert_eq!(
            evaluate_grant(&resp, OWN_NODE, Some(OWN_UID), "n").unwrap_err(),
            AccountHandshakeReject::WrongNode
        );
    }

    #[test]
    fn evaluate_grant_rejects_wrong_account_and_signed_out() {
        // A grant for a different account.
        let mut resp = good_verify("n");
        resp.user_id = Some("intruder-account".to_string());
        assert_eq!(
            evaluate_grant(&resp, OWN_NODE, Some(OWN_UID), "n").unwrap_err(),
            AccountHandshakeReject::WrongAccount
        );
        // Desktop signed out (no own uid) → nothing to match → reject, even for
        // an otherwise-valid grant.
        assert_eq!(
            evaluate_grant(&good_verify("n"), OWN_NODE, None, "n").unwrap_err(),
            AccountHandshakeReject::WrongAccount
        );
    }

    #[test]
    fn evaluate_grant_rejects_nonce_mismatch() {
        // The grant is bound to nonce "bound", but the browser presented "other"
        // — a replay / grant-substitution signal.
        assert_eq!(
            evaluate_grant(&good_verify("bound"), OWN_NODE, Some(OWN_UID), "other").unwrap_err(),
            AccountHandshakeReject::NonceMismatch
        );
    }

    #[test]
    fn account_reject_codes_are_stable() {
        assert_eq!(AccountHandshakeReject::Malformed.code(), "malformed_handshake");
        assert_eq!(AccountHandshakeReject::Unverified.code(), "grant_invalid");
        assert_eq!(AccountHandshakeReject::WrongNode.code(), "wrong_device");
        assert_eq!(AccountHandshakeReject::WrongAccount.code(), "account_mismatch");
        assert_eq!(AccountHandshakeReject::NonceMismatch.code(), "nonce_mismatch");
        assert_eq!(AccountHandshakeReject::Pending.code(), "pending_approval");
        assert_eq!(
            AccountHandshakeReject::Internal("x".into()).code(),
            "internal_error"
        );
    }

    // ── Full account admission: grant verify (mocked) → session mint ──
    //
    // These mock `POST /api/devices/grant/verify` via mockito + `CODEMUX_API_URL`
    // and NEVER hit the real API. `CODEMUX_API_URL` mutation is serialized under
    // the shared `mockserver` group with the auth/account tests.

    fn seed_owner(db: &DatabaseStore) {
        use crate::auth::{save_auth, AuthUser};
        let user = AuthUser {
            id: OWN_UID.to_string(),
            email: "owner@example.com".to_string(),
            name: None,
            image: None,
        };
        save_auth(db, "owner-token", "2099-01-01T00:00:00Z", Some(&user)).unwrap();
    }

    fn shared_with_trust(trust: bool) -> Arc<Shared> {
        let shared = Arc::new(Shared::default());
        {
            let mut cfg = shared.config.lock().unwrap();
            cfg.enabled = true;
            cfg.relay_mode_enabled = true;
            cfg.trust_account_browsers = trust;
        }
        shared
    }

    /// Mock the verify endpoint to echo a full admit response for `grant`.
    async fn mock_verify_admit(server: &mut mockito::Server, nonce: &str) -> mockito::Mock {
        server
            .mock("POST", "/api/devices/grant/verify")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "valid": true,
                    "userId": OWN_UID,
                    "deviceId": "dev-1",
                    "nodeId": OWN_NODE,
                    "nonce": nonce,
                })
                .to_string(),
            )
            .create_async()
            .await
    }

    async fn run_account_admit(
        api_url: &str,
        db: &DatabaseStore,
        shared: &Arc<Shared>,
        hello: &AccountHello,
    ) -> Result<String, AccountHandshakeReject> {
        let prev = std::env::var("CODEMUX_API_URL").ok();
        std::env::set_var("CODEMUX_API_URL", api_url);
        let out = authorize_account_handshake(db, shared, OWN_NODE, hello).await;
        match prev {
            Some(v) => std::env::set_var("CODEMUX_API_URL", v),
            None => std::env::remove_var("CODEMUX_API_URL"),
        }
        out
    }

    #[tokio::test]
    #[serial_test::serial(mockserver)]
    async fn account_handshake_admits_good_grant_and_mints_account_session() {
        let db = init_test_database();
        seed_owner(&db);
        let shared = shared_with_trust(true); // trust opt-out → approved immediately

        let mut server = mockito::Server::new_async().await;
        let mock = mock_verify_admit(&mut server, "nonce-xyz").await;
        let url = server.url();

        let hello = AccountHello { grant: "good-grant".into(), nonce: "nonce-xyz".into() };
        let session_id = run_account_admit(&url, &db, &shared, &hello)
            .await
            .expect("a good grant for our node + account is admitted");
        mock.assert_async().await;

        // A real account-tagged session row was minted for the verified user.
        let row = db.web_remote_get_session(&session_id).expect("session row exists");
        assert_eq!(row.source, "account");
        assert_eq!(row.account_user_id.as_deref(), Some(OWN_UID));
        assert!(row.approved, "trust_account_browsers admits immediately");

        // A second dial for the same account REUSES the row (no duplicate).
        let mock2 = mock_verify_admit(&mut server, "nonce-xyz").await;
        let again = run_account_admit(&url, &db, &shared, &hello).await.unwrap();
        mock2.assert_async().await;
        assert_eq!(again, session_id, "repeat dials reuse the account session row");
        assert_eq!(db.web_remote_list_sessions().len(), 1, "no duplicate rows");
    }

    #[tokio::test]
    #[serial_test::serial(mockserver)]
    async fn account_handshake_pends_untrusted_account_browser() {
        let db = init_test_database();
        seed_owner(&db);
        let shared = shared_with_trust(false); // approval on (default) → pends

        let mut server = mockito::Server::new_async().await;
        let _mock = mock_verify_admit(&mut server, "n").await;
        let url = server.url();

        let hello = AccountHello { grant: "g".into(), nonce: "n".into() };
        let err = run_account_admit(&url, &db, &shared, &hello)
            .await
            .expect_err("an untrusted account browser pends until approved");
        assert_eq!(err, AccountHandshakeReject::Pending);
        // The pending row still exists so the desktop can approve it.
        let row = db.web_remote_list_sessions().into_iter().next().expect("a pending row");
        assert_eq!(row.source, "account");
        assert!(!row.approved);
    }

    #[tokio::test]
    #[serial_test::serial(mockserver)]
    async fn account_handshake_rejects_invalid_grant_without_minting() {
        let db = init_test_database();
        seed_owner(&db);
        let shared = shared_with_trust(true);

        let mut server = mockito::Server::new_async().await;
        // A bad/expired grant: the endpoint returns 200 {valid:false}.
        let _mock = server
            .mock("POST", "/api/devices/grant/verify")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(json!({ "valid": false, "reason": "bad_signature" }).to_string())
            .create_async()
            .await;
        let url = server.url();

        let hello = AccountHello { grant: "forged".into(), nonce: "n".into() };
        let err = run_account_admit(&url, &db, &shared, &hello)
            .await
            .expect_err("an invalid grant is rejected");
        assert_eq!(err, AccountHandshakeReject::Unverified);
        assert!(db.web_remote_list_sessions().is_empty(), "no row on a bad grant");
    }

    #[tokio::test]
    #[serial_test::serial(mockserver)]
    async fn account_handshake_rejects_grant_for_wrong_node() {
        let db = init_test_database();
        seed_owner(&db);
        let shared = shared_with_trust(true);

        let mut server = mockito::Server::new_async().await;
        // Valid grant, but bound to a DIFFERENT device's node_id.
        let _mock = server
            .mock("POST", "/api/devices/grant/verify")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "valid": true,
                    "userId": OWN_UID,
                    "deviceId": "dev-other",
                    "nodeId": "a-different-node",
                    "nonce": "n",
                })
                .to_string(),
            )
            .create_async()
            .await;
        let url = server.url();

        let hello = AccountHello { grant: "g".into(), nonce: "n".into() };
        let err = run_account_admit(&url, &db, &shared, &hello)
            .await
            .expect_err("a grant for another node is rejected");
        assert_eq!(err, AccountHandshakeReject::WrongNode);
        assert!(db.web_remote_list_sessions().is_empty());
    }

    #[tokio::test]
    #[serial_test::serial(mockserver)]
    async fn account_handshake_rejects_wrong_account_grant() {
        let db = init_test_database();
        seed_owner(&db);
        let shared = shared_with_trust(true);

        let mut server = mockito::Server::new_async().await;
        // Valid grant for OUR node, but a DIFFERENT account.
        let _mock = server
            .mock("POST", "/api/devices/grant/verify")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                json!({
                    "valid": true,
                    "userId": "intruder-account",
                    "deviceId": "dev-1",
                    "nodeId": OWN_NODE,
                    "nonce": "n",
                })
                .to_string(),
            )
            .create_async()
            .await;
        let url = server.url();

        let hello = AccountHello { grant: "g".into(), nonce: "n".into() };
        let err = run_account_admit(&url, &db, &shared, &hello)
            .await
            .expect_err("a grant for a different account is rejected");
        assert_eq!(err, AccountHandshakeReject::WrongAccount);
        assert!(db.web_remote_list_sessions().is_empty());
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

        let (close_tx, mut close_rx) = watch::channel(false);
        let (out_tx, out_rx) = crate::web_remote::server::outbound_channel(close_tx.clone());
        let conn_id = shared
            .connections
            .register(&session_id, crate::web_remote::server::Transport::Relay, out_tx.clone(), close_tx.clone());

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
