use crate::git::{git_status, scan_files_for_conflict_markers, FileStatus};
use std::path::Path;
use std::process::Command;
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

    let output = match cli {
        "codex" => {
            let mut args = vec!["--quiet".to_string()];
            if let Some(m) = model {
                args.push("--model".to_string());
                args.push(m.to_string());
            }
            args.push(format!("{}\n\n{}", system_prompt, user_prompt));
            AsyncCommand::new("codex")
                .args(&args)
                .current_dir(repo_path)
                .output()
                .await
                .map_err(|e| format!("Failed to run codex: {e}"))?
        }
        "opencode" => {
            let prompt = format!("{}\n\n{}", system_prompt, user_prompt);
            AsyncCommand::new("opencode")
                .args(["--print", &prompt])
                .current_dir(repo_path)
                .output()
                .await
                .map_err(|e| format!("Failed to run opencode: {e}"))?
        }
        _ => {
            // Default: claude
            let mut args = vec![
                "--print".to_string(),
                user_prompt,
                "--system-prompt".to_string(),
                system_prompt,
            ];
            if let Some(m) = model {
                args.push("--model".to_string());
                args.push(m.to_string());
            }
            AsyncCommand::new("claude")
                .args(&args)
                .current_dir(repo_path)
                .output()
                .await
                .map_err(|e| format!("Failed to run claude: {e}"))?
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
}
