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
    // Stage 5 refinement: distinguish *strong* soft signals (a single
    // one is enough to escalate to soft-warn) from *weak* soft signals
    // (need at least two to escalate). Stage 1 over-warned on skills
    // like omarchy that mentioned `docker` once in prose without
    // actually depending on it; this two-tier approach drops those to
    // compatible while preserving warnings for skills with bash blocks
    // or sibling-file references that are unambiguously tool-bound.
    let mut strong_soft = 0usize;
    let mut weak_soft = 0usize;

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

    // -- STRONG soft signals (1 → soft-warn) ----------------------------
    // Triple-backtick bash block — concrete shell instructions the
    // skill expects to run.
    if SOFT_BASH_BLOCK.is_match(body) {
        strong_soft += 1;
        signals.push("contains bash code blocks".to_string());
    }
    // References to the bundled-skill conventional subdirs. If the
    // skill ships `scripts/foo.sh` or `references/bar.md` the model
    // is meant to use them — the skill would degrade if those bundled
    // files don't get carried along.
    if SOFT_SIBLING_FILES.is_match(body) {
        strong_soft += 1;
        signals.push("references sibling files (scripts/ or references/)".to_string());
    }

    // -- WEAK soft signals (2+ → soft-warn) -----------------------------
    // Single CLI mention is often incidental prose ("docker" in a
    // changelog, "ssh" in a paragraph about server access). Counting
    // every match individually so a skill that mentions both `gh` and
    // `docker` in code-context actually trips the threshold.
    let cli_count = SOFT_CLI_TOOLS.find_iter(body).count();
    if cli_count > 0 {
        weak_soft += cli_count;
        // Surface a representative sample so the tooltip is concrete.
        if let Some(first) = SOFT_CLI_TOOLS.find(body) {
            signals.push(format!(
                "mentions CLI tool: {}{}",
                first.as_str(),
                if cli_count > 1 {
                    format!(" (and {} more)", cli_count - 1)
                } else {
                    String::new()
                },
            ));
        }
    }

    let bucket = if hard {
        SkillCompatibility::HardWarn
    } else if strong_soft >= 1 || weak_soft >= 2 {
        SkillCompatibility::SoftWarn
    } else {
        // A single weak signal (one CLI mention in prose) keeps the
        // skill labelled compatible. Signals are still returned so the
        // tooltip can show "we noticed this — but didn't promote it"
        // if the UI ever wants to surface that.
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
        let (bucket, signals) = classify_compatibility(
            body,
            &empty_fm(),
            SkillProvider::Claude,
            SkillProvider::Claude,
        );
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
        // Strong signal — single bash block is enough.
        let body = "Run this:\n```bash\ngit status\n```\n";
        let (bucket, signals) = classify_compatibility(
            body,
            &empty_fm(),
            SkillProvider::Claude,
            SkillProvider::Claude,
        );
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
    fn single_cli_mention_in_prose_stays_compatible() {
        // Stage 5 refinement: a lone CLI mention is often incidental
        // ("docker" in a changelog, "ssh" in a paragraph about server
        // access). The Stage 1 omarchy false-positive lived here.
        // Signals are still recorded for tooltip reuse.
        let body = "First check `gh auth status` to confirm login.";
        let (bucket, signals) = classify_compatibility(
            body,
            &empty_fm(),
            SkillProvider::Claude,
            SkillProvider::Claude,
        );
        assert_eq!(bucket, SkillCompatibility::Compatible);
        assert!(signals.iter().any(|s| s.contains("gh")));
    }

    #[test]
    fn two_cli_mentions_escalate_to_soft_warn() {
        // Two weak signals = soft-warn (the threshold).
        let body = "Run `gh release create`, then `docker push`.";
        let (bucket, signals) = classify_compatibility(
            body,
            &empty_fm(),
            SkillProvider::Claude,
            SkillProvider::Claude,
        );
        assert_eq!(bucket, SkillCompatibility::SoftWarn);
        assert!(signals.iter().any(|s| s.contains("(and 1 more)")));
    }

    #[test]
    fn bash_block_alone_is_soft_warn_strong_signal() {
        // A bash code block is a strong signal — one is enough.
        let body = "Run this:\n```bash\nls\n```\n";
        let (bucket, _) = classify_compatibility(
            body,
            &empty_fm(),
            SkillProvider::Claude,
            SkillProvider::Claude,
        );
        assert_eq!(bucket, SkillCompatibility::SoftWarn);
    }

    #[test]
    fn sibling_files_alone_is_soft_warn_strong_signal() {
        // Sibling-file refs imply the skill expects bundled assets.
        let body = "See `references/elicitation.md` for examples.";
        let (bucket, _) = classify_compatibility(
            body,
            &empty_fm(),
            SkillProvider::Claude,
            SkillProvider::Claude,
        );
        assert_eq!(bucket, SkillCompatibility::SoftWarn);
    }

    #[test]
    fn references_sibling_files_is_soft_warn() {
        // Strong signal — sibling-file ref is enough on its own.
        let body = "See `references/elicitation.md` and run `scripts/build.sh`.";
        let (bucket, signals) = classify_compatibility(
            body,
            &empty_fm(),
            SkillProvider::Claude,
            SkillProvider::Claude,
        );
        assert_eq!(bucket, SkillCompatibility::SoftWarn);
        assert!(signals.iter().any(|s| s.contains("sibling files")));
    }

    #[test]
    fn codex_home_reference_is_hard_warn() {
        let body = "Open $CODEX_HOME/skills/.system/imagegen/scripts/foo.py";
        let (bucket, _) = classify_compatibility(
            body,
            &empty_fm(),
            SkillProvider::Codex,
            SkillProvider::Claude,
        );
        assert_eq!(bucket, SkillCompatibility::HardWarn);
    }

    #[test]
    fn claude_paths_only_hard_warn_when_cross_provider() {
        let body = "Write to ~/.claude/channels/discord/access.json";

        // Same provider — path is fine.
        let (own, _) = classify_compatibility(
            body,
            &empty_fm(),
            SkillProvider::Claude,
            SkillProvider::Claude,
        );
        // The body has no other hard signals, so it should be compatible.
        // Note: it does mention "Write" but that's not in our soft set.
        assert_eq!(own, SkillCompatibility::Compatible);

        // Cross provider — hard-warn.
        let (cross, signals) = classify_compatibility(
            body,
            &empty_fm(),
            SkillProvider::Claude,
            SkillProvider::Codex,
        );
        assert_eq!(cross, SkillCompatibility::HardWarn);
        assert!(signals.iter().any(|s| s.contains("~/.claude/")));
    }

    #[test]
    fn hard_signal_outranks_soft_signal() {
        let body = "Run `gh status`.\n```bash\ngit status\n```\nAlso uses mcp__foo__bar.";
        let (bucket, _) = classify_compatibility(
            body,
            &empty_fm(),
            SkillProvider::Claude,
            SkillProvider::Claude,
        );
        assert_eq!(bucket, SkillCompatibility::HardWarn);
    }

    #[test]
    fn cli_word_boundary_avoids_false_positives() {
        // "ghost" must not match `gh`, "dockerized" must not match `docker`.
        let body = "The ghost in the machine and dockerized apps.";
        let (bucket, _) = classify_compatibility(
            body,
            &empty_fm(),
            SkillProvider::Claude,
            SkillProvider::Claude,
        );
        assert_eq!(bucket, SkillCompatibility::Compatible);
    }
}
