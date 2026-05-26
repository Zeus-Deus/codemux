//! Bearer-token authentication middleware for the daemon's HTTP API.
//!
//! Every request must carry `Authorization: Bearer <secret>` matching
//! the secret in the daemon's manifest. On match, the request is
//! tagged with [`Identity::Local`] via a request extension that
//! downstream handlers extract.
//!
//! Constant-time comparison is used so timing-side-channel attacks
//! can't whittle the secret out byte by byte (32-byte secret with
//! 256 bits of entropy is well over the practical-attack threshold,
//! but constant-time comparison is free here and the right hygiene).

use axum::{
    body::Body,
    extract::State,
    http::{Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

use super::identity::Identity;
use super::server::SharedState;

/// Axum middleware that enforces the bearer-token header and
/// attaches an [`Identity`] extension to the request before handing
/// it off to the inner handler.
pub async fn require_bearer(
    State(state): State<SharedState>,
    mut req: Request<Body>,
    next: Next,
) -> Response {
    let provided = match req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
    {
        Some(value) => value.trim(),
        None => return unauthorized("missing Authorization header"),
    };

    let token = match provided.strip_prefix("Bearer ") {
        Some(t) => t.trim(),
        None => return unauthorized("expected Bearer scheme"),
    };

    if !constant_time_eq(token.as_bytes(), state.secret.as_bytes()) {
        return unauthorized("invalid bearer token");
    }

    // v1: every authenticated caller is Identity::Local. A future
    // relay layer would override this by validating a forwarded
    // identity header *before* this middleware sees the request,
    // and constructing Identity::Cloud { … } instead.
    req.extensions_mut().insert(Identity::local());

    next.run(req).await
}

fn unauthorized(reason: &'static str) -> Response {
    (
        StatusCode::UNAUTHORIZED,
        Json(json!({ "error": "unauthorized", "reason": reason })),
    )
        .into_response()
}

/// Constant-time byte-slice equality. Returns false if lengths
/// differ; otherwise XORs all bytes and folds without branching on
/// individual comparisons.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_time_eq_matches_normal_eq() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
        assert!(!constant_time_eq(b"", b"a"));
        assert!(constant_time_eq(b"", b""));
    }
}
