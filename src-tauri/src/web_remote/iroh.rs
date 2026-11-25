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
    AncestorReadOnly,
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

#[cfg(unix)]
fn open_key_dir(parent: &Path, access: KeyDirAccess) -> Result<std::fs::File, String> {
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
    let mut remaining = parts(parent)?;
    let mut links = 0;
    let uid = unsafe { libc::geteuid() };

    while let Some(part) = remaining.pop_front() {
        let name = match part {
            Part::Root => {
                directories.truncate(1);
                continue;
            }
            Part::Parent => {
                if directories.len() > 1 {
                    directories.pop();
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
            && access == KeyDirAccess::PrivateCreate
            && std::io::Error::last_os_error().kind() == std::io::ErrorKind::NotFound
        {
            result = unsafe { libc::mkdirat(directory.as_raw_fd(), name.as_ptr(), 0o700) };
            if result < 0 && std::io::Error::last_os_error().kind() != std::io::ErrorKind::AlreadyExists {
                return Err(format!("create iroh key directory: {}", std::io::Error::last_os_error()));
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
    }
    let search_handle = directories.pop().expect("root directory remains open");
    let search_meta = search_handle
        .metadata()
        .map_err(|e| format!("inspect iroh key directory: {e}"))?;
    // Legacy lookup needs only search permission on the app-data root. Keep
    // the validated descriptor: reopening it for read would reject an owned
    // 0300 directory even though its private key is accessible by name.
    if access == KeyDirAccess::AncestorReadOnly {
        if !search_meta.is_dir()
            || search_meta.uid() != uid
            || search_meta.permissions().mode() & 0o022 != 0
        {
            return Err("iroh key directory permissions remain unsafe".to_string());
        }
        check_key_acl(&search_handle, "iroh key directory", KeyAclScope::Ancestor)?;
        return Ok(search_handle);
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
    Ok(directory)
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
            return Err(format!("publish iroh key file: {}", std::io::Error::last_os_error()));
        }
        directory.sync_all().map_err(|e| format!("sync iroh key directory: {e}"))?;
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
fn persist_secret_key(path: &Path, key: &SecretKey) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    let parent = path.parent().ok_or("iroh key path has no parent")?;
    std::fs::create_dir_all(parent).map_err(|e| format!("create iroh key directory: {e}"))?;
    secure_existing_key_file(path)?;
    let sealed = crate::auth::encrypt_data(&key.to_bytes())?;
    let temporary = parent.join(format!(".{KEY_FILE}.{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|e| format!("create iroh key temp file: {e}"))?;
        file.write_all(&sealed).map_err(|e| format!("write iroh key temp file: {e}"))?;
        file.sync_all().map_err(|e| format!("sync iroh key temp file: {e}"))?;
        drop(file);
        let source: Vec<u16> = temporary.as_os_str().encode_wide().chain(Some(0)).collect();
        let destination: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        // Same-directory WRITE_THROUGH keeps a partially written replacement
        // out of the visible key path and waits for the move to reach disk.
        if unsafe {
            MoveFileExW(
                source.as_ptr(),
                destination.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        } == 0 {
            return Err(format!("publish iroh key file: {}", std::io::Error::last_os_error()));
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
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

#[cfg(not(unix))]
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
fn migrate_legacy_secret_key(path: &Path, directory: &std::fs::File) -> Result<Option<SecretKey>, String> {
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::fs::MetadataExt;

    let Some(legacy) = legacy_key_file_path(path) else {
        return Ok(None);
    };
    let legacy_dir = open_key_dir(
        legacy.parent().ok_or("legacy iroh key has no parent")?,
        KeyDirAccess::AncestorReadOnly,
    )?;
    let Some(mut file) = open_private_key_file(&legacy_dir, true)? else {
        return Ok(None);
    };
    let old_meta = file.metadata().map_err(|e| format!("inspect legacy iroh key: {e}"))?;
    let mut sealed = Vec::new();
    file.read_to_end(&mut sealed)
        .map_err(|e| format!("read legacy iroh key: {e}"))?;
    let Some(key) = decode_secret_key(&sealed) else {
        log::warn!("[codemux::web_remote] legacy iroh identity key is unreadable; regenerating");
        return Ok(None);
    };
    persist_secret_key_in(directory, &key)?;

    // A new iroh-identity directory is itself an entry in the app-data root.
    // Sync that parent before removing the recovery copy. Search-only roots
    // cannot be opened for directory sync, so keep the private legacy key in
    // that case; a crash can then recover the same node ID from either copy.
    let cleanup_dir = (|| -> Result<std::fs::File, String> {
        let dot = std::ffi::CString::new(".").expect("static directory name has no NUL");
        let fd = unsafe {
            libc::openat(
                legacy_dir.as_raw_fd(),
                dot.as_ptr(),
                libc::O_RDONLY | libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(format!("open app-data directory for sync: {}", std::io::Error::last_os_error()));
        }
        let readable = unsafe { std::fs::File::from_raw_fd(fd) };
        let expected = legacy_dir.metadata().map_err(|e| format!("inspect app-data directory: {e}"))?;
        let actual = readable.metadata().map_err(|e| format!("inspect app-data sync handle: {e}"))?;
        if actual.dev() != expected.dev() || actual.ino() != expected.ino() {
            return Err("app-data directory changed before sync".to_string());
        }
        readable.sync_all().map_err(|e| format!("sync app-data directory: {e}"))?;
        Ok(readable)
    })();
    let cleanup_dir = match cleanup_dir {
        Ok(dir) => dir,
        Err(error) => {
            log::warn!("[codemux::web_remote] keeping private legacy iroh key: {error}");
            return Ok(Some(key));
        }
    };

    // Delete only the same entry we validated. A failed cleanup leaves a
    // private legacy copy, while the new key is already safely published.
    let name = std::ffi::CString::new(KEY_FILE).expect("static key filename has no NUL");
    let mut entry: libc::stat = unsafe { std::mem::zeroed() };
    let inspected = unsafe {
        libc::fstatat(legacy_dir.as_raw_fd(), name.as_ptr(), &mut entry, libc::AT_SYMLINK_NOFOLLOW)
    };
    if inspected == 0 && entry.st_dev as u64 == old_meta.dev() && entry.st_ino as u64 == old_meta.ino() {
        if unsafe { libc::unlinkat(legacy_dir.as_raw_fd(), name.as_ptr(), 0) } < 0 {
            log::warn!("[codemux::web_remote] could not remove migrated iroh key: {}", std::io::Error::last_os_error());
        } else if let Err(error) = cleanup_dir.sync_all() {
            log::warn!("[codemux::web_remote] could not sync removed legacy iroh key: {error}");
        }
    }
    Ok(Some(key))
}

#[cfg(not(unix))]
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
    let parent = path.parent().ok_or("iroh key path has no parent")?;
    #[cfg(unix)]
    let directory = open_private_key_dir(parent)?;
    #[cfg(unix)]
    let sealed = read_private_key_in(&directory, true)?;
    #[cfg(not(unix))]
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
            return Ok(key);
        }
        log::warn!(
            "[codemux::web_remote] iroh identity key at {} unreadable; regenerating",
            path.display()
        );
    }
    #[cfg(unix)]
    if let Some(key) = migrate_legacy_secret_key(path, &directory)? {
        return Ok(key);
    }
    #[cfg(not(unix))]
    if let Some(key) = migrate_legacy_secret_key(path)? {
        return Ok(key);
    }
    let key = SecretKey::generate();
    #[cfg(unix)]
    persist_secret_key_in(&directory, &key)?;
    #[cfg(not(unix))]
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
    let current = {
        let private_dir = path.parent()?;
        match open_key_dir(private_dir, KeyDirAccess::PrivateReadOnly) {
            Ok(directory) => read_private_key_in(&directory, false).ok()?,
            Err(_) if matches!(
                std::fs::symlink_metadata(private_dir),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound
            ) => None,
            Err(_) => return None,
        }
    };
    #[cfg(not(unix))]
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
    let legacy = legacy_key_file_path(path)?;
    #[cfg(unix)]
    let sealed = {
        let directory = open_key_dir(legacy.parent()?, KeyDirAccess::AncestorReadOnly).ok()?;
        read_private_key_in(&directory, false).ok()??
    };
    #[cfg(not(unix))]
    let sealed = {
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
        assert!(!legacy.exists());
        assert_eq!(std::fs::metadata(&relocated).unwrap().permissions().mode() & 0o777, 0o750);
        assert_eq!(std::fs::metadata(&unrelated).unwrap().permissions().mode() & 0o777, 0o640);
        assert_eq!(std::fs::metadata(relocated.join(KEY_DIR)).unwrap().permissions().mode() & 0o777, 0o700);
        assert_eq!(std::fs::metadata(relocated.join(KEY_DIR).join(KEY_FILE)).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::read(&unrelated).unwrap(), b"keep");
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
        assert!(!legacy.exists());
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
