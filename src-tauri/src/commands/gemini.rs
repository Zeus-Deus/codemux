//! Launch-time model list for the Gemini CLI.
//!
//! Gemini isn't a chat provider in Codemux (no SDK adapter, no entry in
//! [`ProviderKind`](crate::agent_provider::ProviderKind)), so the launch
//! picker has its own thin path: a hand-maintained fallback, plus a
//! live harvest against Google's `generativelanguage` API when
//! `GEMINI_API_KEY` is in the environment. New Gemini text-generation
//! models surface in the picker automatically for users with an API
//! key; everyone else sees the maintained list.

use std::time::Duration;

use serde::{Deserialize, Serialize};

const GEMINI_API_BASE: &str = "https://generativelanguage.googleapis.com/v1beta";
const HARVEST_TIMEOUT: Duration = Duration::from_secs(10);

/// Shape returned to the frontend launch picker. Matches the shape of
/// `LaunchModel` on the TypeScript side (`src/lib/launch-models.ts`).
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct LaunchGeminiModel {
    pub id: String,
    pub label: String,
}

#[derive(Debug, Deserialize)]
struct ApiModel {
    name: String,
    #[serde(default, rename = "displayName")]
    display_name: String,
    #[serde(default, rename = "supportedGenerationMethods")]
    supported_methods: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct ApiModelsResponse {
    #[serde(default)]
    models: Vec<ApiModel>,
}

/// Maintained fallback served when no API key is set (or the harvest
/// fails). Replaces the previous frontend `GEMINI_MODELS` constant —
/// keeping the canonical list on the backend so the frontend never has
/// to be touched when Google ships a new headline model.
pub fn maintained_gemini_models() -> Vec<LaunchGeminiModel> {
    vec![
        LaunchGeminiModel {
            id: "gemini-2.5-pro".into(),
            label: "Gemini 2.5 Pro".into(),
        },
        LaunchGeminiModel {
            id: "gemini-2.5-flash".into(),
            label: "Gemini 2.5 Flash".into(),
        },
    ]
}

/// Live-harvest the Gemini model list from Google. Returns the
/// filtered text-generation models on success. The caller is expected
/// to fall back to [`maintained_gemini_models`] on error.
async fn harvest_gemini_models() -> Result<Vec<LaunchGeminiModel>, String> {
    let api_key = std::env::var("GEMINI_API_KEY")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| "gemini_no_api_key".to_string())?;
    harvest_gemini_models_from(GEMINI_API_BASE, &api_key).await
}

/// The key rides in the `x-goog-api-key` header, never the URL: reqwest
/// errors print their URL, and the caller logs those errors to stderr.
/// Redirects are off, since reqwest would carry that header to whatever
/// origin a redirect names, and an error reports only the HTTP status and
/// Google's error code, never the response text, which could echo the key.
async fn harvest_gemini_models_from(
    api_base: &str,
    api_key: &str,
) -> Result<Vec<LaunchGeminiModel>, String> {
    let mut key = reqwest::header::HeaderValue::from_str(api_key.trim())
        .map_err(|_| "GEMINI_API_KEY is not a valid header value".to_string())?;
    key.set_sensitive(true);
    let client = reqwest::Client::builder()
        .timeout(HARVEST_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| format!("client build: {}", e.without_url()))?;
    let resp = client
        .get(format!("{api_base}/models"))
        .query(&[("pageSize", "200")])
        .header("x-goog-api-key", key)
        .send()
        .await
        .map_err(|e| format!("request: {}", e.without_url()))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(match google_error_code(&body) {
            Some(code) => format!("HTTP {status} ({code})"),
            None => format!("HTTP {status}"),
        });
    }
    let parsed: ApiModelsResponse = resp
        .json()
        .await
        .map_err(|e| format!("decode: {}", e.without_url()))?;
    Ok(filter_and_map(parsed.models))
}

/// The `error.status` code from a Google API error body (for example
/// `PERMISSION_DENIED`), kept only when it is a plain upper-case code so no
/// server-chosen text reaches the logs.
fn google_error_code(body: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    let code = value.get("error")?.get("status")?.as_str()?;
    (!code.is_empty() && code.len() <= 64 && code.bytes().all(|b| b.is_ascii_uppercase() || b == b'_'))
        .then(|| code.to_string())
}

/// Pure transformer — keep only Gemini text-generation models, strip
/// the `models/` prefix the API tags onto every name, and fall back to
/// the bare id as the label when `displayName` is missing.
fn filter_and_map(raw: Vec<ApiModel>) -> Vec<LaunchGeminiModel> {
    raw.into_iter()
        .filter(|m| {
            m.supported_methods
                .iter()
                .any(|method| method == "generateContent")
        })
        .filter_map(|m| {
            let id = m.name.strip_prefix("models/")?.to_string();
            if !id.starts_with("gemini-") {
                return None;
            }
            let label = if m.display_name.trim().is_empty() {
                id.clone()
            } else {
                m.display_name
            };
            Some(LaunchGeminiModel { id, label })
        })
        .collect()
}

/// Tauri command — returns the live model list when `GEMINI_API_KEY`
/// is in the environment, otherwise the maintained fallback. Any
/// failure inside the live path logs and falls back so the picker is
/// never blank.
#[tauri::command]
pub async fn list_launch_gemini_models() -> Result<Vec<LaunchGeminiModel>, String> {
    if std::env::var("GEMINI_API_KEY")
        .ok()
        .is_some_and(|v| !v.trim().is_empty())
    {
        match harvest_gemini_models().await {
            Ok(live) if !live.is_empty() => Ok(live),
            // Empty live response (filtered to zero models for some
            // reason) — surface the maintained list rather than an
            // empty popover.
            Ok(_) => Ok(maintained_gemini_models()),
            Err(err) => {
                eprintln!(
                    "[gemini] live harvest failed, falling back to maintained: {err}"
                );
                Ok(maintained_gemini_models())
            }
        }
    } else {
        Ok(maintained_gemini_models())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn api_model(name: &str, label: &str, methods: &[&str]) -> ApiModel {
        ApiModel {
            name: name.into(),
            display_name: label.into(),
            supported_methods: methods.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn maintained_list_includes_25_pro_and_flash() {
        let m = maintained_gemini_models();
        let ids: Vec<&str> = m.iter().map(|x| x.id.as_str()).collect();
        assert!(ids.contains(&"gemini-2.5-pro"));
        assert!(ids.contains(&"gemini-2.5-flash"));
    }

    #[test]
    fn filter_keeps_gemini_text_models_and_strips_models_prefix() {
        let raw = vec![
            api_model("models/gemini-2.5-pro", "Gemini 2.5 Pro", &["generateContent"]),
            // Not a gemini- prefix — drop.
            api_model("models/embedding-001", "Embedding", &["generateContent"]),
            // Gemma family — drop.
            api_model("models/gemma-1", "Gemma 1", &["generateContent"]),
            // No `models/` prefix — drop.
            api_model("naked-gemini-3", "Naked", &["generateContent"]),
            // Empty displayName — falls back to id.
            api_model("models/gemini-3.0-flash", "", &["generateContent"]),
            // No generateContent support — drop.
            api_model("models/gemini-2.0-flash", "Old", &["countTokens"]),
        ];
        let result = filter_and_map(raw);
        let ids: Vec<&str> = result.iter().map(|x| x.id.as_str()).collect();
        assert_eq!(ids, vec!["gemini-2.5-pro", "gemini-3.0-flash"]);
        // Empty display falls back to id.
        let flash = result
            .iter()
            .find(|x| x.id == "gemini-3.0-flash")
            .unwrap();
        assert_eq!(flash.label, "gemini-3.0-flash");
    }

    #[test]
    fn filter_returns_empty_on_empty_input() {
        assert!(filter_and_map(vec![]).is_empty());
    }

    const TEST_KEY: &str = "AIzaTEST-not-a-real-key";

    #[tokio::test]
    async fn api_key_is_sent_as_a_header_not_in_the_url() {
        let mut server = mockito::Server::new_async().await;
        let mock = server
            .mock("GET", "/models")
            .match_query(mockito::Matcher::UrlEncoded("pageSize".into(), "200".into()))
            .match_header("x-goog-api-key", TEST_KEY)
            .with_status(200)
            .with_body(
                r#"{"models":[{"name":"models/gemini-9-pro","displayName":"Gemini 9 Pro","supportedGenerationMethods":["generateContent"]}]}"#,
            )
            .create_async()
            .await;
        let models = harvest_gemini_models_from(&server.url(), TEST_KEY)
            .await
            .expect("harvest succeeds");
        mock.assert_async().await;
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "gemini-9-pro");
    }

    #[tokio::test]
    async fn transport_errors_never_contain_the_api_key() {
        // Bind then drop a listener so the port is closed: the request fails
        // fast with a connection error, which reqwest would print with its URL.
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let err = harvest_gemini_models_from(&format!("http://127.0.0.1:{port}"), TEST_KEY)
            .await
            .expect_err("closed port fails");
        assert!(err.starts_with("request:"), "{err}");
        assert!(!err.contains(TEST_KEY), "{err}");
    }

    #[tokio::test]
    async fn redirects_are_not_followed_with_the_key() {
        let mut other = mockito::Server::new_async().await;
        let elsewhere = other
            .mock("GET", mockito::Matcher::Any)
            .expect(0)
            .create_async()
            .await;
        let mut server = mockito::Server::new_async().await;
        let _redirect = server
            .mock("GET", "/models")
            .match_query(mockito::Matcher::Any)
            .with_status(302)
            .with_header("location", &format!("{}/steal", other.url()))
            .create_async()
            .await;
        let err = harvest_gemini_models_from(&server.url(), TEST_KEY)
            .await
            .expect_err("a redirect is not followed");
        assert!(err.starts_with("HTTP 302"), "{err}");
        elsewhere.assert_async().await;
    }

    #[tokio::test]
    async fn http_errors_report_the_code_but_never_the_body() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("GET", "/models")
            .match_query(mockito::Matcher::Any)
            .with_status(403)
            .with_body(format!(
                r#"{{"error":{{"code":403,"message":"API key {TEST_KEY} is not valid","status":"PERMISSION_DENIED"}}}}"#
            ))
            .create_async()
            .await;
        let err = harvest_gemini_models_from(&server.url(), TEST_KEY)
            .await
            .expect_err("403 fails");
        assert_eq!(err, "HTTP 403 Forbidden (PERMISSION_DENIED)");
        assert!(!err.contains(TEST_KEY));
        assert_eq!(google_error_code(r#"{"error":{"status":"bad text"}}"#), None);
        assert_eq!(google_error_code("not json"), None);
    }

    #[tokio::test]
    async fn decode_errors_never_contain_the_api_key() {
        let mut server = mockito::Server::new_async().await;
        let _mock = server
            .mock("GET", "/models")
            .match_query(mockito::Matcher::Any)
            .with_status(200)
            .with_body("not json")
            .create_async()
            .await;
        let err = harvest_gemini_models_from(&server.url(), TEST_KEY)
            .await
            .expect_err("bad body fails");
        assert!(err.starts_with("decode:"), "{err}");
        assert!(!err.contains(TEST_KEY), "{err}");
    }
}
