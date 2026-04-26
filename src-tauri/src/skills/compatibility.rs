// Tool-compatibility classifier.
//
// At scan time we score every skill into one of three buckets — compatible,
// soft-warn, hard-warn — based on text-level signals in the SKILL.md body
// and frontmatter. The frontend uses the bucket to decide which chip to
// show in the slash popup. The slash popup never blocks invocation; the
// chip is purely informational.
//
// Stage 5 polish can refine these heuristics once we observe real false
// positives and negatives. Stage 1 ships a deliberately simple regex set.

use std::sync::LazyLock;

use regex::Regex;
use serde_json::Value as JsonValue;

use super::{SkillCompatibility, SkillProvider};

// HARD signals — strongly indicate the skill won't work outside its origin
// provider.
//   `mcp__` — namespaced MCP tool calls (provider-specific tool names)
//   `$CODEX_HOME` — Codex-rooted absolute paths
//   `~/.claude/`, `~/.codex/` — provider-rooted absolute paths in body
static HARD_MCP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"mcp__").unwrap());
static HARD_CODEX_HOME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\$CODEX_HOME").unwrap());
static HARD_CLAUDE_PATH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"~/\.claude/").unwrap());
static HARD_CODEX_PATH: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"~/\.codex/").unwrap());

// SOFT signals — skill may invoke tools that aren't available in the
// current session, but the wording is generic enough that a different
// provider can probably still execute it.
//   ```bash``` blocks — concrete shell snippets
//   CLI tool names (gh, docker, pacman, makepkg, ssh) — any of these as a
//      whole word
//   sibling-file references — `references/`, `scripts/`, `assets/`
static SOFT_BASH_BLOCK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"```bash\b").unwrap());
static SOFT_CLI_TOOLS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b(gh|docker|pacman|makepkg|ssh)\b").unwrap());
static SOFT_SIBLING_FILES: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b(scripts|references|assets)/").unwrap());

pub fn classify_compatibility(
    body: &str,
    frontmatter: &JsonValue,
    skill_provider: SkillProvider,
    current_provider: SkillProvider,
) -> (SkillCompatibility, Vec<String>) {
    let mut signals: Vec<String> = Vec::new();
    let mut hard = false;
    let mut soft = false;

    // -- HARD signals ---------------------------------------------------
    if frontmatter.get("allowed-tools").is_some() {
        hard = true;
        signals.push("allowed-tools frontmatter".to_string());
    }

    if HARD_MCP.is_match(body) {
        hard = true;
        signals.push("references mcp__ tools".to_string());
    }
    if HARD_CODEX_HOME.is_match(body) {
        hard = true;
        signals.push("uses $CODEX_HOME".to_string());
    }

    // Provider-rooted paths only count as hard signals when the skill is
    // foreign to the current provider — a Claude skill that mentions
    // `~/.claude/` running inside Claude is fine, but the same skill
    // running under Codex is broken.
    let cross_provider = skill_provider != current_provider;
    if cross_provider {
        if HARD_CLAUDE_PATH.is_match(body) && !matches!(current_provider, SkillProvider::Claude) {
            hard = true;
            signals.push("references ~/.claude/ paths".to_string());
        }
        if HARD_CODEX_PATH.is_match(body) && !matches!(current_provider, SkillProvider::Codex) {
            hard = true;
            signals.push("references ~/.codex/ paths".to_string());
        }
    }

    // -- SOFT signals ---------------------------------------------------
    if SOFT_BASH_BLOCK.is_match(body) {
        soft = true;
        signals.push("contains bash code blocks".to_string());
    }
    if let Some(m) = SOFT_CLI_TOOLS.find(body) {
        soft = true;
        signals.push(format!("mentions CLI tool: {}", m.as_str()));
    }
    if SOFT_SIBLING_FILES.is_match(body) {
        soft = true;
        signals.push("references sibling files (scripts/ or references/)".to_string());
    }

    let bucket = if hard {
        SkillCompatibility::HardWarn
    } else if soft {
        SkillCompatibility::SoftWarn
    } else {
        SkillCompatibility::Compatible
    };

    (bucket, signals)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_fm() -> JsonValue {
        JsonValue::Object(serde_json::Map::new())
    }

    #[test]
    fn pure_prompt_skill_is_compatible() {
        let body =
            "You are an aesthetic designer. Use generous whitespace and a calm color palette.";
        let (bucket, signals) =
            classify_compatibility(body, &empty_fm(), SkillProvider::Claude, SkillProvider::Claude);
        assert_eq!(bucket, SkillCompatibility::Compatible);
        assert!(signals.is_empty());
    }

    #[test]
    fn allowed_tools_frontmatter_is_hard_warn() {
        let body = "Some text.";
        let mut fm = serde_json::Map::new();
        fm.insert(
            "allowed-tools".to_string(),
            JsonValue::Array(vec![JsonValue::String("Bash".into())]),
        );
        let (bucket, signals) = classify_compatibility(
            body,
            &JsonValue::Object(fm),
            SkillProvider::Claude,
            SkillProvider::Claude,
        );
        assert_eq!(bucket, SkillCompatibility::HardWarn);
        assert!(signals.iter().any(|s| s.contains("allowed-tools")));
    }

    #[test]
    fn bash_block_is_soft_warn() {
        let body = "Run this:\n```bash\ngit status\n```\n";
        let (bucket, signals) =
            classify_compatibility(body, &empty_fm(), SkillProvider::Claude, SkillProvider::Claude);
        assert_eq!(bucket, SkillCompatibility::SoftWarn);
        assert!(signals.iter().any(|s| s.contains("bash code blocks")));
    }

    #[test]
    fn mcp_tool_reference_is_hard_warn() {
        let body = "Call mcp__openaiDeveloperDocs__search_openai_docs to look it up.";
        let (bucket, signals) = classify_compatibility(
            body,
            &empty_fm(),
            SkillProvider::Codex,
            SkillProvider::Claude,
        );
        assert_eq!(bucket, SkillCompatibility::HardWarn);
        assert!(signals.iter().any(|s| s.contains("mcp__")));
    }

    #[test]
    fn gh_cli_mention_is_soft_warn() {
        let body = "First check `gh auth status` to confirm login.";
        let (bucket, signals) =
            classify_compatibility(body, &empty_fm(), SkillProvider::Claude, SkillProvider::Claude);
        assert_eq!(bucket, SkillCompatibility::SoftWarn);
        assert!(signals.iter().any(|s| s.contains("gh")));
    }

    #[test]
    fn references_sibling_files_is_soft_warn() {
        let body = "See `references/elicitation.md` and run `scripts/build.sh`.";
        let (bucket, signals) =
            classify_compatibility(body, &empty_fm(), SkillProvider::Claude, SkillProvider::Claude);
        assert_eq!(bucket, SkillCompatibility::SoftWarn);
        assert!(signals.iter().any(|s| s.contains("sibling files")));
    }

    #[test]
    fn codex_home_reference_is_hard_warn() {
        let body = "Open $CODEX_HOME/skills/.system/imagegen/scripts/foo.py";
        let (bucket, _) =
            classify_compatibility(body, &empty_fm(), SkillProvider::Codex, SkillProvider::Claude);
        assert_eq!(bucket, SkillCompatibility::HardWarn);
    }

    #[test]
    fn claude_paths_only_hard_warn_when_cross_provider() {
        let body = "Write to ~/.claude/channels/discord/access.json";

        // Same provider — path is fine.
        let (own, _) =
            classify_compatibility(body, &empty_fm(), SkillProvider::Claude, SkillProvider::Claude);
        // The body has no other hard signals, so it should be compatible.
        // Note: it does mention "Write" but that's not in our soft set.
        assert_eq!(own, SkillCompatibility::Compatible);

        // Cross provider — hard-warn.
        let (cross, signals) =
            classify_compatibility(body, &empty_fm(), SkillProvider::Claude, SkillProvider::Codex);
        assert_eq!(cross, SkillCompatibility::HardWarn);
        assert!(signals.iter().any(|s| s.contains("~/.claude/")));
    }

    #[test]
    fn hard_signal_outranks_soft_signal() {
        let body = "Run `gh status`.\n```bash\ngit status\n```\nAlso uses mcp__foo__bar.";
        let (bucket, _) =
            classify_compatibility(body, &empty_fm(), SkillProvider::Claude, SkillProvider::Claude);
        assert_eq!(bucket, SkillCompatibility::HardWarn);
    }

    #[test]
    fn cli_word_boundary_avoids_false_positives() {
        // "ghost" must not match `gh`, "dockerized" must not match `docker`.
        let body = "The ghost in the machine and dockerized apps.";
        let (bucket, _) =
            classify_compatibility(body, &empty_fm(), SkillProvider::Claude, SkillProvider::Claude);
        assert_eq!(bucket, SkillCompatibility::Compatible);
    }
}
