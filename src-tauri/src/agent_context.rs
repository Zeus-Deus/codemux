/// Build the agent context string with actual workspace information.
///
/// When workspace info is available, includes worktree-specific guidance.
/// When a value is `None`, that line is omitted rather than showing an empty value.
/// When `worktree_path` is `None` (workspace opened directly, not as a worktree),
/// the "Your worktree" line is omitted but all other guidance remains.
pub fn build_agent_context(
    workspace_name: Option<&str>,
    worktree_path: Option<&str>,
    branch: Option<&str>,
    root_path: Option<&str>,
) -> String {
    let mut sections = Vec::new();

    sections.push(
        "You are inside Codemux, an Agentic Development Environment \
         that manages isolated git worktrees for parallel agent work."
            .to_string(),
    );

    let mut info_lines = Vec::new();
    if let Some(name) = workspace_name {
        info_lines.push(format!("Your workspace: {name}"));
    }
    if let Some(path) = worktree_path {
        info_lines.push(format!("Your worktree: {path}"));
    }
    if let Some(b) = branch {
        info_lines.push(format!("Your branch: {b}"));
    }
    if let Some(root) = root_path {
        info_lines.push(format!("Main repo root: {root}"));
    }
    if !info_lines.is_empty() {
        sections.push(info_lines.join("\n"));
    }

    sections.push(
        "Rules:\n\
         - Do NOT create additional git worktrees (no -w flag, no git worktree add). \
         Codemux manages worktree lifecycle.\n\
         - Do NOT use system browsers, headless chromium, puppeteer, or grim. \
         Use Codemux browser commands instead.\n\
         - Use `codemux` CLI commands for workspace and browser operations."
            .to_string(),
    );

    sections.push(
        "Available browser commands:\n\
         - codemux browser open <url>\n\
         - codemux browser snapshot --dom\n\
         - codemux browser click \"<selector>\"\n\
         - codemux browser fill \"<selector>\" \"<text>\"\n\
         - codemux browser screenshot\n\
         The user sees the browser pane live. Run codemux --help for all commands."
            .to_string(),
    );

    sections.join("\n\n")
}

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
    fn build_context_with_all_info() {
        let ctx = build_agent_context(
            Some("my-feature"),
            Some("/home/user/.codemux/worktrees/repo/my-feature"),
            Some("feat/my-feature"),
            Some("/home/user/projects/repo"),
        );
        assert!(ctx.contains("Your workspace: my-feature"));
        assert!(ctx.contains("Your worktree: /home/user/.codemux/worktrees/repo/my-feature"));
        assert!(ctx.contains("Your branch: feat/my-feature"));
        assert!(ctx.contains("Main repo root: /home/user/projects/repo"));
        assert!(ctx.contains("codemux browser"));
        assert!(ctx.contains("Do NOT create additional git worktrees"));
    }

    #[test]
    fn build_context_with_no_workspace_info() {
        let ctx = build_agent_context(None, None, None, None);
        assert!(ctx.contains("Codemux"));
        assert!(ctx.contains("codemux browser"));
        assert!(ctx.contains("Do NOT create additional git worktrees"));
        assert!(!ctx.contains("Your workspace:"));
        assert!(!ctx.contains("Your worktree:"));
        assert!(!ctx.contains("Your branch:"));
        assert!(!ctx.contains("Main repo root:"));
    }

    #[test]
    fn build_context_without_worktree() {
        let ctx = build_agent_context(
            Some("main"),
            None,
            Some("main"),
            Some("/home/user/projects/repo"),
        );
        assert!(ctx.contains("Your workspace: main"));
        assert!(!ctx.contains("Your worktree:"));
        assert!(ctx.contains("Your branch: main"));
        assert!(ctx.contains("Main repo root: /home/user/projects/repo"));
    }

    #[test]
    fn build_context_omits_missing_branch() {
        let ctx = build_agent_context(Some("ws"), None, None, Some("/root"));
        assert!(ctx.contains("Your workspace: ws"));
        assert!(!ctx.contains("Your branch:"));
        assert!(ctx.contains("Main repo root: /root"));
    }

    #[test]
    fn build_context_always_has_browser_commands() {
        let ctx = build_agent_context(None, None, None, None);
        assert!(ctx.contains("codemux browser open <url>"));
        assert!(ctx.contains("codemux browser snapshot --dom"));
        assert!(ctx.contains("codemux browser click"));
        assert!(ctx.contains("codemux browser fill"));
        assert!(ctx.contains("codemux browser screenshot"));
    }

    #[test]
    fn build_context_always_has_rules() {
        let ctx = build_agent_context(None, None, None, None);
        assert!(ctx.contains("Do NOT create additional git worktrees"));
        assert!(ctx.contains("Do NOT use system browsers"));
    }

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
