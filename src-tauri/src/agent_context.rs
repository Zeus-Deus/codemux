/// Context string injected into CLI agent system prompts.
/// Update this constant when Codemux gains new capabilities that agents should know about.
pub const CODEMUX_AGENT_CONTEXT: &str = "\n\nYou are running inside Codemux, an Agentic Development Environment.\n\
DO NOT use system browsers, headless chromium, puppeteer, or grim.\n\
Use Codemux built-in browser commands instead:\n\
- codemux browser open <url>\n\
- codemux browser snapshot --dom\n\
- codemux browser click \"<selector>\"\n\
- codemux browser fill \"<selector>\" \"<text>\"\n\
- codemux browser screenshot\n\
The user sees the browser pane live. Run codemux --help for all commands.";

/// Transform a preset command to inject the Codemux agent context as a system prompt,
/// if the command targets a known CLI agent that supports system prompt injection.
///
/// The context is passed via the `$CODEMUX_AGENT_CONTEXT` env var (set on all PTY sessions).
/// Shell double-quote expansion handles multiline text correctly.
pub fn inject_agent_context(command: &str) -> String {
    let binary = command.split_whitespace().next().unwrap_or("");
    match binary {
        "claude" => {
            format!("{command} --system-prompt \"$CODEMUX_AGENT_CONTEXT\"")
        }
        // Other agents: extend as their system prompt flags are known.
        // "codex" | "gemini" | "opencode" => ...
        _ => command.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inject_claude_adds_system_prompt() {
        let result = inject_agent_context("claude --dangerously-skip-permissions");
        assert_eq!(
            result,
            "claude --dangerously-skip-permissions --system-prompt \"$CODEMUX_AGENT_CONTEXT\""
        );
    }

    #[test]
    fn inject_unknown_agent_unchanged() {
        let result = inject_agent_context("codex --full-auto");
        assert_eq!(result, "codex --full-auto");
    }

    #[test]
    fn inject_empty_command_unchanged() {
        let result = inject_agent_context("");
        assert_eq!(result, "");
    }

    #[test]
    fn inject_shell_command_unchanged() {
        let result = inject_agent_context("ls -la");
        assert_eq!(result, "ls -la");
    }

    #[test]
    fn context_constant_is_not_empty() {
        assert!(!CODEMUX_AGENT_CONTEXT.is_empty());
        assert!(CODEMUX_AGENT_CONTEXT.contains("Codemux"));
        assert!(CODEMUX_AGENT_CONTEXT.contains("codemux browser"));
    }

    #[test]
    fn inject_claude_with_p_flag() {
        let result = inject_agent_context("claude -p test");
        assert!(result.contains("--system-prompt"));
        assert!(result.starts_with("claude -p test"));
    }

    #[test]
    fn inject_claude_already_has_system_prompt() {
        // Presets don't have --system-prompt, but if somehow one does,
        // we still append (no dedup needed — double system prompts are fine).
        let result = inject_agent_context("claude --system-prompt \"existing\"");
        assert!(result.contains("$CODEMUX_AGENT_CONTEXT"));
    }
}
