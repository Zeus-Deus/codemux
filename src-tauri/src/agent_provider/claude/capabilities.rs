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
        // `ultracode` (xhigh + workflows) exists only in Claude Code's
        // TUI `/model` effort slider. The headless CLI rejects it as an
        // `--effort` value ("Valid values: low, medium, high, xhigh,
        // max" as of claude 2.1.170), so no maintained entry lists it —
        // the label stays only so a future SDK-reported level renders
        // nicely instead of falling back to the raw token.
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
        // Opus 4.8 — the recommended default, effort defaults to high
        // (the level Claude Code's own `/model` slider marks as
        // "(default)"), supports ultrathink + 1M. The CLI's picker
        // calls this row "Default (recommended)".
        ChatModelInfo {
            id: "claude-opus-4-8".into(),
            label: "Claude Opus 4.8".into(),
            description: Some("Best for everyday, complex tasks".into()),
            effort_levels: vec![
                "low".into(),
                "medium".into(),
                "high".into(),
                "xhigh".into(),
                "max".into(),
            ],
            default_effort: Some("high".into()),
            prompt_injected_effort_levels: vec!["ultrathink".into()],
            context_window_options: vec![ctx_200k(), ctx_1m_default()],
            supports_adaptive_thinking: true,
            supports_thinking_toggle: false,
            supports_fast_mode: false,
            supports_images: true,
            sub_provider: None,
            is_free: false,
        },
        // Fable 5 — top tier above Opus. The deployed CLI reports it
        // with the context window pinned into the id itself
        // (`claude-fable-5[1m]`); the maintained entry is keyed by the
        // bare id and the merge strips the suffix before lookup.
        ChatModelInfo {
            id: "claude-fable-5".into(),
            label: "Claude Fable 5".into(),
            description: Some("Most capable for the hardest, longest-running tasks".into()),
            effort_levels: vec![
                "low".into(),
                "medium".into(),
                "high".into(),
                "xhigh".into(),
                "max".into(),
            ],
            default_effort: Some("high".into()),
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
            description: Some("Previous Opus generation".into()),
            effort_levels: vec![
                "low".into(),
                "medium".into(),
                "high".into(),
                "xhigh".into(),
                "max".into(),
            ],
            default_effort: Some("high".into()),
            prompt_injected_effort_levels: vec!["ultrathink".into()],
            context_window_options: vec![ctx_200k(), ctx_1m_default()],
            supports_adaptive_thinking: true,
            supports_thinking_toggle: false,
            supports_fast_mode: false,
            supports_images: true,
            sub_provider: None,
            is_free: false,
        },
        // Opus 4.6 — default effort is high, supports ultrathink + 1M.
        // (Fast mode existed on this model but is no longer surfaced —
        // see the fast-mode note in `merge_sdk_with_maintained`.)
        ChatModelInfo {
            id: "claude-opus-4-6".into(),
            label: "Claude Opus 4.6".into(),
            description: Some("Previous Opus generation".into()),
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
            supports_fast_mode: false,
            supports_images: true,
            sub_provider: None,
            is_free: false,
        },
        // Opus 4.5 — no ultrathink, no 1M context.
        ChatModelInfo {
            id: "claude-opus-4-5".into(),
            label: "Claude Opus 4.5".into(),
            description: Some("Previous Opus generation".into()),
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
            supports_fast_mode: false,
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

/// Split a model id into its base id and an optional pinned
/// context-window bracket suffix: `claude-fable-5[1m]` →
/// `("claude-fable-5", Some("[1m]"))`. The deployed CLI sometimes
/// reports ids with the window already baked in; metadata lookups key
/// on the base id, and a pinned id must never grow a second suffix.
fn split_context_suffix(id: &str) -> (&str, Option<&str>) {
    if let Some(open) = id.find('[') {
        if id.ends_with(']') {
            return (&id[..open], Some(&id[open..]));
        }
    }
    (id, None)
}

/// The inner window value of a pinned id (`claude-fable-5[1m]` →
/// `Some("1m")`); `None` for a bare id. This is the value that should
/// become the default in the model's context-window picker.
fn pinned_window(id: &str) -> Option<&str> {
    split_context_suffix(id)
        .1
        .map(|s| s.trim_start_matches('[').trim_end_matches(']'))
}

/// Mark `pinned` as the sole default in a context-window option set,
/// clearing the other defaults. Lets the CLI's baked-in window choice
/// (e.g. the `[1m]` the deployed roster pins on Fable) drive the picker
/// default instead of the generic highest-is-default rule, while still
/// offering the full set so the window stays user-selectable. When
/// nothing is pinned (or the pinned value isn't one of the options) the
/// set is returned with its own defaults intact.
fn with_pinned_default(
    mut opts: Vec<ContextWindowOption>,
    pinned: Option<&str>,
) -> Vec<ContextWindowOption> {
    if let Some(win) = pinned {
        if opts.iter().any(|o| o.value == win) {
            for o in &mut opts {
                o.is_default = o.value == win;
            }
        }
    }
    opts
}

/// Apply the reference impl's `resolveClaudeApiModelId` trick: when
/// the context window is `"1m"`, the Anthropic API expects the model
/// id to carry a `[1m]` bracket suffix. Any other value (or `None`)
/// returns the id unchanged — as does an id that already carries a
/// bracket suffix (e.g. the SDK-reported `claude-fable-5[1m]`), so a
/// pinned model never double-appends.
pub fn resolve_claude_api_model_id(
    model_id: &str,
    context_window: Option<&str>,
) -> String {
    let already_pinned = split_context_suffix(model_id).1.is_some();
    match context_window {
        Some("1m") if !already_pinned => format!("{model_id}[1m]"),
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
    // Normalize a pinned id (`claude-fable-5[1m]`) to its base id and
    // surface the pinned window as the picker default — so a pinned and
    // a bare flagship id behave identically (consistent context picker).
    let (base_id, _) = split_context_suffix(id);
    let family = ClaudeFamily::from_id(id);
    let (effort_levels, default_effort, prompt_injected, ctx) = match family {
        f if f.is_flagship() => (
            vec![
                "low".into(),
                "medium".into(),
                "high".into(),
                "xhigh".into(),
                "max".into(),
            ],
            // Claude Code's own `/model` slider marks `high` as the
            // default for every current flagship row.
            Some("high".into()),
            vec!["ultrathink".into()],
            // The `default` alias has its context window chosen by the
            // CLI — offering a picker would synthesize `default[1m]`,
            // which is not a valid model id.
            if family == ClaudeFamily::DefaultAlias {
                vec![]
            } else {
                vec![ctx_200k(), ctx_1m_default()]
            },
        ),
        ClaudeFamily::Sonnet => (
            vec!["low".into(), "medium".into(), "high".into()],
            Some("high".into()),
            vec!["ultrathink".into()],
            vec![ctx_200k(), ctx_1m_default()],
        ),
        _ => (vec![], None, vec![], vec![]),
    };
    let context_window_options = with_pinned_default(ctx, pinned_window(id));
    ChatModelInfo {
        id: base_id.to_string(),
        label,
        description: None,
        effort_levels,
        default_effort,
        prompt_injected_effort_levels: prompt_injected,
        context_window_options,
        supports_adaptive_thinking: family.is_flagship(),
        supports_thinking_toggle: matches!(family, ClaudeFamily::Haiku),
        supports_fast_mode: false,
        supports_images: true,
        sub_provider: None,
        is_free: false,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum ClaudeFamily {
    /// Top tier above Opus (e.g. `claude-fable-5`).
    Fable,
    Opus,
    Sonnet,
    Haiku,
    /// The CLI's `default` alias — the recommended flagship with the
    /// context window already chosen by the CLI.
    DefaultAlias,
    Other,
}

impl ClaudeFamily {
    fn from_id(id: &str) -> Self {
        let (base, _) = split_context_suffix(id);
        let lower = base.to_ascii_lowercase();
        if lower == "default" {
            Self::DefaultAlias
        } else if lower.contains("fable") {
            Self::Fable
        } else if lower.contains("opus") {
            Self::Opus
        } else if lower.contains("sonnet") {
            Self::Sonnet
        } else if lower.contains("haiku") {
            Self::Haiku
        } else {
            Self::Other
        }
    }

    /// Flagship-tier families share the full effort vocabulary, a
    /// `high` default (the level the CLI's own `/model` slider marks
    /// as "(default)"), and prompt-injected `ultrathink`. New top-tier
    /// families only need a `from_id` arm and a mention here to surface
    /// fully-configured in the picker.
    fn is_flagship(self) -> bool {
        matches!(self, Self::Fable | Self::Opus | Self::DefaultAlias)
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
    /// Reported by the SDK but deliberately ignored: fast mode is
    /// clamped off for Claude in `merge_sdk_with_maintained` (the flag
    /// is a capability advertisement, not an entitlement check — see
    /// the note there). Kept so the wire shape stays documented and a
    /// future re-enable is a one-line change.
    #[allow(dead_code)]
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

/// The deployed CLI's `supportedModels()` is a *curated picker roster*,
/// not the full launchable surface — Claude Code accepts previous model
/// names via `--model` even when its picker no longer lists them (the
/// `/model` menu itself says "For other/previous model names, specify
/// with --model"). When the live roster carries no entry of a
/// maintained family (e.g. the CLI demoted Opus into its `default`
/// alias), append that family's maintained entries so every launchable
/// model stays individually selectable.
///
/// The `default` alias deliberately does NOT mark any concrete family
/// as present: it tracks the *user's own* configured default (whatever
/// they last picked in `/model`), so it can't stand in for Opus — or
/// any other family.
fn append_missing_maintained_families(merged: &mut Vec<ChatModelInfo>) {
    use std::collections::HashSet;
    let present: HashSet<ClaudeFamily> = merged
        .iter()
        .map(|m| ClaudeFamily::from_id(&m.id))
        .filter(|f| !matches!(f, ClaudeFamily::DefaultAlias | ClaudeFamily::Other))
        .collect();
    for m in models() {
        let family = ClaudeFamily::from_id(&m.id);
        if !matches!(family, ClaudeFamily::DefaultAlias | ClaudeFamily::Other)
            && !present.contains(&family)
        {
            merged.push(m);
        }
    }
}

/// Build a `ProviderChatCapabilities` bundle from the SDK's live model
/// list, merging with hand-maintained per-id metadata. Models the live
/// roster omits but the CLI still launches are appended afterwards —
/// see [`append_missing_maintained_families`].
fn build_capabilities_from_sdk(
    sdk_models: Vec<SdkModelInfo>,
) -> ProviderChatCapabilities {
    let maintained: HashMap<String, ChatModelInfo> = models()
        .into_iter()
        .map(|m| (m.id.clone(), m))
        .collect();
    let mut merged: Vec<ChatModelInfo> = sdk_models
        .into_iter()
        .filter(|m| !m.value.is_empty())
        .map(|sdk| merge_sdk_with_maintained(sdk, &maintained))
        .collect();
    append_missing_maintained_families(&mut merged);
    ProviderChatCapabilities {
        models: merged,
        effort_granularity: EffortGranularity::PerSession,
        effort_label_map: claude_effort_label_map(),
        permission_modes: claude_permission_modes(),
        default_permission_mode: Some("bypassPermissions".into()),
        permission_granularity: EffortGranularity::PerSession,
    }
}

/// The deployed CLI's `supportedModels()` reports its curated roster
/// under short *alias* ids (`default`, `opus`, `fable`, `sonnet`,
/// `haiku`) rather than the full versioned id, and some deployed CLIs
/// give those alias rows no `description`. This table maps each alias to
/// the canonical full id in the maintained [`models`] table so an alias
/// row can borrow the canonical entry's resolved version + blurb when
/// the SDK reports no description of its own. `default` is the CLI's
/// recommended default and resolves like `opus`. Keeping the mapping in
/// one place means a future model bump is a one-line edit.
const ALIAS_CANONICAL_IDS: &[(&str, &str)] = &[
    ("default", "claude-opus-4-8"),
    ("opus", "claude-opus-4-8"),
    ("fable", "claude-fable-5"),
    ("sonnet", "claude-sonnet-4-6"),
    ("haiku", "claude-haiku-4-5"),
];

/// Resolve an alias id (`default` / `opus` / `fable` / `sonnet` /
/// `haiku`) to its canonical full id. Case-insensitive; returns `None`
/// for a full id or an unknown id (both fall through to the normal
/// lookup / inference path).
fn canonical_id_for_alias(id: &str) -> Option<&'static str> {
    let lower = id.to_ascii_lowercase();
    ALIAS_CANONICAL_IDS
        .iter()
        .find(|(alias, _)| *alias == lower)
        .map(|(_, canonical)| *canonical)
}

/// Build a version-bearing description for an alias row from its
/// canonical maintained entry, per the picker contract:
/// `"<Version>[ with 1M context] · <blurb>"` — e.g.
/// `"Opus 4.8 with 1M context · Best for everyday, complex tasks"`.
///
/// The version is the canonical label with the leading `"Claude "`
/// stripped (`"Claude Opus 4.8"` → `"Opus 4.8"`); `" with 1M context"`
/// is appended only when the canonical model *defaults* to a 1M context
/// window; the blurb is the canonical entry's own description. Returns
/// `None` when the canonical entry carries no blurb, so nothing empty is
/// ever synthesized.
fn alias_description_from_canonical(canonical: &ChatModelInfo) -> Option<String> {
    let blurb = canonical.description.as_deref()?;
    let version = canonical
        .label
        .strip_prefix("Claude ")
        .unwrap_or(canonical.label.as_str());
    let defaults_to_1m = canonical
        .context_window_options
        .iter()
        .any(|o| o.is_default && o.value == "1m");
    let prefix = if defaults_to_1m {
        format!("{version} with 1M context")
    } else {
        version.to_string()
    };
    Some(format!("{prefix} · {blurb}"))
}

/// Merge a single SDK model record with hand-maintained metadata. The
/// SDK is authoritative for the live model id, display name, and
/// effort vocabulary (since the deployed CLI is sometimes ahead of
/// the bundled types); the maintained map fills in Codemux-specific
/// UX bits the SDK doesn't surface (context windows, prompt-injected
/// `ultrathink`, default effort, the Haiku thinking toggle). Family
/// inference covers ids the maintained map doesn't know — including
/// the CLI's alias ids (`default`, `sonnet`, `haiku`) and ids with a
/// pinned context suffix (`claude-fable-5[1m]`, looked up by base id).
fn merge_sdk_with_maintained(
    sdk: SdkModelInfo,
    maintained: &HashMap<String, ChatModelInfo>,
) -> ChatModelInfo {
    // The deployed CLI may pin the context window into the id itself
    // (`claude-fable-5[1m]`). Look maintained metadata up by the base
    // id, normalize the model id to that base, and surface the pinned
    // window as the picker default — captured as owned values upfront
    // so later moves out of `sdk` don't fight the borrow checker.
    let base_id = split_context_suffix(&sdk.value).0.to_string();
    let pinned = pinned_window(&sdk.value).map(str::to_string);
    let known = maintained.get(&base_id).cloned();
    let inferred = infer_model_info(&sdk.value, &sdk.display_name);

    // Effort levels — SDK runtime data wins (so a newly-added level
    // the maintained list hasn't been bumped for still flows in).
    // Fall back to maintained / inferred only when the SDK doesn't
    // report any.
    let effort_levels = if !sdk.supported_effort_levels.is_empty() {
        sdk.supported_effort_levels.clone()
    } else {
        known
            .as_ref()
            .map(|k| k.effort_levels.clone())
            .unwrap_or_else(|| inferred.effort_levels.clone())
    };
    // Default effort: maintained wins, then the family-inferred
    // default — each validated against the live vocabulary so we never
    // default to a level the deployed CLI rejects. Only then fall to
    // `high` / the first reported level (previously this jumped
    // straight to `first()`, which made unrecognized flagship ids
    // default to `low`).
    let supported = |cand: Option<String>| cand.filter(|c| effort_levels.contains(c));
    let default_effort = supported(known.as_ref().and_then(|k| k.default_effort.clone()))
        .or_else(|| supported(inferred.default_effort.clone()))
        .or_else(|| supported(Some("high".into())))
        .or_else(|| effort_levels.first().cloned());
    let prompt_injected_effort_levels = known
        .as_ref()
        .map(|k| k.prompt_injected_effort_levels.clone())
        .unwrap_or_else(|| inferred.prompt_injected_effort_levels.clone());
    // Context-window options. A pinned id (`claude-fable-5[1m]`) is
    // normalized to the base id, and its pinned window becomes the
    // picker default — so it offers the same `[200k, 1m]` toggle as a
    // bare flagship/Sonnet id rather than being silently fixed. The
    // launch path re-applies the suffix from the chosen window via
    // `resolve_claude_api_model_id`, so the base id round-trips.
    let context_window_options = with_pinned_default(
        known
            .as_ref()
            .map(|k| k.context_window_options.clone())
            .unwrap_or_else(|| inferred.context_window_options.clone()),
        pinned.as_deref(),
    );

    let label = if sdk.display_name.trim().is_empty() {
        base_id.clone()
    } else {
        sdk.display_name
    };
    // Description precedence:
    //   1. SDK verbatim — the deployed CLI packs the resolved version +
    //      blurb into `description` (exactly what the terminal `/model`
    //      picker renders), so prefer it untouched.
    //   2. Maintained blurb for a known full id — the row's label already
    //      carries the version (e.g. "Claude Opus 4.8"), so the blurb
    //      stands alone with no version duplication.
    //   3. Alias backfill — an alias row (default/opus/fable/sonnet/haiku)
    //      the SDK gave no description for gets a version-bearing
    //      description synthesized from its canonical maintained entry, so
    //      the picker still shows what the alias actually resolves to.
    let description = if !sdk.description.trim().is_empty() {
        Some(sdk.description)
    } else if let Some(known_desc) = known.as_ref().and_then(|k| k.description.clone()) {
        Some(known_desc)
    } else {
        canonical_id_for_alias(&base_id)
            .and_then(|cid| maintained.get(cid))
            .and_then(alias_description_from_canonical)
    };

    ChatModelInfo {
        id: base_id,
        label,
        description,
        effort_levels,
        default_effort,
        prompt_injected_effort_levels,
        context_window_options,
        supports_adaptive_thinking: sdk.supports_adaptive_thinking.unwrap_or_else(|| {
            known
                .as_ref()
                .map(|k| k.supports_adaptive_thinking)
                .unwrap_or(inferred.supports_adaptive_thinking)
        }),
        supports_thinking_toggle: known
            .as_ref()
            .map(|k| k.supports_thinking_toggle)
            .unwrap_or(inferred.supports_thinking_toggle),
        // Fast mode is deliberately NOT surfaced for Claude, even when the
        // SDK reports `supportsFastMode` (observed on Opus 5, 2026-07).
        // The SDK flag is a capability advertisement, not an entitlement
        // check: on accounts without Extra Usage the server silently
        // serves `usage.speed: "standard"` while the UI claims Fast — a
        // pill that overpromises. Re-enabling requires closing that loop
        // first (pick-time `accountInfo` entitlement gate + post-turn
        // `usage.speed` heal) — see
        // docs/research/opus-5-agent-chat-support.md.
        supports_fast_mode: false,
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
        assert!(ids.contains(&"claude-fable-5"));
        assert!(ids.contains(&"claude-opus-4-7"));
        assert!(ids.contains(&"claude-sonnet-4-6"));
        assert!(ids.contains(&"claude-haiku-4-5"));
        assert_eq!(caps.effort_granularity, EffortGranularity::PerSession);
    }

    #[test]
    fn maintained_list_includes_fable_5() {
        let caps = claude_fallback_capabilities();
        let fable = caps
            .models
            .iter()
            .find(|m| m.id == "claude-fable-5")
            .expect("Fable 5 should be present in the maintained list");
        assert_eq!(fable.default_effort.as_deref(), Some("high"));
        assert!(fable.supports_adaptive_thinking);
        assert!(
            fable
                .prompt_injected_effort_levels
                .contains(&"ultrathink".to_string())
        );
        assert!(
            fable.context_window_options.iter().any(|o| o.value == "1m"),
            "bare Fable id should offer the 1M context window"
        );
        // The recommended default stays Opus 4.8 — `models()[0]` feeds
        // `defaultModelId` on the frontend when capabilities are served
        // from the fallback bundle.
        assert_eq!(caps.models[0].id, "claude-opus-4-8");
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
        assert_eq!(opus.default_effort.as_deref(), Some("high"));
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
    fn resolve_api_model_id_never_double_appends_a_pinned_suffix() {
        // The deployed CLI reports ids with the window already pinned
        // (`claude-fable-5[1m]`) — resolving `"1m"` against one must be
        // a no-op, not `claude-fable-5[1m][1m]`.
        assert_eq!(
            resolve_claude_api_model_id("claude-fable-5[1m]", Some("1m")),
            "claude-fable-5[1m]"
        );
    }

    #[test]
    fn split_context_suffix_handles_pinned_and_bare_ids() {
        assert_eq!(
            split_context_suffix("claude-fable-5[1m]"),
            ("claude-fable-5", Some("[1m]"))
        );
        assert_eq!(split_context_suffix("claude-opus-4-8"), ("claude-opus-4-8", None));
        // Unterminated bracket — treated as part of the id, not a suffix.
        assert_eq!(split_context_suffix("weird[id"), ("weird[id", None));
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
    fn maintained_effort_vocab_matches_the_deployed_cli() {
        // The headless CLI accepts exactly low..max as `--effort`
        // values (`ultracode` is TUI-slider-only and is rejected with
        // a warning as of claude 2.1.170). Maintained flagship entries
        // must never list a level the CLI would refuse at launch.
        let caps = claude_fallback_capabilities();
        for id in ["claude-fable-5", "claude-opus-4-8", "claude-opus-4-7"] {
            let m = caps
                .models
                .iter()
                .find(|m| m.id == id)
                .unwrap_or_else(|| panic!("{id} should be in the maintained list"));
            assert_eq!(
                m.effort_levels,
                vec!["low", "medium", "high", "xhigh", "max"],
                "{id} effort vocabulary must mirror the deployed CLI"
            );
            assert_eq!(
                m.default_effort.as_deref(),
                Some("high"),
                "{id} default effort must match the CLI's `(default)` marker"
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
        assert_eq!(opus48.default_effort.as_deref(), Some("high"));
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
        assert_eq!(ClaudeFamily::from_id("claude-fable-5"), ClaudeFamily::Fable);
        assert_eq!(ClaudeFamily::from_id("claude-opus-9-0"), ClaudeFamily::Opus);
        assert_eq!(ClaudeFamily::from_id("claude-sonnet-5-0"), ClaudeFamily::Sonnet);
        assert_eq!(ClaudeFamily::from_id("claude-haiku-5-0"), ClaudeFamily::Haiku);
        assert_eq!(ClaudeFamily::from_id("default"), ClaudeFamily::DefaultAlias);
        assert_eq!(ClaudeFamily::from_id("xyzzy-thing"), ClaudeFamily::Other);
        // Case-insensitive — Anthropic ids are lowercase but be defensive.
        assert_eq!(ClaudeFamily::from_id("Claude-OPUS-9"), ClaudeFamily::Opus);
        // A pinned context suffix doesn't hide the family.
        assert_eq!(
            ClaudeFamily::from_id("claude-fable-5[1m]"),
            ClaudeFamily::Fable
        );
    }

    #[test]
    fn infer_opus_gets_full_effort_and_1m() {
        let info = infer_model_info("claude-opus-9-9", "Claude Opus 9.9");
        assert_eq!(
            info.effort_levels,
            vec!["low", "medium", "high", "xhigh", "max"]
        );
        assert_eq!(info.default_effort.as_deref(), Some("high"));
        assert!(
            info.prompt_injected_effort_levels
                .contains(&"ultrathink".to_string())
        );
        assert!(info.context_window_options.iter().any(|o| o.value == "1m"));
        assert!(info.supports_adaptive_thinking);
        assert_eq!(info.label, "Claude Opus 9.9");
    }

    #[test]
    fn infer_fable_is_flagship_tier() {
        let info = infer_model_info("claude-fable-6", "Claude Fable 6");
        assert_eq!(info.default_effort.as_deref(), Some("high"));
        assert!(info.supports_adaptive_thinking);
        assert!(
            info.prompt_injected_effort_levels
                .contains(&"ultrathink".to_string())
        );
        assert!(info.context_window_options.iter().any(|o| o.value == "1m"));
    }

    #[test]
    fn infer_default_alias_is_flagship_without_context_picker() {
        // `default` is the CLI's recommended-flagship alias; its window
        // is the CLI's choice — `default[1m]` is not a valid id, so no
        // context-window picker may be offered.
        let info = infer_model_info("default", "Default (recommended)");
        assert_eq!(info.default_effort.as_deref(), Some("high"));
        assert!(info.supports_adaptive_thinking);
        assert!(info.context_window_options.is_empty());
    }

    #[test]
    fn infer_pinned_suffix_normalizes_id_and_keeps_a_consistent_picker() {
        // A pinned id (`claude-fable-5[1m]`) is normalized to the base
        // id and offers the SAME `[200k, 1m]` toggle as a bare flagship
        // id — with the pinned window (1m) as the default — rather than
        // being silently fixed. This is what makes Fable consistent with
        // Sonnet/Opus in the picker.
        let info = infer_model_info("claude-fable-5[1m]", "Fable 5");
        assert_eq!(info.id, "claude-fable-5", "id must be normalized to base");
        let values: Vec<&str> = info
            .context_window_options
            .iter()
            .map(|o| o.value.as_str())
            .collect();
        assert_eq!(values, vec!["200k", "1m"]);
        let default = info
            .context_window_options
            .iter()
            .find(|o| o.is_default)
            .unwrap();
        assert_eq!(default.value, "1m", "pinned window must be the default");
        assert_eq!(info.default_effort.as_deref(), Some("high"));
    }

    #[test]
    fn infer_pinned_non_default_window_becomes_the_picker_default() {
        // If the CLI ever pins a non-1m window, that window — not the
        // generic highest-is-default — drives the picker default.
        let info = infer_model_info("claude-opus-9-9[200k]", "Opus 9.9");
        assert_eq!(info.id, "claude-opus-9-9");
        let default = info
            .context_window_options
            .iter()
            .find(|o| o.is_default)
            .unwrap();
        assert_eq!(default.value, "200k");
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
        // Discriminate maintained-vs-inferred by the precise label: the
        // maintained entry is "Claude Opus 4.7", and the api
        // `display_name` we feed is deliberately different to prove
        // maintained metadata wins over the live payload.
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
        assert_eq!(
            new.effort_levels,
            vec!["low", "medium", "high", "xhigh", "max"]
        );
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
        // SDK reports an effort vocabulary with levels the maintained
        // Opus 4.7 entry doesn't list (`ultracode`, `newlevel`) — the
        // SDK value wins verbatim (regression guard for when the
        // deployed CLI ships a level the maintained list hasn't been
        // bumped for yet).
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
        // Maintained Opus 4.7 mirrors the deployed CLI: low..max.
        assert_eq!(
            opus.effort_levels,
            vec!["low", "medium", "high", "xhigh", "max"]
        );
    }

    #[test]
    fn sdk_merge_skips_empty_value_records() {
        let live = vec![
            sdk_model("", "", &["low"], None, None),
            sdk_model("claude-opus-4-7", "Claude Opus 4.7", &["low"], None, None),
        ];
        let caps = build_capabilities_from_sdk(live);
        // The empty record is dropped; the single live entry leads the
        // list (families the live roster lacks are appended after it).
        assert!(caps.models.iter().all(|m| !m.id.is_empty()));
        assert_eq!(caps.models[0].id, "claude-opus-4-7");
        assert_eq!(
            caps.models
                .iter()
                .filter(|m| ClaudeFamily::from_id(&m.id) == ClaudeFamily::Opus)
                .count(),
            1,
        );
    }

    #[test]
    fn sdk_merge_normalizes_pinned_fable_id_to_a_consistent_picker() {
        // The deployed CLI reports Fable with the window pinned into the
        // id (`claude-fable-5[1m]`). The merge normalizes it to the base
        // id `claude-fable-5` (looked up in the maintained map) and
        // offers the same `[200k, 1m]` context toggle as Sonnet/Opus,
        // with the pinned window (1m) as default. The launch path
        // re-applies `[1m]` from the chosen window, so the base id is
        // correct to store. This is the fix for the Fable-vs-Sonnet
        // picker inconsistency.
        let live = vec![sdk_model(
            "claude-fable-5[1m]",
            "Fable 5",
            &["low", "medium", "high", "xhigh", "max"],
            Some(true),
            None,
        )];
        let caps = build_capabilities_from_sdk(live);
        let fable = caps
            .models
            .iter()
            .find(|m| m.id == "claude-fable-5")
            .expect("pinned id must be normalized to base");
        assert_eq!(fable.label, "Fable 5");
        assert_eq!(fable.default_effort.as_deref(), Some("high"));
        let values: Vec<&str> = fable
            .context_window_options
            .iter()
            .map(|o| o.value.as_str())
            .collect();
        assert_eq!(values, vec!["200k", "1m"]);
        assert_eq!(
            fable
                .context_window_options
                .iter()
                .find(|o| o.is_default)
                .unwrap()
                .value,
            "1m",
        );
        assert!(
            fable
                .prompt_injected_effort_levels
                .contains(&"ultrathink".to_string())
        );
        assert!(fable.supports_adaptive_thinking);
        // No duplicate Fable row from the family-append pass.
        assert_eq!(
            caps.models.iter().filter(|m| m.id == "claude-fable-5").count(),
            1,
        );
    }

    #[test]
    fn sdk_merge_alias_ids_get_family_default_effort_not_first_level() {
        // Regression: alias ids (`default`, `sonnet`, `haiku`) miss the
        // full-id maintained map; the default effort used to fall back
        // to `effort_levels.first()` — i.e. "low" — for every alias.
        let live = vec![
            sdk_model(
                "default",
                "Default (recommended)",
                &["low", "medium", "high", "xhigh", "max"],
                Some(true),
                Some(true),
            ),
            sdk_model(
                "sonnet",
                "Sonnet",
                &["low", "medium", "high", "max"],
                Some(true),
                None,
            ),
            sdk_model("haiku", "Haiku", &[], None, None),
        ];
        let caps = build_capabilities_from_sdk(live);
        let default = caps.models.iter().find(|m| m.id == "default").unwrap();
        assert_eq!(default.default_effort.as_deref(), Some("high"));
        assert!(
            default.context_window_options.is_empty(),
            "`default` must not offer a context-window picker"
        );
        // Fast mode is clamped off for Claude even though the SDK
        // reported `supportsFastMode: Some(true)` for this row — the
        // advertisement is not an entitlement check (silent standard
        // fallback without Extra Usage), so the picker never shows it.
        assert!(!default.supports_fast_mode);
        let sonnet = caps.models.iter().find(|m| m.id == "sonnet").unwrap();
        assert_eq!(sonnet.default_effort.as_deref(), Some("high"));
        let haiku = caps.models.iter().find(|m| m.id == "haiku").unwrap();
        assert_eq!(haiku.default_effort, None);
        assert!(haiku.supports_thinking_toggle);
    }

    #[test]
    fn sdk_merge_default_effort_must_be_in_the_live_vocabulary() {
        // Maintained Opus default is `xhigh`; if the deployed CLI stops
        // reporting that level, the default must degrade to a level the
        // CLI actually accepts instead of erroring at launch.
        let live = vec![sdk_model(
            "claude-opus-4-8",
            "Claude Opus 4.8",
            &["low", "medium", "high"],
            Some(true),
            None,
        )];
        let caps = build_capabilities_from_sdk(live);
        assert_eq!(caps.models[0].default_effort.as_deref(), Some("high"));
    }

    #[test]
    fn sdk_merge_appends_opus_when_live_roster_omits_the_family() {
        // The deployed CLI's curated roster (default / fable / sonnet /
        // haiku) carries no Opus entry, but `--model claude-opus-4-8`
        // still launches. The maintained Opus entries must be appended
        // after the live entries so they stay selectable. The `default`
        // alias does not count as Opus — it tracks the user's own
        // configured default.
        let live = vec![
            sdk_model("default", "Default (recommended)", &["low", "medium", "high", "xhigh", "max"], Some(true), Some(true)),
            sdk_model("claude-fable-5[1m]", "Fable 5", &["low", "medium", "high", "xhigh", "max"], Some(true), None),
            sdk_model("sonnet", "Sonnet", &["low", "medium", "high", "max"], Some(true), None),
            sdk_model("haiku", "Haiku", &[], None, None),
        ];
        let caps = build_capabilities_from_sdk(live);
        let ids: Vec<&str> = caps.models.iter().map(|m| m.id.as_str()).collect();
        // Live entries first, in the CLI's order. The pinned Fable id is
        // normalized to its base id (`claude-fable-5`).
        assert_eq!(&ids[..4], &["default", "claude-fable-5", "sonnet", "haiku"]);
        // Every maintained Opus version appended afterwards.
        for opus in ["claude-opus-4-8", "claude-opus-4-7", "claude-opus-4-6", "claude-opus-4-5"] {
            assert!(ids.contains(&opus), "{opus} should be appended");
        }
        // Families already live must NOT be duplicated from the
        // maintained list (the maintained `claude-sonnet-4-6` /
        // `claude-haiku-4-5` ids stay out because the alias rows cover
        // those families).
        assert!(!ids.contains(&"claude-sonnet-4-6"));
        assert!(!ids.contains(&"claude-haiku-4-5"));
        // Appended Opus keeps its precise maintained metadata.
        let opus48 = caps.models.iter().find(|m| m.id == "claude-opus-4-8").unwrap();
        assert_eq!(opus48.label, "Claude Opus 4.8");
        assert_eq!(opus48.default_effort.as_deref(), Some("high"));
        assert!(opus48.context_window_options.iter().any(|o| o.value == "1m"));
    }

    #[test]
    fn sdk_merge_does_not_append_a_family_the_live_roster_already_has() {
        // If a future CLI lists Opus again (alias or full id), the
        // maintained Opus entries must not duplicate it.
        let live = vec![sdk_model("opus", "Opus", &["low", "medium", "high"], Some(true), None)];
        let caps = build_capabilities_from_sdk(live);
        let opus_rows = caps
            .models
            .iter()
            .filter(|m| ClaudeFamily::from_id(&m.id) == ClaudeFamily::Opus)
            .count();
        assert_eq!(opus_rows, 1, "live opus row must suppress maintained opus entries");
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

    // ── Description backfill (resolved-version blurbs) ────────────────

    #[test]
    fn every_maintained_model_has_a_description() {
        // Contract: no current Claude model may serve `description: None`
        // to the picker — the frontend renders it in the row subtitle.
        for model in models() {
            assert!(
                model.description.as_deref().is_some_and(|d| !d.trim().is_empty()),
                "{} must carry a non-empty description",
                model.id
            );
        }
    }

    #[test]
    fn sdk_merge_alias_rows_backfill_version_bearing_description() {
        // The deployed CLI reports its roster under alias ids with short
        // display names and — on some builds — no `description`. Each
        // alias row must backfill a resolved-version + blurb description
        // from its canonical maintained entry so the picker shows what
        // the alias actually runs. `default` resolves like `opus`.
        let live = vec![
            sdk_model(
                "default",
                "Default (recommended)",
                &["low", "medium", "high", "xhigh", "max"],
                Some(true),
                Some(true),
            ),
            sdk_model(
                "opus",
                "Opus",
                &["low", "medium", "high", "xhigh", "max"],
                Some(true),
                None,
            ),
            sdk_model(
                "fable",
                "Fable",
                &["low", "medium", "high", "xhigh", "max"],
                Some(true),
                None,
            ),
            sdk_model("sonnet", "Sonnet", &["low", "medium", "high"], Some(true), None),
            sdk_model("haiku", "Haiku", &[], None, None),
        ];
        let caps = build_capabilities_from_sdk(live);
        let desc = |id: &str| {
            caps.models
                .iter()
                .find(|m| m.id == id)
                .unwrap_or_else(|| panic!("{id} row must exist"))
                .description
                .clone()
                .unwrap_or_else(|| panic!("{id} row must carry a description"))
        };
        // `default` and `opus` both resolve to Opus 4.8, which defaults to
        // the 1M context window → the "with 1M context" qualifier.
        assert_eq!(
            desc("default"),
            "Opus 4.8 with 1M context · Best for everyday, complex tasks"
        );
        assert_eq!(
            desc("opus"),
            "Opus 4.8 with 1M context · Best for everyday, complex tasks"
        );
        // Fable/Sonnet also default to the 1M window in the maintained
        // table, so they carry the qualifier too.
        assert_eq!(
            desc("fable"),
            "Fable 5 with 1M context · Most capable for the hardest, longest-running tasks"
        );
        assert_eq!(desc("sonnet"), "Sonnet 4.6 with 1M context · Fast and capable");
        // Haiku has no context-window options → no qualifier, blurb only.
        assert_eq!(desc("haiku"), "Haiku 4.5 · Fastest and cheapest");
    }

    #[test]
    fn sdk_merge_full_id_rows_keep_blurb_only_description() {
        // A full-id row's label already carries the version
        // ("Claude Opus 4.8"), so its description must stay the bare
        // blurb — no version duplication.
        let live = vec![
            sdk_model(
                "claude-opus-4-8",
                "Claude Opus 4.8",
                &["low", "medium", "high", "xhigh", "max"],
                Some(true),
                None,
            ),
            sdk_model(
                "claude-opus-4-7",
                "Claude Opus 4.7",
                &["low", "medium", "high", "xhigh", "max"],
                Some(true),
                None,
            ),
        ];
        let caps = build_capabilities_from_sdk(live);
        let opus48 = caps.models.iter().find(|m| m.id == "claude-opus-4-8").unwrap();
        assert_eq!(
            opus48.description.as_deref(),
            Some("Best for everyday, complex tasks")
        );
        let opus47 = caps.models.iter().find(|m| m.id == "claude-opus-4-7").unwrap();
        assert_eq!(opus47.description.as_deref(), Some("Previous Opus generation"));
    }

    #[test]
    fn sdk_description_wins_verbatim_over_backfill() {
        // When the deployed CLI supplies its own description (the common
        // case — it packs the resolved version + blurb in there), that
        // string wins verbatim over any maintained / alias backfill.
        let sdk = SdkModelInfo {
            value: "opus".into(),
            display_name: "Opus".into(),
            description: "Opus 4.9 with 1M context · Freshest blurb from the CLI".into(),
            supported_effort_levels: vec!["low".into(), "high".into()],
            supports_adaptive_thinking: Some(true),
            supports_fast_mode: None,
        };
        let caps = build_capabilities_from_sdk(vec![sdk]);
        let opus = caps.models.iter().find(|m| m.id == "opus").unwrap();
        assert_eq!(
            opus.description.as_deref(),
            Some("Opus 4.9 with 1M context · Freshest blurb from the CLI")
        );
    }

    #[test]
    fn canonical_id_for_alias_maps_every_alias_and_ignores_full_ids() {
        assert_eq!(canonical_id_for_alias("default"), Some("claude-opus-4-8"));
        assert_eq!(canonical_id_for_alias("opus"), Some("claude-opus-4-8"));
        assert_eq!(canonical_id_for_alias("fable"), Some("claude-fable-5"));
        assert_eq!(canonical_id_for_alias("sonnet"), Some("claude-sonnet-4-6"));
        assert_eq!(canonical_id_for_alias("haiku"), Some("claude-haiku-4-5"));
        // Case-insensitive.
        assert_eq!(canonical_id_for_alias("OPUS"), Some("claude-opus-4-8"));
        // Full ids and unknown ids are not aliases.
        assert_eq!(canonical_id_for_alias("claude-opus-4-8"), None);
        assert_eq!(canonical_id_for_alias("mystery"), None);
        // Every alias must resolve to an id that exists in the maintained
        // table, so backfill can never point at a missing entry.
        let maintained: std::collections::HashSet<String> =
            models().into_iter().map(|m| m.id).collect();
        for (alias, canonical) in ALIAS_CANONICAL_IDS {
            assert!(
                maintained.contains(*canonical),
                "alias {alias} maps to unknown canonical id {canonical}",
            );
        }
    }
}
