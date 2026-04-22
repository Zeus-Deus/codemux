//! Chat-side capabilities for the Codex provider.
//!
//! MVP ships fallback data only. Codex's `codex app-server` exposes a
//! `model/list` RPC that reports each model's `supportedReasoningEfforts`
//! (T3Code calls it at startup per their `CodexProvider.ts:183-199`) —
//! wiring that up is deferred to a follow-up and would replace the
//! fallback at runtime via the same command surface.

use std::collections::HashMap;

use crate::agent_provider::{
    ChatModelInfo, EffortGranularity, PermissionModeOption, ProviderChatCapabilities,
};

/// Canonical labels for Codex reasoning-effort strings.
fn codex_effort_label_map() -> HashMap<String, String> {
    let pairs = [
        ("none", "None"),
        ("minimal", "Minimal"),
        ("low", "Low"),
        ("medium", "Medium"),
        ("high", "High"),
        ("xhigh", "Extra High"),
    ];
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// Hardcoded Codex model list. Keep in rough parity with
/// `src-tauri/src/commands/openflow.rs::codex_default_models` and
/// T3Code's `packages/contracts/src/model.ts` roster — the canonical
/// GPT-5 family plus `codex-mini-latest`.
fn models() -> Vec<ChatModelInfo> {
    vec![
        ChatModelInfo {
            id: "gpt-5.4".into(),
            label: "GPT-5.4 (Codex)".into(),
            description: Some("Default Codex model".into()),
            effort_levels: vec![
                "minimal".into(),
                "low".into(),
                "medium".into(),
                "high".into(),
                "xhigh".into(),
            ],
            default_effort: Some("medium".into()),
            prompt_injected_effort_levels: vec![],
            context_window_options: vec![],
            supports_adaptive_thinking: false,
            supports_thinking_toggle: false,
            supports_fast_mode: true,
        },
        ChatModelInfo {
            id: "gpt-5.4-mini".into(),
            label: "GPT-5.4 Mini".into(),
            description: Some("Faster, cheaper Codex model".into()),
            effort_levels: vec!["low".into(), "medium".into(), "high".into()],
            default_effort: Some("medium".into()),
            prompt_injected_effort_levels: vec![],
            context_window_options: vec![],
            supports_adaptive_thinking: false,
            supports_thinking_toggle: false,
            supports_fast_mode: true,
        },
        ChatModelInfo {
            id: "gpt-5.3-codex".into(),
            label: "GPT-5.3 Codex".into(),
            description: None,
            effort_levels: vec!["low".into(), "medium".into(), "high".into()],
            default_effort: Some("medium".into()),
            prompt_injected_effort_levels: vec![],
            context_window_options: vec![],
            supports_adaptive_thinking: false,
            supports_thinking_toggle: false,
            supports_fast_mode: false,
        },
        ChatModelInfo {
            id: "codex-mini-latest".into(),
            label: "codex-mini-latest".into(),
            description: None,
            effort_levels: vec![],
            default_effort: None,
            prompt_injected_effort_levels: vec![],
            context_window_options: vec![],
            supports_adaptive_thinking: false,
            supports_thinking_toggle: false,
            supports_fast_mode: false,
        },
    ]
}

/// Codex sandbox-policy names used as `permission_mode` values. The
/// Codex CLI / RPC expose three sandbox policies (`read-only`,
/// `workspace-write`, `danger-full-access`) paired with approval
/// policies (`untrusted`, `on-request`, `never`). T3Code treats the
/// pair as a single logical "runtime mode"
/// (`apps/server/src/provider/Layers/CodexSessionRuntime.ts:237-258`);
/// we follow the same approach. The Rust Codex adapter translates the
/// mode to both `approvalPolicy` + `sandbox` / `sandboxPolicy` on the
/// RPC side.
fn codex_permission_modes() -> Vec<PermissionModeOption> {
    vec![
        PermissionModeOption {
            value: "read-only".into(),
            label: "Read only".into(),
            description: "Allow reads, block writes and commands.".into(),
            is_default: false,
        },
        PermissionModeOption {
            value: "workspace-write".into(),
            label: "Workspace write".into(),
            description: "Allow edits within the workspace, block commands.".into(),
            is_default: false,
        },
        PermissionModeOption {
            value: "danger-full-access".into(),
            label: "Full access".into(),
            description: "Allow commands and edits without prompts.".into(),
            is_default: true,
        },
    ]
}

pub fn codex_fallback_capabilities() -> ProviderChatCapabilities {
    ProviderChatCapabilities {
        models: models(),
        effort_granularity: EffortGranularity::PerTurn,
        effort_label_map: codex_effort_label_map(),
        permission_modes: codex_permission_modes(),
        default_permission_mode: Some("danger-full-access".into()),
        // Codex's RPC accepts permission params on both `thread/start`
        // and `turn/start`. Codemux's adapter wires them at session
        // start only for MVP — mode changes trigger a silent restart
        // (same pattern as Claude). Per-turn override on `turn/start`
        // (`sandboxPolicy`) is a follow-up.
        permission_granularity: EffortGranularity::PerSession,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_is_per_turn_and_has_no_context_window_or_ultrathink() {
        let caps = codex_fallback_capabilities();
        assert_eq!(caps.effort_granularity, EffortGranularity::PerTurn);
        for model in &caps.models {
            assert!(
                model.context_window_options.is_empty(),
                "Codex model {} must not expose a context window picker",
                model.id
            );
            assert!(
                model.prompt_injected_effort_levels.is_empty(),
                "Codex model {} must not expose ultrathink",
                model.id
            );
        }
    }

    #[test]
    fn gpt_54_is_present_as_default() {
        let caps = codex_fallback_capabilities();
        assert_eq!(caps.models.first().map(|m| m.id.as_str()), Some("gpt-5.4"));
    }
}
