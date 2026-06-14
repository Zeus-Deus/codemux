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
    let client = reqwest::Client::builder()
        .timeout(HARVEST_TIMEOUT)
        .build()
        .map_err(|e| format!("client build: {e}"))?;
    let url = format!("{GEMINI_API_BASE}/models?key={api_key}&pageSize=200");
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("request: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!(
            "HTTP {}: {}",
            resp.status(),
            resp.text().await.unwrap_or_default()
        ));
    }
    let parsed: ApiModelsResponse =
        resp.json().await.map_err(|e| format!("decode: {e}"))?;
    Ok(filter_and_map(parsed.models))
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
}
