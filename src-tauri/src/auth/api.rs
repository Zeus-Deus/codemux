// HTTP client for `/api/auth/desktop/*` endpoints that accept a
// password field. All functions here take an `AuthSecret` (not a
// raw `&str`) so the compiler physically prevents a caller from
// sending the user's raw password to the server — the `AuthSecret`
// newtype can only be constructed by
// `crate::auth::derive_auth_secret`, which stretches
// `(password, email)` via Argon2id + HKDF-SHA256 into a
// deterministic high-entropy value.
//
// Layout rationale: this file exists so the HTTP body construction
// lives in one place and can be unit-tested for structural leak
// safety (raw password must not appear in the serialized body) and
// for correctness (the `password` JSON field must equal
// `auth_secret.as_str()` byte-for-byte). The Tauri command handlers
// in `commands/auth.rs` call these helpers and never build the JSON
// body themselves.

use serde::Deserialize;

use crate::auth::{api_base_url, AuthSecret};

// ── Wire types ───────────────────────────────────────────────────
//
// These exactly match the JSON shape Better Auth's
// `/api/auth/desktop/*` endpoints return. They're `pub(crate)` so
// callers in `commands/auth.rs` can destructure the response
// without re-defining the shape.

#[derive(Debug, Deserialize)]
pub(crate) struct ApiAuthResp {
    pub(crate) token: String,
    #[serde(rename = "expiresAt")]
    pub(crate) expires_at: String,
    pub(crate) user: ApiUserResp,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ApiUserResp {
    pub(crate) id: String,
    pub(crate) email: String,
    pub(crate) name: Option<String>,
    pub(crate) image: Option<String>,
}

// ── Request-body builders ────────────────────────────────────────
//
// Factored out so structural leak tests can assert on the exact
// bytes that reach the wire without spinning up an HTTP mock.
// Taking `&AuthSecret` here — rather than `&str` — means the
// compiler will reject any callsite that tries to pass a raw
// password: the `AuthSecret` newtype is only constructible via the
// derivation in `auth/derivation.rs`, so by the time a value
// reaches these builders it has already been stretched through
// Argon2id + HKDF.

pub(crate) fn build_signin_body(email: &str, secret: &AuthSecret) -> serde_json::Value {
    serde_json::json!({
        "email": email,
        "password": secret.as_str(),
    })
}

pub(crate) fn build_signup_body(
    email: &str,
    secret: &AuthSecret,
    name: &str,
) -> serde_json::Value {
    serde_json::json!({
        "email": email,
        "password": secret.as_str(),
        "name": name,
    })
}

// ── Public API ───────────────────────────────────────────────────

/// POST `/api/auth/desktop/signin` with `{email, password: auth_secret}`.
/// The `secret` parameter is an `AuthSecret` — the compiler guarantees
/// the callsite has already run `derive_auth_secret(password, email)`
/// so the raw password cannot reach this function.
pub(crate) async fn login_email_api(
    email: &str,
    secret: &AuthSecret,
) -> Result<ApiAuthResp, String> {
    let base = api_base_url();
    let url = format!("{base}/api/auth/desktop/signin");

    let resp = reqwest::Client::new()
        .post(&url)
        .json(&build_signin_body(email, secret))
        .send()
        .await
        .map_err(|e| format!("Request failed: {e}"))?;

    if !resp.status().is_success() {
        let body: serde_json::Value = resp.json().await.unwrap_or_default();
        let msg = body["error"]
            .as_str()
            .unwrap_or("Authentication failed");
        return Err(msg.to_string());
    }

    resp.json::<ApiAuthResp>()
        .await
        .map_err(|e| format!("Parse response: {e}"))
}

/// POST `/api/auth/desktop/signup` with `{email, password: auth_secret, name}`.
/// Returns `()` on success — the user must verify their email before
/// they can sign in, so no token is returned.
pub(crate) async fn signup_email_api(
    email: &str,
    secret: &AuthSecret,
    name: &str,
) -> Result<(), String> {
    let base = api_base_url();
    let url = format!("{base}/api/auth/desktop/signup");

    let resp = reqwest::Client::new()
        .post(&url)
        .json(&build_signup_body(email, secret, name))
        .send()
        .await
        .map_err(|e| format!("Request failed: {e}"))?;

    if !resp.status().is_success() {
        let body: serde_json::Value = resp.json().await.unwrap_or_default();
        let msg = body["error"]
            .as_str()
            .unwrap_or("Sign-up failed");
        return Err(msg.to_string());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::derive_auth_secret;

    const DISTINCTIVE_PASSWORD: &str = "super-distinctive-test-password-12345";
    const DISTINCTIVE_EMAIL: &str = "leak-test@example.com";
    const DISTINCTIVE_NAME: &str = "Leak Test User";

    // ----------------------------------------------------------------
    // Test 3 (and its signup twin) — raw password does NOT leak into
    // the serialized HTTP body.
    //
    // Regression guard: if a future refactor accidentally writes
    // `"password": password` in the JSON body instead of the
    // AuthSecret, the distinctive password substring will show up in
    // the body bytes and this test will fire.
    // ----------------------------------------------------------------

    #[test]
    fn signin_body_does_not_contain_raw_password() {
        let secret = derive_auth_secret(DISTINCTIVE_PASSWORD, DISTINCTIVE_EMAIL).unwrap();
        let body = build_signin_body(DISTINCTIVE_EMAIL, &secret);
        let bytes = serde_json::to_vec(&body).expect("serialize body");

        let needle = DISTINCTIVE_PASSWORD.as_bytes();
        let contains = bytes.windows(needle.len()).any(|w| w == needle);
        assert!(
            !contains,
            "raw password leaked into signin request body: {:?}",
            String::from_utf8_lossy(&bytes)
        );
    }

    #[test]
    fn signup_body_does_not_contain_raw_password() {
        let secret = derive_auth_secret(DISTINCTIVE_PASSWORD, DISTINCTIVE_EMAIL).unwrap();
        let body = build_signup_body(DISTINCTIVE_EMAIL, &secret, DISTINCTIVE_NAME);
        let bytes = serde_json::to_vec(&body).expect("serialize body");

        let needle = DISTINCTIVE_PASSWORD.as_bytes();
        let contains = bytes.windows(needle.len()).any(|w| w == needle);
        assert!(
            !contains,
            "raw password leaked into signup request body: {:?}",
            String::from_utf8_lossy(&bytes)
        );
    }

    // ----------------------------------------------------------------
    // Test 5 — the AuthSecret IS what reaches the wire.
    //
    // Complement of the leak check: pin that the `password` field in
    // the JSON body equals exactly `auth_secret.as_str()`. If a
    // refactor ever accidentally sends an empty string or the wrong
    // field, this test fires.
    // ----------------------------------------------------------------

    #[test]
    fn signin_body_sends_auth_secret_as_password_field() {
        let secret = derive_auth_secret(DISTINCTIVE_PASSWORD, DISTINCTIVE_EMAIL).unwrap();
        let body = build_signin_body(DISTINCTIVE_EMAIL, &secret);
        assert_eq!(
            body["password"].as_str().expect("password is a string"),
            secret.as_str(),
            "signin body's password field must be the AuthSecret string"
        );
        assert_eq!(
            body["email"].as_str().expect("email is a string"),
            DISTINCTIVE_EMAIL,
            "signin body's email field must be the verbatim email"
        );
        // Exactly the two fields — no stray `rawPassword` or similar.
        let obj = body.as_object().expect("body is an object");
        assert_eq!(
            obj.len(),
            2,
            "signin body must have exactly two fields (email, password), got {:?}",
            obj.keys().collect::<Vec<_>>()
        );
    }

    #[test]
    fn signup_body_sends_auth_secret_as_password_field() {
        let secret = derive_auth_secret(DISTINCTIVE_PASSWORD, DISTINCTIVE_EMAIL).unwrap();
        let body = build_signup_body(DISTINCTIVE_EMAIL, &secret, DISTINCTIVE_NAME);
        assert_eq!(
            body["password"].as_str().expect("password is a string"),
            secret.as_str(),
            "signup body's password field must be the AuthSecret string"
        );
        assert_eq!(
            body["email"].as_str().expect("email is a string"),
            DISTINCTIVE_EMAIL,
        );
        assert_eq!(
            body["name"].as_str().expect("name is a string"),
            DISTINCTIVE_NAME,
        );
        let obj = body.as_object().expect("body is an object");
        assert_eq!(
            obj.len(),
            3,
            "signup body must have exactly three fields (email, password, name), got {:?}",
            obj.keys().collect::<Vec<_>>()
        );
    }

    // ----------------------------------------------------------------
    // Test 4 — end-to-end log leak check.
    //
    // Capture stderr during a full login_email_api call (mocked
    // server) with a distinctive password. Assert the raw password
    // does not appear anywhere in the captured stderr. The structural
    // body test above covers the wire; this covers the host process's
    // logs — a bug like `eprintln!("signing in with {password}")`
    // would never reach the wire but would still leak the credential
    // to anyone reading stderr (journald, crash reports, `tee` piped
    // logs, etc.).
    //
    // Unix-only because the capture uses dup2 via the `gag` crate.
    // The equivalent guarantee on Windows is provided statically by
    // the typed `AuthSecret` boundary plus the Debug-redaction test
    // in `derivation.rs`.
    // ----------------------------------------------------------------

    // `#[serial]` because BufferRedirect::stderr dup2's fd 2 process-
    // globally, and we mutate `CODEMUX_API_URL` — two parallel
    // instances would clash on both. Using the `mockserver` group
    // so these two tests serialize with each other but not with
    // every other unrelated #[serial] test in the crate.
    #[cfg(unix)]
    #[tokio::test]
    #[serial_test::serial(mockserver)]
    async fn signin_flow_does_not_leak_raw_password_to_stderr() {
        use std::io::Read;

        let secret = derive_auth_secret(DISTINCTIVE_PASSWORD, DISTINCTIVE_EMAIL).unwrap();

        // Mock server that 200-ok's the signin and returns a
        // well-formed session response.
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/api/auth/desktop/signin")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(
                serde_json::json!({
                    "token": "tok-leak-test",
                    "expiresAt": "2099-01-01T00:00:00Z",
                    "user": {
                        "id": "user-leak-test",
                        "email": DISTINCTIVE_EMAIL,
                        "name": null,
                        "image": null,
                    }
                })
                .to_string(),
            )
            .create_async()
            .await;

        // Point login_email_api at the mock via the same env var the
        // production path reads in `api_base_url()`.
        let prev_url = std::env::var("CODEMUX_API_URL").ok();
        std::env::set_var("CODEMUX_API_URL", server.url());

        // Capture stderr for the duration of the call. gag::
        // BufferRedirect dup2's the fd so even raw writes via libc
        // or reqwest internals end up in our buffer.
        let mut buf = gag::BufferRedirect::stderr().expect("redirect stderr");
        let result = login_email_api(DISTINCTIVE_EMAIL, &secret).await;
        let mut stderr_out = String::new();
        buf.read_to_string(&mut stderr_out)
            .expect("read captured stderr");
        drop(buf);

        // Restore env so other serial tests aren't affected.
        match prev_url {
            Some(v) => std::env::set_var("CODEMUX_API_URL", v),
            None => std::env::remove_var("CODEMUX_API_URL"),
        }

        let resp = result.expect("login should succeed against mock");
        assert_eq!(resp.token, "tok-leak-test");
        mock.assert_async().await;

        assert!(
            !stderr_out.contains(DISTINCTIVE_PASSWORD),
            "raw password leaked to stderr during signin:\n{stderr_out}"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    #[serial_test::serial(mockserver)]
    async fn signup_flow_does_not_leak_raw_password_to_stderr() {
        use std::io::Read;

        let secret = derive_auth_secret(DISTINCTIVE_PASSWORD, DISTINCTIVE_EMAIL).unwrap();

        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("POST", "/api/auth/desktop/signup")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{"user":{"id":"u1","email":"leak-test@example.com"}}"#)
            .create_async()
            .await;

        let prev_url = std::env::var("CODEMUX_API_URL").ok();
        std::env::set_var("CODEMUX_API_URL", server.url());

        let mut buf = gag::BufferRedirect::stderr().expect("redirect stderr");
        let result = signup_email_api(DISTINCTIVE_EMAIL, &secret, DISTINCTIVE_NAME).await;
        let mut stderr_out = String::new();
        buf.read_to_string(&mut stderr_out)
            .expect("read captured stderr");
        drop(buf);

        match prev_url {
            Some(v) => std::env::set_var("CODEMUX_API_URL", v),
            None => std::env::remove_var("CODEMUX_API_URL"),
        }

        result.expect("signup should succeed against mock");
        mock.assert_async().await;

        assert!(
            !stderr_out.contains(DISTINCTIVE_PASSWORD),
            "raw password leaked to stderr during signup:\n{stderr_out}"
        );
    }
}
