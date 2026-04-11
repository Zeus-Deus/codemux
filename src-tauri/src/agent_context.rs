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

    let has_worktree = worktree_path.is_some();

    let mut info_lines = Vec::new();
    if let Some(name) = workspace_name {
        info_lines.push(format!("Your workspace: {name}"));
    }
    if let Some(path) = worktree_path {
        info_lines.push(format!("Your working directory: {path}"));
    }
    if let Some(b) = branch {
        info_lines.push(format!("Your branch: {b}"));
    }
    if let Some(root) = root_path {
        if has_worktree {
            info_lines.push(format!("Original repo (reference only): {root}"));
        } else {
            info_lines.push(format!("Project root: {root}"));
        }
    }
    if !info_lines.is_empty() {
        sections.push(info_lines.join("\n"));
    }

    let mut rules = Vec::new();
    if has_worktree {
        rules.push(
            "- Your working directory is your project root. \
             Run all build, test, and execute commands here.",
        );
        rules.push(
            "- Do NOT cd to the original repo path — it has a different branch checked out.",
        );
    }
    rules.push(
        "- Do NOT create additional git worktrees (no -w flag, no git worktree add). \
         Codemux manages worktree lifecycle.",
    );
    rules.push(
        "- Do NOT use system browsers, headless chromium, puppeteer, or grim. \
         Use Codemux browser commands instead.",
    );
    rules.push("- Use `codemux` CLI commands for workspace and browser operations.");
    sections.push(format!("Rules:\n{}", rules.join("\n")));

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
///
/// Gemini CLI has no CLI flag — it reads `GEMINI_SYSTEM_MD` pointing to a file. For Gemini,
/// we prefix the command with an inline write that dumps `$CODEMUX_AGENT_CONTEXT` to a temp
/// file and sets the env var, so the file is only created when Gemini actually launches.
pub fn inject_agent_context(command: &str, workspace_id: &str) -> String {
    let binary = command.split_whitespace().next().unwrap_or("");
    match binary {
        "claude" => {
            format!("{command} --system-prompt \"$CODEMUX_AGENT_CONTEXT\"")
        }
        "codex" => {
            format!("{command} -c instructions=\"$CODEMUX_AGENT_CONTEXT\"")
        }
        "pi" => {
            format!("{command} --append-system-prompt \"$CODEMUX_AGENT_CONTEXT\"")
        }
        "gemini" => {
            let path = format!("/tmp/codemux-{workspace_id}-gemini-system.md");
            format!(
                "printf '%s' \"$CODEMUX_AGENT_CONTEXT\" > {path} && GEMINI_SYSTEM_MD={path} {command}"
            )
        }
        // OpenCode: no CLI injection mechanism available.
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
        assert!(ctx.contains("Your working directory: /home/user/.codemux/worktrees/repo/my-feature"));
        assert!(ctx.contains("Your branch: feat/my-feature"));
        assert!(ctx.contains("Original repo (reference only): /home/user/projects/repo"));
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
        assert!(!ctx.contains("Your working directory:"));
        assert!(ctx.contains("Your branch: main"));
        assert!(ctx.contains("Project root: /home/user/projects/repo"));
    }

    #[test]
    fn build_context_omits_missing_branch() {
        let ctx = build_agent_context(Some("ws"), None, None, Some("/root"));
        assert!(ctx.contains("Your workspace: ws"));
        assert!(!ctx.contains("Your branch:"));
        assert!(ctx.contains("Project root: /root"));
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
    fn build_context_worktree_has_do_not_cd_rule() {
        let ctx = build_agent_context(
            Some("my-feature"),
            Some("/home/user/.codemux/worktrees/repo/my-feature"),
            Some("feat/my-feature"),
            Some("/home/user/projects/repo"),
        );
        assert!(ctx.contains("Do NOT cd to the original repo path"));
        assert!(ctx.contains("Your working directory is your project root"));
    }

    #[test]
    fn build_context_no_worktree_omits_do_not_cd_rule() {
        let ctx = build_agent_context(
            Some("main"),
            None,
            Some("main"),
            Some("/home/user/projects/repo"),
        );
        assert!(!ctx.contains("Do NOT cd to the original repo path"));
        assert!(!ctx.contains("Your working directory is your project root"));
        assert!(!ctx.contains("Original repo (reference only)"));
    }

    #[test]
    fn inject_claude_adds_system_prompt() {
        let result = inject_agent_context("claude --dangerously-skip-permissions", "ws-1");
        assert_eq!(
            result,
            "claude --dangerously-skip-permissions --system-prompt \"$CODEMUX_AGENT_CONTEXT\""
        );
    }

    #[test]
    fn inject_unknown_agent_unchanged() {
        let result = inject_agent_context("vim main.rs", "ws-1");
        assert_eq!(result, "vim main.rs");
    }

    #[test]
    fn inject_empty_command_unchanged() {
        let result = inject_agent_context("", "ws-1");
        assert_eq!(result, "");
    }

    #[test]
    fn inject_shell_command_unchanged() {
        let result = inject_agent_context("ls -la", "ws-1");
        assert_eq!(result, "ls -la");
    }

    #[test]
    fn inject_claude_with_p_flag() {
        let result = inject_agent_context("claude -p test", "ws-1");
        assert!(result.contains("--system-prompt"));
        assert!(result.starts_with("claude -p test"));
    }

    #[test]
    fn inject_claude_already_has_system_prompt() {
        // Presets don't have --system-prompt, but if somehow one does,
        // we still append (no dedup needed — double system prompts are fine).
        let result = inject_agent_context("claude --system-prompt \"existing\"", "ws-1");
        assert!(result.contains("$CODEMUX_AGENT_CONTEXT"));
    }

    #[test]
    fn inject_codex_adds_instructions() {
        let result = inject_agent_context("codex --full-auto", "ws-1");
        assert_eq!(
            result,
            "codex --full-auto -c instructions=\"$CODEMUX_AGENT_CONTEXT\""
        );
    }

    #[test]
    fn inject_pi_adds_append_system_prompt() {
        let result = inject_agent_context("pi", "ws-1");
        assert_eq!(
            result,
            "pi --append-system-prompt \"$CODEMUX_AGENT_CONTEXT\""
        );
    }

    #[test]
    fn inject_pi_with_flags() {
        let result = inject_agent_context("pi --model sonnet", "ws-1");
        assert!(result.starts_with("pi --model sonnet"));
        assert!(result.contains("--append-system-prompt"));
    }

    #[test]
    fn inject_gemini_writes_file_and_sets_env() {
        let result = inject_agent_context("gemini --yolo", "test-ws-gemini");
        // Should prefix with inline file write + env var, then the original command
        assert!(result.contains("GEMINI_SYSTEM_MD=/tmp/codemux-test-ws-gemini-gemini-system.md"));
        assert!(result.ends_with("gemini --yolo"));
        assert!(result.contains("$CODEMUX_AGENT_CONTEXT"));
    }

    #[test]
    fn inject_opencode_unchanged() {
        let result = inject_agent_context("opencode", "ws-1");
        assert_eq!(result, "opencode");
    }
}
