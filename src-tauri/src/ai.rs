use std::path::Path;
use std::process::Command;

pub fn claude_available() -> bool {
    Command::new("which")
        .arg("claude")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

pub fn generate_commit_message(repo_path: &Path, model: Option<&str>) -> Result<String, String> {
    let diff = {
        let output = Command::new("git")
            .args(["diff", "--cached"])
            .current_dir(repo_path)
            .output()
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

    let output = Command::new("claude")
        .args(&args)
        .current_dir(repo_path)
        .output()
        .map_err(|e| format!("Failed to run claude: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("claude failed: {}", stderr.trim()));
    }

    Ok(String::from_utf8_lossy(&output.stdout)
        .trim()
        .to_string())
}
