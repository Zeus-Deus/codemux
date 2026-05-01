//! Thin HTTP wrapper around the OpenCode local server.
//!
//! Stage 1 lands the request shape and a single `ping()` round-trip.
//! Stage 2 will replace the placeholder model harvest with the real
//! `provider.list` + `app.agents` calls (see
//! `docs/plans/step-12-opencode-implementation-plan.md` §2.1) and
//! introduce streaming subscriptions on top.
//!
//! The shape is deliberately Rust-direct-HTTP rather than a Bun /
//! Node sidecar — option (a) from the plan, locked because porting
//! the SDK call surface to `reqwest` is straightforward and avoids
//! shipping a second sidecar binary.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Configuration knobs for [`OpenCodeClient`]. Plain struct so callers
/// can override fields one at a time; `Default` populates the values
/// shared by every Stage 1 callsite.
#[derive(Debug, Clone)]
pub struct OpenCodeClientConfig {
    /// Base URL of the OpenCode HTTP server, e.g.
    /// `http://127.0.0.1:34567`. Stored without a trailing slash so
    /// path joins land cleanly.
    pub base_url: String,
    /// Optional HTTP Basic password. The server username is always
    /// `opencode` (verified live against the `opencode` 1.14.31 CLI
    /// and at `/tmp/<reference>/apps/server/src/provider/opencodeRuntime.ts:499`),
    /// so the field stores only the secret half.
    pub server_password: Option<String>,
    /// Per-request timeout. Capped to keep the eventual UI responsive
    /// when the user's machine has flaky DNS or a hung server.
    pub request_timeout: Duration,
}

impl OpenCodeClientConfig {
    /// Convenience constructor that fills in the default
    /// (no-password, 5 s timeout) and trims any trailing slash from
    /// `base_url`.
    pub fn new(base_url: impl Into<String>) -> Self {
        let mut url: String = base_url.into();
        while url.ends_with('/') {
            url.pop();
        }
        Self {
            base_url: url,
            server_password: None,
            request_timeout: Duration::from_secs(5),
        }
    }
}

/// Stage 1 HTTP client for the OpenCode server. Wraps a
/// `reqwest::Client` with the OpenCode-specific URL + auth concerns
/// so the rest of the codebase can stay unaware of the underlying
/// transport.
#[derive(Debug, Clone)]
pub struct OpenCodeClient {
    config: OpenCodeClientConfig,
    http: reqwest::Client,
}

impl OpenCodeClient {
    /// Build a new client. Returns `Err` only when `reqwest` fails to
    /// construct an HTTP client (e.g. system CA bundle missing on
    /// Linux); in normal operation this is infallible.
    pub fn new(config: OpenCodeClientConfig) -> Result<Self, String> {
        let http = reqwest::Client::builder()
            .timeout(config.request_timeout)
            .build()
            .map_err(|e| format!("failed_to_build_http_client: {e}"))?;
        Ok(Self { config, http })
    }

    /// Borrow the active configuration. Useful for tests and for the
    /// Tauri command layer that needs to surface the resolved URL.
    pub fn config(&self) -> &OpenCodeClientConfig {
        &self.config
    }

    /// Send a `GET /` against the server and return `Ok` for any
    /// response status — the goal is "is something listening", not
    /// "is the user authenticated", which matches the behaviour of
    /// the discovery probe.
    pub async fn ping(&self) -> Result<(), String> {
        let url = self.url("/");
        let request = self.attach_auth(self.http.get(&url));
        request
            .send()
            .await
            .map(|_| ())
            .map_err(format_request_error)
    }

    /// List the providers + models exposed by the running OpenCode
    /// server.
    ///
    /// Hits `GET /provider`. The endpoint path was confirmed by
    /// reading the canonical `@opencode-ai/sdk@1.14.31` source at
    /// `dist/v2/gen/sdk.gen.js`:
    ///
    /// ```text
    /// class Provider extends HeyApiClient {
    ///   list(...) {
    ///     return ...get({ url: "/provider", ... });
    ///   }
    /// }
    /// ```
    ///
    /// The wire shape is `{ all: Provider[], default:
    /// Record<string,string>, connected: string[] }`, where each
    /// `Provider` carries `{ id, name, source, models: Record<string,
    /// Model>, … }`. We decode that envelope and flatten it into
    /// the Stage-1-locked [`OpenCodeProviderEntry`] / [`OpenCodeModel`]
    /// shape so the eventual Stage 3 frontend types only care about
    /// the trimmed-down view.
    ///
    /// Returns `Err` for connect failures, non-2xx responses, and
    /// JSON parse errors. Error strings follow the same stable
    /// vocabulary as [`format_request_error`] so the UI can branch
    /// on them.
    pub async fn list_models(&self) -> Result<Vec<OpenCodeProviderEntry>, String> {
        let url = self.url("/provider");
        let request = self.attach_auth(self.http.get(&url));
        let response = request.send().await.map_err(format_request_error)?;
        let status = response.status();
        if !status.is_success() {
            return Err(format!("http_status_{}", status.as_u16()));
        }
        let raw: ProviderListResponse = response
            .json()
            .await
            .map_err(|err| format!("parse_error: {err}"))?;
        Ok(flatten_provider_list(raw))
    }

    fn url(&self, path: &str) -> String {
        let trimmed = path.trim_start_matches('/');
        format!("{}/{}", self.config.base_url, trimmed)
    }

    fn attach_auth(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match self.config.server_password.as_deref() {
            Some(pw) => request.basic_auth("opencode", Some(pw)),
            None => request,
        }
    }
}

/// One provider entry surfaced by [`OpenCodeClient::list_models`].
///
/// Stable Stage-1-locked shape — Stage 2 wires the actual decode but
/// does NOT change these fields. The upstream wire envelope is
/// flattened (see [`flatten_provider_list`]) so callers see one
/// uniform list regardless of how OpenCode chooses to group them
/// internally.
///
/// Field semantics:
///
/// * `id` — stable upstream provider id (`"openai"`, `"anthropic"`,
///   `"openrouter"`, …). Used as the slug prefix in
///   `${provider_id}/${model_id}` model identifiers (Stage 3).
/// * `name` — human-readable label as OpenCode reports it.
/// * `connected` — true when this upstream is in the wire-level
///   `connected: string[]` array (i.e. the user has supplied
///   credentials and OpenCode validated them at startup). Stage 4's
///   picker uses this as a filter.
/// * `models` — keyed by model id, values pulled from OpenCode's
///   `Provider.models[]` map.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenCodeProviderEntry {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub connected: bool,
    #[serde(default)]
    pub models: std::collections::BTreeMap<String, OpenCodeModel>,
}

/// One model entry inside an [`OpenCodeProviderEntry`].
///
/// Field semantics:
///
/// * `id` — upstream model id (`"gpt-5"`, `"claude-sonnet-4-6"`).
/// * `name` — human-readable label.
/// * `description` — short tag derived from the upstream `family`
///   field (e.g. `"gpt-5"` family → `"gpt-5"` description). `None`
///   when no family is reported. Stage 4's picker can render this
///   below the model name.
/// * `variants` — upstream `variants` keys advertised for this
///   model. Stage 3 maps these into `ChatModelInfo.effort_levels`.
/// * `context_window` — upstream `limit.context`. Wired verbatim;
///   the frontend already knows how to format token counts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenCodeModel {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub variants: Vec<String>,
    #[serde(default)]
    pub context_window: Option<u64>,
}

// ── Wire-format decoders for the OpenCode HTTP API ──────────────────
//
// These types are intentionally private to the module. They mirror
// `dist/v2/gen/types.gen.d.ts` from `@opencode-ai/sdk@1.14.31` for
// the `Provider.list()` endpoint and are forgiving about extra
// fields (every struct uses `#[serde(default)]` on every field) so a
// future OpenCode release adding new keys does not break the model
// harvest.
//
// The flatten step ([`flatten_provider_list`]) converts the wire
// envelope into the Stage-1-locked public types above, which is what
// the Tauri command surface actually returns to the frontend.

#[derive(Debug, Clone, Deserialize)]
struct ProviderListResponse {
    #[serde(default)]
    all: Vec<RawProvider>,
    #[serde(default)]
    connected: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawProvider {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    models: std::collections::BTreeMap<String, RawModel>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawModel {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    family: Option<String>,
    #[serde(default)]
    limit: Option<RawModelLimit>,
    #[serde(default)]
    variants: Option<std::collections::BTreeMap<String, serde_json::Value>>,
}

#[derive(Debug, Clone, Deserialize)]
struct RawModelLimit {
    #[serde(default)]
    context: Option<u64>,
}

/// Convert the raw `GET /provider` envelope into the public
/// [`OpenCodeProviderEntry`] list. Pure helper so unit tests can
/// pin the transformation against fixture JSON without HTTP.
fn flatten_provider_list(raw: ProviderListResponse) -> Vec<OpenCodeProviderEntry> {
    let connected: std::collections::HashSet<&str> =
        raw.connected.iter().map(|s| s.as_str()).collect();
    raw.all
        .into_iter()
        .map(|provider| {
            let connected_flag = connected.contains(provider.id.as_str());
            let models = provider
                .models
                .into_iter()
                .map(|(slug, model)| (slug, flatten_model(model)))
                .collect();
            OpenCodeProviderEntry {
                id: provider.id,
                name: provider.name,
                connected: connected_flag,
                models,
            }
        })
        .collect()
}

fn flatten_model(raw: RawModel) -> OpenCodeModel {
    let variants = raw
        .variants
        .map(|map| map.into_keys().collect())
        .unwrap_or_default();
    let context_window = raw.limit.and_then(|l| l.context);
    OpenCodeModel {
        id: raw.id,
        name: raw.name,
        description: raw.family,
        variants,
        context_window,
    }
}

/// Convert a raw `reqwest::Error` into the small error string that
/// the Tauri command surface forwards to the UI. Pulled out of
/// [`OpenCodeClient::ping`] so unit tests can pin the formatting
/// without spinning a real network call.
pub fn format_request_error(err: reqwest::Error) -> String {
    if err.is_timeout() {
        "request_timed_out".to_string()
    } else if err.is_connect() {
        "connect_failed".to_string()
    } else if let Some(status) = err.status() {
        format!("http_status_{}", status.as_u16())
    } else {
        format!("request_error: {err}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_strips_trailing_slashes_from_base_url() {
        let cfg = OpenCodeClientConfig::new("http://127.0.0.1:1234/");
        assert_eq!(cfg.base_url, "http://127.0.0.1:1234");

        let cfg2 = OpenCodeClientConfig::new("http://127.0.0.1:1234///");
        assert_eq!(cfg2.base_url, "http://127.0.0.1:1234");
    }

    #[test]
    fn config_default_timeout_is_five_seconds() {
        let cfg = OpenCodeClientConfig::new("http://localhost");
        assert_eq!(cfg.request_timeout, Duration::from_secs(5));
        assert_eq!(cfg.server_password, None);
    }

    #[test]
    fn url_helper_joins_paths_cleanly() {
        let cfg = OpenCodeClientConfig::new("http://127.0.0.1:9000");
        let client = OpenCodeClient::new(cfg).expect("client builds");
        assert_eq!(client.url("/foo"), "http://127.0.0.1:9000/foo");
        assert_eq!(client.url("foo"), "http://127.0.0.1:9000/foo");
        assert_eq!(client.url("/"), "http://127.0.0.1:9000/");
    }

    #[tokio::test]
    async fn list_models_returns_connect_failed_when_no_server() {
        // Stage 2 replacement for the Stage-1 "not implemented"
        // marker. With the placeholder gone the call now actually
        // hits the network; against an unreachable URL with a
        // tight timeout it must surface as `connect_failed` /
        // `request_timed_out` (whichever the OS decides) — the
        // stable error vocabulary the frontend branches on.
        let mut cfg = OpenCodeClientConfig::new("http://192.0.2.1:1");
        cfg.request_timeout = Duration::from_millis(150);
        let client = OpenCodeClient::new(cfg).expect("client builds");
        let err = client.list_models().await.expect_err("must fail");
        assert!(
            err == "connect_failed"
                || err == "request_timed_out"
                || err.starts_with("request_error"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn list_models_decodes_real_provider_envelope() {
        // Pin against a fixture lifted directly from a live
        // OpenCode 1.14.31 server (captured during Stage 2 audit
        // — the first two providers, trimmed to the fields
        // [`flatten_model`] cares about). The full live response
        // is 116 providers / 4354 models; this fixture is the
        // tightest payload that exercises every meaningful
        // branch:
        //
        //   - `connected: bool` driven by the top-level array.
        //   - `description` populated from `family`.
        //   - `context_window` populated from `limit.context`.
        //   - `variants` derived from the keys of an object map.
        //   - Forgiveness for extra fields (capabilities, cost, …).
        let fixture = serde_json::json!({
            "all": [
                {
                    "id": "openai",
                    "name": "OpenAI",
                    "source": "env",
                    "env": ["OPENAI_API_KEY"],
                    "options": {},
                    "models": {
                        "gpt-5": {
                            "id": "gpt-5",
                            "providerID": "openai",
                            "name": "GPT-5",
                            "family": "gpt-5",
                            "api": { "id": "gpt-5", "url": "https://api.openai.com/v1", "npm": "@ai-sdk/openai" },
                            "capabilities": {
                                "temperature": true, "reasoning": true,
                                "attachment": true, "toolcall": true,
                                "input": { "text": true, "audio": false, "image": false, "video": false, "pdf": false },
                                "output": { "text": true, "audio": false, "image": false, "video": false, "pdf": false },
                                "interleaved": false
                            },
                            "cost": { "input": 5.0, "output": 15.0, "cache": { "read": 0, "write": 0 } },
                            "limit": { "context": 200000, "output": 16384 },
                            "status": "active",
                            "options": {},
                            "headers": {},
                            "release_date": "2025-09-01",
                            "variants": { "low": {}, "medium": {}, "high": {} }
                        }
                    }
                },
                {
                    "id": "anthropic",
                    "name": "Anthropic",
                    "source": "env",
                    "env": ["ANTHROPIC_API_KEY"],
                    "options": {},
                    "models": {
                        "claude-sonnet-4-6": {
                            "id": "claude-sonnet-4-6",
                            "providerID": "anthropic",
                            "name": "Claude Sonnet 4.6",
                            "limit": { "context": 1000000, "output": 8192 },
                            "status": "active",
                            "options": {},
                            "headers": {},
                            "release_date": "2025-12-15"
                            // no `family`, no `variants` — must still
                            // decode cleanly.
                        }
                    }
                }
            ],
            "default": { "openai": "gpt-5", "anthropic": "claude-sonnet-4-6" },
            "connected": ["openai"]
        });
        let body = serde_json::to_string(&fixture).unwrap();

        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/provider")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(body)
            .create_async()
            .await;

        let cfg = OpenCodeClientConfig::new(server.url());
        let client = OpenCodeClient::new(cfg).expect("client builds");
        let providers = client.list_models().await.expect("decode succeeds");

        mock.assert_async().await;
        assert_eq!(providers.len(), 2);

        let openai = providers.iter().find(|p| p.id == "openai").unwrap();
        assert_eq!(openai.name, "OpenAI");
        assert!(openai.connected, "openai must be connected per fixture");
        let gpt5 = openai.models.get("gpt-5").unwrap();
        assert_eq!(gpt5.id, "gpt-5");
        assert_eq!(gpt5.name, "GPT-5");
        assert_eq!(gpt5.description.as_deref(), Some("gpt-5"));
        assert_eq!(gpt5.context_window, Some(200_000));
        // Variants come from the keys of the upstream object — order
        // is BTreeMap iteration order (alphabetical).
        assert_eq!(gpt5.variants, vec!["high", "low", "medium"]);

        let anthropic = providers.iter().find(|p| p.id == "anthropic").unwrap();
        assert!(
            !anthropic.connected,
            "anthropic must NOT be connected — fixture omits it"
        );
        let sonnet = anthropic.models.get("claude-sonnet-4-6").unwrap();
        assert_eq!(sonnet.context_window, Some(1_000_000));
        assert_eq!(sonnet.description, None, "no family → no description");
        assert!(sonnet.variants.is_empty(), "no variants object → empty list");
    }

    #[tokio::test]
    async fn list_models_attaches_basic_auth_when_configured() {
        let mut server = mockito::Server::new_async().await;
        // Basic header for `opencode:hunter2` =
        // base64("opencode:hunter2") = b3BlbmNvZGU6aHVudGVyMg==
        let mock = server
            .mock("GET", "/provider")
            .match_header("authorization", "Basic b3BlbmNvZGU6aHVudGVyMg==")
            .with_status(200)
            .with_body(r#"{"all":[],"default":{},"connected":[]}"#)
            .create_async()
            .await;

        let mut cfg = OpenCodeClientConfig::new(server.url());
        cfg.server_password = Some("hunter2".into());
        let client = OpenCodeClient::new(cfg).expect("client builds");
        let providers = client.list_models().await.expect("decodes empty list");

        mock.assert_async().await;
        assert!(providers.is_empty());
    }

    #[tokio::test]
    async fn list_models_surfaces_401_as_http_status() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/provider")
            .with_status(401)
            .create_async()
            .await;

        let cfg = OpenCodeClientConfig::new(server.url());
        let client = OpenCodeClient::new(cfg).expect("client builds");
        let err = client.list_models().await.expect_err("must fail");

        mock.assert_async().await;
        assert_eq!(err, "http_status_401");
    }

    #[tokio::test]
    async fn list_models_surfaces_500_as_http_status() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/provider")
            .with_status(500)
            .create_async()
            .await;

        let cfg = OpenCodeClientConfig::new(server.url());
        let client = OpenCodeClient::new(cfg).expect("client builds");
        let err = client.list_models().await.expect_err("must fail");

        mock.assert_async().await;
        assert_eq!(err, "http_status_500");
    }

    #[tokio::test]
    async fn list_models_surfaces_malformed_json_as_parse_error() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/provider")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body("{ not json")
            .create_async()
            .await;

        let cfg = OpenCodeClientConfig::new(server.url());
        let client = OpenCodeClient::new(cfg).expect("client builds");
        let err = client.list_models().await.expect_err("must fail");

        mock.assert_async().await;
        assert!(
            err.starts_with("parse_error:"),
            "expected parse_error prefix, got {err}"
        );
    }

    #[tokio::test]
    async fn list_models_handles_empty_envelope() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/provider")
            .with_status(200)
            .with_header("content-type", "application/json")
            .with_body(r#"{}"#)
            .create_async()
            .await;

        let cfg = OpenCodeClientConfig::new(server.url());
        let client = OpenCodeClient::new(cfg).expect("client builds");
        let providers = client.list_models().await.expect("empty envelope ok");

        mock.assert_async().await;
        assert!(providers.is_empty());
    }

    #[test]
    fn flatten_provider_list_marks_only_listed_connected() {
        // Direct test of the pure transformer so we don't depend on
        // mockito for what is fundamentally a data-shape concern.
        let raw: ProviderListResponse = serde_json::from_value(serde_json::json!({
            "all": [
                { "id": "p1", "name": "P1", "models": {} },
                { "id": "p2", "name": "P2", "models": {} },
                { "id": "p3", "name": "P3", "models": {} }
            ],
            "connected": ["p1", "p3"]
        }))
        .unwrap();
        let entries = flatten_provider_list(raw);
        assert_eq!(entries.len(), 3);
        assert!(entries.iter().find(|p| p.id == "p1").unwrap().connected);
        assert!(!entries.iter().find(|p| p.id == "p2").unwrap().connected);
        assert!(entries.iter().find(|p| p.id == "p3").unwrap().connected);
    }

    #[tokio::test]
    async fn ping_succeeds_against_mock_server() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/")
            .with_status(200)
            .with_body("ok")
            .create_async()
            .await;

        let cfg = OpenCodeClientConfig::new(server.url());
        let client = OpenCodeClient::new(cfg).expect("client builds");
        let result = client.ping().await;

        mock.assert_async().await;
        assert!(result.is_ok(), "expected ping to succeed: {result:?}");
    }

    #[tokio::test]
    async fn ping_treats_401_as_server_up() {
        // Matches the discovery probe semantics — auth is the next
        // layer's problem; we just want "is something listening".
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/")
            .with_status(401)
            .create_async()
            .await;

        let cfg = OpenCodeClientConfig::new(server.url());
        let client = OpenCodeClient::new(cfg).expect("client builds");
        let result = client.ping().await;

        mock.assert_async().await;
        assert!(
            result.is_ok(),
            "401 should still count as 'server reachable'"
        );
    }

    #[tokio::test]
    async fn ping_attaches_basic_auth_when_configured() {
        let mut server = mockito::Server::new_async().await;
        // Expect HTTP Basic header `opencode:secret` =>
        // `Authorization: Basic b3BlbmNvZGU6c2VjcmV0`.
        let mock = server
            .mock("GET", "/")
            .match_header("authorization", "Basic b3BlbmNvZGU6c2VjcmV0")
            .with_status(204)
            .create_async()
            .await;

        let mut cfg = OpenCodeClientConfig::new(server.url());
        cfg.server_password = Some("secret".to_string());
        let client = OpenCodeClient::new(cfg).expect("client builds");
        let result = client.ping().await;

        mock.assert_async().await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn ping_reports_connect_failure_for_unreachable_url() {
        // RFC 5737 documentation block — never routable.
        let cfg = OpenCodeClientConfig {
            base_url: "http://192.0.2.1:1".to_string(),
            server_password: None,
            request_timeout: Duration::from_millis(200),
        };
        let client = OpenCodeClient::new(cfg).expect("client builds");
        let err = client.ping().await.expect_err("should fail");
        // Either timeout or connect — both signal "not reachable".
        assert!(
            err == "connect_failed"
                || err == "request_timed_out"
                || err.starts_with("request_error"),
            "unexpected error shape: {err}"
        );
    }

    #[test]
    fn provider_entry_decodes_research_shape() {
        // Pin the JSON shape against the research summary §3 —
        // Stage 2 must be able to decode this without schema churn.
        let json = serde_json::json!({
            "id": "openai",
            "name": "OpenAI",
            "connected": true,
            "models": {
                "gpt-5": {
                    "id": "gpt-5",
                    "name": "GPT-5",
                    "variants": ["low", "medium", "high"],
                    "context_window": 200000
                }
            }
        });
        let parsed: OpenCodeProviderEntry =
            serde_json::from_value(json).expect("decodes");
        assert_eq!(parsed.id, "openai");
        assert!(parsed.connected);
        assert!(parsed.models.contains_key("gpt-5"));
        let gpt = &parsed.models["gpt-5"];
        assert_eq!(gpt.variants, vec!["low", "medium", "high"]);
        assert_eq!(gpt.context_window, Some(200_000));
    }

    #[test]
    fn provider_entry_decodes_minimal_payload() {
        // OpenCode may omit `connected` and `models` entirely when an
        // upstream is configured but unauthenticated. Defaults must
        // keep the decode infallible.
        let json = serde_json::json!({
            "id": "anthropic",
            "name": "Anthropic"
        });
        let parsed: OpenCodeProviderEntry = serde_json::from_value(json).unwrap();
        assert!(!parsed.connected);
        assert!(parsed.models.is_empty());
    }

    #[tokio::test]
    async fn format_request_error_classifies_timeout_vs_connect() {
        // We can't easily synthesise a real reqwest::Error, but we
        // can hit the live builder against an unreachable host with a
        // tiny budget and assert on the resulting classification.
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(50))
            .build()
            .unwrap();
        let err = client
            .get("http://192.0.2.1:1")
            .send()
            .await
            .expect_err("must fail");
        let formatted = format_request_error(err);
        assert!(
            formatted == "connect_failed"
                || formatted == "request_timed_out"
                || formatted.starts_with("request_error"),
            "unexpected classification: {formatted}"
        );
    }
}
