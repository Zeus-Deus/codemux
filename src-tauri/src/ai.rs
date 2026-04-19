use crate::git::{git_status, scan_files_for_conflict_markers, FileStatus};
use std::path::Path;
use std::process::{Command, Output, Stdio};
use std::time::Duration;
use tokio::process::Command as AsyncCommand;

pub fn claude_available() -> bool {
    let mut cmd = Command::new("which");
    cmd.arg("claude");
    crate::execution::sanitize_gui_env_std(&mut cmd);
    cmd.output()
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

/// Outcome of attempting to run an agent CLI from the merge resolver. Split
/// from `String`-typed errors so the caller can surface the "stuck on
/// interactive prompt" timeout message using the real CLI name, regardless
/// of whether the helper is called for `resolve_conflicts_with_agent` or
/// `generate_commit_message`.
#[derive(Debug)]
pub(crate) enum ResolverRunError {
    /// Spawn itself failed (binary missing, permission denied, etc.).
    Spawn(String),
    /// Spawn succeeded but collecting output failed mid-run.
    Io(String),
    /// The command exceeded the supplied timeout. The child was SIGKILLed via
    /// `kill_on_drop(true)` when the future was dropped, so no process is left
    /// behind — callers can safely retry.
    Timeout,
}

/// Spawns an agent CLI and waits for it to finish, with three non-negotiable
/// guarantees:
///
/// 1. **stdin is `/dev/null`** — inheriting the parent's stdin is what caused
///    the "claude did not finish within 600s. The agent may be stuck on an
///    interactive prompt despite skip-permissions flags." bug. Claude Code
///    (and similar CLIs) probe stdin and block waiting for input/EOF that
///    never arrives when stdin is inherited from a Tauri GUI parent. See
///    anthropics/claude-code#43123 and #16306. `--print` /
///    `--dangerously-skip-permissions` do NOT override this — they govern
///    tool-approval prompts, not stdin EOF detection.
///
/// 2. **stdout/stderr are explicitly piped** so we can surface them to the
///    user (verification tail, error reporting).
///
/// 3. **`kill_on_drop(true)`** — when `tokio::time::timeout` fires and drops
///    the wait future, the `Child` is dropped, and tokio SIGKILLs the spawned
///    process. Without this, each "Try Again" leaks an orphan agent process
///    that keeps running detached, accumulating zombies for the life of the
///    Codemux session.
///
/// This is extracted as a dedicated helper (instead of being inlined) so that
/// regression tests can drive it with a cheap binary like `/bin/sh` or
/// `/bin/cat` without having to stand up a fake `claude`.
pub(crate) async fn run_resolver_cli(
    program: &str,
    args: &[String],
    repo_path: &Path,
    timeout: Duration,
) -> Result<Output, ResolverRunError> {
    let mut cmd = AsyncCommand::new(program);
    cmd.args(args)
        .current_dir(repo_path)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    crate::execution::sanitize_gui_env_tokio(&mut cmd);
    let child = cmd
        .spawn()
        .map_err(|e| ResolverRunError::Spawn(e.to_string()))?;

    match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(out)) => Ok(out),
        Ok(Err(e)) => Err(ResolverRunError::Io(e.to_string())),
        // Dropping `wait_with_output`'s future here also drops the Child,
        // which triggers SIGKILL thanks to `kill_on_drop(true)`. Do not
        // remove — see the module-level bug history.
        Err(_) => Err(ResolverRunError::Timeout),
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

    // Timeout guard + stdin-null + kill_on_drop all live inside
    // `run_resolver_cli`. Do NOT inline this spawn — every previous attempt
    // to do so has regressed one of those three properties.
    let output = match run_resolver_cli(program, &args, repo_path, RESOLVER_TIMEOUT).await {
        Ok(out) => out,
        Err(ResolverRunError::Spawn(e)) | Err(ResolverRunError::Io(e)) => {
            return Err(format!("Failed to run {cli}: {e}"));
        }
        Err(ResolverRunError::Timeout) => {
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
        let mut cmd = AsyncCommand::new("git");
        cmd.args(["diff", "--cached"])
            .current_dir(repo_path)
            .stdin(Stdio::null());
        crate::execution::sanitize_gui_env_tokio(&mut cmd);
        let output = cmd
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

    // Same headless-claude requirements as the resolver: must include
    // `--dangerously-skip-permissions` (so a stale permission prompt can't
    // deadlock the call) and must go through `run_resolver_cli` so stdin is
    // /dev/null and the child is killed if it hangs past the timeout. Prior
    // to this, `generate_commit_message` had NO timeout at all — a hung
    // claude would leave the commit-message UI spinning indefinitely.
    let mut args = vec![
        "--print".to_string(),
        "--dangerously-skip-permissions".to_string(),
        prompt,
    ];
    if let Some(m) = model {
        args.push("--model".to_string());
        args.push(m.to_string());
    }

    let output = match run_resolver_cli("claude", &args, repo_path, RESOLVER_TIMEOUT).await {
        Ok(out) => out,
        Err(ResolverRunError::Spawn(e)) | Err(ResolverRunError::Io(e)) => {
            return Err(format!("Failed to run claude: {e}"));
        }
        Err(ResolverRunError::Timeout) => {
            // Keep the message shape aligned with the resolver's timeout
            // error so the UI (and user mental model) stays consistent
            // across the two entry points that go through `run_resolver_cli`.
            return Err(format!(
                "claude did not finish within {}s. The agent may be stuck on an interactive prompt despite skip-permissions flags. Abort and try again, or switch CLI / model.",
                RESOLVER_TIMEOUT.as_secs()
            ));
        }
    };

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

    // ---- run_resolver_cli regression tests ----
    //
    // These three tests encode the contract of the spawn helper that the
    // 600s-hang bug depended on. If any of them ever starts failing, the
    // bug is back.

    #[tokio::test]
    #[cfg(unix)]
    async fn run_resolver_cli_closes_stdin() {
        // `cat` with no arguments reads stdin until EOF and echoes it. If
        // stdin were inherited from the test harness, it would either be
        // a TTY (block forever) or the harness's pipe (block until the
        // harness closes its end, which is never during the test).
        //
        // With `Stdio::null()`, cat sees EOF immediately, exits 0, and
        // produces empty output. This is the exact behavior we need for
        // `claude --print` — which otherwise hangs inheriting Tauri's
        // unusable stdin (see anthropics/claude-code#43123).
        let dir = TempDir::new().unwrap();
        let out = tokio::time::timeout(
            Duration::from_secs(3),
            run_resolver_cli("cat", &[], dir.path(), Duration::from_secs(2)),
        )
        .await
        .expect("helper itself must not hang even if stdin handling broke");

        let out = out.expect("cat should exit cleanly, not error");
        assert!(
            out.status.success(),
            "cat exit should be 0 (EOF on stdin), got: {:?}",
            out.status
        );
        assert!(
            out.stdout.is_empty(),
            "cat should output nothing when stdin is /dev/null, got: {:?}",
            String::from_utf8_lossy(&out.stdout)
        );
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn run_resolver_cli_kills_child_on_timeout() {
        // Spawns a shell that sleeps far longer than the supplied timeout.
        // If `kill_on_drop(true)` were missing, the child would keep running
        // detached after the timeout — the 600s hang's silent side effect.
        // We assert the helper returns Timeout quickly; the Child is dropped
        // when `wait_with_output`'s future is dropped, which SIGKILLs the
        // shell. (We don't assert on the OS process table because that's
        // racy across platforms, but the helper returning fast is what
        // matters for the UX.)
        let dir = TempDir::new().unwrap();
        let start = std::time::Instant::now();
        let res = run_resolver_cli(
            "sh",
            &["-c".to_string(), "sleep 30".to_string()],
            dir.path(),
            Duration::from_millis(200),
        )
        .await;
        let elapsed = start.elapsed();

        assert!(
            matches!(res, Err(ResolverRunError::Timeout)),
            "expected Timeout, got: {res:?}"
        );
        // Timeout was 200ms; if kill_on_drop didn't fire we'd hang for 30s.
        // Allow generous slack for slow CI but catch a real regression.
        assert!(
            elapsed < Duration::from_secs(5),
            "helper should return promptly after timeout, took {elapsed:?}"
        );
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn run_resolver_cli_happy_path_captures_stdout() {
        // Sanity: a fast-exiting command reports its stdout so the
        // verification layer can scan agent output.
        let dir = TempDir::new().unwrap();
        let out = run_resolver_cli(
            "sh",
            &["-c".to_string(), "printf hello".to_string()],
            dir.path(),
            Duration::from_secs(5),
        )
        .await
        .expect("fast command must succeed");
        assert!(out.status.success());
        assert_eq!(&out.stdout, b"hello");
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn run_resolver_cli_kills_child_on_timeout_pid_proof() {
        // Empirical proof that the sleep child is actually reaped after the
        // timeout fires — not just that the helper returns fast. We capture
        // the shell's own PID (via `echo $$`) into a temp file before it
        // execs `sleep`, then after the helper times out we poll `kill -0`
        // until the kernel reports ESRCH (process gone).
        let dir = TempDir::new().unwrap();
        let pid_file = dir.path().join("pid.txt");

        // `echo $$ > pid; exec sleep 60` — after exec, the sleep process
        // keeps the same PID as the shell, so the file holds the right PID.
        let script = format!(
            "echo $$ > {pid_path}; exec sleep 60",
            pid_path = pid_file.display(),
        );

        let start = std::time::Instant::now();
        let res = run_resolver_cli(
            "sh",
            &["-c".to_string(), script],
            dir.path(),
            Duration::from_millis(300),
        )
        .await;
        let elapsed = start.elapsed();

        assert!(
            matches!(res, Err(ResolverRunError::Timeout)),
            "expected Timeout, got: {res:?}"
        );
        assert!(
            elapsed < Duration::from_secs(5),
            "helper should return promptly after timeout, took {elapsed:?}"
        );

        // The shell writes its PID synchronously before exec; the file must
        // exist. If it doesn't, the shell never got far enough — not the
        // scenario we want to test.
        let pid_str = std::fs::read_to_string(&pid_file)
            .expect("shell should have written its pid before exec'ing sleep");
        let pid: i32 = pid_str.trim().parse().expect("pid file should contain an integer");

        // Poll `/proc/<pid>/stat` up to 2s. We want to prove the process is
        // no longer *running* (state Z = zombie or gone entirely counts as
        // proof that SIGKILL was delivered). `kill -0` alone can't
        // distinguish a zombie from a live process: both return success.
        //
        // The zombie may linger briefly because `wait_with_output`'s future
        // was dropped on timeout — there's no one to reap the SIGCHLD until
        // tokio's signal driver gets around to it. That's fine from the
        // hang-bug perspective (the process is no longer *executing*), but
        // we need the more precise check to say so.
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        let stat_path = format!("/proc/{pid}/stat");
        let final_state: Option<String> = loop {
            match std::fs::read_to_string(&stat_path) {
                Err(_) => {
                    // /proc/<pid> gone entirely — reaped. Best outcome.
                    break None;
                }
                Ok(contents) => {
                    // Format: "<pid> (<comm>) <state> ..."
                    let state = contents
                        .rsplit(')')
                        .next()
                        .and_then(|tail| tail.split_whitespace().next())
                        .unwrap_or("?")
                        .to_string();
                    if state == "Z" {
                        // zombie = SIGKILL delivered, process no longer running
                        break Some(state);
                    }
                    // Still R/S/D/T — not dead yet, keep polling.
                    if std::time::Instant::now() >= deadline {
                        break Some(state);
                    }
                    std::thread::sleep(Duration::from_millis(25));
                }
            }
        };

        match final_state.as_deref() {
            None => { /* process fully reaped — kill_on_drop fired and SIGCHLD was handled */ }
            Some("Z") => { /* zombie — SIGKILL delivered, process not executing */ }
            Some(other) => panic!(
                "sleep child (pid {pid}) still in state {other:?} 2s after timeout — \
                 kill_on_drop didn't fire (expected reaped or Z zombie)"
            ),
        }

        // Also do the user-friendly check: was the 'SHOULD_NOT_PRINT'-style
        // side effect avoided? The script doesn't have one, but we can
        // verify the helper's elapsed time is nowhere near the sleep's 60s.
        assert!(
            elapsed < Duration::from_secs(2),
            "sleep 60 ran to completion — kill_on_drop regressed"
        );
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn run_resolver_cli_surfaces_spawn_error_for_missing_binary() {
        // If the user configured a CLI that isn't installed, we must
        // return a Spawn error (which the caller maps to "Failed to run
        // {cli}") — not hang, not silently succeed.
        let dir = TempDir::new().unwrap();
        let res = run_resolver_cli(
            "codemux-no-such-binary-xyz",
            &[],
            dir.path(),
            Duration::from_secs(5),
        )
        .await;
        assert!(
            matches!(res, Err(ResolverRunError::Spawn(_))),
            "expected Spawn error, got: {res:?}"
        );
    }
}
