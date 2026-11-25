//! Authentication primitives for the web-remote server.
//!
//! The trust model:
//!
//! 1. **Pairing token** — 32 random bytes, 10-minute TTL, single use,
//!    in memory only. Handed to a browser out-of-band (QR / link).
//! 2. **Session** — created when a valid pairing token is presented.
//!    Persisted in SQLite as a SHA-256 `token_hash`; the plaintext
//!    session token is returned to the browser once and never stored.
//!    Authentication is a constant-time hash compare.
//! 3. **WS ticket** — 30-second TTL, single use, in memory. A browser
//!    trades its session token for a ticket, then opens `/ws?ticket=…`
//!    so the long-lived session secret never lands in a WS URL or a
//!    server log.
//!
//! Pairing attempts are rate-limited to 5/min/IP. Origin is checked on
//! every state-touching request so a page on another site cannot ride a
//! browser's stored cookie.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::http::HeaderMap;
use rand::RngCore;
use sha2::{Digest, Sha256};

use crate::database::{DatabaseStore, WebRemoteSessionRecord};

/// Pairing tokens live 10 minutes.
pub const PAIRING_TTL: Duration = Duration::from_secs(10 * 60);
/// WS tickets live 30 seconds.
pub const TICKET_TTL: Duration = Duration::from_secs(30);
/// Pairing attempts allowed per IP per window.
pub const RATE_LIMIT_MAX: usize = 5;
/// Rate-limit sliding window.
pub const RATE_LIMIT_WINDOW: Duration = Duration::from_secs(60);
/// Entropy for pairing / session / ticket secrets.
pub const TOKEN_BYTES: usize = 32;

/// Hex-encode a byte slice (lowercase). Small enough to avoid pulling in
/// a hex crate for a handful of call sites.
fn to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0x0f) as usize] as char);
    }
    s
}

/// A fresh random secret as `TOKEN_BYTES * 2` lowercase hex chars.
pub fn random_token() -> String {
    let mut buf = [0u8; TOKEN_BYTES];
    rand::thread_rng().fill_bytes(&mut buf);
    to_hex(&buf)
}

/// SHA-256 of `input`, hex-encoded. Used to hash session tokens at rest.
pub fn sha256_hex(input: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(input.as_bytes());
    to_hex(&hasher.finalize())
}

/// Constant-time byte-slice equality (length-independent branch on the
/// content). Returns false on length mismatch.
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

// ── Pairing tokens ──────────────────────────────────────────────────

/// A live pairing token: when it was issued, plus an optional device name
/// suggested by whoever minted it (the `codemux remote pair --name <label>`
/// CLI). The suggested name is used by the pair handler only as a fallback
/// when the connecting client presents no `device_name` of its own.
struct PairingEntry {
    created: Instant,
    suggested_name: Option<String>,
}

/// Outcome of consuming a pairing token: whether it was valid (single-use,
/// within TTL) plus any suggested device name it carried.
pub struct PairingConsume {
    pub consumed: bool,
    pub suggested_name: Option<String>,
}

#[derive(Default)]
pub struct PairingStore {
    inner: Mutex<HashMap<String, PairingEntry>>,
}

impl PairingStore {
    /// Issue a new single-use pairing token with no suggested device name.
    /// Returns `(token, expires_in)`.
    pub fn issue(&self) -> (String, Duration) {
        self.issue_named(None)
    }

    /// Issue a single-use pairing token carrying an optional suggested
    /// device name. Returns `(token, expires_in)`. The name is surfaced by
    /// [`PairingStore::consume_named`] so the pair handler can label a
    /// device that connects without naming itself (e.g. a QR minted from
    /// the CLI with `--name`).
    pub fn issue_named(&self, suggested_name: Option<String>) -> (String, Duration) {
        let token = random_token();
        let mut map = self.inner.lock().unwrap();
        Self::prune(&mut map);
        map.insert(
            token.clone(),
            PairingEntry {
                created: Instant::now(),
                suggested_name,
            },
        );
        (token, PAIRING_TTL)
    }

    /// Consume a pairing token: succeeds exactly once, and only within
    /// the TTL. Removes it whether or not it was still fresh so a
    /// captured-but-expired token can't be retried.
    pub fn consume(&self, token: &str) -> bool {
        self.consume_named(token).consumed
    }

    /// Consume a pairing token, also returning any suggested device name it
    /// carried so the pair handler can use it as a fallback label. Same
    /// single-use / TTL semantics as [`PairingStore::consume`].
    pub fn consume_named(&self, token: &str) -> PairingConsume {
        let mut map = self.inner.lock().unwrap();
        Self::prune(&mut map);
        match map.remove(token) {
            Some(entry) => PairingConsume {
                consumed: entry.created.elapsed() < PAIRING_TTL,
                suggested_name: entry.suggested_name,
            },
            None => PairingConsume {
                consumed: false,
                suggested_name: None,
            },
        }
    }

    /// Number of live (unexpired) pairing tokens. Test/introspection use.
    pub fn live_count(&self) -> usize {
        let mut map = self.inner.lock().unwrap();
        Self::prune(&mut map);
        map.len()
    }

    fn prune(map: &mut HashMap<String, PairingEntry>) {
        map.retain(|_, entry| entry.created.elapsed() < PAIRING_TTL);
    }
}

// ── WS tickets ──────────────────────────────────────────────────────

struct TicketEntry {
    session_id: String,
    created: Instant,
}

#[derive(Default)]
pub struct TicketStore {
    inner: Mutex<HashMap<String, TicketEntry>>,
}

impl TicketStore {
    /// Mint a single-use ticket bound to `session_id`.
    pub fn issue(&self, session_id: &str) -> String {
        let ticket = random_token();
        let mut map = self.inner.lock().unwrap();
        Self::prune(&mut map);
        map.insert(
            ticket.clone(),
            TicketEntry {
                session_id: session_id.to_string(),
                created: Instant::now(),
            },
        );
        ticket
    }

    /// Redeem a ticket, returning the bound `session_id` if it is valid
    /// and unexpired. Single-use: the ticket is removed regardless.
    pub fn consume(&self, ticket: &str) -> Option<String> {
        let mut map = self.inner.lock().unwrap();
        Self::prune(&mut map);
        match map.remove(ticket) {
            Some(entry) if entry.created.elapsed() < TICKET_TTL => Some(entry.session_id),
            _ => None,
        }
    }

    fn prune(map: &mut HashMap<String, TicketEntry>) {
        map.retain(|_, entry| entry.created.elapsed() < TICKET_TTL);
    }
}

// ── Pairing rate limiter ────────────────────────────────────────────

#[derive(Default)]
pub struct RateLimiter {
    inner: Mutex<HashMap<IpAddr, Vec<Instant>>>,
}

impl RateLimiter {
    /// Record an attempt from `ip` and report whether it is allowed
    /// (≤ `RATE_LIMIT_MAX` within `RATE_LIMIT_WINDOW`). A rejected attempt
    /// is *not* recorded, so a blocked client that backs off recovers.
    pub fn check_and_record(&self, ip: IpAddr) -> bool {
        let mut map = self.inner.lock().unwrap();
        let now = Instant::now();
        let hits = map.entry(ip).or_default();
        hits.retain(|t| now.duration_since(*t) < RATE_LIMIT_WINDOW);
        if hits.len() >= RATE_LIMIT_MAX {
            return false;
        }
        hits.push(now);
        true
    }
}

// ── Session authentication ──────────────────────────────────────────

/// The result of resolving a presented bearer/cookie token to a session.
#[derive(Debug, Clone)]
pub struct AuthedSession {
    pub id: String,
    /// Whether this session has been approved (approval-mode gate).
    pub approved: bool,
}

/// Resolve a plaintext session token to a live (non-revoked) session by
/// constant-time comparing its SHA-256 against every active row's hash.
/// Returns `None` if nothing matches.
pub fn authenticate(db: &DatabaseStore, token: &str) -> Option<AuthedSession> {
    if token.is_empty() {
        return None;
    }
    let presented = sha256_hex(token);
    let mut matched = None;
    // Scan every candidate (don't early-return) so timing does not leak
    // which/whether a row matched.
    for (id, stored_hash, _) in db.web_remote_active_session_hashes() {
        if constant_time_eq(presented.as_bytes(), stored_hash.as_bytes()) {
            matched = Some(id);
        }
    }
    let session = active_session(db, &matched?)?;
    Some(AuthedSession { id: session.id, approved: session.approved })
}

pub(super) enum SessionStatus {
    Active(WebRemoteSessionRecord),
    Suspended,
    Gone,
}

impl SessionStatus {
    pub(super) fn is_approved(&self) -> bool {
        matches!(self, Self::Active(session) if session.approved)
    }
}

/// A host-paired browser is independent of the account on this host. Account
/// browsers must still belong to its current, unexpired account.
pub(super) fn session_status(db: &DatabaseStore, session_id: &str) -> SessionStatus {
    match db.try_web_remote_session_with_auth(session_id) {
        Ok(None) => SessionStatus::Gone,
        Ok(Some((session, _))) if session.revoked => SessionStatus::Gone,
        Ok(Some((session, record))) => {
            if session.source == "account" {
                let owner = record.as_deref().and_then(crate::auth::active_account_id_from_record);
                if owner.is_none() || session.account_user_id.as_deref() != owner.as_deref() {
                    return SessionStatus::Suspended;
                }
            }
            SessionStatus::Active(session)
        }
        // Fail closed during a read error while preserving push subscriptions.
        Err(_) => SessionStatus::Suspended,
    }
}

pub(super) fn active_session(db: &DatabaseStore, session_id: &str) -> Option<WebRemoteSessionRecord> {
    match session_status(db, session_id) {
        SessionStatus::Active(session) => Some(session),
        SessionStatus::Suspended | SessionStatus::Gone => None,
    }
}

pub(super) struct AccountSessionGuard<'a> {
    db: &'a DatabaseStore,
    session_id: &'a str,
    account_session: bool,
}

impl<'a> AccountSessionGuard<'a> {
    pub(super) fn new(db: &'a DatabaseStore, session_id: &'a str) -> Option<Self> {
        let session = active_session(db, session_id).filter(|session| session.approved)?;
        Some(Self { db, session_id, account_session: session.source == "account" })
    }

    pub(super) fn account_session_id(&self) -> Option<&str> {
        self.account_session.then_some(self.session_id)
    }

    pub(super) fn active(&self) -> bool {
        !self.account_session
            || session_status(self.db, self.session_id).is_approved()
    }

    pub(super) async fn expired(&self) {
        if !self.account_session {
            return std::future::pending().await;
        }
        // A separate CLI process may change the shared account record. Check
        // idle streams too, including streams that only receive host events.
        let mut interval = tokio::time::interval(Duration::from_secs(2));
        loop {
            interval.tick().await;
            if !self.active() {
                return;
            }
        }
    }
}

#[cfg(test)]
pub(super) fn seed_account_browser(db: &DatabaseStore, session_id: &str, token: &str) {
    let owner = crate::auth::AuthUser {
        id: "fixture-owner".into(),
        email: "owner@example.test".into(),
        name: None,
        image: None,
    };
    crate::auth::save_auth(db, "fixture-token", "2099-01-01T00:00:00Z", Some(&owner)).unwrap();
    db.web_remote_insert_account_session(
        session_id,
        None,
        None,
        &sha256_hex(token),
        true,
        &owner.id,
    )
    .unwrap();
}

// ── Origin checks ───────────────────────────────────────────────────

/// Enforce same-origin for state-touching requests. A request passes when
/// it has no `Origin` header (native clients, same-origin asset GETs) or
/// when the Origin's host authority matches the request `Host`. A present
/// but mismatched Origin is rejected — that is the cross-site case.
pub fn origin_ok(headers: &HeaderMap) -> bool {
    let origin = match headers.get(axum::http::header::ORIGIN).and_then(|v| v.to_str().ok()) {
        Some(o) => o,
        // No Origin header: not a cross-site browser request. Allow.
        None => return true,
    };
    let host = match headers.get(axum::http::header::HOST).and_then(|v| v.to_str().ok()) {
        Some(h) => h,
        None => return false,
    };
    // Compare host authorities: strip the scheme from Origin, then match
    // the `host[:port]` portion against the Host header verbatim.
    let origin_authority = origin
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(origin);
    origin_authority == host
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::init_test_database;

    #[test]
    fn sha256_known_vector() {
        // SHA-256("abc")
        assert_eq!(
            sha256_hex("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn constant_time_eq_behaviour() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
        assert!(constant_time_eq(b"", b""));
    }

    #[test]
    fn random_token_is_64_hex_chars_and_unique() {
        let a = random_token();
        let b = random_token();
        assert_eq!(a.len(), TOKEN_BYTES * 2);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }

    #[test]
    fn pairing_token_single_use() {
        let store = PairingStore::default();
        let (token, ttl) = store.issue();
        assert_eq!(ttl, PAIRING_TTL);
        assert!(store.consume(&token), "first consume succeeds");
        assert!(!store.consume(&token), "second consume fails (single-use)");
    }

    #[test]
    fn pairing_unknown_token_rejected() {
        let store = PairingStore::default();
        assert!(!store.consume("deadbeef"));
    }

    #[test]
    fn pairing_expired_token_rejected() {
        let store = PairingStore::default();
        let token = random_token();
        // Simulate an already-expired issue by inserting an old timestamp.
        {
            let mut map = store.inner.lock().unwrap();
            map.insert(
                token.clone(),
                PairingEntry {
                    created: Instant::now() - (PAIRING_TTL + Duration::from_secs(1)),
                    suggested_name: None,
                },
            );
        }
        assert!(!store.consume(&token), "expired token must be rejected");
    }

    #[test]
    fn pairing_suggested_name_survives_consume_and_is_single_use() {
        // A token minted with a suggested name (the CLI's `--name` label)
        // returns that name once, on the single successful consume. The
        // second consume is empty — proving the name isn't leaked twice and
        // the single-use invariant holds for the named path too.
        let store = PairingStore::default();
        let (token, _ttl) = store.issue_named(Some("Kitchen iPad".to_string()));

        let first = store.consume_named(&token);
        assert!(first.consumed, "first named consume succeeds");
        assert_eq!(first.suggested_name.as_deref(), Some("Kitchen iPad"));

        let second = store.consume_named(&token);
        assert!(!second.consumed, "second named consume fails (single-use)");
        assert_eq!(second.suggested_name, None);
    }

    #[test]
    fn ticket_single_use_and_binding() {
        let store = TicketStore::default();
        let ticket = store.issue("session-1");
        assert_eq!(store.consume(&ticket).as_deref(), Some("session-1"));
        assert!(store.consume(&ticket).is_none(), "ticket is single-use");
    }

    #[test]
    fn ticket_expiry_rejected() {
        let store = TicketStore::default();
        let ticket = random_token();
        {
            let mut map = store.inner.lock().unwrap();
            map.insert(
                ticket.clone(),
                TicketEntry {
                    session_id: "s".into(),
                    created: Instant::now() - (TICKET_TTL + Duration::from_secs(1)),
                },
            );
        }
        assert!(store.consume(&ticket).is_none(), "expired ticket must be rejected");
    }

    #[test]
    fn rate_limiter_blocks_sixth_attempt() {
        let rl = RateLimiter::default();
        let ip: IpAddr = "203.0.113.7".parse().unwrap();
        for i in 0..RATE_LIMIT_MAX {
            assert!(rl.check_and_record(ip), "attempt {i} within limit");
        }
        assert!(!rl.check_and_record(ip), "attempt beyond limit blocked");
        // A different IP is unaffected.
        assert!(rl.check_and_record("203.0.113.8".parse().unwrap()));
    }

    #[test]
    fn authenticate_roundtrip_and_revocation() {
        let db = init_test_database();
        let token = random_token();
        let hash = sha256_hex(&token);
        db.web_remote_insert_session("sess-a", Some("Phone"), None, &hash, true)
            .unwrap();

        // A valid token resolves to the approved session.
        let authed = authenticate(&db, &token).expect("token authenticates");
        assert_eq!(authed.id, "sess-a");
        assert!(authed.approved);

        // A wrong token does not.
        assert!(authenticate(&db, &random_token()).is_none());

        // After revocation the token is dead.
        db.web_remote_revoke_session("sess-a").unwrap();
        assert!(
            authenticate(&db, &token).is_none(),
            "revoked session must not authenticate"
        );
    }

    #[test]
    fn authenticate_reflects_pending_approval() {
        let db = init_test_database();
        let token = random_token();
        db.web_remote_insert_session("sess-p", None, None, &sha256_hex(&token), false)
            .unwrap();
        let authed = authenticate(&db, &token).expect("authenticates");
        assert!(!authed.approved, "pending session reports approved=false");
    }

    #[tokio::test]
    async fn account_switch_and_logout_revoke_only_account_browsers() {
        let db = init_test_database();
        let mut user = crate::auth::AuthUser {
            id: "first-owner".into(),
            email: "owner@example.test".into(),
            name: None,
            image: None,
        };
        let expires_at = "2099-01-01T00:00:00Z";
        crate::auth::save_auth(&db, "first-account-token", expires_at, Some(&user)).unwrap();
        db.web_remote_insert_account_session(
            "first", None, None, &sha256_hex("first-browser"), true, &user.id,
        ).unwrap();
        db.web_remote_insert_session(
            "host-paired", None, None, &sha256_hex("paired-browser"), true,
        ).unwrap();
        let first = AccountSessionGuard::new(&db, "first").unwrap();

        crate::auth::save_auth(&db, "refreshed-account-token", expires_at, Some(&user)).unwrap();
        assert!(first.active());
        assert!(authenticate(&db, "first-browser").is_some());

        user.id = "second-owner".into();
        crate::auth::save_auth(&db, "other-account-token", expires_at, Some(&user)).unwrap();
        assert!(db.web_remote_get_session("first").unwrap().revoked);
        assert!(!first.active());
        assert!(authenticate(&db, "first-browser").is_none());
        assert!(authenticate(&db, "paired-browser").is_some());

        db.web_remote_insert_account_session(
            "second", None, None, &sha256_hex("second-browser"), true, &user.id,
        ).unwrap();
        let second = AccountSessionGuard::new(&db, "second").unwrap();
        db.clear_auth_token().unwrap();
        tokio::time::timeout(Duration::from_secs(1), second.expired()).await.unwrap();
        assert!(authenticate(&db, "second-browser").is_none());
        assert!(authenticate(&db, "paired-browser").is_some());
    }

    #[test]
    fn withdrawing_removes_only_a_pending_session() {
        let db = init_test_database();
        db.web_remote_insert_session("sess-p", None, None, "hash-p", false)
            .unwrap();
        db.web_remote_insert_session("sess-a", None, None, "hash-a", true)
            .unwrap();
        assert!(db.web_remote_delete_pending_session("sess-p").unwrap());
        assert!(db.web_remote_get_session("sess-p").is_none());
        assert!(
            !db.web_remote_delete_pending_session("sess-a").unwrap(),
            "an approved session is not withdrawn"
        );
        assert!(db.web_remote_get_session("sess-a").is_some());
    }

    fn headers_with(origin: Option<&str>, host: &str) -> HeaderMap {
        let mut h = HeaderMap::new();
        if let Some(o) = origin {
            h.insert(axum::http::header::ORIGIN, o.parse().unwrap());
        }
        h.insert(axum::http::header::HOST, host.parse().unwrap());
        h
    }

    #[test]
    fn origin_check() {
        // Same origin passes.
        assert!(origin_ok(&headers_with(Some("http://192.168.1.5:4377"), "192.168.1.5:4377")));
        // Cross-site is rejected.
        assert!(!origin_ok(&headers_with(Some("http://evil.example"), "192.168.1.5:4377")));
        // Missing Origin (native client / same-origin GET) passes.
        assert!(origin_ok(&headers_with(None, "192.168.1.5:4377")));
        // HTTPS origin with matching authority passes.
        assert!(origin_ok(&headers_with(
            Some("https://host.tailnet.ts.net"),
            "host.tailnet.ts.net"
        )));
    }
}
