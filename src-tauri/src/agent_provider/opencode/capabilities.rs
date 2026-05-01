//! Capability metadata for the OpenCode provider.
//!
//! Live model harvest landed in Stage 3:
//!
//! 1. [`opencode_stage1_placeholder`] returns an empty
//!    [`ProviderChatCapabilities`] — kept as the structural fallback
//!    for the "binary not installed / harvest failed" path.
//! 2. [`harvest_opencode_capabilities`] is the real Stage 3 entry
//!    point. It drives [`OpenCodeServerManager::ensure_running`] +
//!    [`OpenCodeClient::list_models`] and flattens the response
//!    into a `ChatModelInfo` list with `sub_provider` populated per
//!    upstream provider id. Errors surface verbatim so the frontend
//!    capabilities store can record them.
//! 3. [`flatten_into_chat_models`] is the pure transform between the
//!    Stage 2 wire shape (`Vec<OpenCodeProviderEntry>`) and the
//!    chat-side `ChatModelInfo` list. Connected-only filter applied
//!    here so the picker doesn't surface ~110 unauthenticated
//!    providers; Stage 6 can extend the helper to optionally
//!    return the disconnected list for a "configure more providers"
//!    hint.

use super::client::{
    OpenCodeClient, OpenCodeClientConfig, OpenCodeModel, OpenCodeProviderEntry,
};
use super::manager::OpenCodeServerManager;
use crate::agent_provider::types::{
    ChatModelInfo, EffortGranularity, ProviderChatCapabilities,
};

/// Empty capabilities bundle returned for `ProviderKind::OpenCode` when
/// the live harvest cannot complete (binary missing, server failed to
/// start, network error, etc.).
///
/// Effort granularity is `PerTurn` because OpenCode's per-call `variant`
/// field applies on every `session.promptAsync`, never restarting the
/// session — the choice here only affects whether the picker shows a
/// "restart on change" hint, and `PerTurn` is the friendlier default.
pub fn opencode_stage1_placeholder() -> ProviderChatCapabilities {
    ProviderChatCapabilities {
        models: Vec::new(),
        effort_granularity: EffortGranularity::PerTurn,
        effort_label_map: Default::default(),
        permission_modes: Vec::new(),
        default_permission_mode: None,
        permission_granularity: EffortGranularity::PerTurn,
    }
}

/// Drive the full Stage 3 harvest pipeline: spawn the OpenCode server
/// (idempotent), fetch the provider catalogue, flatten into the
/// chat-side model list. Returns `Err` (with a stable error string)
/// on every failure path so the frontend store can route them into
/// `opencodeError`.
///
/// Caching strategy: `manager.ensure_running()` is the spawn-once
/// boundary, so subsequent calls reuse the same child. Each
/// `harvest_opencode_capabilities()` call still issues a fresh
/// `GET /provider` round-trip — ~30 ms on the dev box, so the
/// frontend store calling it once per app session is negligible.
/// Stage 6 will add an explicit "refresh providers" button that
/// re-uses this same entry point.
pub async fn harvest_opencode_capabilities(
    manager: &OpenCodeServerManager,
) -> Result<ProviderChatCapabilities, String> {
    let handle = manager.ensure_running().await?;
    let mut config = OpenCodeClientConfig::new(handle.base_url);
    config.server_password = Some(handle.server_password);
    let client = OpenCodeClient::new(config)?;
    let providers = client.list_models().await?;
    Ok(build_capabilities(providers))
}

/// Pure constructor exposed for tests — wraps
/// [`flatten_into_chat_models`] in the surrounding bundle so the
/// effort/permission scaffolding stays in one place.
pub fn build_capabilities(
    providers: Vec<OpenCodeProviderEntry>,
) -> ProviderChatCapabilities {
    ProviderChatCapabilities {
        models: flatten_into_chat_models(&providers),
        effort_granularity: EffortGranularity::PerTurn,
        // No canonical effort vocabulary — variant slugs come straight
        // from each model's `variants` map and have no shared label
        // across providers (`low`/`medium`/`high` for OpenAI, but
        // `concise`/`balanced`/`thorough` for some others). Stage 4's
        // descriptor renderer falls back to title-casing the slug.
        effort_label_map: Default::default(),
        // OpenCode's permission system lives on each spawn-time
        // `permission` config option, NOT on the chat-side picker —
        // Stage 5/6 may surface it as a Codemux setting, but it is not
        // a per-turn knob the way Codex's sandbox modes are.
        permission_modes: Vec::new(),
        default_permission_mode: None,
        permission_granularity: EffortGranularity::PerTurn,
    }
}

/// Flatten the wire-format `Vec<OpenCodeProviderEntry>` into a
/// chat-side `ChatModelInfo` list.
///
/// Filters out providers with `connected: false` so the picker only
/// surfaces models whose upstream credentials are configured. Without
/// this filter the dev box's response (116 providers / ~4 354 models)
/// would overwhelm the picker; with it, the user sees only the
/// providers they've actually authenticated against (5 on the same
/// box).
///
/// Pure — extracted so unit tests can pin the transformation without
/// HTTP plumbing.
pub fn flatten_into_chat_models(
    providers: &[OpenCodeProviderEntry],
) -> Vec<ChatModelInfo> {
    let mut out = Vec::new();
    for provider in providers {
        if !provider.connected {
            continue;
        }
        for (model_id, model) in &provider.models {
            out.push(flatten_model(provider, model_id, model));
        }
    }
    out
}

/// Upstream provider id whose `cost: {input: 0, output: 0}` reliably
/// means "free to the user." Restricted because the cost field is
/// unreliable for every other upstream:
///
/// * User-supplied API-key providers (OpenAI / Anthropic / Google /
///   most of the long tail) — OpenCode doesn't track the user's
///   billing tier, so it reports cost as 0 even when the user pays
///   per-token to the upstream directly.
/// * Subscription-based providers (GitHub Copilot, ChatGPT
///   subscriptions) — flat billing, so per-token cost is always 0
///   even though the user pays a monthly fee.
/// * OpenRouter free models — the upstream already suffixes the
///   model name with `(free)`, so a redundant badge would just add
///   noise.
///
/// `opencode` upstream specifically maps to OpenCode Zen, OpenCode's
/// own hosted free-tier service. Cost-zero from that upstream IS
/// reliable.
const FREE_BADGE_PROVIDER_ID: &str = "opencode";

fn flatten_model(
    provider: &OpenCodeProviderEntry,
    slug_tail: &str,
    model: &OpenCodeModel,
) -> ChatModelInfo {
    ChatModelInfo {
        // Slug shape `${providerId}/${modelId}` — locked by the
        // research summary §3 so Stage 4's picker can deterministically
        // route a chosen model back to the right upstream.
        id: format!("{}/{}", provider.id, slug_tail),
        // Surface only the human-readable model name; the upstream
        // provider label rides in `sub_provider` so the picker can
        // render it as a secondary chip / rail group rather than
        // wedged into the primary label.
        label: if model.name.is_empty() {
            slug_tail.to_string()
        } else {
            model.name.clone()
        },
        description: model.description.clone(),
        effort_levels: model.variants.clone(),
        default_effort: model.variants.first().cloned(),
        prompt_injected_effort_levels: Vec::new(),
        context_window_options: Vec::new(),
        supports_adaptive_thinking: false,
        supports_thinking_toggle: false,
        // Fast-mode / vision flags don't ride on the OpenCode wire
        // shape we decode (they're under each model's
        // `capabilities.input.image` etc.). Stage 5/6 polish can plumb
        // those through `RawModel`; for Stage 3 the conservative
        // defaults match the Codex baseline.
        supports_fast_mode: false,
        supports_images: false,
        sub_provider: Some(provider.id.clone()),
        // Only OpenCode Zen's own free-tier signal makes it onto the
        // chat-side `is_free` flag. Every other provider's
        // cost-zero data is too noisy to act on (see
        // `FREE_BADGE_PROVIDER_ID` for the full list of false
        // positives — user creds, subscriptions, OpenRouter's
        // already-named `(free)` variants).
        is_free: model.is_free && provider.id == FREE_BADGE_PROVIDER_ID,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_provider(
        id: &str,
        name: &str,
        connected: bool,
        models: &[(&str, OpenCodeModel)],
    ) -> OpenCodeProviderEntry {
        let mut map = std::collections::BTreeMap::new();
        for (slug, model) in models {
            map.insert((*slug).to_string(), model.clone());
        }
        OpenCodeProviderEntry {
            id: id.to_string(),
            name: name.to_string(),
            connected,
            models: map,
        }
    }

    fn make_model(
        id: &str,
        name: &str,
        variants: &[&str],
        context_window: Option<u64>,
    ) -> OpenCodeModel {
        OpenCodeModel {
            id: id.to_string(),
            name: name.to_string(),
            description: Some("test-family".into()),
            variants: variants.iter().map(|s| (*s).to_string()).collect(),
            context_window,
            is_free: false,
        }
    }

    #[test]
    fn stage1_placeholder_is_empty_and_per_turn() {
        let caps = opencode_stage1_placeholder();
        assert!(caps.models.is_empty());
        assert!(caps.permission_modes.is_empty());
        assert_eq!(caps.default_permission_mode, None);
        assert!(matches!(
            caps.effort_granularity,
            EffortGranularity::PerTurn
        ));
        assert!(matches!(
            caps.permission_granularity,
            EffortGranularity::PerTurn
        ));
    }

    #[test]
    fn stage1_placeholder_serialises_round_trip() {
        let caps = opencode_stage1_placeholder();
        let json = serde_json::to_string(&caps).expect("serialises");
        let back: ProviderChatCapabilities =
            serde_json::from_str(&json).expect("round-trips");
        assert!(back.models.is_empty());
    }

    #[test]
    fn flatten_emits_namespaced_slug_with_sub_provider() {
        let openai = make_provider(
            "openai",
            "OpenAI",
            true,
            &[("gpt-5", make_model("gpt-5", "GPT-5", &["low", "medium", "high"], Some(200_000)))],
        );

        let models = flatten_into_chat_models(&[openai]);
        assert_eq!(models.len(), 1);
        let m = &models[0];
        // Pinned: slug shape is `${providerId}/${modelId}`. Changing
        // it silently breaks any downstream that compares slugs to
        // round-trip a model selection.
        assert_eq!(m.id, "openai/gpt-5");
        assert_eq!(m.label, "GPT-5");
        assert_eq!(m.sub_provider.as_deref(), Some("openai"));
        // `variants` is a `Vec<String>` so order is preserved from
        // the upstream wire payload (NOT alphabetical). Stage 4's
        // descriptor renderer iterates in this order.
        assert_eq!(m.effort_levels, vec!["low", "medium", "high"]);
        assert_eq!(m.default_effort.as_deref(), Some("low"));
    }

    #[test]
    fn flatten_skips_disconnected_providers() {
        // Connected provider's models surface; disconnected
        // provider's models do not. Pinned at the data layer so a
        // future picker rewrite that forgets the filter doesn't
        // suddenly dump 4k entries into the dropdown.
        let openai = make_provider(
            "openai",
            "OpenAI",
            true,
            &[("gpt-5", make_model("gpt-5", "GPT-5", &[], None))],
        );
        let unconfigured = make_provider(
            "anthropic",
            "Anthropic",
            false,
            &[("claude-sonnet-4-6", make_model("claude-sonnet-4-6", "Sonnet", &[], None))],
        );

        let models = flatten_into_chat_models(&[openai, unconfigured]);
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "openai/gpt-5");
    }

    #[test]
    fn flatten_handles_models_with_no_variants() {
        // Some providers report empty `variants` maps. Default effort
        // should fall through to None so the picker can suppress the
        // effort chip entirely (matches Claude's Haiku behaviour).
        let p = make_provider(
            "ollama",
            "Ollama",
            true,
            &[("llama3", make_model("llama3", "Llama 3", &[], None))],
        );
        let models = flatten_into_chat_models(&[p]);
        assert_eq!(models.len(), 1);
        assert!(models[0].effort_levels.is_empty());
        assert!(models[0].default_effort.is_none());
    }

    #[test]
    fn flatten_falls_back_to_slug_when_name_empty() {
        let p = make_provider(
            "openrouter",
            "OpenRouter",
            true,
            &[(
                "x-ai/grok-2",
                OpenCodeModel {
                    id: "x-ai/grok-2".into(),
                    name: String::new(),
                    description: None,
                    variants: Vec::new(),
                    context_window: None,
                    is_free: false,
                },
            )],
        );
        let models = flatten_into_chat_models(&[p]);
        // Model-id slug is preserved verbatim as the label fallback;
        // the outer slug wraps it once.
        assert_eq!(models[0].id, "openrouter/x-ai/grok-2");
        assert_eq!(models[0].label, "x-ai/grok-2");
    }

    #[test]
    fn flatten_carries_description_from_family() {
        let p = make_provider(
            "openai",
            "OpenAI",
            true,
            &[("gpt-5", make_model("gpt-5", "GPT-5", &[], None))],
        );
        let models = flatten_into_chat_models(&[p]);
        assert_eq!(models[0].description.as_deref(), Some("test-family"));
    }

    #[test]
    fn build_capabilities_wraps_flatten_with_per_turn_grants() {
        let p = make_provider(
            "openai",
            "OpenAI",
            true,
            &[("gpt-5", make_model("gpt-5", "GPT-5", &["medium"], None))],
        );
        let caps = build_capabilities(vec![p]);
        assert_eq!(caps.models.len(), 1);
        assert!(matches!(caps.effort_granularity, EffortGranularity::PerTurn));
        assert!(caps.permission_modes.is_empty());
        assert!(caps.default_permission_mode.is_none());
    }

    #[test]
    fn build_capabilities_with_no_connected_providers_yields_empty_models() {
        let p = make_provider(
            "openai",
            "OpenAI",
            false,
            &[("gpt-5", make_model("gpt-5", "GPT-5", &[], None))],
        );
        let caps = build_capabilities(vec![p]);
        assert!(caps.models.is_empty());
    }

    #[test]
    fn is_free_only_propagates_for_opencode_provider() {
        // The cost-zero signal is unreliable for every upstream
        // EXCEPT OpenCode Zen (`provider.id == "opencode"`). Pin
        // both the positive case (opencode + cost-zero → is_free
        // true) and three negative cases (other providers with
        // cost-zero → is_free false), so a future relaxation of
        // the gate has to delete this test on purpose.
        let opencode_zen = make_provider(
            "opencode",
            "OpenCode",
            true,
            &[(
                "free-thinker",
                OpenCodeModel {
                    id: "free-thinker".into(),
                    name: "Free Thinker".into(),
                    description: None,
                    variants: Vec::new(),
                    context_window: None,
                    is_free: true,
                },
            )],
        );
        let openrouter = make_provider(
            "openrouter",
            "OpenRouter",
            true,
            &[(
                "x-ai/grok-2:free",
                OpenCodeModel {
                    id: "x-ai/grok-2:free".into(),
                    name: "Grok 2 (free)".into(),
                    description: None,
                    variants: Vec::new(),
                    context_window: None,
                    is_free: true,
                },
            )],
        );
        let copilot = make_provider(
            "github-copilot",
            "GitHub Copilot",
            true,
            &[(
                "claude-haiku-4-5",
                OpenCodeModel {
                    id: "claude-haiku-4-5".into(),
                    name: "Claude Haiku 4.5".into(),
                    description: None,
                    variants: Vec::new(),
                    context_window: None,
                    // Copilot is subscription-billed; cost-zero is a
                    // false positive at the wire layer.
                    is_free: true,
                },
            )],
        );
        let openai_user_creds = make_provider(
            "openai",
            "OpenAI",
            true,
            &[(
                "gpt-5",
                OpenCodeModel {
                    id: "gpt-5".into(),
                    name: "GPT-5".into(),
                    description: None,
                    variants: Vec::new(),
                    context_window: None,
                    // User-supplied creds: OpenCode reports cost-zero
                    // because it doesn't track the user's billing
                    // tier. False positive.
                    is_free: true,
                },
            )],
        );

        let models = flatten_into_chat_models(&[
            opencode_zen,
            openrouter,
            copilot,
            openai_user_creds,
        ]);

        let by_id: std::collections::HashMap<_, _> =
            models.iter().map(|m| (m.id.as_str(), m)).collect();

        assert!(
            by_id["opencode/free-thinker"].is_free,
            "opencode upstream + cost-zero → is_free true"
        );
        assert!(
            !by_id["openrouter/x-ai/grok-2:free"].is_free,
            "openrouter cost-zero → is_free false (model name already \
             carries '(free)' suffix; badge would be redundant)"
        );
        assert!(
            !by_id["github-copilot/claude-haiku-4-5"].is_free,
            "github-copilot cost-zero → is_free false (subscription \
             billing makes cost-zero meaningless)"
        );
        assert!(
            !by_id["openai/gpt-5"].is_free,
            "openai cost-zero → is_free false (user-supplied creds \
             billed directly to user, OpenCode doesn't track tier)"
        );
    }

    #[test]
    fn flatten_preserves_alphabetical_order_within_provider() {
        // BTreeMap-driven iteration order is alphabetical by slug.
        // Pinned so a future API change can't quietly alter the
        // picker's display order.
        let p = make_provider(
            "openai",
            "OpenAI",
            true,
            &[
                ("gpt-3.5", make_model("gpt-3.5", "GPT-3.5", &[], None)),
                ("gpt-5", make_model("gpt-5", "GPT-5", &[], None)),
                ("gpt-4o", make_model("gpt-4o", "GPT-4o", &[], None)),
            ],
        );
        let models = flatten_into_chat_models(&[p]);
        let ids: Vec<_> = models.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, vec!["openai/gpt-3.5", "openai/gpt-4o", "openai/gpt-5"]);
    }
}
