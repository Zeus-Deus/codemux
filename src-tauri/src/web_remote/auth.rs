//! Authentication primitives for the web-remote server.
//!
//! The trust model (see `docs/plans/web-remote-access.md`):
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

#[derive(Default)]
pub struct PairingStore {
    inner: Mutex<HashMap<String, Instant>>,
}

impl PairingStore {
    /// Issue a new single-use pairing token. Returns `(token, expires_in)`.
    pub fn issue(&self) -> (String, Duration) {
        let token = random_token();
        let mut map = self.inner.lock().unwrap();
        Self::prune(&mut map);
        map.insert(token.clone(), Instant::now());
        (token, PAIRING_TTL)
    }

    /// Consume a pairing token: succeeds exactly once, and only within
    /// the TTL. Removes it whether or not it was still fresh so a
    /// captured-but-expired token can't be retried.
    pub fn consume(&self, token: &str) -> bool {
        let mut map = self.inner.lock().unwrap();
        Self::prune(&mut map);
        match map.remove(token) {
            Some(created) => created.elapsed() < PAIRING_TTL,
            None => false,
        }
    }

    /// Number of live (unexpired) pairing tokens. Test/introspection use.
    pub fn live_count(&self) -> usize {
        let mut map = self.inner.lock().unwrap();
        Self::prune(&mut map);
        map.len()
    }

    fn prune(map: &mut HashMap<String, Instant>) {
        map.retain(|_, created| created.elapsed() < PAIRING_TTL);
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
pub fn authenticate(db: &crate::database::DatabaseStore, token: &str) -> Option<AuthedSession> {
    if token.is_empty() {
        return None;
    }
    let presented = sha256_hex(token);
    let mut matched: Option<AuthedSession> = None;
    // Scan every candidate (don't early-return) so timing does not leak
    // which/whether a row matched.
    for (id, stored_hash, approved) in db.web_remote_active_session_hashes() {
        if constant_time_eq(presented.as_bytes(), stored_hash.as_bytes()) {
            matched = Some(AuthedSession { id, approved });
        }
    }
    matched
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
            map.insert(token.clone(), Instant::now() - (PAIRING_TTL + Duration::from_secs(1)));
        }
        assert!(!store.consume(&token), "expired token must be rejected");
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
