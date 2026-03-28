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
        .to_lowercase()
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

    // Trim leading/trailing hyphens and dots
    name.trim_matches(|c| c == '-' || c == '.').to_string()
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

/// Use an AI CLI to generate a branch name from a prompt.
/// Falls back to random on any failure.
pub async fn generate_ai_name(prompt: &str, repo_path: &Path) -> String {
    let meta_prompt = format!(
        "Generate a short git branch name (2-4 words, hyphenated, lowercase) for this task: {}. Return ONLY the branch name, nothing else.",
        prompt
    );

    // Try claude first
    if let Some(name) = try_ai_cli("claude", &["--print"], &meta_prompt).await {
        let sanitized = sanitize_branch_name(&name);
        if !sanitized.is_empty() {
            return deconflict_branch_name(&sanitized, repo_path);
        }
    }

    // Fallback to random
    generate_random_name(repo_path)
}

async fn try_ai_cli(binary: &str, args: &[&str], prompt: &str) -> Option<String> {
    let mut cmd = tokio::process::Command::new(binary);
    cmd.args(args);
    cmd.stdin(std::process::Stdio::piped());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::null());

    let mut child = cmd.spawn().ok()?;

    // Write prompt to stdin
    if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        let _ = stdin.write_all(prompt.as_bytes()).await;
        drop(stdin);
    }

    let result = tokio::time::timeout(Duration::from_secs(10), child.wait_with_output()).await;

    match result {
        Ok(Ok(output)) if output.status.success() => {
            let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if text.is_empty() { None } else { Some(text) }
        }
        _ => None,
    }
}

/// Escape a string for safe embedding in double-quoted shell arguments.
pub fn shell_escape_for_double_quotes(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('$', "\\$")
        .replace('`', "\\`")
        .replace('!', "\\!")
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

    let escaped = shell_escape_for_double_quotes(prompt);

    match preset_id {
        "builtin-claude" => {
            // Claude CLI: append prompt as initial message argument
            (format!("{base_command} \"{escaped}\""), false)
        }
        "builtin-codex" => {
            // Codex: prompt as positional argument
            (format!("{base_command} \"{escaped}\""), false)
        }
        _ => {
            // Gemini, OpenCode, custom: inject prompt via PTY after startup
            (base_command.to_string(), true)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_basic() {
        assert_eq!(sanitize_branch_name("Fix Login Bug"), "fix-login-bug");
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
    fn sanitize_uppercase() {
        assert_eq!(sanitize_branch_name("FIX-BUG"), "fix-bug");
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
    fn shell_escape_quotes() {
        assert_eq!(
            shell_escape_for_double_quotes(r#"say "hello""#),
            r#"say \"hello\""#
        );
    }

    #[test]
    fn shell_escape_dollar() {
        assert_eq!(
            shell_escape_for_double_quotes("use $HOME"),
            "use \\$HOME"
        );
    }
}
