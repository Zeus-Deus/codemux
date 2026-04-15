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
/// The context is passed via the `CODEMUX_AGENT_CONTEXT` env var (set on all PTY sessions).
/// The injected command string expands that env var at shell parse time — the **expansion
/// syntax is platform-specific**:
///
/// - **Unix** (bash/zsh/sh/fish): `"$CODEMUX_AGENT_CONTEXT"` — standard POSIX shell expansion
///   inside double quotes. Works under every mainstream Unix shell.
/// - **Windows** (PowerShell 5.1+ / PowerShell 7+): `"$env:CODEMUX_AGENT_CONTEXT"` — PowerShell
///   env-var syntax. Codemux's Windows default shell is now PowerShell (pwsh preferred, then
///   Windows PowerShell 5.1), so this is the right syntax for the shell actually running.
///   Note: users who override their shell to `cmd.exe` will get the literal `$env:...` string
///   as the system prompt (broken) — cmd.exe can't execute the preset command cleanly because
///   its environment-variable syntax is `%VAR%` AND it can't handle multi-line env vars at all.
///   Windows users who need agents should stick with the default PowerShell.
///
/// Gemini CLI has no CLI flag — it reads `GEMINI_SYSTEM_MD` pointing to a file. For Gemini,
/// we prefix the command with an inline write that dumps `CODEMUX_AGENT_CONTEXT` to a temp
/// file and sets the env var, so the file is only created when Gemini actually launches.
/// The Gemini inline pipeline also needs a PowerShell rewrite on Windows (no `printf`,
/// no `&&` on PS 5.1, and `GEMINI_SYSTEM_MD=val command` inline-env syntax is Unix-only).
pub fn inject_agent_context(command: &str, workspace_id: &str) -> String {
    let binary = command.split_whitespace().next().unwrap_or("");
    match binary {
        "claude" => {
            format!(
                "{command} --system-prompt \"{}\"",
                agent_context_shell_expansion()
            )
        }
        "codex" => {
            format!(
                "{command} -c instructions=\"{}\"",
                agent_context_shell_expansion()
            )
        }
        "pi" => {
            format!(
                "{command} --append-system-prompt \"{}\"",
                agent_context_shell_expansion()
            )
        }
        "gemini" => {
            let path = std::env::temp_dir()
                .join(format!("codemux-{workspace_id}-gemini-system.md"))
                .to_string_lossy()
                .into_owned();
            gemini_injection_command(&path, command)
        }
        // OpenCode: no CLI injection mechanism available.
        _ => command.to_string(),
    }
}

/// Returns the shell-level expansion of the `CODEMUX_AGENT_CONTEXT` env var for
/// the platform's default shell. Used inside `inject_agent_context` to produce a
/// preset command string that the running shell will parse correctly. See the
/// `inject_agent_context` doc comment for the per-platform shell rationale.
#[cfg(unix)]
fn agent_context_shell_expansion() -> &'static str {
    "$CODEMUX_AGENT_CONTEXT"
}

#[cfg(windows)]
fn agent_context_shell_expansion() -> &'static str {
    "$env:CODEMUX_AGENT_CONTEXT"
}

/// Builds the Gemini pre-command pipeline that writes the agent context to a
/// temp file and points `GEMINI_SYSTEM_MD` at it, then chains the actual gemini
/// invocation. Platform-specific because the shell syntax differs between
/// POSIX shells and PowerShell.
#[cfg(unix)]
fn gemini_injection_command(path: &str, command: &str) -> String {
    // POSIX shell (bash/zsh/sh/fish): `printf` → file redirect → inline env
    // var → command. `&&` short-circuits if the write fails.
    format!(
        "printf '%s' \"$CODEMUX_AGENT_CONTEXT\" > {path} && GEMINI_SYSTEM_MD={path} {command}"
    )
}

#[cfg(windows)]
fn gemini_injection_command(path: &str, command: &str) -> String {
    // PowerShell (5.1 / 7+): pipe `$env:CODEMUX_AGENT_CONTEXT` into
    // `Set-Content -NoNewline` so we get the raw env var value with no
    // trailing newline (matches the `printf '%s'` semantics on Unix). Then
    // set `$env:GEMINI_SYSTEM_MD` so the next command in the same statement
    // list inherits it, and invoke gemini. Uses `;` instead of `&&` because
    // `&&` is PowerShell 7+ only — Windows PowerShell 5.1 (the always-present
    // fallback) doesn't recognize it.
    //
    // Note: on PS 5.1 `Set-Content` defaults to UTF-8-with-BOM; on PS 7+ it
    // defaults to UTF-8 without BOM. Gemini CLI accepts both, so we don't
    // force `-Encoding` and instead let each host pick its default.
    format!(
        "$env:CODEMUX_AGENT_CONTEXT | Set-Content -Path '{path}' -NoNewline; $env:GEMINI_SYSTEM_MD = '{path}'; {command}"
    )
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

    #[cfg(unix)]
    #[test]
    fn inject_claude_adds_system_prompt_unix() {
        let result = inject_agent_context("claude --dangerously-skip-permissions", "ws-1");
        assert_eq!(
            result,
            "claude --dangerously-skip-permissions --system-prompt \"$CODEMUX_AGENT_CONTEXT\""
        );
    }

    #[cfg(windows)]
    #[test]
    fn inject_claude_adds_system_prompt_windows() {
        // Windows default shell is PowerShell (pwsh → powershell.exe fallback).
        // PowerShell env-var expansion uses `$env:VAR` syntax, NOT `$VAR`.
        // Asserting the POSIX form on Windows would produce a literal
        // "$CODEMUX_AGENT_CONTEXT" string being passed to claude as the
        // system prompt — the bug this test guards against.
        let result = inject_agent_context("claude --dangerously-skip-permissions", "ws-1");
        assert_eq!(
            result,
            "claude --dangerously-skip-permissions --system-prompt \"$env:CODEMUX_AGENT_CONTEXT\""
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
        // Uses `contains(agent_context_shell_expansion())` so the assertion is
        // cross-platform: on Unix it checks for `$CODEMUX_AGENT_CONTEXT`, on
        // Windows for `$env:CODEMUX_AGENT_CONTEXT`.
        let result = inject_agent_context("claude --system-prompt \"existing\"", "ws-1");
        assert!(result.contains(agent_context_shell_expansion()));
    }

    #[cfg(unix)]
    #[test]
    fn inject_codex_adds_instructions_unix() {
        let result = inject_agent_context("codex --full-auto", "ws-1");
        assert_eq!(
            result,
            "codex --full-auto -c instructions=\"$CODEMUX_AGENT_CONTEXT\""
        );
    }

    #[cfg(windows)]
    #[test]
    fn inject_codex_adds_instructions_windows() {
        let result = inject_agent_context("codex --full-auto", "ws-1");
        assert_eq!(
            result,
            "codex --full-auto -c instructions=\"$env:CODEMUX_AGENT_CONTEXT\""
        );
    }

    #[cfg(unix)]
    #[test]
    fn inject_pi_adds_append_system_prompt_unix() {
        let result = inject_agent_context("pi", "ws-1");
        assert_eq!(
            result,
            "pi --append-system-prompt \"$CODEMUX_AGENT_CONTEXT\""
        );
    }

    #[cfg(windows)]
    #[test]
    fn inject_pi_adds_append_system_prompt_windows() {
        let result = inject_agent_context("pi", "ws-1");
        assert_eq!(
            result,
            "pi --append-system-prompt \"$env:CODEMUX_AGENT_CONTEXT\""
        );
    }

    #[test]
    fn inject_pi_with_flags() {
        let result = inject_agent_context("pi --model sonnet", "ws-1");
        assert!(result.starts_with("pi --model sonnet"));
        assert!(result.contains("--append-system-prompt"));
    }

    #[cfg(unix)]
    #[test]
    fn inject_gemini_writes_file_and_sets_env_unix() {
        // Unix pipeline: `printf '%s' "$CODEMUX_AGENT_CONTEXT" > path && GEMINI_SYSTEM_MD=path gemini ...`
        let result = inject_agent_context("gemini --yolo", "test-ws-gemini");
        assert!(result.contains("printf '%s'"));
        assert!(result.contains("GEMINI_SYSTEM_MD="));
        assert!(result.contains("codemux-test-ws-gemini-gemini-system.md"));
        assert!(result.ends_with("gemini --yolo"));
        assert!(result.contains("$CODEMUX_AGENT_CONTEXT"));
    }

    #[cfg(windows)]
    #[test]
    fn inject_gemini_writes_file_and_sets_env_windows() {
        // Windows pipeline uses PowerShell `Set-Content -NoNewline` +
        // `$env:GEMINI_SYSTEM_MD = 'path'` (inline env-var syntax is
        // Unix-only; PS needs an explicit assignment statement). The
        // statements are chained with `;` because `&&` is PowerShell 7+
        // only and Windows PowerShell 5.1 doesn't recognize it.
        let result = inject_agent_context("gemini --yolo", "test-ws-gemini");
        assert!(result.contains("Set-Content"));
        assert!(result.contains("-NoNewline"));
        assert!(result.contains("$env:GEMINI_SYSTEM_MD"));
        assert!(result.contains("$env:CODEMUX_AGENT_CONTEXT"));
        assert!(result.contains("codemux-test-ws-gemini-gemini-system.md"));
        assert!(result.ends_with("gemini --yolo"));
        // Must NOT contain the Unix `printf` form — that would be a
        // regression to a broken-on-Windows shell command.
        assert!(!result.contains("printf"));
    }

    #[test]
    fn inject_opencode_unchanged() {
        let result = inject_agent_context("opencode", "ws-1");
        assert_eq!(result, "opencode");
    }

    /// Regression guard: the Gemini injection path MUST resolve its temp
    /// file via `std::env::temp_dir()` — never a hardcoded `/tmp/`. On
    /// Linux `std::env::temp_dir()` typically returns `/tmp/`, so a
    /// hardcoded `/tmp/` would silently "work" locally but break on
    /// Windows (where `%TEMP%` is under `C:\Users\...\AppData\Local\Temp`).
    ///
    /// The test asserts the generated shell command contains the OS-
    /// appropriate temp dir as a prefix of the gemini-system-md path.
    #[test]
    fn test_gemini_context_path_is_cross_platform() {
        let result = inject_agent_context("gemini", "test-ws-xplat");

        // Build what the expected temp path SHOULD look like on this OS.
        let expected_temp = std::env::temp_dir()
            .join("codemux-test-ws-xplat-gemini-system.md")
            .to_string_lossy()
            .into_owned();

        assert!(
            result.contains(&expected_temp),
            "gemini injection must reference {expected_temp:?}, got {result:?}",
        );

        // On Windows, std::env::temp_dir() must NOT start with /tmp/.
        // On Linux it typically DOES, so this check is only meaningful
        // on Windows — it's harmless on Linux.
        #[cfg(windows)]
        {
            // Build the forbidden prefix at runtime so the literal
            // "/tmp/" never appears in the source file — otherwise the
            // sibling meta-test `test_no_hardcoded_tmp_paths_in_modified_sources`
            // would flag this assertion as a regression.
            let unix_tmp_prefix = format!("/{}/", "tmp");
            assert!(
                !expected_temp.starts_with(&unix_tmp_prefix),
                "on Windows the temp dir must never be under /tmp, got {expected_temp:?}",
            );
        }

        // The file name component is the same on every platform.
        assert!(result.contains("codemux-test-ws-xplat-gemini-system.md"));
    }

    /// Meta-test: grep the source files we modified for Windows support
    /// against regressions that would reintroduce hardcoded Unix-temp-dir
    /// paths for **codemux-specific** artifacts in non-comment code.
    ///
    /// This is a "soft" check — we can't AST-parse Rust from a unit test
    /// without pulling in heavyweight deps, so we do a line-level grep
    /// that strips trailing `//` comments before matching, then looks
    /// for the literal pattern `"<slash>tmp<slash>codemux`. That narrow
    /// pattern catches the actual regressions we care about (the
    /// Gemini system prompt, control socket fallback, diagnostics log,
    /// CLI shim dir — all of which used to be `/tmp/codemux-*`) without
    /// flagging unrelated test fixtures that happen to use `/tmp/` as
    /// a placeholder workspace CWD.
    ///
    /// **Suppression**: to allow a legitimate Unix-only hardcoded
    /// `/tmp/codemux-*` path (e.g. inside `#[cfg(unix)]` code where the
    /// XDG fallback is Unix-specific by definition), add the trailing
    /// marker comment `// tmp-literal-ok` to the offending line. The
    /// meta-test won't flag any line whose body contains that marker.
    ///
    /// The forbidden pattern is built at runtime from constituent parts
    /// so this test source file doesn't contain the literal pattern
    /// itself — otherwise the test would flag its own source.
    #[test]
    fn test_no_hardcoded_tmp_paths_in_modified_sources() {
        // Files we audited and updated during the Windows support pass.
        // When a new file gets touched for Windows, add it here.
        let files = [
            "src/agent_context.rs",
            "src/agent_browser.rs",
            "src/commands/mod.rs",
            "src/commands/browser.rs",
            "src/control.rs",
            "src/diagnostics.rs",
            "src/git.rs",
            "src/terminal/mod.rs",
            "src/ports.rs",
        ];

        // Construct the forbidden literal at runtime: a double-quote
        // followed by `/tmp/codemux`. Any source line that contains
        // this exact byte sequence is almost certainly a hardcoded
        // codemux-specific temp path and should use std::env::temp_dir()
        // instead.
        //
        // Split into parts so the source file of THIS test never
        // contains the full literal — otherwise the test would
        // trivially flag its own source.
        let needle = format!("{}{}{}{}{}", '"', '/', "tmp", '/', "codemux");
        // Suppression marker — also built at runtime so greps for
        // "tmp-literal-ok" don't match THIS test's implementation.
        let ok_marker = format!("{}-{}-{}", "tmp", "literal", "ok");

        let manifest_dir = env!("CARGO_MANIFEST_DIR");

        for rel in files {
            let path = std::path::Path::new(manifest_dir).join(rel);
            let Ok(content) = std::fs::read_to_string(&path) else {
                // File might not exist yet — skip rather than fail.
                // (CI runs this test; if a file is missing, the other
                //  tests for that module will fail too and that's a
                //  clearer signal.)
                continue;
            };

            for (lineno, line) in content.lines().enumerate() {
                // If the line carries a suppression marker anywhere,
                // skip it entirely. The marker lives in the comment
                // tail; authors put it there when a unix-only code
                // path legitimately needs a hardcoded codemux temp path.
                if line.contains(&ok_marker) {
                    continue;
                }

                // Strip single-line trailing comments before matching.
                let comment_start = line.find("//");
                let code_part = match comment_start {
                    Some(idx) => &line[..idx],
                    None => line,
                };

                if code_part.contains(&needle) {
                    panic!(
                        "regression: hardcoded unix codemux temp path found in non-comment code\n\
                         file: {rel}\n\
                         line {}: {line}\n\
                         use std::env::temp_dir().join(\"codemux-...\") instead — hardcoded unix paths break Windows\n\
                         (if this is inside #[cfg(unix)] code, add `// {ok_marker}` at end of line)",
                        lineno + 1,
                    );
                }
            }
        }
    }
}
