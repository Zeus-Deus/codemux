//! MCP-only OAuth primitives. URLs never derive from request/forwarded headers.
use url::Url;

fn clean_url(value: &str) -> Result<Url, String> {
    if value.is_empty()
        || value.len() > 512
        || value.chars().any(|c| c.is_whitespace() || c.is_control())
        || value.contains(['\\', '#', '*'])
    {
        return Err("Invalid URL".into());
    }
    let rest = value
        .strip_prefix("https://")
        .or_else(|| value.strip_prefix("http://"))
        .ok_or("Use an absolute HTTP(S) URL")?;
    let authority = rest.split(['/', '?']).next().unwrap_or("");
    if authority.is_empty() || authority.contains('@') {
        return Err("URL credentials are not allowed".into());
    }
    let url = Url::parse(value).map_err(|_| "Invalid URL")?;
    if url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || !(url.scheme() == "https" || (url.scheme() == "http" && loopback(raw_host(authority))))
    {
        return Err("Use HTTPS (HTTP is allowed only on loopback) without credentials".into());
    }
    Ok(url)
}

pub fn loopback(host: &str) -> bool {
    matches!(host, "localhost" | "127.0.0.1" | "[::1]")
}

fn raw_host(authority: &str) -> &str {
    if authority.starts_with('[') {
        authority
            .find(']')
            .map(|end| &authority[..=end])
            .unwrap_or("")
    } else {
        authority.split(':').next().unwrap_or("")
    }
}

pub fn public_origin(value: &str) -> Result<String, String> {
    let url = clean_url(value)?;
    let rest = value.split_once("://").ok_or("Invalid origin")?.1;
    let tail = rest.find('/').map(|i| &rest[i..]).unwrap_or("");
    if !matches!(tail, "" | "/") || url.query().is_some() {
        return Err("Configure an origin, not a URL path/query".into());
    }
    Ok(url.origin().ascii_serialization())
}

pub fn redirect_valid(value: &str) -> bool {
    clean_url(value).is_ok()
}

pub fn redirect_matches(registered: &str, actual: &str) -> bool {
    let (Ok(left), Ok(right)) = (clean_url(registered), clean_url(actual)) else {
        return false;
    };
    // HTTPS matching is deliberately literal, not URL-parser normalization.
    if left.scheme() == "https" || right.scheme() == "https" {
        return registered == actual;
    }
    // RFC 8252 permits only the loopback port to differ. Preserve every other
    // character, including percent encodings, query and path spelling.
    fn without_port(value: &str) -> String {
        let rest = value.strip_prefix("http://").unwrap_or("");
        let end = rest.find(['/', '?']).unwrap_or(rest.len());
        format!("http://{}{}", raw_host(&rest[..end]), &rest[end..])
    }
    without_port(registered) == without_port(actual)
}

use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rand::RngCore;
use serde::Deserialize;
use sha2::{Digest, Sha256};

pub const SCOPES: [&str; 3] = ["codemux:read", "codemux:supervised", "codemux:full"];

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Registration {
    pub client_name: Option<String>,
    pub redirect_uris: Vec<String>,
    pub token_endpoint_auth_method: Option<String>,
    pub grant_types: Option<Vec<String>>,
    pub response_types: Option<Vec<String>>,
    pub scope: Option<String>,
}

impl Registration {
    pub fn validate(&self) -> Result<(String, Vec<String>), String> {
        let name = self.client_name.as_deref().unwrap_or("MCP client");
        if name.trim() != name
            || name.is_empty()
            || name.chars().count() > 100
            || name.chars().any(char::is_control)
        {
            return Err("Client name must be a nonempty label of at most 100 characters".into());
        }
        if self.redirect_uris.is_empty()
            || self.redirect_uris.len() > 5
            || !self.redirect_uris.iter().all(|u| redirect_valid(u))
            || self
                .redirect_uris
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .len()
                != self.redirect_uris.len()
        {
            return Err("Register 1–5 distinct HTTPS or loopback HTTP redirect URIs".into());
        }
        if self
            .token_endpoint_auth_method
            .as_ref()
            .is_some_and(|v| v.len() > 100 || v.chars().any(char::is_control))
            || [&self.grant_types, &self.response_types].iter().any(|v| {
                v.as_ref().is_some_and(|a| {
                    a.len() > 5
                        || a.iter()
                            .any(|s| s.len() > 100 || s.chars().any(char::is_control))
                })
            })
            || self
                .scope
                .as_ref()
                .is_some_and(|s| s.len() > 256 || s.chars().any(char::is_control))
        {
            return Err("Client metadata exceeds limits".into());
        }
        Ok((name.into(), self.redirect_uris.clone()))
    }
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthorizationInput {
    pub client_id: String,
    pub redirect_uri: String,
    pub response_type: String,
    pub code_challenge_method: String,
    pub code_challenge: String,
    pub resource: String,
    pub state: Option<String>,
    pub scope: Option<String>,
}

#[derive(Clone)]
pub struct Authorization {
    pub client_id: String,
    pub client_name: String,
    pub redirect_uri: String,
    pub challenge: String,
    pub resource: String,
    pub issuer: String,
    pub state: Option<String>,
}

pub fn validate_authorization(
    input: AuthorizationInput,
    name: String,
    registered: &[String],
    issuer: &str,
) -> Result<Authorization, String> {
    if input.client_id.is_empty()
        || input.client_id.len() > 128
        || !registered
            .iter()
            .any(|r| redirect_matches(r, &input.redirect_uri))
    {
        return Err("Unknown client or unregistered redirect URI".into());
    }
    if input.response_type != "code" {
        return Err("Only response_type=code is supported".into());
    }
    if input.code_challenge_method != "S256" || !challenge_valid(&input.code_challenge) {
        return Err("PKCE S256 is required".into());
    }
    if input.resource != format!("{issuer}/mcp") {
        return Err("The resource must be the canonical MCP URL".into());
    }
    if input
        .state
        .as_ref()
        .is_some_and(|s| s.len() > 1024 || s.chars().any(char::is_control))
        || input
            .scope
            .as_ref()
            .is_some_and(|s| s.len() > 256 || s.split_whitespace().any(|v| !SCOPES.contains(&v)))
    {
        return Err("Invalid state or scope".into());
    }
    Ok(Authorization {
        client_id: input.client_id,
        client_name: name,
        redirect_uri: input.redirect_uri,
        challenge: input.code_challenge,
        resource: input.resource,
        issuer: issuer.into(),
        state: input.state,
    })
}

pub fn challenge_valid(value: &str) -> bool {
    value.len() == 43
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"_-".contains(&c))
}

pub fn pkce_verifies(verifier: &str, challenge: &str) -> bool {
    if !(43..=128).contains(&verifier.len())
        || !verifier
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"-._~".contains(&c))
        || !challenge_valid(challenge)
    {
        return false;
    }
    fixed_equal(
        URL_SAFE_NO_PAD
            .encode(Sha256::digest(verifier.as_bytes()))
            .as_bytes(),
        challenge.as_bytes(),
    )
}

pub fn fixed_equal(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0u8, |acc, (a, b)| acc | (a ^ b))
        == 0
}

pub fn opaque() -> String {
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

pub fn credential_hash(value: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(
        [b"codemux:mcp:v1:".as_slice(), value.as_bytes()].concat(),
    ))
}

use std::collections::HashMap;

pub const PENDING_TTL_MS: i64 = 300_000;
pub const CODE_TTL_MS: i64 = 60_000;
pub const MAX_PENDING: usize = 64;

pub struct Pending {
    pub authorization: Authorization,
    pub requested_at: i64,
    pub expires_at: i64,
    nonce_hash: String,
    cookie_hash: String,
    decision: Option<Option<String>>,
}

#[derive(Clone, Copy, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthorizationPhase {
    AwaitingApproval,
    ApprovedAwaitingClient,
    Denied,
}

impl Pending {
    // Only the decision's public phase and selected ceiling may leave native
    // state. The authorization tuple and browser bindings remain private.
    pub(crate) fn consent(&self) -> (AuthorizationPhase, Option<&str>) {
        match &self.decision {
            None => (AuthorizationPhase::AwaitingApproval, None),
            Some(Some(access)) => (AuthorizationPhase::ApprovedAwaitingClient, Some(access)),
            Some(None) => (AuthorizationPhase::Denied, None),
        }
    }
}

#[derive(Clone)]
pub struct Code {
    pub authorization: Authorization,
    pub access: String,
    expires_at: i64,
}

#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct TokenInput {
    pub grant_type: String,
    pub client_id: String,
    pub code: String,
    pub redirect_uri: String,
    pub resource: String,
    pub code_verifier: String,
}

#[derive(Default)]
pub struct AuthState {
    pub pending: HashMap<String, Pending>,
    codes: HashMap<String, Code>,
}

impl AuthState {
    pub fn begin(
        &mut self,
        authorization: Authorization,
        now: i64,
    ) -> Result<(String, String, String), String> {
        self.prune(now);
        if self.pending.len() + self.codes.len() >= MAX_PENDING {
            return Err("Too many pending authorizations; try again later".into());
        }
        let id = opaque();
        let nonce = opaque();
        let cookie = opaque();
        self.pending.insert(
            id.clone(),
            Pending {
                authorization,
                requested_at: now,
                expires_at: now + PENDING_TTL_MS,
                nonce_hash: credential_hash(&nonce),
                cookie_hash: credential_hash(&cookie),
                decision: None,
            },
        );
        Ok((id, nonce, cookie))
    }
    pub fn decide(&mut self, id: &str, access: Option<String>, now: i64) -> Result<(), String> {
        self.prune(now);
        if access
            .as_ref()
            .is_some_and(|a| !matches!(a.as_str(), "read_only" | "supervised" | "full_access"))
        {
            return Err("Unknown access ceiling".into());
        }
        let pending = self.pending.get_mut(id).ok_or("Request no longer exists")?;
        if pending.decision.is_some() {
            return Err("Request already decided".into());
        }
        pending.decision = Some(access);
        Ok(())
    }
    pub fn complete(
        &mut self,
        id: &str,
        nonce: &str,
        cookie: &str,
        now: i64,
    ) -> Result<Option<String>, String> {
        self.prune(now);
        let p = self.pending.get(id).ok_or("Request no longer exists")?;
        if !challenge_valid(nonce)
            || !challenge_valid(cookie)
            || !fixed_equal(credential_hash(nonce).as_bytes(), p.nonce_hash.as_bytes())
            || !fixed_equal(credential_hash(cookie).as_bytes(), p.cookie_hash.as_bytes())
        {
            return Err("Browser authorization binding does not match".into());
        }
        if p.decision.is_none() {
            return Ok(None);
        }
        let p = self.pending.remove(id).unwrap();
        let Some(access) = p.decision.flatten() else {
            return Ok(Some(redirect_response(&p.authorization, None)));
        };
        let code = opaque();
        self.codes.insert(
            credential_hash(&code),
            Code {
                authorization: p.authorization.clone(),
                access,
                expires_at: now + CODE_TTL_MS,
            },
        );
        Ok(Some(redirect_response(&p.authorization, Some(&code))))
    }
    pub fn exchange<T>(
        &mut self,
        token: &TokenInput,
        resource: &str,
        now: i64,
        mint: impl FnOnce(&Code) -> Result<T, String>,
    ) -> Result<T, String> {
        self.prune(now);
        if token.grant_type != "authorization_code" || !challenge_valid(&token.code) {
            return Err("Invalid authorization-code grant".into());
        }
        let hash = credential_hash(&token.code);
        let code = self.codes.get(&hash).ok_or("Invalid authorization code")?;
        if code.authorization.client_id != token.client_id
            || code.authorization.redirect_uri != token.redirect_uri
            || code.authorization.resource != token.resource
            || resource != token.resource
            || !pkce_verifies(&token.code_verifier, &code.authorization.challenge)
        {
            return Err("Invalid authorization-code grant".into());
        }
        let result = mint(code)?;
        self.codes.remove(&hash);
        Ok(result)
    }
    pub(crate) fn retain_issuer(&mut self, issuer: &str) {
        let resource = format!("{issuer}/mcp");
        self.pending.retain(|_, p| {
            p.authorization.issuer == issuer && p.authorization.resource == resource
        });
        self.codes.retain(|_, c| {
            c.authorization.issuer == issuer && c.authorization.resource == resource
        });
    }
    pub(crate) fn protected_clients(&mut self, now: i64) -> Vec<String> {
        self.prune(now);
        self.pending
            .values()
            .map(|p| p.authorization.client_id.clone())
            .chain(
                self.codes
                    .values()
                    .map(|c| c.authorization.client_id.clone()),
            )
            .collect()
    }
    pub fn prune(&mut self, now: i64) {
        self.pending.retain(|_, p| p.expires_at > now);
        self.codes.retain(|_, c| c.expires_at > now);
    }
    pub fn clear(&mut self) {
        self.pending.clear();
        self.codes.clear();
    }
    pub fn revoke_client(&mut self, id: &str) {
        self.pending.retain(|_, p| p.authorization.client_id != id);
        self.codes.retain(|_, c| c.authorization.client_id != id);
    }
}

fn redirect_response(authorization: &Authorization, code: Option<&str>) -> String {
    let mut url = Url::parse(&authorization.redirect_uri).expect("validated redirect");
    let retained: Vec<_> = url
        .query_pairs()
        .filter(|(k, _)| {
            !matches!(
                k.as_ref(),
                "code" | "state" | "iss" | "error" | "error_description"
            )
        })
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    url.set_query(None);
    {
        let mut query = url.query_pairs_mut();
        query.extend_pairs(retained);
        if let Some(code) = code {
            query.append_pair("code", code);
        } else {
            query.append_pair("error", "access_denied");
        }
        if let Some(state) = &authorization.state {
            query.append_pair("state", state);
        }
        query.append_pair("iss", &authorization.issuer);
    }
    url.to_string()
}

pub mod protocol {
    //! Stateless MCP transport boundaries, shared with the exact-source test lane.
    use axum::http::{HeaderMap, StatusCode};
    use serde_json::{json, Value};

    pub const PROTOCOL: &str = "2025-06-18";
    pub const MAX_MCP_BODY: usize = 131_072;
    pub const MAX_OAUTH_BODY: usize = 16_384;

    #[derive(Default)]
    pub struct Rates {
        oauth: std::collections::VecDeque<i64>,
        mcp: std::collections::VecDeque<i64>,
    }
    impl Rates {
        pub fn admit(&mut self, mcp: bool, now: i64) -> bool {
            let (queue, limit) = if mcp {
                (&mut self.mcp, 600)
            } else {
                (&mut self.oauth, 60)
            };
            while queue
                .front()
                .is_some_and(|time| now.saturating_sub(*time) >= 60_000)
            {
                queue.pop_front();
            }
            if queue.len() >= limit {
                return false;
            }
            queue.push_back(now);
            true
        }
    }

    pub fn header_value<'a>(headers: &'a HeaderMap, name: &str) -> Result<Option<&'a str>, ()> {
        let mut values = headers.get_all(name).iter();
        let value = values.next();
        if values.next().is_some() {
            return Err(());
        }
        value.map(|v| v.to_str().map_err(|_| ())).transpose()
    }

    pub fn canonical_host(headers: &HeaderMap, issuer: &str) -> bool {
        let Ok(Some(host)) = header_value(headers, "host") else {
            return false;
        };
        if host.is_empty()
            || host.chars().any(|c| c.is_whitespace() || c.is_control())
            || host.contains(['/', '\\', '?', '#', '@'])
        {
            return false;
        }
        let scheme = if issuer.starts_with("https://") {
            "https"
        } else {
            "http"
        };
        crate::mcp_connector::oauth::public_origin(&format!("{scheme}://{host}"))
            .is_ok_and(|v| v == issuer)
    }

    pub fn network_ok(headers: &HeaderMap, issuer: &str, port: u16, completion: bool) -> bool {
        let locals = [
            format!("127.0.0.1:{port}"),
            format!("localhost:{port}"),
            format!("[::1]:{port}"),
        ];
        let Ok(Some(host)) = header_value(headers, "host") else {
            return false;
        };
        if !canonical_host(headers, issuer) && !locals.iter().any(|h| h == host) {
            return false;
        }
        let Ok(origin) = header_value(headers, "origin") else {
            return false;
        };
        if completion {
            return origin == Some(issuer);
        }
        origin.is_none_or(|o| o == issuer || locals.iter().any(|h| o == format!("http://{h}")))
    }

    pub fn content_type(headers: &HeaderMap, wanted: &str) -> bool {
        let Ok(Some(value)) = header_value(headers, "content-type") else {
            return false;
        };
        let mut parts = value.split(';');
        parts
            .next()
            .is_some_and(|s| s.trim().eq_ignore_ascii_case(wanted))
            && parts.all(|s| s.trim().eq_ignore_ascii_case("charset=utf-8"))
    }

    pub fn mcp_headers(headers: &HeaderMap, initialize: bool) -> Result<(), StatusCode> {
        if !content_type(headers, "application/json") {
            return Err(StatusCode::UNSUPPORTED_MEDIA_TYPE);
        }
        let accept = header_value(headers, "accept").ok().flatten().unwrap_or("");
        let has = |wanted| {
            accept.split(',').any(|part| {
                let mut p = part.trim().split(';');
                p.next() == Some(wanted)
                    && p.all(|s| {
                        s.trim()
                            .strip_prefix("q=")
                            .is_some_and(|q| q.parse::<f32>().is_ok_and(|v| v > 0.0 && v <= 1.0))
                    })
            })
        };
        if !has("application/json") || !has("text/event-stream") {
            return Err(StatusCode::NOT_ACCEPTABLE);
        }
        match header_value(headers, "mcp-protocol-version") {
            Ok(Some(PROTOCOL)) => Ok(()),
            Ok(None) if initialize => Ok(()),
            _ => Err(StatusCode::BAD_REQUEST),
        }
    }

    struct StrictValue(Value);
    impl<'de> serde::Deserialize<'de> for StrictValue {
        fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
            struct Visitor;
            impl<'de> serde::de::Visitor<'de> for Visitor {
                type Value = StrictValue;
                fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                    f.write_str("JSON without duplicate keys")
                }
                fn visit_bool<E: serde::de::Error>(self, v: bool) -> Result<Self::Value, E> {
                    Ok(StrictValue(Value::Bool(v)))
                }
                fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<Self::Value, E> {
                    Ok(StrictValue(v.into()))
                }
                fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<Self::Value, E> {
                    Ok(StrictValue(v.into()))
                }
                fn visit_f64<E: serde::de::Error>(self, v: f64) -> Result<Self::Value, E> {
                    serde_json::Number::from_f64(v)
                        .map(|n| StrictValue(Value::Number(n)))
                        .ok_or_else(|| E::custom("Invalid number"))
                }
                fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Self::Value, E> {
                    Ok(StrictValue(v.into()))
                }
                fn visit_string<E: serde::de::Error>(self, v: String) -> Result<Self::Value, E> {
                    Ok(StrictValue(v.into()))
                }
                fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                    Ok(StrictValue(Value::Null))
                }
                fn visit_none<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                    Ok(StrictValue(Value::Null))
                }
                fn visit_seq<A: serde::de::SeqAccess<'de>>(
                    self,
                    mut seq: A,
                ) -> Result<Self::Value, A::Error> {
                    let mut values = Vec::new();
                    while let Some(StrictValue(v)) = seq.next_element()? {
                        values.push(v);
                    }
                    Ok(StrictValue(Value::Array(values)))
                }
                fn visit_map<A: serde::de::MapAccess<'de>>(
                    self,
                    mut map: A,
                ) -> Result<Self::Value, A::Error> {
                    let mut values = serde_json::Map::new();
                    while let Some(k) = map.next_key::<String>()? {
                        if values.contains_key(&k) {
                            return Err(serde::de::Error::custom("Duplicate JSON member"));
                        }
                        let StrictValue(v) = map.next_value()?;
                        values.insert(k, v);
                    }
                    Ok(StrictValue(Value::Object(values)))
                }
            }
            deserializer.deserialize_any(Visitor)
        }
    }

    pub fn strict_json(bytes: &[u8]) -> Result<Value, String> {
        serde_json::from_slice::<StrictValue>(bytes)
            .map(|v| v.0)
            .map_err(|_| "Invalid or ambiguous JSON".into())
    }

    pub fn form_json(bytes: &[u8]) -> Result<Value, String> {
        fn decode(value: &str) -> Result<String, String> {
            let bytes = value.as_bytes();
            let mut out = Vec::new();
            let mut i = 0;
            while i < bytes.len() {
                match bytes[i] {
                    b'%' => {
                        if i + 2 >= bytes.len()
                            || !bytes[i + 1].is_ascii_hexdigit()
                            || !bytes[i + 2].is_ascii_hexdigit()
                        {
                            return Err("Invalid form encoding".into());
                        }
                        out.push(
                            u8::from_str_radix(&value[i + 1..i + 3], 16)
                                .map_err(|_| "Invalid form encoding")?,
                        );
                        i += 3;
                    }
                    b'+' => {
                        out.push(b' ');
                        i += 1;
                    }
                    b => {
                        out.push(b);
                        i += 1;
                    }
                }
            }
            String::from_utf8(out).map_err(|_| "Invalid form UTF-8".into())
        }
        if bytes.len() > MAX_OAUTH_BODY {
            return Err("Form exceeds limit".into());
        }
        let text = std::str::from_utf8(bytes).map_err(|_| "Invalid form UTF-8")?;
        let mut out = serde_json::Map::new();
        for pair in text.split('&').filter(|p| !p.is_empty()) {
            let (k, v) = pair.split_once('=').ok_or("Invalid form")?;
            let (k, v) = (decode(k)?, decode(v)?);
            if k.is_empty()
                || k.len() > 100
                || k.chars().any(char::is_control)
                || v.chars().any(char::is_control)
                || out.contains_key(&k)
            {
                return Err("Invalid or duplicate form field".into());
            }
            out.insert(k, Value::String(v));
        }
        Ok(Value::Object(out))
    }

    pub struct Rpc {
        pub id: Option<Value>,
        pub method: String,
        pub params: Value,
    }
    fn keys(value: &Value, allowed: &[&str]) -> bool {
        value
            .as_object()
            .is_some_and(|o| o.keys().all(|k| allowed.contains(&k.as_str())))
    }
    fn label(value: &Value) -> bool {
        value
            .as_str()
            .is_some_and(|s| !s.is_empty() && s.len() <= 128 && !s.chars().any(char::is_control))
    }
    fn valid_id(value: &Value) -> bool {
        label(value) || value.as_i64().is_some() || value.as_u64().is_some()
    }
    impl Rpc {
        pub fn parse(value: Value) -> Result<Self, (i64, &'static str)> {
            if !keys(&value, &["jsonrpc", "id", "method", "params"])
                || value["jsonrpc"] != "2.0"
                || !label(&value["method"])
                || value.get("id").is_some_and(|id| !valid_id(id))
                || value.get("params").is_some_and(|p| !p.is_object())
            {
                return Err((-32600, "Invalid Request"));
            }
            Ok(Self {
                id: value.get("id").cloned(),
                method: value["method"].as_str().unwrap().into(),
                params: value.get("params").cloned().unwrap_or(json!({})),
            })
        }
        pub fn validate_method(&self) -> Result<(), (i64, &'static str)> {
            if self.params.get("_meta").is_some_and(|v| !v.is_object()) {
                return Err((-32602, "Invalid params"));
            }
            if self.id.is_none() {
                let valid = match self.method.as_str() {
                    "notifications/initialized" => keys(&self.params, &["_meta"]),
                    "notifications/cancelled" => {
                        keys(&self.params, &["requestId", "reason", "_meta"])
                            && valid_id(&self.params["requestId"])
                            && self
                                .params
                                .get("reason")
                                .is_none_or(|r| r.as_str().is_some_and(|s| s.len() <= 256))
                    }
                    _ => false,
                };
                return if valid {
                    Ok(())
                } else {
                    Err((-32600, "Unsupported notification"))
                };
            }
            let valid = match self.method.as_str() {
                "initialize" => {
                    keys(
                        &self.params,
                        &["protocolVersion", "capabilities", "clientInfo", "_meta"],
                    ) && self.params["protocolVersion"] == PROTOCOL
                        && self.params["capabilities"].is_object()
                        && keys(&self.params["clientInfo"], &["name", "version", "title"])
                        && label(&self.params["clientInfo"]["name"])
                        && label(&self.params["clientInfo"]["version"])
                        && self.params["clientInfo"].get("title").is_none_or(label)
                }
                "ping" | "tools/list" => keys(&self.params, &["_meta"]),
                "tools/call" => {
                    keys(&self.params, &["name", "arguments", "_meta"])
                        && label(&self.params["name"])
                        && self.params.get("arguments").is_none_or(Value::is_object)
                }
                _ => return Err((-32601, "Method not found")),
            };
            if valid {
                Ok(())
            } else {
                Err((-32602, "Invalid params"))
            }
        }
    }

    // This evaluates the deliberately small schema vocabulary used by the native
    // catalog, not arbitrary client-supplied schemas. Unknown keys are rejected at
    // each object boundary; native typed deserialization remains a second gate.
    pub fn schema_valid(schema: &Value, value: &Value) -> bool {
        let matches_type = |t: &str| match t {
            "object" => value.is_object(),
            "array" => value.is_array(),
            "string" => value.is_string(),
            "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
            "boolean" => value.is_boolean(),
            "null" => value.is_null(),
            _ => false,
        };
        let types = &schema["type"];
        if !types.as_str().is_some_and(matches_type)
            && !types
                .as_array()
                .is_some_and(|a| a.iter().any(|t| t.as_str().is_some_and(matches_type)))
        {
            return false;
        }
        if schema
            .get("enum")
            .is_some_and(|e| !e.as_array().is_some_and(|a| a.contains(value)))
        {
            return false;
        }
        if let Some(o) = value.as_object() {
            let properties = schema.get("properties").and_then(Value::as_object);
            if schema
                .get("required")
                .and_then(Value::as_array)
                .is_some_and(|required| {
                    required
                        .iter()
                        .any(|k| !k.as_str().is_some_and(|k| o.contains_key(k)))
                })
            {
                return false;
            }
            for (k, v) in o {
                match properties.and_then(|p| p.get(k)) {
                    Some(s) if schema_valid(s, v) => {}
                    None if schema["additionalProperties"] != false => {}
                    _ => return false,
                }
            }
        }
        if let Some(s) = value.as_str() {
            let length = s.chars().count() as u64;
            if schema["minLength"].as_u64().is_some_and(|min| length < min)
                || schema["maxLength"].as_u64().is_some_and(|max| length > max)
            {
                return false;
            }
        }
        if let Some(n) = value.as_f64() {
            if schema["minimum"].as_f64().is_some_and(|min| n < min)
                || schema["maximum"].as_f64().is_some_and(|max| n > max)
            {
                return false;
            }
        }
        true
    }

    pub fn escaped(value: &str) -> String {
        value
            .replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
            .replace('\'', "&#39;")
    }

    pub fn consent_csp(callback_origin: &str) -> Result<String, String> {
        if crate::mcp_connector::oauth::public_origin(callback_origin)? != callback_origin
            || callback_origin.contains([';', '\'', '"'])
        {
            return Err("Invalid callback origin".into());
        }
        // Chromium applies form-action to the 303 callback as well as the POST.
        // Allow only this already validated origin, not arbitrary navigation.
        Ok(format!("default-src 'none'; base-uri 'none'; form-action 'self' {callback_origin}; frame-ancestors 'none'"))
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use axum::http::header;
        #[test]
        fn rates_are_global_bounded_and_do_not_trust_forwarded_client_keys() {
            let mut rates = Rates::default();
            for _ in 0..60 {
                assert!(rates.admit(false, 0));
            }
            assert!(!rates.admit(false, 1));
            assert!(rates.admit(true, 1));
            assert!(!rates.admit(false, -1));
            assert!(rates.admit(false, 60_000));
            assert!(rates.oauth.len() <= 60);
        }
        #[test]
        fn consent_csp_allows_only_validated_callback_redirects_without_inline_scripts() {
            let policy = consent_csp("https://client.example").unwrap();
            assert!(policy.contains("form-action 'self' https://client.example;"));
            assert!(policy.contains("frame-ancestors 'none'"));
            assert!(!policy.contains("unsafe-inline"));
            assert!(consent_csp("https://client.example;script-src *").is_err());
            assert!(consent_csp("http://evil.example").is_err());
        }
        fn headers() -> HeaderMap {
            let mut h = HeaderMap::new();
            h.insert(header::HOST, "mcp.example".parse().unwrap());
            h.insert(header::CONTENT_TYPE, "application/json".parse().unwrap());
            h.insert(
                header::ACCEPT,
                "application/json, text/event-stream".parse().unwrap(),
            );
            h.insert("mcp-protocol-version", PROTOCOL.parse().unwrap());
            h
        }
        #[test]
        fn network_rejects_host_origin_forwarded_spoofing_and_completion_csrf() {
            let mut h = headers();
            assert!(network_ok(&h, "https://mcp.example", 4377, false));
            h.insert(header::HOST, "evil.example".parse().unwrap());
            h.insert("x-forwarded-host", "mcp.example".parse().unwrap());
            assert!(!network_ok(&h, "https://mcp.example", 4377, false));
            h.insert(header::HOST, "127.0.0.1:4377".parse().unwrap());
            assert!(network_ok(&h, "https://mcp.example", 4377, false));
            h.insert(header::ORIGIN, "https://evil.example".parse().unwrap());
            assert!(!network_ok(&h, "https://mcp.example", 4377, false));
            h.insert(header::ORIGIN, "https://mcp.example".parse().unwrap());
            assert!(network_ok(&h, "https://mcp.example", 4377, true));
            h.remove(header::ORIGIN);
            assert!(!network_ok(&h, "https://mcp.example", 4377, true));
            h.append(header::HOST, "mcp.example".parse().unwrap());
            assert!(!network_ok(&h, "https://mcp.example", 4377, false));
        }
        #[test]
        fn content_accept_and_protocol_headers_have_exact_statuses() {
            let mut h = headers();
            assert_eq!(mcp_headers(&h, false), Ok(()));
            h.insert(header::CONTENT_TYPE, "text/plain".parse().unwrap());
            assert_eq!(
                mcp_headers(&h, false),
                Err(StatusCode::UNSUPPORTED_MEDIA_TYPE)
            );
            h.insert(header::CONTENT_TYPE, "application/json".parse().unwrap());
            h.insert(header::ACCEPT, "application/json".parse().unwrap());
            assert_eq!(mcp_headers(&h, false), Err(StatusCode::NOT_ACCEPTABLE));
            h.insert(
                header::ACCEPT,
                "application/json, text/event-stream".parse().unwrap(),
            );
            h.insert("mcp-protocol-version", "2024-11-05".parse().unwrap());
            assert_eq!(mcp_headers(&h, false), Err(StatusCode::BAD_REQUEST));
            h.remove("mcp-protocol-version");
            assert_eq!(mcp_headers(&h, true), Ok(()));
            assert_eq!(mcp_headers(&h, false), Err(StatusCode::BAD_REQUEST));
        }
        #[test]
        fn strict_json_and_form_reject_ambiguous_duplicate_members() {
            assert!(strict_json(br#"{"id":1,"id":2}"#).is_err());
            assert!(
                strict_json(br#"{"arguments":{"access":"read_only","access":"full_access"}}"#)
                    .is_err()
            );
            assert!(form_json(b"client_id=one&client_id=two").is_err());
            assert!(form_json(b"state=%GG").is_err());
        }
        #[test]
        fn rpc_rejects_batch_null_float_missing_method_and_envelope_extras() {
            for v in [
                json!([]),
                json!({"jsonrpc":"2.0","id":null,"method":"ping"}),
                json!({"jsonrpc":"2.0","id":1.5,"method":"ping"}),
                json!({"jsonrpc":"1.0","id":1,"method":"ping"}),
                json!({"jsonrpc":"2.0","id":1}),
                json!({"jsonrpc":"2.0","id":1,"method":"ping","access":"full_access"}),
            ] {
                assert!(Rpc::parse(v).is_err());
            }
            let r = Rpc::parse(json!({"jsonrpc":"2.0","id":1,"method":"ping"})).unwrap();
            assert_eq!(r.id, Some(json!(1)));
            assert!(r.validate_method().is_ok());
        }
        #[test]
        fn methods_validate_initialize_and_disallow_notification_mutations() {
            for v in [
                json!({"jsonrpc":"2.0","method":"tools/call","params":{"name":"thread_stop","arguments":{}}}),
                json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"wrong","capabilities":{},"clientInfo":{"name":"agent","version":"1"}}}),
                json!({"jsonrpc":"2.0","id":1,"method":"ping","params":{"unexpected":true}}),
            ] {
                assert!(Rpc::parse(v).unwrap().validate_method().is_err());
            }
            let n =
                Rpc::parse(json!({"jsonrpc":"2.0","method":"notifications/initialized"})).unwrap();
            assert!(n.validate_method().is_ok());
        }
        #[test]
        fn schema_rejects_unknown_args_wrong_types_and_nested_authority() {
            let s = json!({"type":"object","additionalProperties":false,"required":["target"],"properties":{"target":{"type":"string","minLength":1},"timeout":{"type":"integer","minimum":1,"maximum":20},"nested":{"type":"object","additionalProperties":false,"properties":{"branch":{"type":"string"}}}}});
            assert!(schema_valid(&s, &json!({"target":"known","timeout":10})));
            for v in [
                json!({}),
                json!({"target":""}),
                json!({"target":"known","access":"full_access"}),
                json!({"target":"known","timeout":1.5}),
                json!({"target":"known","timeout":21}),
                json!({"target":"known","nested":{"env":{"API_KEY":"no"}}}),
            ] {
                assert!(!schema_valid(&s, &v));
            }
            assert_eq!(escaped("<script>\"&'"), "&lt;script&gt;&quot;&amp;&#39;");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn authorization() -> Authorization {
        validate_authorization(
            input(),
            "Agent".into(),
            &["https://client.example/cb".into()],
            "https://mcp.example",
        )
        .unwrap()
    }
    fn approved() -> (AuthState, TokenInput) {
        let mut s = AuthState::default();
        let (id, nonce, cookie) = s.begin(authorization(), 0).unwrap();
        s.decide(&id, Some("read_only".into()), 1).unwrap();
        let redirect = s.complete(&id, &nonce, &cookie, 2).unwrap().unwrap();
        let code = Url::parse(&redirect)
            .unwrap()
            .query_pairs()
            .find(|(k, _)| k == "code")
            .unwrap()
            .1
            .into_owned();
        (
            s,
            TokenInput {
                grant_type: "authorization_code".into(),
                client_id: "registered".into(),
                code,
                redirect_uri: "https://client.example/cb".into(),
                resource: "https://mcp.example/mcp".into(),
                code_verifier: "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk".into(),
            },
        )
    }
    #[test]
    fn raw_origin_and_redirect_spelling_do_not_normalize_authority() {
        for origin in [
            "https://mcp.example/./",
            "https://mcp.example/x/../",
            "https://@mcp.example",
            "https:mcp.example",
        ] {
            assert!(public_origin(origin).is_err(), "normalized origin {origin}");
        }
    }
    #[test]
    fn loopback_redirects_preserve_raw_path_spelling() {
        assert!(!redirect_matches(
            "http://localhost:1/x/../cb",
            "http://localhost:2/cb"
        ));
        assert!(!redirect_matches(
            "http://localhost:1/cb",
            "http://localhost:2/cb/"
        ));
    }
    #[test]
    fn callback_response_replaces_reserved_oauth_parameters() {
        let mut a = authorization();
        a.redirect_uri =
            "https://client.example/cb?state=old&code=old&iss=old&error=old&keep=ok".into();
        let redirect = redirect_response(&a, Some("new-code"));
        let url = Url::parse(&redirect).unwrap();
        let query: Vec<_> = url.query_pairs().collect();
        assert_eq!(query.iter().filter(|(k, _)| k == "code").count(), 1);
        assert_eq!(
            query.iter().find(|(k, _)| k == "code").unwrap().1,
            "new-code"
        );
        assert!(query.iter().all(|(k, _)| k != "error"));
        assert!(query.iter().any(|(k, v)| k == "keep" && v == "ok"));
    }
    #[test]
    fn the_visible_form_nonce_cannot_forge_the_http_only_browser_cookie() {
        let mut s = AuthState::default();
        let (id, nonce, cookie) = s.begin(authorization(), 0).unwrap();
        s.decide(&id, Some("read_only".into()), 1).unwrap();
        assert!(s.complete(&id, &nonce, &nonce, 2).is_err());
        assert!(s.complete(&id, &nonce, &cookie, 2).is_ok());
    }
    #[test]
    fn browser_completion_requires_both_bound_nonce_and_cookie() {
        let mut s = AuthState::default();
        let (id, n, cookie) = s.begin(authorization(), 0).unwrap();
        s.decide(&id, Some("read_only".into()), 1).unwrap();
        assert!(s
            .complete(&id, "known-id-is-not-nonce", &cookie, 2)
            .is_err());
        assert!(s.complete(&id, &n, "", 2).is_err());
        let redirect = s.complete(&id, &n, &cookie, 2).unwrap().unwrap();
        assert!(redirect.contains("state=client-state"));
        assert!(redirect.contains("iss=https%3A%2F%2Fmcp.example"));
        assert!(s.complete(&id, &n, &cookie, 3).is_err());
    }
    #[test]
    fn denial_never_mints_a_code() {
        let mut s = AuthState::default();
        let (id, n, cookie) = s.begin(authorization(), 0).unwrap();
        s.decide(&id, None, 1).unwrap();
        let redirect = s.complete(&id, &n, &cookie, 2).unwrap().unwrap();
        assert!(redirect.contains("error=access_denied"));
        assert!(s.codes.is_empty());
    }
    #[test]
    fn codes_verify_tuple_before_atomic_consume_and_never_replay() {
        for field in ["client", "redirect", "resource", "pkce", "grant"] {
            let (mut s, t) = approved();
            let mut bad = t.clone();
            match field {
                "client" => bad.client_id = "other".into(),
                "redirect" => bad.redirect_uri.push_str("?changed"),
                "resource" => bad.resource.push_str("/"),
                "pkce" => bad.code_verifier = "x".repeat(43),
                _ => bad.grant_type = "refresh_token".into(),
            }
            assert!(
                s.exchange(&bad, &t.resource, 3, |_| Ok(())).is_err(),
                "accepted {field}"
            );
            assert_eq!(
                s.exchange(&t, &t.resource, 3, |c| Ok(c.access.clone()))
                    .unwrap(),
                "read_only"
            );
            assert!(s.exchange(&t, &t.resource, 3, |_| Ok(())).is_err());
        }
        let (mut s, t) = approved();
        assert!(s
            .exchange::<()>(&t, &t.resource, 3, |_| Err("store failed".into()))
            .is_err());
        assert!(s.exchange(&t, &t.resource, 3, |_| Ok(())).is_ok());
    }
    #[test]
    fn expiry_bounds_revocation_and_disable_prevent_late_resurrection() {
        let (mut s, t) = approved();
        assert!(s
            .exchange(&t, &t.resource, 2 + CODE_TTL_MS, |_| Ok(()))
            .is_err());
        let mut s = AuthState::default();
        let (id, n, cookie) = s.begin(authorization(), 0).unwrap();
        assert!(s
            .decide(&id, Some("read_only".into()), PENDING_TTL_MS)
            .is_err());
        assert!(s.complete(&id, &n, &cookie, PENDING_TTL_MS).is_err());
        for _ in 0..MAX_PENDING {
            s.begin(authorization(), PENDING_TTL_MS).unwrap();
        }
        assert!(s.begin(authorization(), PENDING_TTL_MS).is_err());
        s.clear();
        assert!(s.pending.is_empty());
        assert!(s
            .decide(&id, Some("full_access".into()), PENDING_TTL_MS)
            .is_err());
        let (mut s, t) = approved();
        s.revoke_client("registered");
        assert!(s.exchange(&t, &t.resource, 3, |_| Ok(())).is_err());
    }
    fn input() -> AuthorizationInput {
        AuthorizationInput {
            client_id: "registered".into(),
            redirect_uri: "https://client.example/cb".into(),
            response_type: "code".into(),
            code_challenge_method: "S256".into(),
            code_challenge: "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM".into(),
            resource: "https://mcp.example/mcp".into(),
            state: Some("client-state".into()),
            scope: None,
        }
    }
    #[test]
    fn pkce_requires_s256_with_rfc_verifier_bounds() {
        let challenge = input().code_challenge;
        assert!(pkce_verifies(
            "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk",
            &challenge
        ));
        for verifier in [
            "bad".into(),
            "a".repeat(129),
            "!".repeat(43),
            "a".repeat(43),
        ] {
            assert!(!pkce_verifies(&verifier, &challenge));
        }
    }
    #[test]
    fn authorization_rejects_resource_pkce_redirect_and_state_changes() {
        let check = |i| {
            validate_authorization(
                i,
                "Untrusted app".into(),
                &["https://client.example/cb".into()],
                "https://mcp.example",
            )
        };
        assert!(check(input()).is_ok());
        for field in [
            "resource",
            "method",
            "challenge",
            "redirect",
            "response",
            "state",
            "scope",
        ] {
            let mut i = input();
            match field {
                "resource" => i.resource.push_str("?foo"),
                "method" => i.code_challenge_method = "plain".into(),
                "challenge" => i.code_challenge = "!".repeat(43),
                "redirect" => i.redirect_uri = "https://evil.example/cb".into(),
                "response" => i.response_type = "token".into(),
                "state" => i.state = Some("s".repeat(1025)),
                _ => i.scope = Some("admin".into()),
            }
            assert!(check(i).is_err(), "accepted {field}");
        }
    }
    #[test]
    fn registration_is_strict_bounded_and_accepts_requested_public_client_auth() {
        let parse = |s| serde_json::from_str::<Registration>(s).unwrap();
        let r = parse(
            r#"{"client_name":"Agent","redirect_uris":["https://client.example/cb"],"token_endpoint_auth_method":"client_secret_post","grant_types":["authorization_code","refresh_token"]}"#,
        );
        assert_eq!(r.validate().unwrap().0, "Agent");
        for s in [
            r#"{"client_name":"","redirect_uris":["https://client.example/cb"]}"#,
            r#"{"client_name":"bad\nlabel","redirect_uris":["https://client.example/cb"]}"#,
            r#"{"redirect_uris":[]}"#,
            r#"{"redirect_uris":["http://evil.example/cb"]}"#,
        ] {
            assert!(parse(s).validate().is_err());
        }
        assert!(serde_json::from_str::<Registration>(
            r#"{"redirect_uris":["https://client.example/cb"],"access":"full_access"}"#
        )
        .is_err());
    }
    #[test]
    fn origins_reject_untrusted_components() {
        for bad in [
            "https://user:pw@mcp.example",
            "https://mcp.example/path",
            "https://mcp.example?",
            "https://mcp.example#",
            " https://mcp.example",
            "http://evil.example",
            "https://mcp.example/ ",
            "https://mcp.example\\x",
            "https://mcp.example//",
        ] {
            assert!(public_origin(bad).is_err(), "accepted {bad}");
        }
        assert_eq!(
            public_origin("https://mcp.example/").unwrap(),
            "https://mcp.example"
        );
        assert_eq!(
            public_origin("http://127.0.0.1:4377").unwrap(),
            "http://127.0.0.1:4377"
        );
    }
    #[test]
    fn redirects_match_exactly_except_loopback_port() {
        assert!(redirect_matches(
            "https://client.example/cb",
            "https://client.example/cb"
        ));
        assert!(redirect_matches(
            "http://127.0.0.1:1/cb?x=1",
            "http://127.0.0.1:2/cb?x=1"
        ));
        for (a, b) in [
            (
                "https://client.example/cb",
                "https://client.example:8443/cb",
            ),
            ("http://127.0.0.1:1/cb", "http://localhost:2/cb"),
            ("http://127.0.0.1/cb", "http://127.0.0.1/other"),
            ("https://client.example/cb", "https://client.example/cb#"),
            ("http://evil.example/cb", "http://evil.example/cb"),
            (
                "https://user@client.example/cb",
                "https://user@client.example/cb",
            ),
            ("https://client.example/*", "https://client.example/*"),
        ] {
            assert!(!redirect_matches(a, b), "accepted {a} → {b}");
        }
    }
}
