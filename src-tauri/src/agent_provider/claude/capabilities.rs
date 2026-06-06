//! Hand-maintained chat-side capability data for Claude models.
//!
//! The Claude Agent SDK exposes a live `supportedModels()` API, but it
//! doesn't report `promptInjectedEffortLevels` or `contextWindowOptions`
//! — those are product decisions rather than SDK metadata. We therefore
//! keep a hand-maintained per-model map and, when live data arrives,
//! merge the SDK's model list with our hand-maintained extras by id.
//!
//! Mirrors a reference multi-provider client's `BUILT_IN_MODELS`
//! table (`apps/server/src/provider/Layers/ClaudeProvider.ts:48-141`).

use std::collections::HashMap;
use std::time::Duration;

use serde::Deserialize;
use serde_json::json;
use tokio::sync::Mutex;

use crate::agent_provider::{
    ChatModelInfo, ContextWindowOption, EffortGranularity, PermissionModeOption,
    ProviderChatCapabilities,
};
use crate::json_rpc_child::{JsonRpcChild, SpawnConfig};

// Highest-is-default rule: when a model exposes multiple context
// windows, the largest is flagged as the default. Users who want the
// smaller window still get a visible picker pill they can click to
// switch. This flipped from "200k is default" in the Stage C
// Context-Window-visibility pass — previously the picker hid itself
// on the default, so the 1M option was effectively undiscoverable.
fn ctx_200k() -> ContextWindowOption {
    ContextWindowOption {
        value: "200k".into(),
        label: "200k".into(),
        is_default: false,
    }
}

fn ctx_1m_default() -> ContextWindowOption {
    ContextWindowOption {
        value: "1m".into(),
        label: "1M".into(),
        is_default: true,
    }
}

fn claude_effort_label_map() -> HashMap<String, String> {
    let pairs = [
        ("low", "Low"),
        ("medium", "Medium"),
        ("high", "High"),
        ("xhigh", "Extra High"),
        ("max", "Max"),
        // `ultracode` is xhigh + workflows in Claude Code's TUI slider
        // (see `/effort` in claude >= 2.1.x). The deployed CLI accepts
        // it as an `--effort` value even though the bundled SDK's
        // `EffortLevel` type union doesn't list it yet.
        ("ultracode", "Ultracode"),
        ("ultrathink", "Ultrathink"),
    ];
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn models() -> Vec<ChatModelInfo> {
    vec![
        // Opus 4.8 — flagship, effort defaults to xhigh, supports ultrathink + 1M.
        ChatModelInfo {
            id: "claude-opus-4-8".into(),
            label: "Claude Opus 4.8".into(),
            description: Some("Strongest Claude model".into()),
            effort_levels: vec![
                "low".into(),
                "medium".into(),
                "high".into(),
                "xhigh".into(),
                "max".into(),
                "ultracode".into(),
            ],
            default_effort: Some("xhigh".into()),
            prompt_injected_effort_levels: vec!["ultrathink".into()],
            context_window_options: vec![ctx_200k(), ctx_1m_default()],
            supports_adaptive_thinking: true,
            supports_thinking_toggle: false,
            supports_fast_mode: false,
            supports_images: true,
            sub_provider: None,
            is_free: false,
        },
        // Opus 4.7 — previous flagship.
        ChatModelInfo {
            id: "claude-opus-4-7".into(),
            label: "Claude Opus 4.7".into(),
            description: None,
            effort_levels: vec![
                "low".into(),
                "medium".into(),
                "high".into(),
                "xhigh".into(),
                "max".into(),
                "ultracode".into(),
            ],
            default_effort: Some("xhigh".into()),
            prompt_injected_effort_levels: vec!["ultrathink".into()],
            context_window_options: vec![ctx_200k(), ctx_1m_default()],
            supports_adaptive_thinking: true,
            supports_thinking_toggle: false,
            supports_fast_mode: false,
            supports_images: true,
            sub_provider: None,
            is_free: false,
        },
        // Opus 4.6 — default effort is high, supports fast mode + ultrathink + 1M.
        ChatModelInfo {
            id: "claude-opus-4-6".into(),
            label: "Claude Opus 4.6".into(),
            description: None,
            effort_levels: vec![
                "low".into(),
                "medium".into(),
                "high".into(),
                "max".into(),
            ],
            default_effort: Some("high".into()),
            prompt_injected_effort_levels: vec!["ultrathink".into()],
            context_window_options: vec![ctx_200k(), ctx_1m_default()],
            supports_adaptive_thinking: true,
            supports_thinking_toggle: false,
            supports_fast_mode: true,
            supports_images: true,
            sub_provider: None,
            is_free: false,
        },
        // Opus 4.5 — no ultrathink, no 1M context.
        ChatModelInfo {
            id: "claude-opus-4-5".into(),
            label: "Claude Opus 4.5".into(),
            description: None,
            effort_levels: vec![
                "low".into(),
                "medium".into(),
                "high".into(),
                "max".into(),
            ],
            default_effort: Some("high".into()),
            prompt_injected_effort_levels: vec![],
            context_window_options: vec![],
            supports_adaptive_thinking: false,
            supports_thinking_toggle: false,
            supports_fast_mode: true,
            supports_images: true,
            sub_provider: None,
            is_free: false,
        },
        // Sonnet 4.6 — narrower effort range, supports ultrathink + 1M.
        ChatModelInfo {
            id: "claude-sonnet-4-6".into(),
            label: "Claude Sonnet 4.6".into(),
            description: Some("Fast and capable".into()),
            effort_levels: vec!["low".into(), "medium".into(), "high".into()],
            default_effort: Some("high".into()),
            prompt_injected_effort_levels: vec!["ultrathink".into()],
            context_window_options: vec![ctx_200k(), ctx_1m_default()],
            supports_adaptive_thinking: false,
            supports_thinking_toggle: false,
            supports_fast_mode: false,
            supports_images: true,
            sub_provider: None,
            is_free: false,
        },
        // Haiku 4.5 — no effort, no context-window picker. Has a thinking
        // toggle we don't render in MVP.
        ChatModelInfo {
            id: "claude-haiku-4-5".into(),
            label: "Claude Haiku 4.5".into(),
            description: Some("Fastest and cheapest".into()),
            effort_levels: vec![],
            default_effort: None,
            prompt_injected_effort_levels: vec![],
            context_window_options: vec![],
            supports_adaptive_thinking: false,
            supports_thinking_toggle: true,
            supports_fast_mode: false,
            supports_images: true,
            sub_provider: None,
            is_free: false,
        },
    ]
}

fn claude_permission_modes() -> Vec<PermissionModeOption> {
    vec![
        PermissionModeOption {
            value: "default".into(),
            label: "Supervised".into(),
            description: "Ask before commands and file changes.".into(),
            is_default: false,
        },
        PermissionModeOption {
            value: "acceptEdits".into(),
            label: "Auto-accept edits".into(),
            description: "Auto-approve edits, ask before other actions.".into(),
            is_default: false,
        },
        PermissionModeOption {
            value: "bypassPermissions".into(),
            label: "Full access".into(),
            description: "Allow commands and edits without prompts.".into(),
            is_default: true,
        },
    ]
}

/// Fallback capabilities snapshot used until — if ever — a live session
/// reports different model metadata. The `prompt_injected_effort_levels`
/// and `context_window_options` fields are NOT reported by the SDK and
/// must be preserved verbatim during any future live-merge pass.
pub fn claude_fallback_capabilities() -> ProviderChatCapabilities {
    ProviderChatCapabilities {
        models: models(),
        effort_granularity: EffortGranularity::PerSession,
        effort_label_map: claude_effort_label_map(),
        permission_modes: claude_permission_modes(),
        default_permission_mode: Some("bypassPermissions".into()),
        permission_granularity: EffortGranularity::PerSession,
    }
}

/// Apply the reference impl's `resolveClaudeApiModelId` trick: when
/// the context window is `"1m"`, the Anthropic API expects the model
/// id to carry a `[1m]` bracket suffix. Any other value (or `None`)
/// returns the id unchanged.
pub fn resolve_claude_api_model_id(
    model_id: &str,
    context_window: Option<&str>,
) -> String {
    match context_window {
        Some("1m") => format!("{model_id}[1m]"),
        _ => model_id.to_string(),
    }
}

// ── Live harvest from the Anthropic /v1/models API ───────────────────
//
// Anthropic's SDK / OAuth path exposes no model-list endpoint, so for
// subscription users we still serve the hand-maintained `models()` list
// above. Users with `ANTHROPIC_API_KEY` set get a live harvest:
// `GET /v1/models` returns the current model id list, which we merge
// against the maintained per-id metadata. Ids we recognise keep their
// precise metadata; ids we don't (e.g. a freshly-released model) get
// family-pattern-inferred defaults so they appear in the picker without
// a code change.

const ANTHROPIC_API_BASE: &str = "https://api.anthropic.com";
const ANTHROPIC_API_VERSION: &str = "2023-06-01";
const HARVEST_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Deserialize)]
struct ApiModel {
    id: String,
    #[serde(default)]
    display_name: String,
}

#[derive(Debug, Deserialize)]
struct ApiModelsResponse {
    data: Vec<ApiModel>,
}

#[derive(Debug, Clone)]
pub enum HarvestError {
    /// `ANTHROPIC_API_KEY` is not set — caller should fall back to the
    /// maintained list rather than treat this as a failure.
    NoApiKey,
    /// Network / HTTP / decode failure — original message preserved.
    HarvestFailed { message: String },
}

impl HarvestError {
    pub fn to_command_string(&self) -> String {
        match self {
            Self::NoApiKey => "claude_no_api_key".into(),
            Self::HarvestFailed { message } => {
                format!("claude_harvest_failed: {message}")
            }
        }
    }
}

/// Process-wide cache of the harvested capabilities. Mirrors
/// `CodexCapabilityCache` — populated on the first call and reused for
/// the rest of the app's lifetime; an eventual settings-panel button
/// can `invalidate()` to force a refresh.
#[derive(Default)]
pub struct ClaudeCapabilityCache {
    inner: Mutex<Option<ProviderChatCapabilities>>,
}

impl ClaudeCapabilityCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Return the cached value if present; otherwise run a live
    /// harvest, cache it, and return. Cascades the harvest paths:
    ///
    ///   1. **Sidecar** — calls the SDK's `supportedModels()` via the
    ///      `list-models` RPC method on the claude-agent sidecar.
    ///      Works for every Claude Code user (subscription, OAuth, or
    ///      API key) and surfaces the *deployed* CLI's actual
    ///      effort vocabulary (including levels like `ultracode` the
    ///      bundled SDK type union doesn't enumerate yet).
    ///   2. **Anthropic `/v1/models`** — only when
    ///      `ANTHROPIC_API_KEY` is set. Kept as a fallback for the
    ///      narrow case where the sidecar can't reach the SDK but the
    ///      user has an API key handy.
    ///
    /// The dispatcher falls back to the hand-maintained
    /// [`claude_fallback_capabilities`] when both paths fail.
    pub async fn get_or_harvest(
        &self,
    ) -> Result<ProviderChatCapabilities, HarvestError> {
        {
            let guard = self.inner.lock().await;
            if let Some(cached) = guard.clone() {
                return Ok(cached);
            }
        }
        let result = match harvest_via_sidecar().await {
            Ok(caps) => Ok(caps),
            Err(sidecar_err) => match harvest_via_api().await {
                Ok(caps) => Ok(caps),
                Err(api_err) => {
                    // If the API path bailed only because there's no
                    // key, surface the sidecar's error (more
                    // actionable: install / log into Claude Code);
                    // otherwise surface the API's error.
                    eprintln!(
                        "[claude] sidecar harvest failed: {}",
                        sidecar_err.to_command_string()
                    );
                    Err(match api_err {
                        HarvestError::NoApiKey => sidecar_err,
                        other => other,
                    })
                }
            },
        };
        if let Ok(ref caps) = result {
            let mut guard = self.inner.lock().await;
            *guard = Some(caps.clone());
        }
        result
    }

    /// Drop any cached value. The next call re-harvests.
    pub async fn invalidate(&self) {
        let mut guard = self.inner.lock().await;
        *guard = None;
    }
}

/// Live-harvest the Claude model list from Anthropic's REST API.
/// Requires `ANTHROPIC_API_KEY` in the environment; returns
/// [`HarvestError::NoApiKey`] otherwise so the caller (`get_or_harvest`)
/// can fall through to the maintained bundle.
async fn harvest_via_api() -> Result<ProviderChatCapabilities, HarvestError> {
    let api_key = std::env::var("ANTHROPIC_API_KEY")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .ok_or(HarvestError::NoApiKey)?;
    let client = reqwest::Client::builder()
        .timeout(HARVEST_TIMEOUT)
        .build()
        .map_err(|e| HarvestError::HarvestFailed {
            message: format!("client build: {e}"),
        })?;
    let resp = client
        .get(format!("{ANTHROPIC_API_BASE}/v1/models?limit=1000"))
        .header("x-api-key", api_key)
        .header("anthropic-version", ANTHROPIC_API_VERSION)
        .send()
        .await
        .map_err(|e| HarvestError::HarvestFailed {
            message: format!("request: {e}"),
        })?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(HarvestError::HarvestFailed {
            message: format!("HTTP {status}: {body}"),
        });
    }
    let parsed: ApiModelsResponse =
        resp.json()
            .await
            .map_err(|e| HarvestError::HarvestFailed {
                message: format!("decode: {e}"),
            })?;
    Ok(build_capabilities_from_live(parsed.data))
}

/// Merge a live model list with the hand-maintained per-id metadata.
/// Pulled out so it's unit-testable against fixture data without a
/// live API call (`#[cfg(test)]` tests in this module reach it via
/// `use super::*`).
fn build_capabilities_from_live(live: Vec<ApiModel>) -> ProviderChatCapabilities {
    let maintained: HashMap<String, ChatModelInfo> = models()
        .into_iter()
        .map(|m| (m.id.clone(), m))
        .collect();
    let merged: Vec<ChatModelInfo> = live
        .into_iter()
        .filter(|m| !m.id.is_empty())
        .map(|m| {
            maintained
                .get(&m.id)
                .cloned()
                .unwrap_or_else(|| infer_model_info(&m.id, &m.display_name))
        })
        .collect();
    ProviderChatCapabilities {
        models: merged,
        effort_granularity: EffortGranularity::PerSession,
        effort_label_map: claude_effort_label_map(),
        permission_modes: claude_permission_modes(),
        default_permission_mode: Some("bypassPermissions".into()),
        permission_granularity: EffortGranularity::PerSession,
    }
}

/// Build a `ChatModelInfo` for a Claude model id Codemux's maintained
/// list doesn't (yet) know about. Capabilities are inferred from the
/// family prefix; conservative defaults keep an unknown model usable
/// without overpromising (an unsupported effort level would error at
/// launch).
fn infer_model_info(id: &str, display_name: &str) -> ChatModelInfo {
    let label = if display_name.trim().is_empty() {
        id.to_string()
    } else {
        display_name.to_string()
    };
    let family = ClaudeFamily::from_id(id);
    let (effort_levels, default_effort, prompt_injected, ctx) = match family {
        ClaudeFamily::Opus => (
            vec![
                "low".into(),
                "medium".into(),
                "high".into(),
                "xhigh".into(),
                "max".into(),
                "ultracode".into(),
            ],
            Some("xhigh".into()),
            vec!["ultrathink".into()],
            vec![ctx_200k(), ctx_1m_default()],
        ),
        ClaudeFamily::Sonnet => (
            vec!["low".into(), "medium".into(), "high".into()],
            Some("high".into()),
            vec!["ultrathink".into()],
            vec![ctx_200k(), ctx_1m_default()],
        ),
        ClaudeFamily::Haiku | ClaudeFamily::Other => {
            (vec![], None, vec![], vec![])
        }
    };
    ChatModelInfo {
        id: id.to_string(),
        label,
        description: None,
        effort_levels,
        default_effort,
        prompt_injected_effort_levels: prompt_injected,
        context_window_options: ctx,
        supports_adaptive_thinking: matches!(family, ClaudeFamily::Opus),
        supports_thinking_toggle: matches!(family, ClaudeFamily::Haiku),
        supports_fast_mode: false,
        supports_images: true,
        sub_provider: None,
        is_free: false,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClaudeFamily {
    Opus,
    Sonnet,
    Haiku,
    Other,
}

impl ClaudeFamily {
    fn from_id(id: &str) -> Self {
        let lower = id.to_ascii_lowercase();
        if lower.contains("opus") {
            Self::Opus
        } else if lower.contains("sonnet") {
            Self::Sonnet
        } else if lower.contains("haiku") {
            Self::Haiku
        } else {
            Self::Other
        }
    }
}

// ── Live harvest via the sidecar / SDK `supportedModels()` ───────────
//
// Anthropic's Agent SDK exposes a live `query.supportedModels()` API
// over whichever auth Claude Code is using (subscription, OAuth, API
// key — the SDK handles it transparently). The sidecar exposes a
// `list-models` JSON-RPC method that opens a transient `query()`,
// awaits `supportedModels()`, and returns the array. The result is
// the *deployed* CLI's actual supported model + effort vocabulary —
// so effort levels the bundled SDK type union doesn't enumerate yet
// (currently `ultracode`) still surface.

#[derive(Debug, Deserialize)]
struct SdkModelInfo {
    /// SDK's identifier for the model (e.g. `claude-opus-4-8`).
    value: String,
    #[serde(default, rename = "displayName")]
    display_name: String,
    #[serde(default)]
    description: String,
    /// Effort levels the SDK reports for this model. Open-ended
    /// `Vec<String>` so a runtime addition (like `ultracode`) the
    /// `.d.ts` union doesn't list still flows through verbatim.
    #[serde(default, rename = "supportedEffortLevels")]
    supported_effort_levels: Vec<String>,
    #[serde(default, rename = "supportsAdaptiveThinking")]
    supports_adaptive_thinking: Option<bool>,
    #[serde(default, rename = "supportsFastMode")]
    supports_fast_mode: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct ListModelsResponse {
    #[serde(default)]
    models: Vec<SdkModelInfo>,
}

/// Live-harvest the Claude model list via the SDK, by sending a
/// `list-models` JSON-RPC request to a transient claude-agent sidecar.
/// Works for every Claude Code user — the SDK uses whatever auth
/// Claude Code already has — and returns the deployed CLI's actual
/// supported model + effort vocabulary.
async fn harvest_via_sidecar() -> Result<ProviderChatCapabilities, HarvestError> {
    let sidecar = crate::agent_provider::claude::sidecar_path::resolve_sidecar_path()
        .map_err(|e| HarvestError::HarvestFailed {
            message: format!("resolve sidecar: {e:?}"),
        })?;
    let claude_binary = which::which("claude").map_err(|_| HarvestError::HarvestFailed {
        message: "claude binary not on PATH (install Claude Code or sign in to it)".into(),
    })?;
    let cwd = std::env::temp_dir().to_string_lossy().to_string();

    let child = tokio::time::timeout(
        Duration::from_secs(20),
        JsonRpcChild::spawn(SpawnConfig {
            program: sidecar,
            args: vec![],
            env: HashMap::new(),
            cwd: None,
            default_timeout: Duration::from_secs(20),
        }),
    )
    .await
    .map_err(|_| HarvestError::HarvestFailed {
        message: "sidecar spawn timed out".into(),
    })?
    .map_err(|e| HarvestError::HarvestFailed {
        message: format!("sidecar spawn: {e}"),
    })?;

    let response = child
        .request(
            "list-models",
            json!({
                "cwd": cwd,
                "pathToClaudeCodeExecutable": claude_binary.to_string_lossy(),
            }),
        )
        .await;
    let _ = child.shutdown().await;
    let response = response.map_err(|e| HarvestError::HarvestFailed {
        message: format!("list-models RPC: {e}"),
    })?;

    let parsed: ListModelsResponse =
        serde_json::from_value(response).map_err(|e| HarvestError::HarvestFailed {
            message: format!("decode: {e}"),
        })?;

    if parsed.models.is_empty() {
        return Err(HarvestError::HarvestFailed {
            message: "SDK returned an empty model list".into(),
        });
    }

    Ok(build_capabilities_from_sdk(parsed.models))
}

/// Build a `ProviderChatCapabilities` bundle from the SDK's live model
/// list, merging with hand-maintained per-id metadata.
fn build_capabilities_from_sdk(
    sdk_models: Vec<SdkModelInfo>,
) -> ProviderChatCapabilities {
    let maintained: HashMap<String, ChatModelInfo> = models()
        .into_iter()
        .map(|m| (m.id.clone(), m))
        .collect();
    let merged: Vec<ChatModelInfo> = sdk_models
        .into_iter()
        .filter(|m| !m.value.is_empty())
        .map(|sdk| merge_sdk_with_maintained(sdk, &maintained))
        .collect();
    ProviderChatCapabilities {
        models: merged,
        effort_granularity: EffortGranularity::PerSession,
        effort_label_map: claude_effort_label_map(),
        permission_modes: claude_permission_modes(),
        default_permission_mode: Some("bypassPermissions".into()),
        permission_granularity: EffortGranularity::PerSession,
    }
}

/// Merge a single SDK model record with hand-maintained metadata. The
/// SDK is authoritative for the live model id, display name, and
/// effort vocabulary (since the deployed CLI is sometimes ahead of
/// the bundled types); the maintained map fills in Codemux-specific
/// UX bits the SDK doesn't surface (context windows, prompt-injected
/// `ultrathink`, default effort, the Haiku thinking toggle).
fn merge_sdk_with_maintained(
    sdk: SdkModelInfo,
    maintained: &HashMap<String, ChatModelInfo>,
) -> ChatModelInfo {
    let known = maintained.get(&sdk.value).cloned();
    let inferred = || infer_model_info(&sdk.value, &sdk.display_name);

    // Effort levels — SDK runtime data wins (so newly-added levels
    // like `ultracode` flow in). Fall back to maintained / inferred
    // only when the SDK doesn't report any.
    let effort_levels = if !sdk.supported_effort_levels.is_empty() {
        sdk.supported_effort_levels.clone()
    } else {
        known
            .as_ref()
            .map(|k| k.effort_levels.clone())
            .unwrap_or_else(|| inferred().effort_levels)
    };
    let default_effort = known
        .as_ref()
        .and_then(|k| k.default_effort.clone())
        .or_else(|| effort_levels.first().cloned());
    let prompt_injected_effort_levels = known
        .as_ref()
        .map(|k| k.prompt_injected_effort_levels.clone())
        .unwrap_or_else(|| inferred().prompt_injected_effort_levels);
    let context_window_options = known
        .as_ref()
        .map(|k| k.context_window_options.clone())
        .unwrap_or_else(|| inferred().context_window_options);

    let label = if sdk.display_name.trim().is_empty() {
        sdk.value.clone()
    } else {
        sdk.display_name
    };
    let description = if sdk.description.trim().is_empty() {
        known.as_ref().and_then(|k| k.description.clone())
    } else {
        Some(sdk.description)
    };

    ChatModelInfo {
        id: sdk.value,
        label,
        description,
        effort_levels,
        default_effort,
        prompt_injected_effort_levels,
        context_window_options,
        supports_adaptive_thinking: sdk
            .supports_adaptive_thinking
            .unwrap_or_else(|| known.as_ref().map(|k| k.supports_adaptive_thinking).unwrap_or(false)),
        supports_thinking_toggle: known
            .as_ref()
            .map(|k| k.supports_thinking_toggle)
            .unwrap_or(false),
        supports_fast_mode: sdk
            .supports_fast_mode
            .unwrap_or_else(|| known.as_ref().map(|k| k.supports_fast_mode).unwrap_or(false)),
        supports_images: true,
        sub_provider: None,
        is_free: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_includes_core_roster() {
        let caps = claude_fallback_capabilities();
        let ids: Vec<&str> = caps.models.iter().map(|m| m.id.as_str()).collect();
        assert!(ids.contains(&"claude-opus-4-7"));
        assert!(ids.contains(&"claude-sonnet-4-6"));
        assert!(ids.contains(&"claude-haiku-4-5"));
        assert_eq!(caps.effort_granularity, EffortGranularity::PerSession);
    }

    #[test]
    fn opus_47_has_ultrathink_and_1m() {
        let caps = claude_fallback_capabilities();
        let opus = caps.models.iter().find(|m| m.id == "claude-opus-4-7").unwrap();
        assert!(opus.prompt_injected_effort_levels.contains(&"ultrathink".into()));
        assert!(opus.context_window_options.iter().any(|o| o.value == "1m"));
        let default_ctx = opus
            .context_window_options
            .iter()
            .find(|o| o.is_default)
            .unwrap();
        // Highest-is-default rule (Stage C follow-up): the larger
        // context window is now flagged as the default across every
        // multi-option Claude model.
        assert_eq!(default_ctx.value, "1m");
        assert_eq!(opus.default_effort.as_deref(), Some("xhigh"));
    }

    #[test]
    fn every_multi_option_model_defaults_to_the_largest_context_window() {
        // "Largest" here is a lexical shortcut: "1m" > "200k" in our
        // fixed two-option universe. If a third option lands later
        // (e.g. "2m"), update this ordering helper to sort by
        // numeric bytes.
        fn size_rank(value: &str) -> u32 {
            match value {
                "200k" => 200_000,
                "1m" => 1_000_000,
                other => other
                    .trim_end_matches(|c: char| !c.is_ascii_digit())
                    .parse()
                    .unwrap_or(0),
            }
        }

        let caps = claude_fallback_capabilities();
        for model in &caps.models {
            if model.context_window_options.len() <= 1 {
                continue;
            }
            let default_option = model
                .context_window_options
                .iter()
                .find(|o| o.is_default)
                .unwrap_or_else(|| {
                    panic!(
                        "model {} has multiple context windows but no default",
                        model.id
                    )
                });
            let max_rank = model
                .context_window_options
                .iter()
                .map(|o| size_rank(&o.value))
                .max()
                .unwrap();
            assert_eq!(
                size_rank(&default_option.value),
                max_rank,
                "model {} default context window must be the largest option",
                model.id,
            );
        }
    }

    #[test]
    fn haiku_has_no_effort_or_context_window() {
        let caps = claude_fallback_capabilities();
        let haiku = caps.models.iter().find(|m| m.id == "claude-haiku-4-5").unwrap();
        assert!(haiku.effort_levels.is_empty());
        assert!(haiku.prompt_injected_effort_levels.is_empty());
        assert!(haiku.context_window_options.is_empty());
        // The thinking toggle flag is preserved even though we don't
        // surface it in MVP.
        assert!(haiku.supports_thinking_toggle);
    }

    #[test]
    fn opus_45_has_no_ultrathink_no_1m() {
        let caps = claude_fallback_capabilities();
        let opus45 = caps.models.iter().find(|m| m.id == "claude-opus-4-5").unwrap();
        assert!(opus45.prompt_injected_effort_levels.is_empty());
        assert!(opus45.context_window_options.is_empty());
    }

    #[test]
    fn resolve_api_model_id_appends_1m_bracket() {
        assert_eq!(
            resolve_claude_api_model_id("claude-opus-4-7", Some("1m")),
            "claude-opus-4-7[1m]"
        );
    }

    #[test]
    fn resolve_api_model_id_passthrough_for_default_and_null() {
        assert_eq!(
            resolve_claude_api_model_id("claude-opus-4-7", Some("200k")),
            "claude-opus-4-7"
        );
        assert_eq!(resolve_claude_api_model_id("claude-opus-4-7", None), "claude-opus-4-7");
        assert_eq!(resolve_claude_api_model_id("claude-sonnet-4-6", Some("")), "claude-sonnet-4-6");
    }

    #[test]
    fn every_listed_model_supports_images() {
        // Stage 6: vision is universal across the Claude 4.x roster
        // we expose. Lock the contract so a future model addition
        // forces an explicit decision on whether vision is included.
        let caps = claude_fallback_capabilities();
        for model in &caps.models {
            assert!(
                model.supports_images,
                "{} should expose supports_images=true",
                model.id
            );
        }
    }

    #[test]
    fn label_map_covers_all_effort_levels() {
        let caps = claude_fallback_capabilities();
        assert_eq!(caps.effort_label_map.get("xhigh").map(String::as_str), Some("Extra High"));
        assert_eq!(caps.effort_label_map.get("ultracode").map(String::as_str), Some("Ultracode"));
        assert_eq!(caps.effort_label_map.get("ultrathink").map(String::as_str), Some("Ultrathink"));
    }

    #[test]
    fn opus_models_offer_ultracode_in_maintained_list() {
        // `ultracode` is the top tier in Claude Code's `/effort` slider
        // (xhigh + workflows) — every current-flagship Opus must expose
        // it. Sonnet/Haiku stay at their narrower effort sets.
        let caps = claude_fallback_capabilities();
        for id in ["claude-opus-4-8", "claude-opus-4-7"] {
            let m = caps
                .models
                .iter()
                .find(|m| m.id == id)
                .unwrap_or_else(|| panic!("{id} should be in the maintained list"));
            assert!(
                m.effort_levels.contains(&"ultracode".to_string()),
                "{id} should expose the `ultracode` effort level"
            );
        }
    }

    // ── Live-harvest mapping tests ───────────────────────────────────

    #[test]
    fn maintained_list_includes_opus_4_8() {
        let caps = claude_fallback_capabilities();
        let opus48 = caps
            .models
            .iter()
            .find(|m| m.id == "claude-opus-4-8")
            .expect("Opus 4.8 should be present in the maintained list");
        assert_eq!(opus48.default_effort.as_deref(), Some("xhigh"));
        assert!(
            opus48
                .context_window_options
                .iter()
                .any(|o| o.value == "1m"),
            "Opus 4.8 should support the 1M context window"
        );
    }

    #[test]
    fn family_inference_from_id_handles_each_known_prefix() {
        assert_eq!(ClaudeFamily::from_id("claude-opus-9-0"), ClaudeFamily::Opus);
        assert_eq!(ClaudeFamily::from_id("claude-sonnet-5-0"), ClaudeFamily::Sonnet);
        assert_eq!(ClaudeFamily::from_id("claude-haiku-5-0"), ClaudeFamily::Haiku);
        assert_eq!(ClaudeFamily::from_id("xyzzy-thing"), ClaudeFamily::Other);
        // Case-insensitive — Anthropic ids are lowercase but be defensive.
        assert_eq!(ClaudeFamily::from_id("Claude-OPUS-9"), ClaudeFamily::Opus);
    }

    #[test]
    fn infer_opus_gets_full_effort_and_1m() {
        let info = infer_model_info("claude-opus-9-9", "Claude Opus 9.9");
        assert_eq!(info.effort_levels.len(), 6);
        assert!(info.effort_levels.contains(&"ultracode".to_string()));
        assert_eq!(info.default_effort.as_deref(), Some("xhigh"));
        assert!(
            info.prompt_injected_effort_levels
                .contains(&"ultrathink".to_string())
        );
        assert!(info.context_window_options.iter().any(|o| o.value == "1m"));
        assert!(info.supports_adaptive_thinking);
        assert_eq!(info.label, "Claude Opus 9.9");
    }

    #[test]
    fn infer_sonnet_gets_three_efforts_and_1m_and_falls_back_to_id_when_label_empty() {
        let info = infer_model_info("claude-sonnet-5-0", "");
        assert_eq!(info.effort_levels, vec!["low", "medium", "high"]);
        assert_eq!(info.default_effort.as_deref(), Some("high"));
        assert!(info.context_window_options.iter().any(|o| o.value == "1m"));
        // Empty display_name → label falls back to the id.
        assert_eq!(info.label, "claude-sonnet-5-0");
    }

    #[test]
    fn infer_haiku_is_conservative_no_effort_no_1m() {
        let info = infer_model_info("claude-haiku-9-0", "Haiku 9");
        assert!(info.effort_levels.is_empty());
        assert!(info.context_window_options.is_empty());
        assert!(info.supports_thinking_toggle);
        assert!(!info.supports_adaptive_thinking);
    }

    #[test]
    fn infer_unknown_family_gets_minimal_metadata() {
        let info = infer_model_info("some-future-model", "Future Model");
        assert!(info.effort_levels.is_empty());
        assert!(info.context_window_options.is_empty());
    }

    #[test]
    fn merge_uses_maintained_metadata_for_known_ids() {
        // The maintained Opus 4.7 entry has `description: None` (demoted
        // from flagship). An inferred Opus would carry no description
        // either, so we discriminate by the precise label — maintained
        // is "Claude Opus 4.7", and the api `display_name` we feed is
        // deliberately different to prove maintained wins.
        let live = vec![ApiModel {
            id: "claude-opus-4-7".into(),
            display_name: "DIFFERENT NAME".into(),
        }];
        let caps = build_capabilities_from_live(live);
        let opus47 = &caps.models[0];
        assert_eq!(opus47.id, "claude-opus-4-7");
        // Maintained label wins, NOT the api display_name.
        assert_eq!(opus47.label, "Claude Opus 4.7");
    }

    #[test]
    fn merge_surfaces_unknown_ids_via_inference() {
        // A model id Codemux has never heard of must still appear in
        // the picker, with family-inferred metadata.
        let live = vec![ApiModel {
            id: "claude-opus-9-9".into(),
            display_name: "Claude Opus 9.9".into(),
        }];
        let caps = build_capabilities_from_live(live);
        let new = &caps.models[0];
        assert_eq!(new.id, "claude-opus-9-9");
        assert_eq!(new.label, "Claude Opus 9.9");
        assert_eq!(new.effort_levels.len(), 6);
    }

    #[test]
    fn merge_skips_empty_ids() {
        let live = vec![
            ApiModel { id: "".into(), display_name: "".into() },
            ApiModel {
                id: "claude-opus-4-7".into(),
                display_name: "Opus 4.7".into(),
            },
        ];
        let caps = build_capabilities_from_live(live);
        assert_eq!(caps.models.len(), 1);
    }

    #[test]
    fn merge_carries_the_correct_provider_chrome() {
        // The merged bundle should serve the same effort label map,
        // permission modes, and granularity as the fallback bundle.
        let caps = build_capabilities_from_live(vec![]);
        assert_eq!(caps.effort_granularity, EffortGranularity::PerSession);
        assert_eq!(caps.default_permission_mode.as_deref(), Some("bypassPermissions"));
        assert!(caps.permission_modes.iter().any(|m| m.value == "bypassPermissions"));
    }

    #[test]
    fn harvest_error_command_strings_have_stable_prefixes() {
        assert_eq!(HarvestError::NoApiKey.to_command_string(), "claude_no_api_key");
        assert!(
            HarvestError::HarvestFailed { message: "x".into() }
                .to_command_string()
                .starts_with("claude_harvest_failed:")
        );
    }

    // ── SDK / sidecar harvest mapping tests ──────────────────────────

    fn sdk_model(
        value: &str,
        display: &str,
        effort: &[&str],
        adaptive: Option<bool>,
        fast: Option<bool>,
    ) -> SdkModelInfo {
        SdkModelInfo {
            value: value.into(),
            display_name: display.into(),
            description: "".into(),
            supported_effort_levels: effort.iter().map(|s| s.to_string()).collect(),
            supports_adaptive_thinking: adaptive,
            supports_fast_mode: fast,
        }
    }

    #[test]
    fn sdk_merge_uses_sdk_effort_levels_for_a_known_id() {
        // SDK reports an effort vocabulary that includes `ultracode` —
        // even though the maintained Opus 4.7 entry already lists it,
        // the test confirms the SDK value wins (regression guard for
        // when the deployed CLI ships a level the maintained list
        // hasn't been bumped for yet).
        let live = vec![sdk_model(
            "claude-opus-4-7",
            "Claude Opus 4.7",
            &["low", "medium", "high", "xhigh", "max", "ultracode", "newlevel"],
            Some(true),
            Some(false),
        )];
        let caps = build_capabilities_from_sdk(live);
        let opus = &caps.models[0];
        assert!(opus.effort_levels.contains(&"newlevel".to_string()));
        // Maintained metadata still fills in context windows + ultrathink.
        assert!(opus.context_window_options.iter().any(|o| o.value == "1m"));
        assert!(
            opus.prompt_injected_effort_levels
                .contains(&"ultrathink".to_string())
        );
    }

    #[test]
    fn sdk_merge_surfaces_unknown_id_with_inferred_metadata() {
        // SDK reports a brand-new id Codemux's maintained map has
        // never seen — must surface in the picker with family-pattern
        // inferred metadata (1M context for opus, ultrathink, etc.).
        let live = vec![sdk_model(
            "claude-opus-5-0",
            "Claude Opus 5.0",
            &["low", "medium", "high", "xhigh", "max", "ultracode"],
            Some(true),
            Some(false),
        )];
        let caps = build_capabilities_from_sdk(live);
        let new = &caps.models[0];
        assert_eq!(new.id, "claude-opus-5-0");
        assert_eq!(new.label, "Claude Opus 5.0");
        assert!(new.effort_levels.contains(&"ultracode".to_string()));
        assert!(new.context_window_options.iter().any(|o| o.value == "1m"));
        assert!(
            new.prompt_injected_effort_levels
                .contains(&"ultrathink".to_string())
        );
    }

    #[test]
    fn sdk_merge_falls_back_to_maintained_when_sdk_reports_no_effort() {
        // SDK returns the id with an empty effort vocabulary —
        // maintained metadata fills in.
        let live = vec![sdk_model("claude-opus-4-7", "Claude Opus 4.7", &[], None, None)];
        let caps = build_capabilities_from_sdk(live);
        let opus = &caps.models[0];
        // Maintained Opus 4.7 has 6 effort levels including ultracode.
        assert_eq!(opus.effort_levels.len(), 6);
    }

    #[test]
    fn sdk_merge_skips_empty_value_records() {
        let live = vec![
            sdk_model("", "", &["low"], None, None),
            sdk_model("claude-opus-4-7", "Claude Opus 4.7", &["low"], None, None),
        ];
        let caps = build_capabilities_from_sdk(live);
        assert_eq!(caps.models.len(), 1);
    }

    #[test]
    fn sdk_merge_uses_id_as_label_when_displayname_is_blank() {
        let live = vec![sdk_model("claude-opus-9-9", "", &["high"], None, None)];
        let caps = build_capabilities_from_sdk(live);
        assert_eq!(caps.models[0].label, "claude-opus-9-9");
    }

    #[test]
    fn sdk_merge_carries_provider_chrome_through() {
        let caps = build_capabilities_from_sdk(vec![]);
        assert_eq!(caps.effort_granularity, EffortGranularity::PerSession);
        assert_eq!(
            caps.default_permission_mode.as_deref(),
            Some("bypassPermissions")
        );
    }
}
