//! Hand-maintained chat-side capability data for Claude models.
//!
//! The Claude Agent SDK exposes a live `supportedModels()` API, but it
//! doesn't report `promptInjectedEffortLevels` or `contextWindowOptions`
//! — those are product decisions rather than SDK metadata. We therefore
//! keep a hand-maintained per-model map and, when live data arrives,
//! merge the SDK's model list with our hand-maintained extras by id.
//!
//! Mirrors T3Code's `BUILT_IN_MODELS` table
//! (`apps/server/src/provider/Layers/ClaudeProvider.ts:48-141`).

use std::collections::HashMap;

use crate::agent_provider::{
    ChatModelInfo, ContextWindowOption, EffortGranularity, PermissionModeOption,
    ProviderChatCapabilities,
};

fn ctx_200k_default() -> ContextWindowOption {
    ContextWindowOption {
        value: "200k".into(),
        label: "200k".into(),
        is_default: true,
    }
}

fn ctx_1m() -> ContextWindowOption {
    ContextWindowOption {
        value: "1m".into(),
        label: "1M".into(),
        is_default: false,
    }
}

fn claude_effort_label_map() -> HashMap<String, String> {
    let pairs = [
        ("low", "Low"),
        ("medium", "Medium"),
        ("high", "High"),
        ("xhigh", "Extra High"),
        ("max", "Max"),
        ("ultrathink", "Ultrathink"),
    ];
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn models() -> Vec<ChatModelInfo> {
    vec![
        // Opus 4.7 — flagship, effort defaults to xhigh, supports ultrathink + 1M.
        ChatModelInfo {
            id: "claude-opus-4-7".into(),
            label: "Claude Opus 4.7".into(),
            description: Some("Strongest Claude model".into()),
            effort_levels: vec![
                "low".into(),
                "medium".into(),
                "high".into(),
                "xhigh".into(),
                "max".into(),
            ],
            default_effort: Some("xhigh".into()),
            prompt_injected_effort_levels: vec!["ultrathink".into()],
            context_window_options: vec![ctx_200k_default(), ctx_1m()],
            supports_adaptive_thinking: true,
            supports_thinking_toggle: false,
            supports_fast_mode: false,
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
            context_window_options: vec![ctx_200k_default(), ctx_1m()],
            supports_adaptive_thinking: true,
            supports_thinking_toggle: false,
            supports_fast_mode: true,
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
        },
        // Sonnet 4.6 — narrower effort range, supports ultrathink + 1M.
        ChatModelInfo {
            id: "claude-sonnet-4-6".into(),
            label: "Claude Sonnet 4.6".into(),
            description: Some("Fast and capable".into()),
            effort_levels: vec!["low".into(), "medium".into(), "high".into()],
            default_effort: Some("high".into()),
            prompt_injected_effort_levels: vec!["ultrathink".into()],
            context_window_options: vec![ctx_200k_default(), ctx_1m()],
            supports_adaptive_thinking: false,
            supports_thinking_toggle: false,
            supports_fast_mode: false,
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

/// Apply T3Code's `resolveClaudeApiModelId` trick: when context window is
/// `"1m"`, the Anthropic API expects the model id to carry a `[1m]`
/// bracket suffix. Any other value (or `None`) returns the id unchanged.
pub fn resolve_claude_api_model_id(
    model_id: &str,
    context_window: Option<&str>,
) -> String {
    match context_window {
        Some("1m") => format!("{model_id}[1m]"),
        _ => model_id.to_string(),
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
        assert_eq!(default_ctx.value, "200k");
        assert_eq!(opus.default_effort.as_deref(), Some("xhigh"));
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
    fn label_map_covers_all_effort_levels() {
        let caps = claude_fallback_capabilities();
        assert_eq!(caps.effort_label_map.get("xhigh").map(String::as_str), Some("Extra High"));
        assert_eq!(caps.effort_label_map.get("ultrathink").map(String::as_str), Some("Ultrathink"));
    }
}
