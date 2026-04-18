use crate::git::{git_status, scan_files_for_conflict_markers, FileStatus};
use std::path::Path;
use std::process::Command;
use std::time::Duration;
use tokio::process::Command as AsyncCommand;

pub fn claude_available() -> bool {
    Command::new("which")
        .arg("claude")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn strategy_description(strategy: &str) -> &str {
    match strategy {
        "keep_both" => "Preserve ALL functionality from both sides. Combine changes so nothing is lost.",
        "prefer_ours" => "Keep our (current branch) changes as the baseline. Carefully integrate their changes where they don't conflict with ours.",
        "prefer_theirs" => "Keep their (target branch) changes as the baseline. Carefully integrate our changes where they don't conflict with theirs.",
        _ => "Understand the intent of both changes and write the optimal resolution that preserves all intended functionality.",
    }
}

/// Upper bound on how long the resolver will wait for the agent CLI to
/// finish. Large conflicts on big files can take a while, but if the agent
/// deadlocks (e.g. blocking on an interactive permission prompt despite our
/// skip-permissions flags, or stuck on a network call), we must bail so the
/// UI can surface an error instead of spinning forever.
pub(crate) const RESOLVER_TIMEOUT: Duration = Duration::from_secs(600); // 10 min

/// Builds the argv (program + args) for spawning the given agent CLI in a
/// fully non-interactive, pre-approved mode suitable for a headless merge
/// resolver. Extracted as a pure function so tests can assert the exact
/// flags without spawning a real process.
///
/// Returns `(program, args)`. The caller is responsible for setting
/// `current_dir` and running with a timeout.
///
/// CRITICAL: every branch MUST include the CLI's "skip all permission
/// prompts" flag. Without it the agent blocks on the first Edit/Bash call,
/// the process never exits, and the user sees the resolver hang (or, worse,
/// stdout saying "please grant me permissions" with no file changes — the
/// exact failure mode this function exists to prevent).
pub(crate) fn build_resolver_argv(
    cli: &str,
    model: Option<&str>,
    system_prompt: &str,
    user_prompt: &str,
) -> (&'static str, Vec<String>) {
    match cli {
        "codex" => {
            // `codex exec` is the non-interactive form; bare `codex` starts
            // the TUI. `--full-auto` enables workspace-write sandbox + no
            // approval prompts, matching the `codex --full-auto` pattern
            // used in src-tauri/src/presets.rs for regular sessions.
            let mut args = vec!["exec".to_string(), "--full-auto".to_string()];
            if let Some(m) = model {
                args.push("--model".to_string());
                args.push(m.to_string());
            }
            args.push(format!("{}\n\n{}", system_prompt, user_prompt));
            ("codex", args)
        }
        "opencode" => {
            // `opencode run` is the non-interactive form.
            // `--dangerously-skip-permissions` is documented in
            // `opencode run --help` as "auto-approve permissions that are
            // not explicitly denied".
            let prompt = format!("{}\n\n{}", system_prompt, user_prompt);
            let mut args = vec![
                "run".to_string(),
                "--dangerously-skip-permissions".to_string(),
            ];
            if let Some(m) = model {
                args.push("--model".to_string());
                args.push(m.to_string());
            }
            args.push(prompt);
            ("opencode", args)
        }
        _ => {
            // Default: claude. `--print` is non-interactive.
            // `--dangerously-skip-permissions` is the same flag used across
            // the rest of Codemux (presets.rs, branch_name.rs,
            // agent_context.rs) for headless claude invocations.
            let mut args = vec![
                "--print".to_string(),
                "--dangerously-skip-permissions".to_string(),
                user_prompt.to_string(),
                "--system-prompt".to_string(),
                system_prompt.to_string(),
            ];
            if let Some(m) = model {
                args.push("--model".to_string());
                args.push(m.to_string());
            }
            ("claude", args)
        }
    }
}

pub async fn resolve_conflicts_with_agent(
    repo_path: &Path,
    cli: &str,
    model: Option<&str>,
    strategy: &str,
    files: &[String],
) -> Result<String, String> {
    let file_list = files.join("\n");
    let cwd = repo_path.to_string_lossy();

    let system_prompt = format!(
        "You are resolving git merge conflicts. Strategy: {}\n\n\
         Rules:\n\
         - Read each conflicting file carefully\n\
         - Understand the INTENT of both sides, not just the text\n\
         - After resolving each file, run: git add <file>\n\
         - After all files resolved, run: git diff --cached to show what you did\n\
         - Do NOT commit — the user will review and commit\n\n\
         Conflicting files:\n{}\n\n\
         Working directory: {}",
        strategy_description(strategy),
        file_list,
        cwd
    );

    let user_prompt = format!(
        "Resolve the merge conflicts in these files: {}. \
         Read each file, understand both sides, resolve the conflicts according to the strategy, \
         then git add each resolved file.",
        files.join(", ")
    );

    let (program, args) = build_resolver_argv(cli, model, &system_prompt, &user_prompt);

    // Timeout guard: if the agent deadlocks, bail cleanly instead of
    // leaving the UI spinning forever.
    let spawn = AsyncCommand::new(program)
        .args(&args)
        .current_dir(repo_path)
        .output();
    let output = match tokio::time::timeout(RESOLVER_TIMEOUT, spawn).await {
        Ok(res) => res.map_err(|e| format!("Failed to run {cli}: {e}"))?,
        Err(_) => {
            return Err(format!(
                "{cli} did not finish within {}s. The agent may be stuck on an interactive prompt despite skip-permissions flags. Abort and try again, or switch CLI / model.",
                RESOLVER_TIMEOUT.as_secs()
            ));
        }
    };

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("{} failed: {}", cli, stderr.trim()));
    }

    let agent_stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();

    // ---- Post-agent verification gate ----
    //
    // The agent CLI exiting 0 does NOT mean it actually resolved anything.
    // `claude --print` / `codex --quiet` / `opencode --print` are non-
    // interactive: they may print "I cannot do this" or partially edit a
    // file and still return success. Without this gate the UI advances to
    // "review" and only fails at apply_resolution time, leaving the user
    // with the misleading "Cannot apply resolution: N unresolved conflict(s)"
    // they reported. Catch it here, with a clear message.
    verify_resolution(repo_path, files, cli, &agent_stdout)?;

    Ok(agent_stdout)
}

/// Returns Ok(()) iff:
///   1. `git status` reports zero files with `Conflicted` status, AND
///   2. None of the originally-conflicting files contain `<<<<<<<` /
///      `=======` / `>>>>>>>` markers in their working-tree content
///      (catches the case where the agent ran `git add` on a file that
///      still has markers, which clears the U flag from porcelain output).
///
/// The error message tells the user *which* CLI was used and includes the
/// trimmed agent stdout tail so they can see what the model said when it
/// gave up — without dumping a 50-line transcript into the sidebar.
pub(crate) fn verify_resolution(
    repo_path: &Path,
    files: &[String],
    cli: &str,
    agent_stdout: &str,
) -> Result<(), String> {
    // Check 1: git index still tracking unmerged paths.
    let status = git_status(repo_path).map_err(|e| format!("verify: git status failed: {e}"))?;
    let still_unmerged: Vec<String> = status
        .iter()
        .filter(|f| f.status == FileStatus::Conflicted)
        .map(|f| f.path.clone())
        .collect();

    // Check 2: file content scan over the originally-conflicting files.
    let still_marked = scan_files_for_conflict_markers(repo_path, files);

    if still_unmerged.is_empty() && still_marked.is_empty() {
        return Ok(());
    }

    // Build a useful error. Distinguish the two failure modes so the user
    // (and our future debugging) knows whether the agent did nothing vs.
    // wrote a half-baked resolution.
    let tail: String = agent_stdout
        .lines()
        .rev()
        .take(3)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join(" / ");

    let mut parts = Vec::new();
    if !still_unmerged.is_empty() {
        parts.push(format!(
            "{} file(s) still unmerged ({})",
            still_unmerged.len(),
            still_unmerged.join(", ")
        ));
    }
    if !still_marked.is_empty() {
        parts.push(format!(
            "{} file(s) still contain conflict markers ({})",
            still_marked.len(),
            still_marked.join(", ")
        ));
    }

    Err(format!(
        "{} did not finish the resolution: {}. Agent said: \"{}\"",
        cli,
        parts.join("; "),
        if tail.is_empty() { "(no output)" } else { tail.as_str() }
    ))
}

pub async fn generate_commit_message(
    repo_path: &Path,
    model: Option<&str>,
) -> Result<String, String> {
    let diff = {
        let output = AsyncCommand::new("git")
            .args(["diff", "--cached"])
            .current_dir(repo_path)
            .output()
            .await
            .map_err(|e| format!("Failed to run git diff: {e}"))?;
        if !output.status.success() {
            return Err("Failed to get staged diff".into());
        }
        String::from_utf8_lossy(&output.stdout)
            .trim_end()
            .to_string()
    };

    if diff.is_empty() {
        return Err("No staged changes to describe".into());
    }

    let prompt = format!(
        "Write a concise git commit message for this diff. \
         Use conventional commit format (feat:, fix:, refactor:, etc.). \
         One line, max 72 chars. Return ONLY the message, no quotes, no explanation.\n\n{}",
        diff
    );

    let mut args = vec!["--print".to_string(), prompt];
    if let Some(m) = model {
        args.push("--model".to_string());
        args.push(m.to_string());
    }

    let output = AsyncCommand::new("claude")
        .args(&args)
        .current_dir(repo_path)
        .output()
        .await
        .map_err(|e| format!("Failed to run claude: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("claude failed: {}", stderr.trim()));
    }

    Ok(String::from_utf8_lossy(&output.stdout)
        .trim()
        .to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::TempDir;

    /// Helper: build a real git repo in a temp dir with a real merge
    /// conflict on `shared.txt`. Returns the repo path; cwd is on
    /// `feature` mid-merge with `target` (UU on shared.txt).
    fn setup_conflict() -> (TempDir, PathBuf) {
        let dir = TempDir::new().unwrap();
        let repo = dir.path().to_path_buf();
        let g = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(&repo)
                .output()
                .unwrap()
        };
        g(&["init"]);
        g(&["config", "user.name", "Test"]);
        g(&["config", "user.email", "t@t"]);
        g(&["commit", "--allow-empty", "-m", "base"]);
        let default = String::from_utf8(g(&["branch", "--show-current"]).stdout)
            .unwrap()
            .trim()
            .to_string();
        // target branch
        g(&["checkout", "-b", "target"]);
        std::fs::write(repo.join("shared.txt"), "TARGET\n").unwrap();
        g(&["add", "shared.txt"]);
        g(&["commit", "-m", "target edit"]);
        // feature branch
        g(&["checkout", &default]);
        g(&["checkout", "-b", "feature"]);
        std::fs::write(repo.join("shared.txt"), "FEATURE\n").unwrap();
        g(&["add", "shared.txt"]);
        g(&["commit", "-m", "feature edit"]);
        // Now merge target into feature -> conflict
        g(&["merge", "--no-edit", "target"]);
        (dir, repo)
    }

    #[test]
    fn verify_resolution_rejects_when_agent_did_nothing() {
        // Common failure mode: claude --print exits 0 but the file is
        // untouched. git status still shows UU. We must reject here, not
        // let the UI advance to "review".
        let (_d, repo) = setup_conflict();
        let err = verify_resolution(
            &repo,
            &["shared.txt".to_string()],
            "claude",
            "I cannot help with this.",
        )
        .expect_err("must reject");
        assert!(err.contains("did not finish"), "msg: {err}");
        assert!(err.contains("shared.txt"), "msg should name the file: {err}");
        assert!(err.contains("claude"), "msg should name the CLI: {err}");
    }

    #[test]
    fn verify_resolution_rejects_when_markers_remain_after_git_add() {
        // The masked-failure mode: agent ran `git add` on a file that
        // still has <<<<<<< / =======/ >>>>>>> markers. Porcelain status
        // no longer shows U because of the add — but content still has
        // markers. The content scan must catch it.
        let (_d, repo) = setup_conflict();
        let bad = "<<<<<<< HEAD\nFEATURE\n=======\nTARGET\n>>>>>>> target\n";
        std::fs::write(repo.join("shared.txt"), bad).unwrap();
        std::process::Command::new("git")
            .args(["add", "shared.txt"])
            .current_dir(&repo)
            .output()
            .unwrap();
        let err = verify_resolution(
            &repo,
            &["shared.txt".to_string()],
            "codex",
            "Done!",
        )
        .expect_err("must reject markers");
        assert!(err.contains("conflict markers"), "msg: {err}");
        assert!(err.contains("codex"), "msg should name the CLI: {err}");
    }

    #[test]
    fn verify_resolution_passes_when_truly_resolved() {
        let (_d, repo) = setup_conflict();
        std::fs::write(repo.join("shared.txt"), "RESOLVED\n").unwrap();
        std::process::Command::new("git")
            .args(["add", "shared.txt"])
            .current_dir(&repo)
            .output()
            .unwrap();
        verify_resolution(
            &repo,
            &["shared.txt".to_string()],
            "claude",
            "Resolved by combining both sides.",
        )
        .expect("clean resolution must pass");
    }

    #[test]
    fn verify_resolution_includes_agent_tail_in_error() {
        // The error should surface the last few lines of agent output so
        // the user can see what the model actually said when it gave up.
        let (_d, repo) = setup_conflict();
        let stdout = "line one\nline two\nfinal: cannot determine intent\n";
        let err = verify_resolution(
            &repo,
            &["shared.txt".to_string()],
            "opencode",
            stdout,
        )
        .expect_err("must reject");
        assert!(
            err.contains("cannot determine intent"),
            "tail should be surfaced, got: {err}"
        );
    }

    // ---- Argv construction tests ----
    //
    // These are the tests that should have existed before the
    // permission-prompt regression shipped. Each CLI branch MUST include
    // the flag that bypasses interactive approvals; otherwise the spawned
    // agent deadlocks on its first Edit/Bash call and the user sees the
    // "I need you to grant permissions" failure reported in the bug.
    //
    // Keep these assertions flag-literal (not just "contains permissions")
    // so a rename or typo in the real flag name is caught immediately.

    #[test]
    fn build_argv_claude_passes_dangerously_skip_permissions() {
        let (program, args) = build_resolver_argv(
            "claude",
            None,
            "sys prompt",
            "user prompt",
        );
        assert_eq!(program, "claude");
        assert!(
            args.iter().any(|a| a == "--dangerously-skip-permissions"),
            "claude argv MUST include --dangerously-skip-permissions, got: {args:?}"
        );
        assert!(args.iter().any(|a| a == "--print"), "need --print for non-interactive: {args:?}");
        assert!(args.iter().any(|a| a == "--system-prompt"), "argv: {args:?}");
        assert!(args.iter().any(|a| a == "user prompt"), "argv: {args:?}");
        assert!(args.iter().any(|a| a == "sys prompt"), "argv: {args:?}");
    }

    #[test]
    fn build_argv_claude_appends_model_when_set() {
        let (_, args) = build_resolver_argv(
            "claude",
            Some("sonnet"),
            "sys",
            "user",
        );
        let idx = args.iter().position(|a| a == "--model").expect("model flag missing");
        assert_eq!(args.get(idx + 1).map(String::as_str), Some("sonnet"));
    }

    #[test]
    fn build_argv_claude_omits_model_when_none() {
        let (_, args) = build_resolver_argv("claude", None, "sys", "user");
        assert!(!args.iter().any(|a| a == "--model"), "argv: {args:?}");
    }

    #[test]
    fn build_argv_codex_uses_exec_full_auto() {
        let (program, args) = build_resolver_argv(
            "codex",
            None,
            "sys prompt",
            "user prompt",
        );
        assert_eq!(program, "codex");
        // Bare `codex` starts the TUI; `codex exec` is the non-interactive
        // form. `--full-auto` disables approval prompts (workspace-write
        // sandbox). Without these flags codex hangs waiting for confirmation.
        assert_eq!(args.first().map(String::as_str), Some("exec"), "argv: {args:?}");
        assert!(
            args.iter().any(|a| a == "--full-auto"),
            "codex argv MUST include --full-auto, got: {args:?}"
        );
        // Prompt is concatenated system+user and appended last.
        assert!(
            args.last().map(String::as_str).unwrap_or_default().contains("user prompt"),
            "prompt missing from argv: {args:?}"
        );
        assert!(
            args.last().map(String::as_str).unwrap_or_default().contains("sys prompt"),
            "system prompt missing from argv: {args:?}"
        );
    }

    #[test]
    fn build_argv_codex_appends_model_when_set() {
        let (_, args) = build_resolver_argv(
            "codex",
            Some("gpt-5"),
            "sys",
            "user",
        );
        let idx = args.iter().position(|a| a == "--model").expect("model flag missing");
        assert_eq!(args.get(idx + 1).map(String::as_str), Some("gpt-5"));
    }

    #[test]
    fn build_argv_opencode_passes_dangerously_skip_permissions() {
        let (program, args) = build_resolver_argv(
            "opencode",
            None,
            "sys prompt",
            "user prompt",
        );
        assert_eq!(program, "opencode");
        // Bare `opencode` starts the TUI; `opencode run` is non-interactive.
        assert_eq!(args.first().map(String::as_str), Some("run"), "argv: {args:?}");
        assert!(
            args.iter().any(|a| a == "--dangerously-skip-permissions"),
            "opencode argv MUST include --dangerously-skip-permissions, got: {args:?}"
        );
    }

    #[test]
    fn build_argv_opencode_appends_model_when_set() {
        let (_, args) = build_resolver_argv(
            "opencode",
            Some("anthropic/claude-sonnet-4-6"),
            "sys",
            "user",
        );
        let idx = args.iter().position(|a| a == "--model").expect("model flag missing");
        assert_eq!(args.get(idx + 1).map(String::as_str), Some("anthropic/claude-sonnet-4-6"));
    }

    #[test]
    fn build_argv_unknown_cli_falls_back_to_claude() {
        // Defensive: an unexpected `cli` string from a stale settings file
        // should still produce a safe argv (with skip-permissions), not a
        // bare `--print` that would hang on prompts.
        let (program, args) = build_resolver_argv(
            "some-future-cli",
            None,
            "sys",
            "user",
        );
        assert_eq!(program, "claude");
        assert!(
            args.iter().any(|a| a == "--dangerously-skip-permissions"),
            "fallback argv MUST include --dangerously-skip-permissions, got: {args:?}"
        );
    }

    #[test]
    fn resolver_timeout_is_reasonable() {
        // Guardrail: not so short that a real agent run gets killed, not so
        // long that a stuck agent blocks the UI for the rest of the day.
        let s = RESOLVER_TIMEOUT.as_secs();
        assert!((60..=1800).contains(&s), "timeout {s}s outside [60, 1800]");
    }
}
