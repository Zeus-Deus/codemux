use rand::seq::SliceRandom;
use std::collections::HashSet;
use std::path::Path;
use std::time::Duration;

const ADJECTIVES: &[&str] = &[
    "swift", "bold", "calm", "dark", "eager", "fair", "glad", "keen",
    "lean", "mild", "neat", "pale", "rare", "safe", "tall", "vast",
    "warm", "wise", "able", "bare", "cold", "deep", "easy", "fast",
    "good", "hard", "idle", "just", "kind", "lazy", "loud", "main",
    "odd", "open", "pure", "real", "rich", "slim", "soft", "thin",
    "true", "ugly", "void", "weak", "wide", "wild", "worn", "zero",
    "blue", "red",
];

const NOUNS: &[&str] = &[
    "arch", "beam", "bolt", "cape", "chip", "core", "dart", "dock",
    "edge", "fern", "flux", "gate", "grid", "helm", "hive", "iris",
    "jade", "kite", "knot", "lamp", "leaf", "link", "loom", "mast",
    "mesa", "nest", "node", "opal", "palm", "path", "peak", "pine",
    "pond", "rail", "reef", "root", "rust", "sage", "seed", "silo",
    "slab", "span", "stem", "surf", "tide", "vale", "vine", "volt",
    "wave", "zinc",
];

/// Sanitize a string into a valid git branch name.
pub fn sanitize_branch_name(raw: &str) -> String {
    let mut name: String = raw
        .chars()
        .map(|c| match c {
            ' ' | '_' => '-',
            c if c.is_ascii_alphanumeric() || c == '-' || c == '.' => c,
            _ => '\0',
        })
        .filter(|&c| c != '\0')
        .collect();

    // Collapse consecutive hyphens
    while name.contains("--") {
        name = name.replace("--", "-");
    }
    // Collapse consecutive dots
    while name.contains("..") {
        name = name.replace("..", ".");
    }

    // Truncate to 50 chars at a word boundary if possible
    if name.len() > 50 {
        if let Some(pos) = name[..50].rfind('-') {
            name.truncate(pos);
        } else {
            name.truncate(50);
        }
    }

    // Trim leading/trailing hyphens and dots.
    let mut name = name.trim_matches(|c| c == '-' || c == '.').to_string();

    // Git rejects refs ending in ".lock", so strip any such suffix and
    // re-trim any separator it exposes (loop handles e.g. "foo.lock.lock").
    while let Some(stripped) = name.strip_suffix(".lock") {
        name = stripped
            .trim_end_matches(|c| c == '-' || c == '.')
            .to_string();
    }
    name
}

/// Check name against a set of existing branches, appending -2..-99 on conflict.
pub fn deconflict_against(name: &str, existing: &HashSet<String>) -> String {
    if !existing.contains(name) {
        return name.to_string();
    }
    for suffix in 2..=99 {
        let candidate = format!("{name}-{suffix}");
        if !existing.contains(&candidate) {
            return candidate;
        }
    }
    format!("{name}-{}", &uuid::Uuid::new_v4().to_string()[..8])
}

/// Check name against local and remote branches in a git repo.
pub fn deconflict_branch_name(name: &str, repo_path: &Path) -> String {
    let mut existing = HashSet::new();
    if let Ok(local) = crate::git::git_list_branches(repo_path, false) {
        existing.extend(local);
    }
    if let Ok(remote) = crate::git::git_list_branches(repo_path, true) {
        for b in remote {
            // Strip origin/ prefix for comparison
            let stripped = b.strip_prefix("origin/").unwrap_or(&b).to_string();
            existing.insert(stripped);
        }
    }
    deconflict_against(name, &existing)
}

/// Generate a random adjective-noun pair.
pub fn random_name_pair() -> String {
    let mut rng = rand::thread_rng();
    let adj = ADJECTIVES.choose(&mut rng).unwrap_or(&"swift");
    let noun = NOUNS.choose(&mut rng).unwrap_or(&"bolt");
    format!("{adj}-{noun}")
}

/// Generate a random branch name, deconflicted against the repo.
pub fn generate_random_name(repo_path: &Path) -> String {
    let name = random_name_pair();
    deconflict_branch_name(&name, repo_path)
}

/// System prompt for the AI naming call. Kept terse and imperative so the
/// model returns a bare token rather than a chatty sentence — models otherwise
/// prepend prose ("Branch name only, as requested:\n\n...") that used to leak
/// straight into the branch name.
const AI_NAME_SYSTEM_PROMPT: &str = "You generate git branch names. Reply with ONLY a short lowercase hyphenated branch name of 2-4 words describing the task. No prose, no explanation, no punctuation other than hyphens, no code formatting or markdown.";

/// Cap on how much of the user's first message we embed in the naming prompt.
/// The branch name only needs the gist; an overlong message wastes tokens and
/// distracts the model.
const TASK_PROMPT_MAX_CHARS: usize = 400;

/// Nested-session env vars the desktop app's own Claude Code session exports.
/// We scrub them from the spawned naming CLI so it doesn't treat itself as a
/// child of that session (which would pull in the parent's context).
const NESTED_SESSION_ENV_VARS: &[&str] = &[
    "CLAUDECODE",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_SSE_PORT",
];

/// Truncate `s` to at most `max_chars` characters on a char boundary,
/// appending an ellipsis when truncation happened. Uses `char_indices` so we
/// never slice through a multi-byte UTF-8 sequence (prompts are free text).
fn truncate_on_char_boundary(s: &str, max_chars: usize) -> String {
    match s.char_indices().nth(max_chars) {
        Some((byte_idx, _)) => format!("{}…", &s[..byte_idx]),
        None => s.to_string(),
    }
}

/// Extract a plausible branch-name token from raw CLI stdout.
///
/// Even with a strict system prompt, models sometimes wrap the answer in prose
/// ("Branch name only:\n\nfoo-bar"), backticks, or `**bold**`. We scan
/// non-empty lines, strip that decoration, and keep only lines that look like a
/// single branch token: no internal whitespace and 2..=80 chars. The real
/// answer is almost always last, after any preamble, so we return the LAST
/// qualifying line. Returns `None` when nothing qualifies — the caller treats
/// that like a CLI failure and falls back to a random name.
pub fn extract_branch_candidate(raw: &str) -> Option<String> {
    let mut candidate = None;
    for line in raw.lines() {
        // Strip surrounding whitespace, then code/emphasis decoration, then
        // whitespace again in case the decoration hid some.
        let stripped = line
            .trim()
            .trim_matches('`')
            .trim_start_matches("**")
            .trim_end_matches("**")
            .trim();
        if stripped.is_empty() || stripped.chars().any(char::is_whitespace) {
            continue;
        }
        let len = stripped.chars().count();
        if !(2..=80).contains(&len) {
            continue;
        }
        candidate = Some(stripped.to_string());
    }
    candidate
}

/// Use an AI CLI to generate a branch name from a prompt.
/// Falls back to random on any failure.
pub async fn generate_ai_name(prompt: &str, repo_path: &Path) -> String {
    let task = truncate_on_char_boundary(prompt, TASK_PROMPT_MAX_CHARS);
    let meta_prompt = format!("Generate a git branch name for this task: {task}");

    // Run hermetically: a neutral cwd, no inherited settings/memory/CLAUDE.md,
    // no tools, no session persistence. Without this the CLI ran a full session
    // in the app's cwd and pulled that directory's auto-memory/CLAUDE.md into
    // the name, producing branch names about unrelated topics. Per-flag "why"
    // follows in the args list.
    let cwd = std::env::temp_dir();
    let args = [
        "--print",
        // Load no user/project settings: no CLAUDE.md, auto-memory, skills,
        // plugins, or MCP servers can bleed into the branch name.
        "--setting-sources",
        "",
        // Don't write a transcript into ~/.claude/projects for these throwaway
        // calls.
        "--no-session-persistence",
        // The model only needs to emit text; deny all tools.
        "--disallowedTools",
        "*",
        "--system-prompt",
        AI_NAME_SYSTEM_PROMPT,
    ];

    match try_ai_cli("claude", &args, &meta_prompt, NESTED_SESSION_ENV_VARS, &cwd).await {
        Ok(raw) => match extract_branch_candidate(&raw) {
            Some(candidate) => {
                let sanitized = sanitize_branch_name(&candidate);
                if !sanitized.is_empty() {
                    return deconflict_branch_name(&sanitized, repo_path);
                }
            }
            None => {
                let excerpt: String = raw.trim().chars().take(120).collect();
                log::warn!(
                    "AI branch naming produced no usable name, falling back to random: {excerpt:?}"
                );
            }
        },
        Err(reason) => {
            log::warn!("AI branch naming failed, falling back to random name: {reason}");
        }
    }

    // Fallback to random
    generate_random_name(repo_path)
}

/// AI naming timeout. Warm `claude --print` calls measured at 5-9s on this
/// machine; 30s gives real headroom for cold starts / network latency while
/// still bounding a hung CLI (the child is killed on timeout, see
/// `kill_on_drop` below).
const AI_CLI_TIMEOUT: Duration = Duration::from_secs(30);

/// Number of stderr bytes to keep in failure logs (claude CLI's error
/// reasons are usually short; this avoids dumping huge output into logs).
const STDERR_EXCERPT_LEN: usize = 200;

/// Run an AI CLI with `prompt` on stdin. Returns the trimmed stdout on
/// success, or `Err(reason)` describing why it failed (spawn error, timeout,
/// non-zero exit, empty output) so callers can log a diagnosable reason.
///
/// `env_scrub` names env vars to remove from the child (e.g. inherited
/// nested-session markers), and `cwd` sets its working directory — both keep
/// the call from picking up context from the desktop app's environment.
async fn try_ai_cli(
    binary: &str,
    args: &[&str],
    prompt: &str,
    env_scrub: &[&str],
    cwd: &Path,
) -> Result<String, String> {
    let mut cmd = tokio::process::Command::new(binary);
    cmd.args(args);
    cmd.current_dir(cwd);
    for var in env_scrub {
        cmd.env_remove(var);
    }
    cmd.stdin(std::process::Stdio::piped());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    // Reap the child if we time out below, rather than leaking it.
    cmd.kill_on_drop(true);

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("failed to spawn `{binary}`: {e}"))?;

    // Write prompt to stdin
    if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        let _ = stdin.write_all(prompt.as_bytes()).await;
        drop(stdin);
    }

    let result = tokio::time::timeout(AI_CLI_TIMEOUT, child.wait_with_output()).await;

    let output = match result {
        Ok(Ok(output)) => output,
        Ok(Err(e)) => return Err(format!("`{binary}` process error: {e}")),
        Err(_) => {
            return Err(format!(
                "`{binary}` timed out after {}s",
                AI_CLI_TIMEOUT.as_secs()
            ));
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let excerpt: String = stderr.trim().chars().take(STDERR_EXCERPT_LEN).collect();
        return Err(format!(
            "`{binary}` exited with {}: {excerpt}",
            output.status
        ));
    }

    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if text.is_empty() {
        return Err(format!("`{binary}` produced empty output"));
    }
    Ok(text)
}

/// Escape a string for safe embedding in double-quoted shell arguments.
pub fn shell_escape_for_double_quotes(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('$', "\\$")
        .replace('`', "\\`")
        .replace('!', "\\!")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

/// Escape a string for safe embedding in `$'...'` (ANSI-C quoted) shell arguments.
/// Unlike double-quote escaping, `$'...'` interprets `\n` as a real newline,
/// so multi-line prompts are preserved correctly.
pub fn shell_escape_for_ansi_c_quotes(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('\'', "\\'")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

/// Build the agent launch command, optionally embedding the prompt.
/// Returns (command, needs_pty_injection).
/// If needs_pty_injection is true, the prompt should be written to PTY after agent startup.
pub fn prepare_agent_command(
    preset_id: &str,
    base_command: &str,
    initial_prompt: Option<&str>,
) -> (String, bool) {
    let prompt = match initial_prompt {
        Some(p) if !p.trim().is_empty() => p,
        _ => return (base_command.to_string(), false),
    };

    let escaped = shell_escape_for_ansi_c_quotes(prompt);

    match preset_id {
        "builtin-claude" => {
            // Claude CLI: append prompt as ANSI-C quoted argument (preserves newlines)
            (format!("{base_command} $'{escaped}'"), false)
        }
        "builtin-codex" => {
            // Codex: prompt as positional argument
            (format!("{base_command} $'{escaped}'"), false)
        }
        _ => {
            // Gemini, Antigravity, Copilot, Cursor Agent, Amp, Grok,
            // Droid, OpenCode, custom: inject prompt via PTY after startup
            (base_command.to_string(), true)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_basic() {
        assert_eq!(sanitize_branch_name("Fix Login Bug"), "Fix-Login-Bug");
    }

    #[test]
    fn sanitize_special_chars() {
        assert_eq!(
            sanitize_branch_name("feat: add @user auth!"),
            "feat-add-user-auth"
        );
    }

    #[test]
    fn sanitize_max_length() {
        let long = "a-".repeat(60);
        assert!(sanitize_branch_name(&long).len() <= 50);
    }

    #[test]
    fn sanitize_preserves_dots_and_hyphens() {
        assert_eq!(sanitize_branch_name("v1.2-beta"), "v1.2-beta");
    }

    #[test]
    fn sanitize_collapses_consecutive_hyphens() {
        assert_eq!(sanitize_branch_name("foo---bar"), "foo-bar");
    }

    #[test]
    fn sanitize_collapses_consecutive_dots() {
        assert_eq!(sanitize_branch_name("foo...bar"), "foo.bar");
    }

    #[test]
    fn sanitize_trims_leading_trailing() {
        assert_eq!(sanitize_branch_name("-foo-bar-"), "foo-bar");
        assert_eq!(sanitize_branch_name(".foo.bar."), "foo.bar");
    }

    #[test]
    fn sanitize_strips_trailing_lock_suffix() {
        // Git forbids refs ending in ".lock".
        assert_eq!(sanitize_branch_name("update.lock"), "update");
        assert_eq!(sanitize_branch_name("hotfix.lock.lock"), "hotfix");
        assert!(!sanitize_branch_name("release.lock").ends_with(".lock"));
        // Only the trailing suffix is stripped — ".lock" elsewhere stays.
        assert_eq!(sanitize_branch_name("my.lockfile"), "my.lockfile");
    }

    #[test]
    fn sanitize_uppercase() {
        assert_eq!(sanitize_branch_name("FIX-BUG"), "FIX-BUG");
    }

    #[test]
    fn sanitize_mixed_case_preserved() {
        assert_eq!(sanitize_branch_name("Feature/MyBranch"), "FeatureMyBranch");
    }

    #[test]
    fn sanitize_camel_case() {
        assert_eq!(sanitize_branch_name("fixLoginPage"), "fixLoginPage");
    }

    #[test]
    fn sanitize_empty_input() {
        assert_eq!(sanitize_branch_name(""), "");
    }

    #[test]
    fn random_name_pair_format() {
        let name = random_name_pair();
        assert!(name.contains('-'));
        let parts: Vec<&str> = name.split('-').collect();
        assert_eq!(parts.len(), 2);
        assert!(ADJECTIVES.contains(&parts[0]));
        assert!(NOUNS.contains(&parts[1]));
    }

    #[test]
    fn random_name_pair_varies() {
        let names: Vec<String> = (0..20).map(|_| random_name_pair()).collect();
        let unique: HashSet<&String> = names.iter().collect();
        assert!(unique.len() >= 2);
    }

    #[test]
    fn deconflict_no_conflict() {
        let existing = HashSet::new();
        assert_eq!(deconflict_against("foo", &existing), "foo");
    }

    #[test]
    fn deconflict_first_conflict() {
        let existing: HashSet<String> = ["foo".to_string()].into();
        assert_eq!(deconflict_against("foo", &existing), "foo-2");
    }

    #[test]
    fn deconflict_multiple_conflicts() {
        let existing: HashSet<String> = ["foo", "foo-2", "foo-3"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(deconflict_against("foo", &existing), "foo-4");
    }

    #[test]
    fn prepare_claude_with_prompt() {
        let (cmd, via_pty) = prepare_agent_command(
            "builtin-claude",
            "claude --dangerously-skip-permissions",
            Some("fix the login bug"),
        );
        assert!(cmd.contains("fix the login bug"));
        assert!(cmd.contains("$'"));
        assert!(!via_pty);
    }

    #[test]
    fn prepare_claude_with_multiline_prompt() {
        let (cmd, via_pty) = prepare_agent_command(
            "builtin-claude",
            "claude --dangerously-skip-permissions",
            Some("Issue #42: broken\nStatus: Open\n\n---\nFix it"),
        );
        // Uses $'...' ANSI-C quoting so bash interprets \n as real newlines
        assert!(cmd.starts_with("claude --dangerously-skip-permissions $'"));
        assert!(cmd.contains("\\n"));
        assert!(!cmd.contains('\n')); // no literal newline bytes in the command
        assert!(!via_pty);
    }

    #[test]
    fn prepare_gemini_via_pty() {
        let (cmd, via_pty) = prepare_agent_command(
            "builtin-gemini",
            "gemini --yolo",
            Some("fix the login bug"),
        );
        assert_eq!(cmd, "gemini --yolo");
        assert!(via_pty);
    }

    #[test]
    fn prepare_no_prompt_no_change() {
        let (cmd, via_pty) = prepare_agent_command(
            "builtin-claude",
            "claude --dangerously-skip-permissions",
            None,
        );
        assert_eq!(cmd, "claude --dangerously-skip-permissions");
        assert!(!via_pty);
    }

    #[test]
    fn shell_escape_double_quotes() {
        assert_eq!(
            shell_escape_for_double_quotes(r#"say "hello""#),
            r#"say \"hello\""#
        );
    }

    #[test]
    fn shell_escape_double_dollar() {
        assert_eq!(
            shell_escape_for_double_quotes("use $HOME"),
            "use \\$HOME"
        );
    }

    #[test]
    fn shell_escape_double_newlines() {
        assert_eq!(
            shell_escape_for_double_quotes("line1\nline2\nline3"),
            "line1\\nline2\\nline3"
        );
    }

    #[test]
    fn shell_escape_double_carriage_return() {
        assert_eq!(
            shell_escape_for_double_quotes("line1\r\nline2"),
            "line1\\r\\nline2"
        );
    }

    #[test]
    fn ansi_c_escape_single_quotes() {
        assert_eq!(
            shell_escape_for_ansi_c_quotes("it's a test"),
            "it\\'s a test"
        );
    }

    #[test]
    fn ansi_c_escape_newlines() {
        assert_eq!(
            shell_escape_for_ansi_c_quotes("line1\nline2\nline3"),
            "line1\\nline2\\nline3"
        );
    }

    #[test]
    fn ansi_c_escape_preserves_dollar_and_backtick() {
        // $'...' does not expand $vars or `backticks`, so they need no escaping
        assert_eq!(
            shell_escape_for_ansi_c_quotes("use $HOME and `cmd`"),
            "use $HOME and `cmd`"
        );
    }

    #[test]
    fn ansi_c_escape_backslash_before_newline() {
        // Existing backslash must be escaped BEFORE newline replacement
        // Input: literal \ then newline then text
        assert_eq!(
            shell_escape_for_ansi_c_quotes("path\\\nline2"),
            "path\\\\\\nline2"
        );
        // bash $'path\\\\\\nline2' → path\ + newline + line2
    }

    #[tokio::test]
    async fn try_ai_cli_reports_spawn_failure_reason() {
        // No network/real-CLI dependency: this binary should never exist.
        let result = try_ai_cli(
            "codemux-definitely-not-a-real-binary",
            &["--print"],
            "prompt",
            &[],
            &std::env::temp_dir(),
        )
        .await;
        let err = result.expect_err("nonexistent binary must fail to spawn");
        assert!(
            err.contains("failed to spawn"),
            "expected spawn-failure reason, got: {err}"
        );
    }

    #[test]
    fn extract_plain_single_line_name() {
        assert_eq!(
            extract_branch_candidate("add-user-auth"),
            Some("add-user-auth".to_string())
        );
    }

    #[test]
    fn extract_strips_leading_prose_line() {
        assert_eq!(
            extract_branch_candidate("Branch name only, as requested:\n\nsome-feature-name"),
            Some("some-feature-name".to_string())
        );
    }

    #[test]
    fn extract_unwraps_backticked_name_after_prose() {
        assert_eq!(
            extract_branch_candidate(
                "The user only wants a branch name back.\n\n`fix-review-feedback`"
            ),
            Some("fix-review-feedback".to_string())
        );
    }

    #[test]
    fn extract_unwraps_bold_name() {
        assert_eq!(
            extract_branch_candidate("**investigate-full-access-approvals**"),
            Some("investigate-full-access-approvals".to_string())
        );
    }

    #[test]
    fn extract_rejects_pure_prose() {
        // Every line has internal whitespace, so nothing qualifies.
        assert_eq!(
            extract_branch_candidate("Sure, here you go.\nHappy to help with that."),
            None
        );
    }

    #[test]
    fn extract_rejects_empty_and_whitespace() {
        assert_eq!(extract_branch_candidate(""), None);
        assert_eq!(extract_branch_candidate("   \n\t\n  "), None);
    }

    #[test]
    fn extract_picks_last_plausible_over_trailing_prose() {
        // The name comes first, then a prose sign-off; the prose line has
        // whitespace and is skipped, so the name still wins.
        assert_eq!(
            extract_branch_candidate("some-name\nHope that helps!"),
            Some("some-name".to_string())
        );
    }

    #[test]
    fn truncate_on_char_boundary_short_input_unchanged() {
        assert_eq!(truncate_on_char_boundary("short task", 400), "short task");
    }

    #[test]
    fn truncate_on_char_boundary_appends_ellipsis() {
        let input = "a".repeat(500);
        let out = truncate_on_char_boundary(&input, 400);
        assert_eq!(out.chars().count(), 401); // 400 chars + ellipsis
        assert!(out.ends_with('…'));
    }

    #[test]
    fn truncate_on_char_boundary_respects_multibyte() {
        // Multi-byte chars must not be split mid-sequence.
        let input = "🚀".repeat(10);
        let out = truncate_on_char_boundary(&input, 4);
        assert_eq!(out, "🚀🚀🚀🚀…");
    }

    #[test]
    fn ansi_c_escape_multiline_issue_prompt() {
        let prompt = "Issue #42: Backend broken\nStatus: Open\n\nDescription:\nThe API returns 500\n\n---\nFix the bug";
        let escaped = shell_escape_for_ansi_c_quotes(prompt);
        assert!(!escaped.contains('\n')); // no literal newline bytes
        assert!(escaped.contains("\\n")); // escaped newlines present
        assert!(escaped.contains("Issue #42"));
        assert!(escaped.contains("Fix the bug"));
    }
}
