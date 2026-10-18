//! Webhook-triggered automations.
//!
//! A webhook automation runs when its URL receives a request. The HTTP
//! route hands each request to [`accept_delivery`], which checks the URL
//! token and the optional signature, renders the prompt from the request,
//! and records the run. `executor::dispatch_webhook` wraps it to start the
//! agent.
//!
//! [`render_prompt`] and [`verify_signature`] are pure. They follow T3
//! Code's `webhookTemplate` / `webhookVerification`, so a template or a
//! signature setup written for one works in the other.
//!
//! The rendered prompt carries request content, which the sender controls.
//! Anyone holding the URL can therefore steer the agent, so the URL token
//! and the signing secret matter as much as the prompt itself.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use chrono::{DateTime, SecondsFormat, Utc};
use hmac::{Hmac, Mac};
use regex::Regex;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::database::automation_triggers::{SignatureEncoding, WebhookSignature};
use crate::database::{AutomationRecord, AutomationRunRecord, DatabaseStore};

/// Largest request body accepted. The HTTP route should refuse bigger
/// bodies before reading them; [`accept_delivery`] checks again.
pub const MAX_WEBHOOK_BODY_BYTES: usize = 1024 * 1024;

/// Largest rendered prompt. The prompt is passed to the agent CLI as one
/// argument, and Linux caps a single argument at 128 KiB.
pub const MAX_RENDERED_PROMPT_BYTES: usize = 100_000;

/// One incoming webhook request.
#[derive(Debug, Clone)]
pub struct WebhookRequest {
    pub method: String,
    pub path: String,
    /// Raw query string without the leading `?`.
    pub query: String,
    /// Header names are lowercase.
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
}

impl WebhookRequest {
    /// Build a request, lowercasing header names and joining repeated
    /// headers with `", "` as HTTP allows.
    pub fn new<'a>(
        method: &str,
        path: &str,
        query: &str,
        headers: impl IntoIterator<Item = (&'a str, &'a str)>,
        body: Vec<u8>,
    ) -> Self {
        let mut map: BTreeMap<String, String> = BTreeMap::new();
        for (name, value) in headers {
            map.entry(name.to_ascii_lowercase())
                .and_modify(|existing| {
                    existing.push_str(", ");
                    existing.push_str(value);
                })
                .or_insert_with(|| value.to_string());
        }
        Self {
            method: method.to_string(),
            path: path.to_string(),
            query: query.trim_start_matches('?').to_string(),
            headers: map,
            body,
        }
    }

    fn body_text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

// ── Prompt rendering ──

/// A prompt rendered from a request, with the placeholders that had no
/// value (rendered empty).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderedPrompt {
    pub prompt: String,
    pub missing: Vec<String>,
}

/// Header and query names that commonly carry credentials.
static CREDENTIAL_NAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)^(authorization|proxy-authorization|cookie|set-cookie)$|token|secret|signature|key|password|auth",
    )
    .expect("credential-name pattern compiles")
});

/// `{{ expression }}`. Braces cannot appear inside, so a stray `{{` never
/// swallows the rest of the template.
static PLACEHOLDER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\{\{([^{}]*)\}\}").expect("placeholder pattern compiles"));

const REDACTED: &str = "[redacted]";

fn redact_headers(headers: &BTreeMap<String, String>) -> Map<String, Value> {
    headers
        .iter()
        .map(|(name, value)| {
            let shown = if CREDENTIAL_NAME.is_match(name) {
                REDACTED.to_string()
            } else {
                value.clone()
            };
            (name.clone(), Value::String(shown))
        })
        .collect()
}

/// Redact credential-named values in a raw query string, keeping the rest
/// exactly as sent.
fn redact_query(query: &str) -> String {
    if query.is_empty() {
        return String::new();
    }
    query
        .split('&')
        .map(|part| match part.split_once('=') {
            Some((name, _)) => {
                let decoded = url::form_urlencoded::parse(name.as_bytes())
                    .next()
                    .map(|(key, _)| key.into_owned())
                    .unwrap_or_else(|| name.to_string());
                if CREDENTIAL_NAME.is_match(&decoded) {
                    format!("{name}={REDACTED}")
                } else {
                    part.to_string()
                }
            }
            None => part.to_string(),
        })
        .collect::<Vec<_>>()
        .join("&")
}

/// The body as JSON, or as an object of fields for a form-encoded body.
/// Senders are inconsistent about content types, so any other body that
/// parses as JSON is addressable too.
fn parse_body(request: &WebhookRequest) -> Option<Value> {
    let content_type = request
        .headers
        .get("content-type")
        .map(|value| value.to_ascii_lowercase())
        .unwrap_or_default();
    if content_type.contains("application/x-www-form-urlencoded") {
        let fields: Map<String, Value> = url::form_urlencoded::parse(&request.body)
            .map(|(key, value)| (key.into_owned(), Value::String(value.into_owned())))
            .collect();
        return Some(Value::Object(fields));
    }
    serde_json::from_slice(&request.body).ok()
}

fn lookup<'a>(root: &'a Value, segments: &[&str]) -> Option<&'a Value> {
    segments.iter().try_fold(root, |current, segment| match current {
        Value::Object(map) => map.get(*segment),
        Value::Array(items) => segment.parse::<usize>().ok().and_then(|i| items.get(i)),
        _ => None,
    })
}

/// Strings and numbers render as text, objects and arrays as JSON. `null`
/// counts as missing.
fn stringify(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::String(text) => Some(text.clone()),
        Value::Bool(flag) => Some(flag.to_string()),
        Value::Number(number) => Some(number.to_string()),
        other => serde_json::to_string_pretty(other).ok(),
    }
}

fn format_request(request: &WebhookRequest) -> String {
    let query = redact_query(&request.query);
    let mut lines = vec![if query.is_empty() {
        format!("{} {}", request.method, request.path)
    } else {
        format!("{} {}?{query}", request.method, request.path)
    }];
    for (name, value) in redact_headers(&request.headers) {
        lines.push(format!("{name}: {}", value.as_str().unwrap_or_default()));
    }
    lines.push(String::new());
    lines.push(request.body_text());
    lines.join("\n")
}

/// Render a webhook automation's prompt from the request that triggered
/// it. The syntax is plain `{{path}}` lookups, with no conditionals or
/// filters:
///
/// - `{{body.a.b.0}}`: a field of a JSON or form-encoded body
/// - `{{headers.name}}`: a request header (case-insensitive)
/// - `{{query.name}}`: a URL query parameter
/// - `{{body}}`: the raw body text
/// - `{{request}}`: method, path, query, headers and body
///
/// A path with no value renders empty and is listed in `missing`.
/// Credential-looking headers and query values are redacted wherever the
/// whole set renders (`{{request}}`, `{{headers}}`, `{{query}}`); naming
/// one (`{{headers.authorization}}`) gives its raw value. Substitution is
/// a single pass, so placeholders inside the request are left as text.
///
/// Returns `None` as soon as the prompt would exceed `max_bytes`, before
/// building it, so a template that repeats `{{body}}` cannot balloon.
pub fn render_prompt(
    template: &str,
    request: &WebhookRequest,
    max_bytes: usize,
) -> Option<RenderedPrompt> {
    let mut missing: Vec<String> = Vec::new();
    let mut parsed: Option<Option<Value>> = None;
    let mut body_text: Option<String> = None;
    let mut whole_request: Option<String> = None;
    let mut prompt = String::new();
    let push = |prompt: &mut String, text: &str| -> Option<()> {
        (prompt.len() + text.len() <= max_bytes).then(|| prompt.push_str(text))
    };

    let mut last = 0;
    for caps in PLACEHOLDER.captures_iter(template) {
        let whole = caps.get(0).expect("match has a whole group");
        push(&mut prompt, &template[last..whole.start()])?;
        last = whole.end();

        let expression = caps[1].trim();
        let mut parts = expression.split('.');
        let root = parts.next().unwrap_or_default();
        let segments: Vec<&str> = parts.collect();

        let value: Option<String> = match root {
            "request" if segments.is_empty() => Some(
                whole_request
                    .get_or_insert_with(|| format_request(request))
                    .clone(),
            ),
            "body" if segments.is_empty() => {
                Some(body_text.get_or_insert_with(|| request.body_text()).clone())
            }
            "body" => parsed
                .get_or_insert_with(|| parse_body(request))
                .as_ref()
                .and_then(|body| lookup(body, &segments))
                .and_then(stringify),
            "headers" if segments.is_empty() => {
                stringify(&Value::Object(redact_headers(&request.headers)))
            }
            "headers" => request
                .headers
                .get(&segments.join(".").to_ascii_lowercase())
                .cloned(),
            "query" if segments.is_empty() => {
                let fields: Map<String, Value> =
                    url::form_urlencoded::parse(redact_query(&request.query).as_bytes())
                        .map(|(key, value)| (key.into_owned(), Value::String(value.into_owned())))
                        .collect();
                stringify(&Value::Object(fields))
            }
            "query" => {
                let name = segments.join(".");
                url::form_urlencoded::parse(request.query.as_bytes())
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| value.into_owned())
            }
            _ => None,
        };

        match value {
            Some(text) => push(&mut prompt, &text)?,
            None => {
                if !missing.iter().any(|seen| seen == expression) {
                    missing.push(expression.to_string());
                }
            }
        }
    }
    push(&mut prompt, &template[last..])?;

    Some(RenderedPrompt { prompt, missing })
}

// ── Signature verification ──

/// Compare two byte strings in time that depends on neither their content
/// nor their length.
pub fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    let (a, b) = (Sha256::digest(a), Sha256::digest(b));
    a.iter().zip(b.iter()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn decode_hex(text: &str) -> Option<Vec<u8>> {
    if text.len() % 2 != 0 {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok())
        .collect()
}

/// Check an HMAC-SHA256 signature over the raw body bytes, as GitHub,
/// Linear, Shopify and most other signing senders do. The header value is
/// `<prefix><digest>`; the prefix matches case-insensitively and a hex
/// digest may use either case.
pub fn verify_signature(
    signature: &WebhookSignature,
    secret: &[u8],
    headers: &BTreeMap<String, String>,
    body: &[u8],
) -> bool {
    let Some(received) = headers.get(&signature.header.to_ascii_lowercase()) else {
        return false;
    };
    let value = received.trim();
    let prefix = signature.prefix.as_str();
    let Some(head) = value.get(..prefix.len()) else {
        return false;
    };
    if !head.eq_ignore_ascii_case(prefix) {
        return false;
    }
    let candidate = &value[prefix.len()..];
    let decoded = match signature.encoding {
        SignatureEncoding::Hex => decode_hex(candidate),
        SignatureEncoding::Base64 => {
            use base64::Engine;
            base64::engine::general_purpose::STANDARD.decode(candidate).ok()
        }
    };
    let Some(decoded) = decoded else {
        return false;
    };
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(secret) else {
        return false;
    };
    mac.update(body);
    // `verify_slice` compares in constant time.
    mac.verify_slice(&decoded).is_ok()
}

/// Normalise and check a signature setup before it is saved.
pub fn clean_signature(mut signature: WebhookSignature) -> Result<WebhookSignature, String> {
    signature.header = signature.header.trim().to_ascii_lowercase();
    if signature.header.is_empty() {
        return Err("Name the header that carries the signature".into());
    }
    if signature.header.len() > 100
        || !signature
            .header
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err("The signature header name is not a valid HTTP header".into());
    }
    if signature.prefix.len() > 64
        || !signature.prefix.bytes().all(|b| b.is_ascii_graphic())
    {
        return Err("The signature prefix must be up to 64 visible ASCII characters".into());
    }
    Ok(signature)
}

// ── Delivery ──

/// Why a webhook request did not start a run. `Display` text is safe to
/// return to the sender: it never includes the token, the secret or the
/// request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeliveryRejection {
    /// No webhook automation has this id, or the token is wrong. The two
    /// are indistinguishable on purpose.
    NotFound,
    Disabled,
    PayloadTooLarge,
    /// A signature is configured and the request's did not match.
    BadSignature,
    /// A signature is configured but it or its secret could not be read.
    /// Fails closed rather than running unverified.
    SignatureUnavailable,
    /// The rendered prompt was empty.
    EmptyPrompt,
    PromptTooLarge,
    /// A run is already in flight; this delivery is recorded as
    /// `skipped_busy`.
    Busy,
    /// A run for the same instant already exists.
    Duplicate,
    Storage(String),
}

impl std::fmt::Display for DeliveryRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            Self::NotFound => "Unknown webhook",
            Self::Disabled => "This automation is paused",
            Self::PayloadTooLarge => "Request body is too large",
            Self::BadSignature => "Signature check failed",
            Self::SignatureUnavailable => "The signature check is unavailable",
            Self::EmptyPrompt => "The rendered prompt is empty",
            Self::PromptTooLarge => "The rendered prompt is too large",
            Self::Busy => "A previous run is still in progress",
            Self::Duplicate => "This delivery was already recorded",
            Self::Storage(_) => "Could not record the run",
        };
        f.write_str(text)
    }
}

/// A delivery that started a run.
#[derive(Debug, Clone)]
pub struct AcceptedDelivery {
    /// The automation, with `prompt` replaced by the rendered prompt.
    pub automation: AutomationRecord,
    pub run: AutomationRunRecord,
    /// Placeholders that had no value in this request.
    pub missing: Vec<String>,
}

/// Validate one webhook request and record its run.
///
/// Checks, in order: body size, the URL token, that the automation is
/// enabled, and the signature (when configured). Then it renders the
/// prompt, with the URL token redacted from the path and query, and records a `scheduled` run, or a `skipped_busy` one when a
/// run is already in flight. The caller executes the returned run.
pub fn accept_delivery(
    db: &DatabaseStore,
    webhook_id: &str,
    token: &str,
    request: &WebhookRequest,
    now: DateTime<Utc>,
) -> Result<AcceptedDelivery, DeliveryRejection> {
    if request.body.len() > MAX_WEBHOOK_BODY_BYTES {
        return Err(DeliveryRejection::PayloadTooLarge);
    }
    let automation = db
        .find_webhook_automation(webhook_id)
        .ok_or(DeliveryRejection::NotFound)?;
    let expected = automation.webhook_token.as_deref().unwrap_or_default();
    if expected.is_empty() || !constant_time_eq(expected.as_bytes(), token.as_bytes()) {
        return Err(DeliveryRejection::NotFound);
    }
    if !automation.enabled {
        return Err(DeliveryRejection::Disabled);
    }
    // The signature and its secret are saved together, so a secret without
    // a readable signature means the stored check could not be decoded.
    // Refuse rather than run unverified.
    match (&automation.webhook_signature, automation.webhook_secret_ref.as_deref()) {
        (None, None) => {}
        (Some(signature), Some(secret_ref)) => {
            let secret = crate::secrets::reveal(db, secret_ref)
                .map_err(|_| DeliveryRejection::SignatureUnavailable)?;
            if !verify_signature(signature, secret.as_bytes(), &request.headers, &request.body) {
                return Err(DeliveryRejection::BadSignature);
            }
        }
        _ => return Err(DeliveryRejection::SignatureUnavailable),
    }

    // The URL token must never reach the agent, even when the route passes
    // the full request path and `{{request}}` renders it.
    let mut visible = request.clone();
    visible.path = visible.path.replace(expected, REDACTED);
    visible.query = visible.query.replace(expected, REDACTED);
    let rendered = render_prompt(&automation.prompt, &visible, MAX_RENDERED_PROMPT_BYTES)
        .ok_or(DeliveryRejection::PromptTooLarge)?;
    if rendered.prompt.trim().is_empty() {
        return Err(DeliveryRejection::EmptyPrompt);
    }

    // Microsecond precision keeps each delivery's key distinct under the
    // `UNIQUE(automation_id, scheduled_for)` constraint.
    let key = now.to_rfc3339_opts(SecondsFormat::Micros, true);
    let run = db
        .record_webhook_run(automation.id, &key)
        .map_err(DeliveryRejection::Storage)?
        .ok_or(DeliveryRejection::Duplicate)?;
    if run.status != "scheduled" {
        return Err(DeliveryRejection::Busy);
    }

    let mut automation = automation;
    automation.prompt = rendered.prompt;
    Ok(AcceptedDelivery {
        automation,
        run,
        missing: rendered.missing,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::automation_triggers::AutomationTrigger;
    use crate::database::AutomationInput;
    use chrono::TimeZone;

    fn request(content_type: &str, body: &str) -> WebhookRequest {
        WebhookRequest::new(
            "POST",
            "/hooks/abc",
            "ref=main&token=hunter2",
            [
                ("Content-Type", content_type),
                ("X-GitHub-Event", "issues"),
                ("Authorization", "Bearer abc"),
            ],
            body.as_bytes().to_vec(),
        )
    }

    fn github_signature() -> WebhookSignature {
        WebhookSignature {
            header: "x-hub-signature-256".into(),
            encoding: SignatureEncoding::Hex,
            prefix: "sha256=".into(),
        }
    }

    fn hmac_hex(secret: &str, body: &[u8]) -> String {
        let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(body);
        mac.finalize()
            .into_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect()
    }

    // ── render_prompt ──

    fn render(template: &str, request: &WebhookRequest) -> RenderedPrompt {
        render_prompt(template, request, usize::MAX).expect("within budget")
    }

    #[test]
    fn rendering_stops_at_the_byte_budget() {
        let req = request("text/plain", &"x".repeat(1000));
        let template = "{{body}}".repeat(10_000);
        assert!(render_prompt(&template, &req, 5_000).is_none());
        assert!(render_prompt("short", &req, 5).is_some());
        assert!(render_prompt("short!", &req, 5).is_none());
        // Missing placeholders cost nothing.
        assert_eq!(
            render_prompt("{{nope}}ab", &req, 2).unwrap().prompt,
            "ab"
        );
    }

    #[test]
    fn renders_json_body_paths_including_array_indices() {
        let req = request(
            "application/json",
            r#"{"issue":{"title":"Crash on start","number":42,"labels":[{"name":"bug"}]},"draft":false}"#,
        );
        let out = render(
            "Fix #{{body.issue.number}}: {{ body.issue.title }} [{{body.issue.labels.0.name}}] draft={{body.draft}}",
            &req,
        );
        assert_eq!(out.prompt, "Fix #42: Crash on start [bug] draft=false");
        assert!(out.missing.is_empty());
    }

    #[test]
    fn objects_render_as_json_and_missing_paths_render_empty() {
        let req = request("application/json", r#"{"a":{"b":1},"n":null}"#);
        let out = render("{{body.a}}|{{body.x.y}}|{{body.n}}|{{body.x.y}}|{{nope}}", &req);
        assert_eq!(out.prompt, "{\n  \"b\": 1\n}||||");
        assert_eq!(out.missing, vec!["body.x.y", "body.n", "nope"]);
    }

    #[test]
    fn parses_json_even_without_a_json_content_type() {
        let req = request("text/plain", r#"{"ok":"yes"}"#);
        assert_eq!(render("{{body.ok}}", &req).prompt, "yes");
    }

    #[test]
    fn form_encoded_bodies_are_addressable() {
        let req = request("application/x-www-form-urlencoded", "name=Ada+Lovelace&id=7");
        assert_eq!(
            render("{{body.name}} #{{body.id}}", &req).prompt,
            "Ada Lovelace #7"
        );
    }

    #[test]
    fn raw_body_headers_and_query_lookups() {
        let req = request("text/plain", "hello");
        let out = render(
            "{{body}} {{headers.x-github-event}} {{headers.X-GITHUB-EVENT}} {{query.ref}}",
            &req,
        );
        assert_eq!(out.prompt, "hello issues issues main");
    }

    #[test]
    fn whole_sets_redact_credentials_but_named_lookups_do_not() {
        let req = request("text/plain", "body text");
        let headers = render("{{headers}}", &req).prompt;
        assert!(headers.contains("[redacted]"));
        assert!(!headers.contains("Bearer abc"));
        assert!(headers.contains("issues"));

        let query = render("{{query}}", &req).prompt;
        assert!(query.contains("main"));
        assert!(!query.contains("hunter2"));

        let whole = render("{{request}}", &req).prompt;
        assert!(whole.starts_with("POST /hooks/abc?ref=main&token=[redacted]\n"));
        assert!(!whole.contains("Bearer abc"));
        assert!(whole.ends_with("\n\nbody text"));

        // Naming a credential explicitly is the user's choice.
        assert_eq!(
            render("{{headers.authorization}}", &req).prompt,
            "Bearer abc"
        );
    }

    #[test]
    fn placeholders_inside_the_request_are_not_expanded() {
        let req = request("application/json", r#"{"title":"{{headers.authorization}}"}"#);
        let out = render("{{body.title}}", &req);
        assert_eq!(out.prompt, "{{headers.authorization}}");
    }

    #[test]
    fn text_without_placeholders_and_stray_braces_pass_through() {
        let req = request("text/plain", "");
        assert_eq!(
            render("Plain {{ prompt { }}", &req).prompt,
            "Plain {{ prompt { }}"
        );
    }

    // ── verify_signature ──

    #[test]
    fn accepts_a_valid_github_signature() {
        let body = br#"{"action":"opened"}"#;
        let mut headers = BTreeMap::new();
        headers.insert(
            "x-hub-signature-256".to_string(),
            format!("sha256={}", hmac_hex("shh", body)),
        );
        assert!(verify_signature(&github_signature(), b"shh", &headers, body));
    }

    #[test]
    fn matches_githubs_documented_example() {
        // From GitHub's "Validating webhook deliveries" docs.
        let mut headers = BTreeMap::new();
        headers.insert(
            "x-hub-signature-256".to_string(),
            "sha256=757107ea0eb2509fc211221cce984b8a37570b6d7586c22c46f4379c8b043e17".to_string(),
        );
        assert!(verify_signature(
            &github_signature(),
            b"It's a Secret to Everybody",
            &headers,
            b"Hello, World!"
        ));
    }

    #[test]
    fn rejects_wrong_secret_tampered_body_and_missing_header() {
        let body = b"payload";
        let mut headers = BTreeMap::new();
        headers.insert(
            "x-hub-signature-256".to_string(),
            format!("sha256={}", hmac_hex("right", body)),
        );
        assert!(!verify_signature(&github_signature(), b"wrong", &headers, body));
        assert!(!verify_signature(&github_signature(), b"right", &headers, b"payload!"));
        assert!(!verify_signature(&github_signature(), b"right", &BTreeMap::new(), body));
    }

    #[test]
    fn prefix_is_required_and_case_insensitive_and_hex_case_is_ignored() {
        let body = b"x";
        let digest = hmac_hex("k", body);
        let mut headers = BTreeMap::new();
        headers.insert("x-hub-signature-256".to_string(), digest.clone());
        assert!(!verify_signature(&github_signature(), b"k", &headers, body));

        headers.insert(
            "x-hub-signature-256".to_string(),
            format!("SHA256={}", digest.to_uppercase()),
        );
        assert!(verify_signature(&github_signature(), b"k", &headers, body));
    }

    #[test]
    fn verifies_base64_without_a_prefix() {
        use base64::Engine;
        let body = b"shopify";
        let mut mac = Hmac::<Sha256>::new_from_slice(b"key").unwrap();
        mac.update(body);
        let digest = base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes());
        let signature = WebhookSignature {
            header: "X-Shopify-Hmac-Sha256".into(),
            encoding: SignatureEncoding::Base64,
            prefix: String::new(),
        };
        let mut headers = BTreeMap::new();
        headers.insert("x-shopify-hmac-sha256".to_string(), digest);
        assert!(verify_signature(&signature, b"key", &headers, body));
        assert!(!verify_signature(&signature, b"other", &headers, body));
    }

    #[test]
    fn malformed_digests_are_rejected_not_panicked_on() {
        let mut headers = BTreeMap::new();
        for bad in ["sha256=zz", "sha256=abc", "sha256=", "sha", "sha256=é1"] {
            headers.insert("x-hub-signature-256".to_string(), bad.to_string());
            assert!(!verify_signature(&github_signature(), b"k", &headers, b""));
        }
    }

    #[test]
    fn constant_time_eq_compares_content() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"abcd"));
    }

    #[test]
    fn clean_signature_normalises_and_validates() {
        let cleaned = clean_signature(WebhookSignature {
            header: " X-Hub-Signature-256 ".into(),
            encoding: SignatureEncoding::Hex,
            prefix: "sha256=".into(),
        })
        .unwrap();
        assert_eq!(cleaned.header, "x-hub-signature-256");

        for header in ["", "bad header", "x:y"] {
            assert!(clean_signature(WebhookSignature {
                header: header.into(),
                ..github_signature()
            })
            .is_err());
        }
        assert!(clean_signature(WebhookSignature {
            prefix: "has space".into(),
            ..github_signature()
        })
        .is_err());
    }

    // ── accept_delivery ──

    fn webhook_automation(db: &DatabaseStore, prompt: &str) -> AutomationRecord {
        db.insert_automation(&AutomationInput {
            name: "On issue".into(),
            prompt: prompt.into(),
            agent: "claude".into(),
            schedule: String::new(),
            timezone: "UTC".into(),
            host_id: None,
            project_path: None,
            project_remote: None,
            retention_limit: 10,
            trigger_type: Some(AutomationTrigger::Webhook),
        })
        .unwrap()
    }

    fn at(second: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 1, 2, 9, 0, second).unwrap()
    }

    #[test]
    fn accepts_a_delivery_and_renders_the_prompt_into_the_run() {
        let db = crate::database::init_test_database();
        let a = webhook_automation(&db, "Triage {{body.issue}} {{body.nope}}");
        let req = request("application/json", r##"{"issue":"#9"}"##);

        let accepted = accept_delivery(
            &db,
            a.webhook_id.as_deref().unwrap(),
            a.webhook_token.as_deref().unwrap(),
            &req,
            at(0),
        )
        .unwrap();
        assert_eq!(accepted.automation.prompt, "Triage #9 ");
        assert_eq!(accepted.missing, vec!["body.nope"]);
        assert_eq!(accepted.run.status, "scheduled");
        assert_eq!(accepted.run.trigger_type, AutomationTrigger::Webhook);
        // The stored template is untouched.
        assert_eq!(
            db.get_automation(a.id).unwrap().prompt,
            "Triage {{body.issue}} {{body.nope}}"
        );
    }

    #[test]
    fn wrong_token_and_unknown_id_look_the_same() {
        let db = crate::database::init_test_database();
        let a = webhook_automation(&db, "p");
        let req = request("text/plain", "");
        let id = a.webhook_id.as_deref().unwrap();
        assert_eq!(
            accept_delivery(&db, id, "wrong", &req, at(0)).unwrap_err(),
            DeliveryRejection::NotFound
        );
        assert_eq!(
            accept_delivery(&db, "missing", a.webhook_token.as_deref().unwrap(), &req, at(0))
                .unwrap_err(),
            DeliveryRejection::NotFound
        );
        assert!(db.list_automation_runs(a.id, 10).is_empty());
    }

    #[test]
    fn rotating_the_token_revokes_the_old_url() {
        let db = crate::database::init_test_database();
        let a = webhook_automation(&db, "p");
        let rotated = db.rotate_automation_webhook_token(a.id).unwrap();
        assert_ne!(rotated.webhook_token, a.webhook_token);
        assert_eq!(rotated.webhook_id, a.webhook_id);
        let req = request("text/plain", "");
        let id = a.webhook_id.as_deref().unwrap();
        assert_eq!(
            accept_delivery(&db, id, a.webhook_token.as_deref().unwrap(), &req, at(0))
                .unwrap_err(),
            DeliveryRejection::NotFound
        );
        assert!(accept_delivery(&db, id, rotated.webhook_token.as_deref().unwrap(), &req, at(0))
            .is_ok());
    }

    #[test]
    fn paused_automations_and_oversized_bodies_are_rejected() {
        let db = crate::database::init_test_database();
        let a = webhook_automation(&db, "p");
        let (id, token) = (a.webhook_id.clone().unwrap(), a.webhook_token.clone().unwrap());

        let big = WebhookRequest::new("POST", "/", "", [], vec![b'x'; MAX_WEBHOOK_BODY_BYTES + 1]);
        assert_eq!(
            accept_delivery(&db, &id, &token, &big, at(0)).unwrap_err(),
            DeliveryRejection::PayloadTooLarge
        );

        db.set_automation_enabled(a.id, false).unwrap();
        assert_eq!(
            accept_delivery(&db, &id, &token, &request("text/plain", ""), at(0)).unwrap_err(),
            DeliveryRejection::Disabled
        );
    }

    #[test]
    fn signature_is_enforced_when_configured() {
        let db = crate::database::init_test_database();
        let a = webhook_automation(&db, "Run {{body.x}}");
        let secret = crate::secrets::store(&db, "GitHub", "topsecret").unwrap();
        db.set_automation_webhook_signature(a.id, Some((&github_signature(), &secret.id)))
            .unwrap();
        let (id, token) = (a.webhook_id.clone().unwrap(), a.webhook_token.clone().unwrap());

        let body = br#"{"x":"1"}"#;
        let unsigned = WebhookRequest::new("POST", "/", "", [], body.to_vec());
        assert_eq!(
            accept_delivery(&db, &id, &token, &unsigned, at(0)).unwrap_err(),
            DeliveryRejection::BadSignature
        );

        let good = format!("sha256={}", hmac_hex("topsecret", body));
        let signed = WebhookRequest::new(
            "POST",
            "/",
            "",
            [("X-Hub-Signature-256", good.as_str())],
            body.to_vec(),
        );
        assert_eq!(
            accept_delivery(&db, &id, &token, &signed, at(1)).unwrap().automation.prompt,
            "Run 1"
        );
    }

    #[test]
    fn the_url_token_never_reaches_the_prompt() {
        let db = crate::database::init_test_database();
        let a = webhook_automation(&db, "{{request}}");
        let (id, token) = (a.webhook_id.clone().unwrap(), a.webhook_token.clone().unwrap());
        let path = format!("/hooks/automations/{id}/{token}");
        let query = format!("t={token}");
        let req = WebhookRequest::new("POST", &path, &query, [], b"hi".to_vec());

        let prompt = accept_delivery(&db, &id, &token, &req, at(0)).unwrap().automation.prompt;
        assert!(!prompt.contains(&token), "{prompt}");
        assert!(prompt.starts_with(&format!("POST /hooks/automations/{id}/[redacted]?")));
    }

    #[test]
    fn a_signing_secret_with_whitespace_still_verifies() {
        let db = crate::database::init_test_database();
        let a = webhook_automation(&db, "go");
        let secret = crate::secrets::store(&db, "Spaced", " signing-secret ").unwrap();
        db.set_automation_webhook_signature(a.id, Some((&github_signature(), &secret.id)))
            .unwrap();
        let body = b"{}";
        let header = format!("sha256={}", hmac_hex(" signing-secret ", body));
        let req = WebhookRequest::new(
            "POST",
            "/",
            "",
            [("x-hub-signature-256", header.as_str())],
            body.to_vec(),
        );
        let (id, token) = (a.webhook_id.clone().unwrap(), a.webhook_token.clone().unwrap());
        assert!(accept_delivery(&db, &id, &token, &req, at(0)).is_ok());
    }

    #[test]
    fn a_secret_in_use_cannot_be_deleted() {
        let db = crate::database::init_test_database();
        let a = webhook_automation(&db, "p");
        let secret = crate::secrets::store(&db, "GitHub", "s").unwrap();
        db.set_automation_webhook_signature(a.id, Some((&github_signature(), &secret.id)))
            .unwrap();
        assert!(db.delete_secret(&secret.id).is_err());

        db.set_automation_webhook_signature(a.id, None).unwrap();
        db.delete_secret(&secret.id).unwrap();
    }

    #[test]
    fn a_signature_needs_an_existing_secret() {
        let db = crate::database::init_test_database();
        let a = webhook_automation(&db, "p");
        assert!(db
            .set_automation_webhook_signature(a.id, Some((&github_signature(), "secret_gone")))
            .is_err());
    }

    #[test]
    fn a_second_delivery_while_running_is_recorded_busy() {
        let db = crate::database::init_test_database();
        let a = webhook_automation(&db, "p");
        let (id, token) = (a.webhook_id.clone().unwrap(), a.webhook_token.clone().unwrap());
        let req = request("text/plain", "");

        accept_delivery(&db, &id, &token, &req, at(0)).unwrap();
        assert_eq!(
            accept_delivery(&db, &id, &token, &req, at(1)).unwrap_err(),
            DeliveryRejection::Busy
        );
        let runs = db.list_automation_runs(a.id, 10);
        assert_eq!(runs.len(), 2);
        assert!(runs.iter().any(|r| r.status == "skipped_busy"));
    }

    #[test]
    fn empty_rendered_prompt_is_rejected() {
        let db = crate::database::init_test_database();
        let a = webhook_automation(&db, "{{body.missing}}");
        let (id, token) = (a.webhook_id.clone().unwrap(), a.webhook_token.clone().unwrap());
        assert_eq!(
            accept_delivery(&db, &id, &token, &request("application/json", "{}"), at(0))
                .unwrap_err(),
            DeliveryRejection::EmptyPrompt
        );
        assert!(db.list_automation_runs(a.id, 10).is_empty());
    }

    #[test]
    fn scheduled_automations_have_no_webhook() {
        let db = crate::database::init_test_database();
        let a = db
            .insert_automation(&AutomationInput {
                name: "Daily".into(),
                prompt: "p".into(),
                agent: "claude".into(),
                schedule: "DTSTART:20260101T090000Z\nRRULE:FREQ=DAILY".into(),
                timezone: "UTC".into(),
                host_id: None,
                project_path: None,
                project_remote: None,
                retention_limit: 10,
                trigger_type: None,
            })
            .unwrap();
        assert_eq!(a.trigger_type, AutomationTrigger::Schedule);
        assert!(a.webhook_id.is_none() && a.webhook_token.is_none());
        assert!(db.rotate_automation_webhook_token(a.id).is_err());
    }
}
