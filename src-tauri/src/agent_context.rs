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
        rules
            .push("- Do NOT cd to the original repo path — it has a different branch checked out.");
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

/// Match the shell selected when Codemux creates a local or remote pane.
pub(crate) fn launch_shell_for_workspace(
    state: &crate::state::AppStateStore,
    workspace_id: &str,
) -> String {
    if state
        .find_workspace(workspace_id)
        .is_some_and(|workspace| workspace.host_id.is_some())
    {
        "bash".to_string()
    } else {
        crate::terminal::default_shell()
    }
}

fn shell_name(shell: &str) -> &str {
    shell.rsplit(|c| c == '/' || c == '\\').next().unwrap_or(shell)
}

fn uses_posix_shell(shell: &str) -> bool {
    cfg!(unix) || matches!(shell_name(shell), "sh" | "bash" | "zsh" | "fish")
}

/// Transform a preset command to inject the Codemux agent context as a system prompt,
/// if the command targets a known CLI agent that supports system prompt injection.
///
/// The context is passed via the `CODEMUX_AGENT_CONTEXT` env var (set on all PTY sessions).
/// The injected command string expands that env var at shell parse time — the **expansion
/// syntax is platform-specific**:
///
/// - **POSIX shells and Fish**: `"$CODEMUX_AGENT_CONTEXT"` inside double quotes.
/// - **PowerShell 5.1+ / 7+**: `"$env:CODEMUX_AGENT_CONTEXT"`. The caller passes
///   the shell selected for the pane, including Bash for remote panes on Windows.
///   Note: users who override their shell to `cmd.exe` will get the literal `$env:...` string
///   as the system prompt (broken) — cmd.exe can't execute the preset command cleanly because
///   its environment-variable syntax is `%VAR%` AND it can't handle multi-line env vars at all.
///   Windows users who need agents should stick with the default PowerShell.
///
/// Gemini CLI has no CLI flag — it reads `GEMINI_SYSTEM_MD` pointing to a file. For Gemini,
/// we prefix the command with an inline write that dumps `CODEMUX_AGENT_CONTEXT` to a file in
/// the user's private launch folder (see [`launch_file_path`]) and sets the env var, so the
/// file is only created when Gemini actually launches, on whichever host runs the terminal.
/// The Gemini inline pipeline also needs a PowerShell rewrite on Windows (no `printf`,
/// no `&&` on PS 5.1, and `GEMINI_SYSTEM_MD=val command` inline-env syntax is Unix-only).
///
/// Factory's `droid` is a special case for a different reason: interactive `droid` has
/// **no skip-permissions / autonomy CLI flag** (only the non-interactive `droid exec`
/// subcommand does). Full autonomy for an interactive session can only be set through a
/// settings file. So for `droid` we write a tiny runtime settings file enabling
/// `sessionDefaultSettings.autonomyLevel = "high"` and append `--settings <path>` — that
/// is `droid`'s YOLO/skip-permissions equivalent. Same cross-platform inline-write split
/// as Gemini. If the command already carries a `--settings` flag we leave it untouched.
pub fn inject_agent_context(command: &str, workspace_id: &str, shell: &str) -> String {
    let binary = command.split_whitespace().next().unwrap_or("");
    match binary {
        "claude" => {
            format!(
                "{command} --system-prompt \"{}\"",
                agent_context_shell_expansion(shell)
            )
        }
        "codex" => {
            format!(
                "{command} -c instructions=\"{}\"",
                agent_context_shell_expansion(shell)
            )
        }
        "pi" => {
            format!(
                "{command} --append-system-prompt \"{}\"",
                agent_context_shell_expansion(shell)
            )
        }
        "gemini" => {
            let path = launch_file_path(workspace_id, "gemini-system.md", shell);
            gemini_injection_command(&path, command, shell)
        }
        "droid" => {
            // Respect a user-supplied settings file rather than
            // double-injecting our own.
            if command.contains("--settings") {
                command.to_string()
            } else {
                let path = launch_file_path(workspace_id, "droid-settings.json", shell);
                droid_injection_command(&path, command, shell)
            }
        }
        // OpenCode, and the other agent CLIs (Antigravity, Copilot,
        // Cursor Agent, Amp, Grok) carry their skip-permissions flag
        // directly in the preset command — no rewrite needed here.
        _ => command.to_string(),
    }
}

/// Factory Droid runtime settings JSON enabling full autonomy. `droid`
/// has no interactive skip-permissions flag; this file, passed via
/// `--settings`, is its YOLO equivalent. The exact `sessionDefaultSettings`
/// / `autonomyLevel` schema is what the `droid` binary itself documents.
const DROID_AUTONOMY_JSON: &str = r#"{"sessionDefaultSettings":{"autonomyLevel":"high"}}"#;

/// Folder for Gemini and Droid launch files in POSIX and Fish panes. `$HOME`
/// is expanded by the shell running the command, so this is right on whichever host the
/// terminal lives. It must never be the shared temp dir: the files decide
/// what a full-autonomy agent is told and allowed. A shared temp directory
/// lets another local user plant an entry before the file is created.
const LAUNCH_DIR: &str = "$HOME/.codemux-agent-launch";

/// Base name for one launch's `kind` file. The host running the pane adds a
/// fresh suffix at execution time, so even identical rendered commands sent
/// to several panes receive separate files.
fn launch_file_path(workspace_id: &str, kind: &str, shell: &str) -> String {
    // Workspace ids are generated, but the path is spliced into a shell
    // command, so keep only characters that need no quoting.
    let id: String = workspace_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        .collect();
    let name = format!("codemux-{id}-{}-{kind}", uuid::Uuid::new_v4().simple());
    if uses_posix_shell(shell) {
        return format!("{LAUNCH_DIR}/{name}");
    }
    // %TEMP% is already private to the user on Windows.
    std::env::temp_dir().join(name).to_string_lossy().into_owned()
}

/// Allocate a private directory at execution time and return the file path to
/// the pane shell. The writer's no-clobber option stays in its child shell.
fn write_launch_file(path: &str, kind: &str, contents: &str) -> String {
    let name = path.rsplit('/').next().expect("launch file has a name");
    let script = format!(
        "{}cmx_launch=$(mktemp -d \"$cmx_dir/{name}.XXXXXX\") || exit 1\n\
         cmx_file=\"$cmx_launch/{kind}\"\n\
         set -C\n\
         if {contents} > \"$cmx_file\"; then\n\
           printf '%s\\n' \"$cmx_file\"\n\
         else\n\
           rm -f \"$cmx_file\"\n\
           rmdir \"$cmx_launch\"\n\
           exit 1\n\
         fi\n",
        crate::private_dir::shell_private_dir_guard(&[".codemux-agent-launch"], true),
    );
    format!("sh -c {}", shell_sq(&script))
}

fn remove_launch_file(path: &str, kind: &str) -> String {
    let name = path.rsplit('/').next().expect("launch file has a name");
    let script = format!(
        "{}cmx_file=$1\n\
         cmx_launch=${{cmx_file%/*}}\n\
         [ \"${{cmx_launch%/*}}\" = \"$cmx_dir\" ] || exit 1\n\
         [ \"$cmx_file\" = \"$cmx_launch/{kind}\" ] || exit 1\n\
         case \"${{cmx_launch##*/}}\" in {name}.??????) ;; *) exit 1 ;; esac\n\
         cmx_check_owned \"$cmx_launch\" || exit 1\n\
         [ ! -L \"$cmx_file\" ] || exit 1\n\
         rm -f \"$cmx_file\" && rmdir \"$cmx_launch\"\n",
        crate::private_dir::shell_private_dir_guard(&[".codemux-agent-launch"], false),
    );
    format!("sh -c {} sh \"$cmx_file\"", shell_sq(&script))
}

fn run_with_launch_file(path: &str, kind: &str, contents: &str, run: &str, shell: &str) -> String {
    // The agent command must run in the pane's shell: users can define a
    // shell function for an agent and use shell redirects in their preset.
    // The writer returns a fresh path. Cleanup only runs after that writer
    // succeeds, so a failed allocation cannot remove another launch's file.
    let write = write_launch_file(path, kind, contents);
    let remove = remove_launch_file(path, kind);
    if shell_name(shell) == "fish" {
        return format!(
            "set -l cmx_file ({write})\n\
             if test -n \"$cmx_file\"\n\
               {run}\n\
               set -l cmx_agent_status $status\n\
               {remove}\n\
               set -l cmx_cleanup_status $status\n\
               if test $cmx_agent_status -ne 0\n\
                 sh -c 'exit \"$1\"' sh \"$cmx_agent_status\"\n\
               else\n\
                 sh -c 'exit \"$1\"' sh \"$cmx_cleanup_status\"\n\
               end\n\
             else\n\
               false\n\
             end"
        );
    }
    format!(
        "cmx_file=$({write}) && {{\n\
         {run}\n\
         cmx_agent_status=$?\n\
         {remove}\n\
         cmx_cleanup_status=$?\n\
         if [ \"$cmx_agent_status\" -ne 0 ]; then\n\
           (exit \"$cmx_agent_status\")\n\
         else\n\
           (exit \"$cmx_cleanup_status\")\n\
         fi\n\
         }}"
    )
}

fn shell_sq(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn powershell_literal(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

/// Returns the context variable syntax for the pane shell.
fn agent_context_shell_expansion(shell: &str) -> &'static str {
    if uses_posix_shell(shell) {
        "$CODEMUX_AGENT_CONTEXT"
    } else {
        "$env:CODEMUX_AGENT_CONTEXT"
    }
}

/// Builds the Gemini pre-command pipeline that writes the agent context to a
/// private launch file and points `GEMINI_SYSTEM_MD` at it, then chains the actual gemini
/// invocation. Platform-specific because the shell syntax differs between
/// POSIX shells and PowerShell.
fn gemini_injection_command(path: &str, command: &str, shell: &str) -> String {
    if uses_posix_shell(shell) {
        let run = if shell_name(shell) == "fish" {
            format!("set -lx GEMINI_SYSTEM_MD \"$cmx_file\"\n{command}")
        } else {
            format!("GEMINI_SYSTEM_MD=\"$cmx_file\" {command}")
        };
        return run_with_launch_file(
            path,
            "gemini-system.md",
            "printf '%s' \"$CODEMUX_AGENT_CONTEXT\"",
            &run,
            shell,
        );
    }
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
    let path = powershell_literal(path);
    format!(
        "$env:CODEMUX_AGENT_CONTEXT | Set-Content -LiteralPath {path} -NoNewline; $env:GEMINI_SYSTEM_MD = {path}; {command}"
    )
}

/// Builds the Factory Droid pre-command pipeline: write the autonomy
/// settings file, then launch `droid` with `--settings` pointed at it.
/// This is droid's interactive skip-permissions equivalent — see the
/// `inject_agent_context` doc comment.
fn droid_injection_command(path: &str, command: &str, shell: &str) -> String {
    if uses_posix_shell(shell) {
        return run_with_launch_file(
            path,
            "droid-settings.json",
            &format!("printf '%s' {}", shell_sq(DROID_AUTONOMY_JSON)),
            &format!("{command} --settings \"$cmx_file\""),
            shell,
        );
    }
    // PowerShell (5.1 / 7+): a single-quoted literal (the JSON has no
    // single quotes) written with `Set-Content -NoNewline`, then droid
    // launched with `--settings`. Uses `;` rather than `&&` because
    // `&&` is PowerShell 7+ only — Windows PowerShell 5.1 rejects it.
    let path = powershell_literal(path);
    format!(
        "'{DROID_AUTONOMY_JSON}' | Set-Content -LiteralPath {path} -NoNewline; {command} --settings {path}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    const TEST_SHELL: &str = "sh";
    #[cfg(windows)]
    const TEST_SHELL: &str = "powershell.exe";

    fn inject_agent_context(command: &str, workspace_id: &str) -> String {
        super::inject_agent_context(command, workspace_id, TEST_SHELL)
    }

    #[cfg(unix)]
    fn fake_droid_bin(home: &std::path::Path) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let bin = home.join("bin");
        std::fs::create_dir(&bin).unwrap();
        let droid = bin.join("droid");
        std::fs::write(
            &droid,
            "#!/bin/sh\nfor cmx_arg in \"$@\"; do cmx_file=$cmx_arg; done\n[ -f \"$cmx_file\" ] || exit 2\n[ \"$(find \"$cmx_file\" -perm 0600 -print)\" = \"$cmx_file\" ] || exit 3\nprintf 'FILE=%s\\n' \"$cmx_file\"\ncat \"$cmx_file\"\n[ \"${CODEMUX_FAKE_DROID_FAIL:-}\" != 1 ] || exit 42\n",
        )
        .unwrap();
        std::fs::set_permissions(&droid, std::fs::Permissions::from_mode(0o700)).unwrap();
        bin
    }

    #[cfg(unix)]
    fn fake_gemini_bin(home: &std::path::Path) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let bin = home.join("bin");
        std::fs::create_dir(&bin).unwrap();
        let gemini = bin.join("gemini");
        std::fs::write(
            &gemini,
            "#!/bin/sh\n[ -f \"$GEMINI_SYSTEM_MD\" ] || exit 2\n[ \"$(find \"$GEMINI_SYSTEM_MD\" -perm 0600 -print)\" = \"$GEMINI_SYSTEM_MD\" ] || exit 3\nprintf 'FILE=%s\\n' \"$GEMINI_SYSTEM_MD\"\ncat \"$GEMINI_SYSTEM_MD\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&gemini, std::fs::Permissions::from_mode(0o700)).unwrap();
        bin
    }

    #[cfg(unix)]
    fn fake_path(bin: &std::path::Path) -> String {
        format!(
            "{}:{}",
            bin.display(),
            std::env::var("PATH").unwrap_or_default()
        )
    }

    #[cfg(unix)]
    fn executed_launch_file(out: &std::process::Output) -> std::path::PathBuf {
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let output = String::from_utf8_lossy(&out.stdout);
        let (file, contents) = output.split_once('\n').expect("fake droid output");
        assert_eq!(contents, DROID_AUTONOMY_JSON);
        std::path::PathBuf::from(file.strip_prefix("FILE=").expect("settings path"))
    }

    #[test]
    fn build_context_with_all_info() {
        let ctx = build_agent_context(
            Some("my-feature"),
            Some("/home/user/.codemux/worktrees/repo/my-feature"),
            Some("feat/my-feature"),
            Some("/home/user/projects/repo"),
        );
        assert!(ctx.contains("Your workspace: my-feature"));
        assert!(
            ctx.contains("Your working directory: /home/user/.codemux/worktrees/repo/my-feature")
        );
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
        assert!(result.contains(agent_context_shell_expansion(TEST_SHELL)));
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
        // A guarded child shell writes the file before launching Gemini.
        let result = inject_agent_context("gemini --yolo", "test-ws-gemini");
        assert!(result.contains("printf"));
        assert!(result.contains("GEMINI_SYSTEM_MD="));
        assert!(result.contains("codemux-test-ws-gemini-"));
        assert!(result.contains("-gemini-system.md"));
        assert!(result.contains("GEMINI_SYSTEM_MD=\"$cmx_file\" gemini --yolo"));
        assert!(result.contains("$CODEMUX_AGENT_CONTEXT"));
    }

    #[cfg(windows)]
    #[test]
    fn inject_gemini_writes_file_and_sets_env_windows() {
        // Windows pipeline uses PowerShell `Set-Content -LiteralPath -NoNewline` +
        // `$env:GEMINI_SYSTEM_MD = 'path'` (inline env-var syntax is
        // Unix-only; PS needs an explicit assignment statement). The
        // statements are chained with `;` because `&&` is PowerShell 7+
        // only and Windows PowerShell 5.1 doesn't recognize it.
        let result = inject_agent_context("gemini --yolo", "test-ws-gemini");
        assert!(result.contains("Set-Content"));
        assert!(result.contains("-NoNewline"));
        assert!(result.contains("$env:GEMINI_SYSTEM_MD"));
        assert!(result.contains("$env:CODEMUX_AGENT_CONTEXT"));
        assert!(result.contains("codemux-test-ws-gemini-"));
        assert!(result.contains("-gemini-system.md"));
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

    #[cfg(unix)]
    #[test]
    fn inject_droid_writes_settings_and_appends_flag_unix() {
        // A guarded child shell writes the settings before launching Droid.
        let result = inject_agent_context("droid", "test-ws-droid");
        assert!(result.contains("printf"));
        assert!(result.contains(r#"{"sessionDefaultSettings":{"autonomyLevel":"high"}}"#));
        assert!(result.contains("codemux-test-ws-droid-"));
        assert!(result.contains("-droid-settings.json"));
        assert!(result.contains("droid --settings \"$cmx_file\""));
    }

    #[cfg(windows)]
    #[test]
    fn inject_droid_writes_settings_and_appends_flag_windows() {
        // Windows pipeline uses PowerShell `Set-Content -NoNewline`,
        // never the Unix `printf` form.
        let result = inject_agent_context("droid", "test-ws-droid");
        assert!(result.contains("Set-Content"));
        assert!(result.contains("-NoNewline"));
        assert!(result.contains(r#"{"sessionDefaultSettings":{"autonomyLevel":"high"}}"#));
        assert!(result.contains("codemux-test-ws-droid-"));
        assert!(result.contains("-droid-settings.json"));
        assert!(result.contains("droid --settings "));
        assert!(!result.contains("printf"));
    }

    #[cfg(windows)]
    #[test]
    fn windows_launch_paths_are_powershell_literals() {
        let path = "C:\\Users\\O'Brien\\[data]\\settings.json";
        let quoted = "'C:\\Users\\O''Brien\\[data]\\settings.json'";
        for command in [
            gemini_injection_command(path, "gemini", TEST_SHELL),
            droid_injection_command(path, "droid", TEST_SHELL),
        ] {
            assert!(
                command.contains(&format!("Set-Content -LiteralPath {quoted}")),
                "{command}"
            );
            assert!(!command.contains("Set-Content -Path"), "{command}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_remote_bash_uses_posix_launch_syntax() {
        let gemini = super::inject_agent_context("gemini --yolo", "remote-ws", "bash");
        assert!(gemini.starts_with("cmx_file=$(sh -c 'umask 077"), "{gemini}");
        assert!(gemini.contains("GEMINI_SYSTEM_MD=\"$cmx_file\""));
        assert!(!gemini.contains("Set-Content"));
        let codex = super::inject_agent_context("codex --full-auto", "remote-ws", "bash");
        assert!(codex.contains("\"$CODEMUX_AGENT_CONTEXT\""));
    }

    #[test]
    fn inject_droid_respects_existing_settings_flag() {
        // If the user already passed --settings, don't double-inject.
        let result = inject_agent_context("droid --settings /my/own.json", "ws-1");
        assert_eq!(result, "droid --settings /my/own.json");
    }

    #[cfg(windows)]
    #[test]
    fn inject_droid_settings_path_is_cross_platform() {
        // Regression guard mirroring the Gemini path: on Windows the droid
        // settings file resolves via the per-user %TEMP%, never a
        // hardcoded unix path.
        let result = inject_agent_context("droid", "test-ws-dx");
        let expected_temp = std::env::temp_dir()
            .join("codemux-test-ws-dx-")
            .to_string_lossy()
            .replace('\'', "''");
        assert!(
            result.contains(&expected_temp),
            "droid injection must reference {expected_temp:?}, got {result:?}",
        );
    }

    /// On Unix the launch files live in an owner-only folder under the
    /// user's home, never the shared temp dir, where another local user
    /// could create or symlink the predictable name first.
    #[cfg(unix)]
    #[test]
    fn unix_launch_files_stay_out_of_shared_temp() {
        let temp = std::env::temp_dir().to_string_lossy().into_owned();
        for (command, kind) in [
            ("gemini --yolo", "gemini-system.md"),
            ("droid", "droid-settings.json"),
        ] {
            let result = inject_agent_context(command, "ws-1");
            assert!(
                result.starts_with("cmx_file=$(sh -c 'umask 077"),
                "{result}"
            );
            assert!(result.contains("cmx_check_ancestor"), "{result}");
            assert!(result.contains("set -C"), "{result}");
            assert!(
                result.contains("cmx_dir=\"$cmx_dir/.codemux-agent-launch\""),
                "{result}"
            );
            assert!(result.contains(&format!("-{kind}.XXXXXX\"")), "{result}");
            assert!(result.contains("mktemp -d"), "{result}");
            assert!(result.contains("rm -f"), "{result}");
            assert!(!result.contains(&temp), "{result}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn unix_launch_files_run_in_a_shell() {
        // Run the generated command with a scratch HOME and a stand-in
        // agent so the real write, folder mode, and argument are all
        // exercised.
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let home_path = home.path().canonicalize().unwrap();
        let bin = fake_droid_bin(&home_path);
        let command = inject_agent_context("droid", "ws-run");
        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(&command)
            .env("HOME", &home_path)
            .env("PATH", fake_path(&bin))
            .env("CODEMUX_SESSION_ID", "session-droid")
            .output()
            .unwrap();
        let file = executed_launch_file(&out);
        let dir = home_path.join(".codemux-agent-launch");
        let launch_dir = file.parent().unwrap();
        assert_eq!(launch_dir.parent(), Some(dir.as_path()));
        let launch_name = launch_dir.file_name().unwrap().to_string_lossy();
        assert!(
            launch_name.starts_with("codemux-ws-run-")
                && launch_name.contains("-droid-settings.json.")
        );
        assert_eq!(file.file_name().unwrap(), "droid-settings.json");
        let mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o700);
        assert!(
            !launch_dir.exists(),
            "launch files must be removed after the agent exits"
        );
        assert!(std::fs::read_dir(&dir).unwrap().next().is_none());
    }

    #[cfg(unix)]
    #[test]
    fn unix_gemini_context_exists_until_agent_exits() {
        let home = tempfile::tempdir().unwrap();
        let home_path = home.path().canonicalize().unwrap();
        let bin = fake_gemini_bin(&home_path);
        let context = "workspace context\nsecond line";
        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(inject_agent_context("gemini --yolo", "ws-gemini"))
            .env("HOME", &home_path)
            .env("PATH", fake_path(&bin))
            .env("CODEMUX_SESSION_ID", "session-gemini")
            .env("CODEMUX_AGENT_CONTEXT", context)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        let output = String::from_utf8_lossy(&out.stdout);
        let (file, contents) = output.split_once('\n').expect("fake gemini output");
        assert_eq!(contents, context);
        let file = std::path::PathBuf::from(file.strip_prefix("FILE=").unwrap());
        assert_eq!(file.file_name().unwrap(), "gemini-system.md");
        assert!(!file.exists());
        assert!(std::fs::read_dir(home_path.join(".codemux-agent-launch"))
            .unwrap()
            .next()
            .is_none());
    }

    #[cfg(unix)]
    #[test]
    fn unix_gemini_uses_the_pane_shell_function_and_existing_redirect() {
        let home = tempfile::tempdir().unwrap();
        let home_path = home.path().canonicalize().unwrap();
        let log = home_path.join("existing.log");
        std::fs::write(&log, "old").unwrap();
        let command = inject_agent_context("gemini --yolo > \"$CMX_LOG\"", "ws-function");
        let script = format!("gemini() {{ cat \"$GEMINI_SYSTEM_MD\"; }}\n{command}\n");
        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(script)
            .env("HOME", &home_path)
            .env("CMX_LOG", &log)
            .env("CODEMUX_SESSION_ID", "session-function")
            .env("CODEMUX_AGENT_CONTEXT", "current context")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "current context");
        assert!(std::fs::read_dir(home_path.join(".codemux-agent-launch"))
            .unwrap()
            .next()
            .is_none());
    }

    #[cfg(unix)]
    #[test]
    fn fish_gemini_uses_the_pane_function_and_existing_redirect() {
        let fish = std::env::var_os("CODEMUX_TEST_FISH")
            .map(std::path::PathBuf::from)
            .or_else(|| which::which("fish").ok());
        let Some(fish) = fish else { return };
        let home = tempfile::tempdir().unwrap();
        let home_path = home.path().canonicalize().unwrap();
        let log = home_path.join("existing.log");
        std::fs::write(&log, "old").unwrap();
        let command = super::inject_agent_context(
            "gemini --yolo > \"$CMX_LOG\"",
            "ws-fish",
            fish.to_str().unwrap(),
        );
        let script = format!(
            "function gemini\ncat \"$GEMINI_SYSTEM_MD\"\nend\n{command}\n"
        );
        let out = std::process::Command::new(fish)
            .arg("-c")
            .arg(script)
            .env("HOME", &home_path)
            .env("CMX_LOG", &log)
            .env("CODEMUX_AGENT_CONTEXT", "fish context")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(std::fs::read_to_string(&log).unwrap(), "fish context");
        assert!(std::fs::read_dir(home_path.join(".codemux-agent-launch"))
            .unwrap()
            .next()
            .is_none());
    }

    #[cfg(unix)]
    #[test]
    fn fish_droid_reads_its_private_settings_until_exit() {
        let fish = std::env::var_os("CODEMUX_TEST_FISH")
            .map(std::path::PathBuf::from)
            .or_else(|| which::which("fish").ok());
        let Some(fish) = fish else { return };
        let home = tempfile::tempdir().unwrap();
        let home_path = home.path().canonicalize().unwrap();
        let bin = fake_droid_bin(&home_path);
        let command = super::inject_agent_context("droid", "ws-fish-droid", fish.to_str().unwrap());
        let out = std::process::Command::new(fish)
            .arg("-c")
            .arg(command)
            .env("HOME", &home_path)
            .env("PATH", fake_path(&bin))
            .output()
            .unwrap();
        let file = executed_launch_file(&out);
        assert_eq!(file.file_name().unwrap(), "droid-settings.json");
        assert!(!file.exists());
        assert!(std::fs::read_dir(home_path.join(".codemux-agent-launch"))
            .unwrap()
            .next()
            .is_none());
    }

    #[cfg(unix)]
    #[test]
    fn unix_droid_launch_accepts_a_verified_linked_home() {
        let root = tempfile::tempdir().unwrap();
        let root_path = root.path().canonicalize().unwrap();
        let home = root_path.join("real-home");
        std::fs::create_dir(&home).unwrap();
        let linked_home = root_path.join("linked-home");
        std::os::unix::fs::symlink(&home, &linked_home).unwrap();
        let bin = fake_droid_bin(&home);
        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(inject_agent_context("droid", "ws-linked-home"))
            .env("HOME", &linked_home)
            .env("PATH", fake_path(&bin))
            .env("CODEMUX_SESSION_ID", "session-linked")
            .output()
            .unwrap();
        let file = executed_launch_file(&out);
        assert!(file.starts_with(home.join(".codemux-agent-launch")));
        assert!(!file.exists());
        assert!(std::fs::read_dir(home.join(".codemux-agent-launch"))
            .unwrap()
            .next()
            .is_none());
    }

    #[cfg(unix)]
    #[test]
    fn concurrent_unix_launches_keep_separate_settings_files() {
        let home = tempfile::tempdir().unwrap();
        let home_path = home.path().canonicalize().unwrap();
        let bin = fake_droid_bin(&home_path);
        let command = inject_agent_context("droid", "same-workspace");
        let start = std::sync::Arc::new(std::sync::Barrier::new(2));
        let handles: Vec<_> = (0..2)
            .map(|_| {
                let command = command.clone();
                let home_path = home_path.clone();
                let path = fake_path(&bin);
                let start = start.clone();
                std::thread::spawn(move || {
                    start.wait();
                    std::process::Command::new("sh")
                        .arg("-c")
                        .arg(command)
                        .env("HOME", home_path)
                        .env("PATH", path)
                        .env("CODEMUX_SESSION_ID", "same-session")
                        .output()
                        .unwrap()
                })
            })
            .collect();
        let files: Vec<_> = handles
            .into_iter()
            .map(|handle| {
                let out = handle.join().unwrap();
                let file = executed_launch_file(&out);
                assert_eq!(
                    file.parent().unwrap().parent(),
                    Some(home_path.join(".codemux-agent-launch").as_path())
                );
                assert!(!file.exists());
                file
            })
            .collect();
        assert_ne!(files[0], files[1]);
        assert!(std::fs::read_dir(home_path.join(".codemux-agent-launch"))
            .unwrap()
            .next()
            .is_none());
    }

    #[cfg(unix)]
    #[test]
    fn unix_launch_removes_private_file_when_agent_fails() {
        let home = tempfile::tempdir().unwrap();
        let home_path = home.path().canonicalize().unwrap();
        let bin = fake_droid_bin(&home_path);
        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(inject_agent_context("droid", "ws-fail"))
            .env("HOME", &home_path)
            .env("PATH", fake_path(&bin))
            .env("CODEMUX_SESSION_ID", "session-fail")
            .env("CODEMUX_FAKE_DROID_FAIL", "1")
            .output()
            .unwrap();
        assert_eq!(
            out.status.code(),
            Some(42),
            "the agent's failure must remain visible"
        );
        assert!(std::fs::read_dir(home_path.join(".codemux-agent-launch"))
            .unwrap()
            .next()
            .is_none());
    }

    #[cfg(unix)]
    #[test]
    fn unix_launch_does_not_interpolate_an_unsafe_session_id() {
        let home = tempfile::tempdir().unwrap();
        let home_path = home.path().canonicalize().unwrap();
        let bin = fake_droid_bin(&home_path);
        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(inject_agent_context("droid", "ws-unsafe-session"))
            .env("HOME", &home_path)
            .env("PATH", fake_path(&bin))
            .env("CODEMUX_SESSION_ID", "../escape")
            .output()
            .unwrap();
        executed_launch_file(&out);
        assert!(std::fs::read_dir(home_path.join(".codemux-agent-launch"))
            .unwrap()
            .next()
            .is_none());
    }

    #[cfg(unix)]
    #[test]
    fn unix_launch_refuses_writable_and_symlinked_parents_before_writing() {
        use std::os::unix::fs::PermissionsExt;
        let root = tempfile::tempdir().unwrap();
        let home = root.path().canonicalize().unwrap().join("home");
        std::fs::create_dir(&home).unwrap();
        let bin = fake_droid_bin(&home);
        let launch = inject_agent_context("droid", "ws-run");

        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o777)).unwrap();
        let status = std::process::Command::new("sh")
            .arg("-c")
            .arg(&launch)
            .env("HOME", &home)
            .env("PATH", fake_path(&bin))
            .env("CODEMUX_SESSION_ID", "session-guard")
            .status()
            .unwrap();
        assert!(!status.success());
        assert!(!home.join(".codemux-agent-launch").exists());

        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let elsewhere = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(elsewhere.path(), home.join(".codemux")).unwrap();
        let status = std::process::Command::new("sh")
            .arg("-c")
            .arg(&launch)
            .env("HOME", &home)
            .env("PATH", fake_path(&bin))
            .env("CODEMUX_SESSION_ID", "session-guard")
            .status()
            .unwrap();
        assert!(
            status.success(),
            "a linked data directory does not affect launch files"
        );
        let gemini =
            inject_agent_context("gemini --yolo", "ws-gemini").replace("gemini --yolo", "true");
        let status = std::process::Command::new("sh")
            .arg("-c")
            .arg(gemini)
            .env("HOME", &home)
            .env("CODEMUX_SESSION_ID", "session-gemini")
            .status()
            .unwrap();
        assert!(status.success(), "Gemini also works with linked app data");
        assert_eq!(std::fs::read_dir(elsewhere.path()).unwrap().count(), 0);

        std::fs::remove_dir_all(home.join(".codemux-agent-launch")).unwrap();
        std::os::unix::fs::symlink(elsewhere.path(), home.join(".codemux-agent-launch")).unwrap();
        let status = std::process::Command::new("sh")
            .arg("-c")
            .arg(&launch)
            .env("HOME", &home)
            .env("PATH", fake_path(&bin))
            .env("CODEMUX_SESSION_ID", "session-guard")
            .status()
            .unwrap();
        assert!(!status.success());
        assert_eq!(std::fs::read_dir(elsewhere.path()).unwrap().count(), 0);

        std::fs::remove_file(home.join(".codemux-agent-launch")).unwrap();
        std::fs::create_dir(home.join(".codemux-agent-launch")).unwrap();
        std::fs::set_permissions(
            home.join(".codemux-agent-launch"),
            std::fs::Permissions::from_mode(0o777),
        )
        .unwrap();
        let status = std::process::Command::new("sh")
            .arg("-c")
            .arg(&launch)
            .env("HOME", &home)
            .env("PATH", fake_path(&bin))
            .env("CODEMUX_SESSION_ID", "session-guard")
            .status()
            .unwrap();
        assert!(!status.success());
        assert!(std::fs::read_dir(home.join(".codemux-agent-launch"))
            .unwrap()
            .next()
            .is_none());
    }

    #[cfg(unix)]
    #[test]
    fn unix_launch_avoids_a_planted_file_link_without_following_it() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let dir = home.path().join(".codemux-agent-launch");
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let target = home.path().join("target");
        std::fs::write(&target, "keep").unwrap();
        let file = dir.join("codemux-ws-run-droid-settings.json.ABCDEF");
        std::os::unix::fs::symlink(&target, &file).unwrap();

        let path = format!("{LAUNCH_DIR}/codemux-ws-run-droid-settings.json");
        let bin = fake_droid_bin(home.path());
        let launch = droid_injection_command(&path, "droid", "sh");
        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(&launch)
            .env("HOME", home.path().canonicalize().unwrap())
            .env("PATH", fake_path(&bin))
            .env("CODEMUX_SESSION_ID", "session-planted")
            .output()
            .unwrap();
        executed_launch_file(&out);
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "keep");
        assert!(std::fs::symlink_metadata(&file)
            .unwrap()
            .file_type()
            .is_symlink());
    }

    #[cfg(unix)]
    #[test]
    fn unix_existing_launch_entry_does_not_affect_a_fresh_launch() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let home_path = home.path().canonicalize().unwrap();
        let dir = home_path.join(".codemux-agent-launch");
        std::fs::create_dir(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let file = dir.join("existing-droid-settings.json.ABCDEF");
        std::fs::write(&file, "first launch's settings").unwrap();
        let path = format!("{LAUNCH_DIR}/existing-droid-settings.json");
        let bin = fake_droid_bin(&home_path);
        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(droid_injection_command(&path, "droid", "sh"))
            .env("HOME", &home_path)
            .env("PATH", fake_path(&bin))
            .env("CODEMUX_SESSION_ID", "session-reused")
            .output()
            .unwrap();
        executed_launch_file(&out);
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            "first launch's settings"
        );
    }

    #[test]
    fn launch_file_path_drops_shell_metacharacters_from_the_id() {
        let path = launch_file_path("ws\"$(x)`;' 1", "droid-settings.json", TEST_SHELL);
        assert!(path.contains("codemux-wsx1-"), "{path}");
        assert!(path.ends_with("-droid-settings.json"), "{path}");
        assert!(!path.contains("$("), "{path}");
    }

    /// Regression guard: the Gemini injection path MUST resolve its temp
    /// file via `std::env::temp_dir()` — never a hardcoded `/tmp/`. On
    /// Linux `std::env::temp_dir()` typically returns `/tmp/`, so a
    /// hardcoded `/tmp/` would silently "work" locally but break on
    /// Windows (where `%TEMP%` is under `C:\Users\...\AppData\Local\Temp`).
    ///
    /// The test asserts the generated shell command contains the OS-
    /// appropriate temp dir as a prefix of the gemini-system-md path.
    #[cfg(windows)]
    #[test]
    fn test_gemini_context_path_is_cross_platform() {
        let result = inject_agent_context("gemini", "test-ws-xplat");

        // Build what the expected temp path SHOULD look like on this OS.
        let expected_temp = std::env::temp_dir()
            .join("codemux-test-ws-xplat-")
            .to_string_lossy()
            .replace('\'', "''");

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
        assert!(result.contains("codemux-test-ws-xplat-"));
        assert!(result.contains("-gemini-system.md"));
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
