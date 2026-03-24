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

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
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
