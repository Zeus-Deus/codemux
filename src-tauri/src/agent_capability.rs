//! Launch-time model selection for CLI agent presets.
//!
//! The New Workspace dialog lets the user pick a model (and, where the
//! CLI supports it, a reasoning effort) *before* the agent launches.
//! This module is the backend half: it detects which agent family a
//! preset command belongs to and splices the corresponding flags into
//! the command string.
//!
//! # Design
//!
//! Detection keys off the command's first real binary token, so a
//! brand-new preset that launches an already-modeled CLI (`claude`,
//! `codex`, `opencode`, `gemini`) picks up model selection with zero
//! extra wiring. A preset whose binary is unknown is returned
//! unchanged — the dialog simply hides the model pill for it.
//!
//! # Flag matrix (verified against the installed CLIs)
//!
//! | Family   | Model flag                 | Reasoning flag                       |
//! |----------|----------------------------|--------------------------------------|
//! | Claude   | `--model <id>`             | `--effort <low..max>`                |
//! | Codex    | `--model <id>`             | `-c model_reasoning_effort=<id>`     |
//! | OpenCode | `--model <provider/model>` | — (TUI exposes none)                 |
//! | Gemini   | `--model <id>`             | — (CLI exposes none)                 |
//!
//! Injection happens on the raw preset command *before*
//! [`crate::agent_context::inject_agent_context`] wraps it, so the
//! Gemini env-prefix wrapper and the Claude/Codex `--system-prompt`
//! suffix all compose correctly.

use serde::{Deserialize, Serialize};

/// The user's launch-time model / reasoning / context choice.
///
/// Every field is optional: `None` (or empty) means "use the agent's
/// own default" and emits no flag — so a workspace created without
/// touching the picker behaves exactly as it did before this feature.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelSelection {
    /// Model identifier to pass via `--model`. Provider-native shape
    /// (`sonnet`, `gpt-5.4`, `anthropic/claude-sonnet-4-6`, ...).
    #[serde(default)]
    pub model: Option<String>,
    /// Reasoning / effort level. Interpreted per family — Claude's
    /// `--effort` levels, Codex's `model_reasoning_effort` values.
    #[serde(default)]
    pub reasoning: Option<String>,
    /// Context-window selection. Claude-only: `"1m"` encodes the 1M
    /// window as a `[1m]` suffix on the model id (the same trick the
    /// agent-chat SDK path uses); any other value emits no change.
    /// Has no effect without an explicit `model`.
    #[serde(default)]
    pub context: Option<String>,
}

impl ModelSelection {
    /// True when no field carries a usable value — the no-op case.
    pub fn is_empty(&self) -> bool {
        field_value(&self.model).is_none()
            && field_value(&self.reasoning).is_none()
            && field_value(&self.context).is_none()
    }
}

/// A CLI agent family Codemux knows how to inject model flags for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentFamily {
    Claude,
    Codex,
    OpenCode,
    Gemini,
}

impl AgentFamily {
    /// Stable lowercase id, matching the frontend `LaunchFamily` union
    /// and (for Claude/Codex/OpenCode) `AgentChatProviderKind`.
    pub fn as_str(self) -> &'static str {
        match self {
            AgentFamily::Claude => "claude",
            AgentFamily::Codex => "codex",
            AgentFamily::OpenCode => "opencode",
            AgentFamily::Gemini => "gemini",
        }
    }
}

/// Trim a preset field down to a usable value, or `None` when blank.
fn field_value(field: &Option<String>) -> Option<&str> {
    field
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
}

/// Pull the binary name out of a shell command: skips leading
/// `VAR=value` env assignments and strips any directory path, so
/// `/usr/bin/claude`, `FOO=bar claude`, and `claude` all resolve to
/// `claude`.
fn binary_token(command: &str) -> Option<&str> {
    for tok in command.split_whitespace() {
        // Leading env assignment (`KEY=val`) — keep scanning.
        if !tok.starts_with('-') && tok.contains('=') {
            continue;
        }
        let base = tok.rsplit(['/', '\\']).next().unwrap_or(tok);
        return Some(base);
    }
    None
}

/// Detect the agent family a preset command launches, or `None` when
/// the binary is not one Codemux models.
pub fn detect_family(command: &str) -> Option<AgentFamily> {
    match binary_token(command)? {
        "claude" => Some(AgentFamily::Claude),
        "codex" => Some(AgentFamily::Codex),
        "opencode" => Some(AgentFamily::OpenCode),
        "gemini" => Some(AgentFamily::Gemini),
        _ => None,
    }
}

/// Only inject values composed of identifier-safe characters. Model ids
/// and effort levels are always in this set; rejecting anything else
/// keeps the spliced command shell-safe on every platform without
/// per-shell quoting.
fn is_safe_arg(value: &str) -> bool {
    !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/' | ':'))
}

/// Model ids may legitimately carry a bracket context suffix — the
/// deployed Claude CLI reports e.g. `claude-fable-5[1m]`. Validate the
/// base id with the ordinary charset and require the suffix interior
/// to be plain alphanumerics. Bracketed values are single-quoted at
/// injection time, so the glob metacharacters never reach the shell
/// unquoted.
fn is_safe_model_arg(value: &str) -> bool {
    match value.find('[') {
        Some(open) if value.ends_with(']') => {
            let inner = &value[open + 1..value.len() - 1];
            is_safe_arg(&value[..open])
                && !inner.is_empty()
                && inner.chars().all(|c| c.is_ascii_alphanumeric())
        }
        _ => is_safe_arg(value),
    }
}

/// Drop a `--flag <value>` / `--flag=<value>` / `<short> <value>` pair
/// from a token list, wherever it appears. Used so an explicit user
/// selection cleanly overrides a flag a preset already bakes in,
/// instead of producing a duplicate flag.
fn strip_flag(tokens: &mut Vec<String>, long: &str, short: Option<&str>) {
    let eq_prefix = format!("{long}=");
    let mut i = 0;
    while i < tokens.len() {
        let tok = &tokens[i];
        if tok == long || short.is_some_and(|s| tok == s) {
            tokens.remove(i); // the flag
            if i < tokens.len() {
                tokens.remove(i); // its value
            }
        } else if tok.starts_with(&eq_prefix) {
            tokens.remove(i);
        } else {
            i += 1;
        }
    }
}

/// Drop a baked-in `-c model_reasoning_effort=<value>` Codex override
/// (and the `-c` / `--config` flag preceding it).
fn strip_codex_reasoning(tokens: &mut Vec<String>) {
    let mut i = 0;
    while i < tokens.len() {
        if tokens[i].starts_with("model_reasoning_effort=") {
            tokens.remove(i);
            if i > 0 && (tokens[i - 1] == "-c" || tokens[i - 1] == "--config") {
                tokens.remove(i - 1);
                // Both the flag and its value are gone — step back so the
                // token that followed the pair is not skipped.
                i -= 1;
            }
        } else {
            i += 1;
        }
    }
}

/// Splice the user's model / reasoning selection into `command` for its
/// detected agent family.
///
/// Returns the command unchanged when the family is unknown, the
/// selection is empty, or a value fails the [`is_safe_arg`] check. When
/// a field *is* selected it wins: any flag the preset already bakes in
/// is stripped first, so the result never carries a duplicate.
pub fn apply_model_selection(command: &str, selection: Option<&ModelSelection>) -> String {
    let selection = match selection {
        Some(sel) if !sel.is_empty() => sel,
        _ => return command.to_string(),
    };
    let family = match detect_family(command) {
        Some(family) => family,
        None => return command.to_string(),
    };

    let mut tokens: Vec<String> = command.split_whitespace().map(String::from).collect();
    let mut changed = false;

    // Model — every family accepts `--model`.
    if let Some(model) = field_value(&selection.model) {
        if is_safe_model_arg(model) {
            strip_flag(&mut tokens, "--model", Some("-m"));
            // Claude's 1M context window rides on the model id as a
            // `[1m]` bracket suffix (mirrors the agent-chat SDK path).
            // Context applies to Claude only and needs an explicit
            // model to attach to. Delegated to the shared resolver so
            // ids the CLI reports with the window already pinned
            // (`claude-fable-5[1m]`) never grow a second suffix.
            let model_arg = if family == AgentFamily::Claude {
                crate::agent_provider::claude::capabilities::resolve_claude_api_model_id(
                    model,
                    field_value(&selection.context),
                )
            } else {
                model.to_string()
            };
            tokens.push("--model".into());
            // The `[1m]` bracket is a shell glob metacharacter — quote
            // it. Plain identifiers stay unquoted to keep commands tidy.
            tokens.push(if model_arg.contains('[') {
                format!("'{model_arg}'")
            } else {
                model_arg
            });
            changed = true;
        }
    }

    // Reasoning — family-specific; silently skipped for families whose
    // CLI exposes no reasoning flag (OpenCode TUI, Gemini).
    if let Some(reasoning) = field_value(&selection.reasoning) {
        if is_safe_arg(reasoning) {
            match family {
                AgentFamily::Claude => {
                    strip_flag(&mut tokens, "--effort", None);
                    tokens.push("--effort".into());
                    tokens.push(reasoning.to_string());
                    changed = true;
                }
                AgentFamily::Codex => {
                    strip_codex_reasoning(&mut tokens);
                    tokens.push("-c".into());
                    tokens.push(format!("model_reasoning_effort={reasoning}"));
                    changed = true;
                }
                AgentFamily::OpenCode | AgentFamily::Gemini => {}
            }
        }
    }

    if changed {
        tokens.join(" ")
    } else {
        // Nothing safe to inject — return the original verbatim rather
        // than a whitespace-normalised rejoin.
        command.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sel(model: Option<&str>, reasoning: Option<&str>) -> ModelSelection {
        ModelSelection {
            model: model.map(String::from),
            reasoning: reasoning.map(String::from),
            context: None,
        }
    }

    fn sel_ctx(
        model: Option<&str>,
        reasoning: Option<&str>,
        context: Option<&str>,
    ) -> ModelSelection {
        ModelSelection {
            model: model.map(String::from),
            reasoning: reasoning.map(String::from),
            context: context.map(String::from),
        }
    }

    #[test]
    fn detects_each_known_family() {
        assert_eq!(
            detect_family("claude --dangerously-skip-permissions"),
            Some(AgentFamily::Claude)
        );
        assert_eq!(detect_family("codex --full-auto"), Some(AgentFamily::Codex));
        assert_eq!(detect_family("opencode"), Some(AgentFamily::OpenCode));
        assert_eq!(detect_family("gemini --yolo"), Some(AgentFamily::Gemini));
    }

    #[test]
    fn detects_unknown_family_as_none() {
        assert_eq!(detect_family("pi"), None);
        assert_eq!(detect_family("npx some-agent"), None);
        assert_eq!(detect_family(""), None);
    }

    #[test]
    fn detection_strips_path_and_env_prefix() {
        assert_eq!(
            detect_family("/usr/local/bin/claude --foo"),
            Some(AgentFamily::Claude)
        );
        assert_eq!(
            detect_family("FOO=bar BAZ=qux codex --full-auto"),
            Some(AgentFamily::Codex)
        );
    }

    #[test]
    fn empty_selection_is_a_no_op() {
        let cmd = "claude --dangerously-skip-permissions";
        assert_eq!(apply_model_selection(cmd, None), cmd);
        assert_eq!(
            apply_model_selection(cmd, Some(&ModelSelection::default())),
            cmd
        );
        assert_eq!(
            apply_model_selection(cmd, Some(&sel(Some("  "), Some("")))),
            cmd
        );
    }

    #[test]
    fn unknown_family_passes_through_unchanged() {
        let cmd = "pi";
        assert_eq!(
            apply_model_selection(cmd, Some(&sel(Some("some-model"), None))),
            cmd
        );
    }

    #[test]
    fn claude_injects_model_and_effort() {
        let out = apply_model_selection(
            "claude --dangerously-skip-permissions",
            Some(&sel(Some("opus"), Some("high"))),
        );
        assert_eq!(
            out,
            "claude --dangerously-skip-permissions --model opus --effort high"
        );
    }

    #[test]
    fn codex_injects_model_and_config_reasoning() {
        let out = apply_model_selection(
            "codex --full-auto",
            Some(&sel(Some("gpt-5.4"), Some("xhigh"))),
        );
        assert_eq!(
            out,
            "codex --full-auto --model gpt-5.4 -c model_reasoning_effort=xhigh"
        );
    }

    #[test]
    fn opencode_injects_slug_model_and_skips_reasoning() {
        let out = apply_model_selection(
            "opencode",
            Some(&sel(Some("anthropic/claude-sonnet-4-6"), Some("high"))),
        );
        // Reasoning is dropped — the OpenCode TUI has no reasoning flag.
        assert_eq!(out, "opencode --model anthropic/claude-sonnet-4-6");
    }

    #[test]
    fn gemini_injects_model_and_skips_reasoning() {
        let out = apply_model_selection(
            "gemini --yolo",
            Some(&sel(Some("gemini-2.5-pro"), Some("high"))),
        );
        assert_eq!(out, "gemini --yolo --model gemini-2.5-pro");
    }

    #[test]
    fn model_only_selection() {
        let out =
            apply_model_selection("claude --foo", Some(&sel(Some("sonnet"), None)));
        assert_eq!(out, "claude --foo --model sonnet");
    }

    #[test]
    fn reasoning_only_selection() {
        let out = apply_model_selection("claude --foo", Some(&sel(None, Some("max"))));
        assert_eq!(out, "claude --foo --effort max");
    }

    #[test]
    fn explicit_model_overrides_baked_in_model() {
        // Preset bakes `--model opus`; the user's pick wins, no duplicate.
        let out = apply_model_selection(
            "claude --model opus --dangerously-skip-permissions",
            Some(&sel(Some("sonnet"), Some("high"))),
        );
        assert_eq!(
            out,
            "claude --dangerously-skip-permissions --model sonnet --effort high"
        );
    }

    #[test]
    fn explicit_model_overrides_baked_short_flag() {
        let out = apply_model_selection(
            "codex -m gpt-5.4 --full-auto",
            Some(&sel(Some("gpt-other"), None)),
        );
        assert_eq!(out, "codex --full-auto --model gpt-other");
    }

    #[test]
    fn explicit_model_overrides_eq_form() {
        let out = apply_model_selection(
            "claude --model=opus",
            Some(&sel(Some("sonnet"), None)),
        );
        assert_eq!(out, "claude --model sonnet");
    }

    #[test]
    fn explicit_effort_overrides_baked_effort() {
        let out =
            apply_model_selection("claude --effort low", Some(&sel(None, Some("high"))));
        assert_eq!(out, "claude --effort high");
    }

    #[test]
    fn explicit_reasoning_overrides_baked_codex_reasoning() {
        let out = apply_model_selection(
            "codex -c model_reasoning_effort=low --dangerously-bypass-approvals-and-sandbox",
            Some(&sel(None, Some("high"))),
        );
        assert_eq!(
            out,
            "codex --dangerously-bypass-approvals-and-sandbox -c model_reasoning_effort=high"
        );
    }

    #[test]
    fn strips_every_baked_codex_reasoning_override() {
        // Two baked overrides — both must be stripped, not just the
        // first (regression guard for the strip loop-index step-back).
        let out = apply_model_selection(
            "codex -c model_reasoning_effort=low -c model_reasoning_effort=medium",
            Some(&sel(None, Some("high"))),
        );
        assert_eq!(out, "codex -c model_reasoning_effort=high");
    }

    #[test]
    fn untouched_baked_model_survives_when_selection_empty() {
        // No selection → the preset's own baked flags are left intact.
        let cmd = "claude --model opus --effort max";
        assert_eq!(apply_model_selection(cmd, None), cmd);
    }

    #[test]
    fn rejects_unsafe_values() {
        // A value with shell metacharacters is dropped, not quoted.
        let out = apply_model_selection(
            "claude --foo",
            Some(&sel(Some("opus; rm -rf /"), Some("high$(whoami)"))),
        );
        assert_eq!(out, "claude --foo");
    }

    #[test]
    fn claude_1m_context_appends_quoted_bracket_suffix() {
        let out = apply_model_selection(
            "claude --dangerously-skip-permissions",
            Some(&sel_ctx(Some("claude-sonnet-4-6"), None, Some("1m"))),
        );
        assert_eq!(
            out,
            "claude --dangerously-skip-permissions --model 'claude-sonnet-4-6[1m]'"
        );
    }

    #[test]
    fn safe_model_arg_accepts_pinned_suffix_but_rejects_malformed_brackets() {
        assert!(is_safe_model_arg("claude-fable-5[1m]"));
        assert!(is_safe_model_arg("claude-opus-4-8"));
        assert!(!is_safe_model_arg("claude[1m")); // unterminated
        assert!(!is_safe_model_arg("claude[]")); // empty suffix
        assert!(!is_safe_model_arg("claude[$(rm)]")); // unsafe interior
        assert!(!is_safe_model_arg("[1m]")); // no base id
    }

    #[test]
    fn claude_pinned_model_id_never_double_appends_1m() {
        // The deployed CLI reports some models with the window pinned
        // into the id itself — the injector must not append again.
        let out = apply_model_selection(
            "claude",
            Some(&sel_ctx(Some("claude-fable-5[1m]"), None, Some("1m"))),
        );
        assert_eq!(out, "claude --model 'claude-fable-5[1m]'");
    }

    #[test]
    fn claude_1m_context_combines_with_effort() {
        let out = apply_model_selection(
            "claude",
            Some(&sel_ctx(Some("claude-opus-4-7"), Some("high"), Some("1m"))),
        );
        assert_eq!(
            out,
            "claude --model 'claude-opus-4-7[1m]' --effort high"
        );
    }

    #[test]
    fn non_1m_context_leaves_model_id_bare() {
        let out = apply_model_selection(
            "claude",
            Some(&sel_ctx(Some("claude-sonnet-4-6"), None, Some("200k"))),
        );
        assert_eq!(out, "claude --model claude-sonnet-4-6");
    }

    #[test]
    fn context_without_model_is_a_no_op() {
        let cmd = "claude --dangerously-skip-permissions";
        assert_eq!(
            apply_model_selection(cmd, Some(&sel_ctx(None, None, Some("1m")))),
            cmd
        );
    }

    #[test]
    fn context_is_ignored_for_non_claude_families() {
        // Codex has no 1M context concept — the `[1m]` suffix must not
        // leak onto a non-Claude model id.
        let out = apply_model_selection(
            "codex --full-auto",
            Some(&sel_ctx(Some("gpt-5.4"), None, Some("1m"))),
        );
        assert_eq!(out, "codex --full-auto --model gpt-5.4");
    }

    #[test]
    fn family_as_str_round_trips() {
        assert_eq!(AgentFamily::Claude.as_str(), "claude");
        assert_eq!(AgentFamily::Codex.as_str(), "codex");
        assert_eq!(AgentFamily::OpenCode.as_str(), "opencode");
        assert_eq!(AgentFamily::Gemini.as_str(), "gemini");
    }
}
