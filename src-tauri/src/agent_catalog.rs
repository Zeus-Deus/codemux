//! Agent catalog — metadata that drives the structured preset editor.
//!
//! A "structured" preset (see `TerminalPreset::agent_config`) lets a user
//! build a named launcher — e.g. "Git Pull" → `claude --model opus "pull
//! latest and resolve conflicts"` — by picking an agent, an optional model,
//! an optional reasoning level, and a prompt, rather than typing a raw shell
//! command. This module is the single source of truth for the per-agent
//! metadata that editor needs:
//!
//! - the binary to invoke and its icon,
//! - the full-autonomy / skip-permissions flag (when one exists),
//! - whether (and how) the agent takes a `--model` flag, plus curated
//!   model suggestions,
//! - how a reasoning level is expressed (a real CLI flag for agents that
//!   have one, otherwise a prompt prefix that is always safe to inject).
//!
//! The catalog is exposed to the frontend via the `list_agent_catalog`
//! Tauri command. The actual command-string assembly lives in the frontend
//! (`src/lib/presets/agent-command.ts`) so the editor can render a live
//! preview without a round-trip; this module only supplies the metadata it
//! assembles from. The assembled string is stored in
//! `TerminalPreset::commands` and launched by the existing `apply_preset`
//! pipeline unchanged — `agent_config` is persisted alongside purely so the
//! editor can round-trip the structured fields.
//!
//! Coverage is honest, not aspirational: model suggestions and reasoning
//! options are only provided where there is a verified mechanism. Agents
//! without a known model alias still expose a free-text model field
//! (`accepts_model = true`); agents without a known reasoning mechanism
//! expose no reasoning field (`reasoning = None`).

use serde::{Deserialize, Serialize};

/// A curated, editable model suggestion for an agent's model field.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentModelOption {
    /// The value passed to the model flag (e.g. `opus`).
    pub value: String,
    /// Human label shown in the picker (e.g. `Opus`).
    pub label: String,
}

/// One selectable reasoning level for an agent.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentReasoningOption {
    /// Value substituted into [`AgentReasoning::flag_template`] when the
    /// agent has a real reasoning flag. Ignored (may be empty) for
    /// prompt-prefix agents. The empty string denotes the "Default /
    /// none" option and always emits nothing.
    pub value: String,
    /// Human label shown in the picker.
    pub label: String,
    /// When the agent has no real reasoning flag, this string is
    /// prepended to the preset prompt. `None` (or the Default option)
    /// means "no effect".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_prefix: Option<String>,
}

/// How an agent expresses a reasoning / effort level.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentReasoning {
    /// Real CLI flag template with a `{value}` placeholder, e.g.
    /// `-c model_reasoning_effort="{value}"`. When `Some`, the selected
    /// option's `value` is substituted and the result appended to the
    /// command. When `None`, reasoning is applied via the selected
    /// option's `prompt_prefix` instead — which can never break the
    /// command, since it only changes prompt text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flag_template: Option<String>,
    /// Selectable levels. The first should be the "Default / none" option.
    pub options: Vec<AgentReasoningOption>,
}

/// Static, per-agent metadata for the structured preset editor.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentCatalogEntry {
    /// Stable id stored in `AgentConfig::agent_id` (e.g. `claude`).
    pub id: String,
    /// Display label (e.g. `Claude Code`).
    pub label: String,
    /// Icon name understood by the frontend `PresetIcon` component.
    pub icon: String,
    /// The binary to invoke (e.g. `claude`).
    pub binary: String,
    /// Skip-permissions / full-autonomy flag, appended when the preset's
    /// `skip_permissions` is true. `None` for agents that have no such
    /// flag (e.g. `droid`, whose autonomy is a settings file injected at
    /// launch by `agent_context::inject_agent_context`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub autonomy_flag: Option<String>,
    /// Whether the agent accepts a model flag. When false the editor
    /// hides the model field.
    pub accepts_model: bool,
    /// The flag used to pass the model (default `--model`).
    pub model_flag: String,
    /// Curated, editable model suggestions. Empty means "free-text only".
    #[serde(default)]
    pub models: Vec<AgentModelOption>,
    /// Reasoning support. `None` hides the reasoning field entirely.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<AgentReasoning>,
    /// Whether a free-form prompt can be passed as a positional argument.
    pub supports_prompt: bool,
}

/// Default model flag for agents that take `--model`.
const MODEL_FLAG: &str = "--model";

fn model(value: &str, label: &str) -> AgentModelOption {
    AgentModelOption {
        value: value.into(),
        label: label.into(),
    }
}

fn reasoning_default() -> AgentReasoningOption {
    AgentReasoningOption {
        value: String::new(),
        label: "Default".into(),
        prompt_prefix: None,
    }
}

fn prompt_reasoning(value: &str, label: &str, prefix: &str) -> AgentReasoningOption {
    AgentReasoningOption {
        value: value.into(),
        label: label.into(),
        prompt_prefix: Some(prefix.into()),
    }
}

fn flag_reasoning(value: &str, label: &str) -> AgentReasoningOption {
    AgentReasoningOption {
        value: value.into(),
        label: label.into(),
        prompt_prefix: None,
    }
}

/// The full agent catalog. Mirrors the built-in presets' binaries and
/// autonomy flags (see `presets::builtin_presets`) so structured presets
/// behave identically to the built-ins, plus the model/reasoning metadata
/// the editor needs.
pub fn agent_catalog() -> Vec<AgentCatalogEntry> {
    vec![
        // Claude Code. Stable CLI model aliases: opus / sonnet / haiku.
        // The CLI has no numeric reasoning flag — its real lever is the
        // "think" / "ultrathink" prompt keywords, so reasoning is applied
        // as a prompt prefix.
        AgentCatalogEntry {
            id: "claude".into(),
            label: "Claude Code".into(),
            icon: "claude".into(),
            binary: "claude".into(),
            autonomy_flag: Some("--dangerously-skip-permissions".into()),
            accepts_model: true,
            model_flag: MODEL_FLAG.into(),
            models: vec![
                model("opus", "Opus"),
                model("sonnet", "Sonnet"),
                model("haiku", "Haiku"),
            ],
            reasoning: Some(AgentReasoning {
                flag_template: None,
                options: vec![
                    reasoning_default(),
                    prompt_reasoning("think", "Think", "Think step by step. "),
                    prompt_reasoning("think-hard", "Think hard", "Think hard about this. "),
                    prompt_reasoning("ultrathink", "Ultrathink", "Ultrathink. "),
                ],
            }),
            supports_prompt: true,
        },
        // OpenAI Codex. Reasoning effort is a real config override
        // (`-c model_reasoning_effort="..."`). Model values vary, so the
        // field is free-text.
        AgentCatalogEntry {
            id: "codex".into(),
            label: "Codex".into(),
            icon: "codex".into(),
            binary: "codex".into(),
            autonomy_flag: Some("--full-auto".into()),
            accepts_model: true,
            model_flag: MODEL_FLAG.into(),
            models: vec![],
            reasoning: Some(AgentReasoning {
                flag_template: Some("-c model_reasoning_effort=\"{value}\"".into()),
                options: vec![
                    reasoning_default(),
                    flag_reasoning("low", "Low"),
                    flag_reasoning("medium", "Medium"),
                    flag_reasoning("high", "High"),
                ],
            }),
            supports_prompt: true,
        },
        // OpenCode. Models are provider/model strings that vary widely, so
        // free-text. Reasoning is applied as a prompt prefix.
        AgentCatalogEntry {
            id: "opencode".into(),
            label: "OpenCode".into(),
            icon: "opencode".into(),
            binary: "opencode".into(),
            autonomy_flag: None,
            accepts_model: true,
            model_flag: MODEL_FLAG.into(),
            models: vec![],
            reasoning: Some(AgentReasoning {
                flag_template: None,
                options: vec![
                    reasoning_default(),
                    prompt_reasoning("think-hard", "Think hard", "Think hard about this. "),
                    prompt_reasoning(
                        "think-deeply",
                        "Think deeply",
                        "Think deeply and reason thoroughly before responding. ",
                    ),
                ],
            }),
            supports_prompt: true,
        },
        // Gemini CLI. `--yolo` is the full-autonomy flag.
        AgentCatalogEntry {
            id: "gemini".into(),
            label: "Gemini".into(),
            icon: "gemini".into(),
            binary: "gemini".into(),
            autonomy_flag: Some("--yolo".into()),
            accepts_model: true,
            model_flag: MODEL_FLAG.into(),
            models: vec![
                model("gemini-2.5-pro", "2.5 Pro"),
                model("gemini-2.5-flash", "2.5 Flash"),
            ],
            reasoning: None,
            supports_prompt: true,
        },
        AgentCatalogEntry {
            id: "antigravity".into(),
            label: "Antigravity".into(),
            icon: "antigravity".into(),
            binary: "agy".into(),
            autonomy_flag: Some("--dangerously-skip-permissions".into()),
            accepts_model: true,
            model_flag: MODEL_FLAG.into(),
            models: vec![],
            reasoning: None,
            supports_prompt: true,
        },
        AgentCatalogEntry {
            id: "copilot".into(),
            label: "Copilot".into(),
            icon: "copilot".into(),
            binary: "copilot".into(),
            autonomy_flag: Some("--allow-all".into()),
            accepts_model: true,
            model_flag: MODEL_FLAG.into(),
            models: vec![],
            reasoning: None,
            supports_prompt: true,
        },
        AgentCatalogEntry {
            id: "cursor-agent".into(),
            label: "Cursor Agent".into(),
            icon: "cursor-agent".into(),
            binary: "cursor-agent".into(),
            autonomy_flag: Some("--yolo".into()),
            accepts_model: true,
            model_flag: MODEL_FLAG.into(),
            models: vec![],
            reasoning: None,
            supports_prompt: true,
        },
        AgentCatalogEntry {
            id: "amp".into(),
            label: "Amp".into(),
            icon: "amp".into(),
            binary: "amp".into(),
            autonomy_flag: Some("--dangerously-allow-all".into()),
            accepts_model: true,
            model_flag: MODEL_FLAG.into(),
            models: vec![],
            reasoning: None,
            supports_prompt: true,
        },
        AgentCatalogEntry {
            id: "grok".into(),
            label: "Grok".into(),
            icon: "grok".into(),
            binary: "grok".into(),
            autonomy_flag: Some("--always-approve".into()),
            accepts_model: true,
            model_flag: MODEL_FLAG.into(),
            models: vec![],
            reasoning: None,
            supports_prompt: true,
        },
        // Factory Droid. Interactive `droid` has no skip-permissions flag;
        // autonomy is injected as a settings file at launch (see
        // `agent_context`). So there is no autonomy flag to append here.
        AgentCatalogEntry {
            id: "droid".into(),
            label: "Droid".into(),
            icon: "factory".into(),
            binary: "droid".into(),
            autonomy_flag: None,
            accepts_model: true,
            model_flag: MODEL_FLAG.into(),
            models: vec![],
            reasoning: None,
            supports_prompt: true,
        },
        AgentCatalogEntry {
            id: "mastracode".into(),
            label: "Mastracode".into(),
            icon: "mastracode".into(),
            binary: "mastracode".into(),
            autonomy_flag: None,
            accepts_model: true,
            model_flag: MODEL_FLAG.into(),
            models: vec![],
            reasoning: None,
            supports_prompt: true,
        },
        AgentCatalogEntry {
            id: "pi".into(),
            label: "Pi".into(),
            icon: "pi".into(),
            binary: "pi".into(),
            autonomy_flag: None,
            accepts_model: true,
            model_flag: MODEL_FLAG.into(),
            models: vec![],
            reasoning: None,
            supports_prompt: true,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_ids_are_unique() {
        let catalog = agent_catalog();
        let mut ids: Vec<&str> = catalog.iter().map(|e| e.id.as_str()).collect();
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), catalog.len(), "agent catalog ids must be unique");
    }

    #[test]
    fn every_entry_has_a_binary_and_icon() {
        for e in agent_catalog() {
            assert!(!e.binary.is_empty(), "{} missing binary", e.id);
            assert!(!e.icon.is_empty(), "{} missing icon", e.id);
            assert!(!e.label.is_empty(), "{} missing label", e.id);
        }
    }

    #[test]
    fn flag_reasoning_templates_contain_value_placeholder() {
        for e in agent_catalog() {
            if let Some(reasoning) = &e.reasoning {
                if let Some(tmpl) = &reasoning.flag_template {
                    assert!(
                        tmpl.contains("{value}"),
                        "{} reasoning flag_template must contain {{value}}",
                        e.id
                    );
                }
            }
        }
    }

    #[test]
    fn reasoning_first_option_is_default_noop() {
        for e in agent_catalog() {
            if let Some(reasoning) = &e.reasoning {
                let first = &reasoning.options[0];
                assert!(
                    first.value.is_empty() && first.prompt_prefix.is_none(),
                    "{} first reasoning option must be the no-op Default",
                    e.id
                );
            }
        }
    }

    #[test]
    fn known_chat_agents_are_present() {
        let catalog = agent_catalog();
        for id in ["claude", "codex", "opencode"] {
            assert!(
                catalog.iter().any(|e| e.id == id),
                "expected chat-capable agent {id} in catalog"
            );
        }
    }
}
