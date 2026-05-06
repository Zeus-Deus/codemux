// Cross-provider skill discovery + registry.
//
// Stage 1 of Step 7: walks the known skill directories for Claude, Codex,
// OpenCode, and Codemux; parses YAML frontmatter; classifies tool-compat
// based on body heuristics; returns a flat `Vec<Skill>` for the frontend
// to render in the slash popup and Settings.

pub mod compatibility;
pub mod parser;
pub mod paths;
pub mod scanner;
pub mod watcher;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SkillProvider {
    Claude,
    Codex,
    Opencode,
    Codemux,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SkillScope {
    User,
    Project,
    Plugin,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SkillCompatibility {
    Compatible,
    SoftWarn,
    HardWarn,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Skill {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub provider: SkillProvider,
    pub scope: SkillScope,
    pub skill_dir: String,
    pub file_path: String,
    pub body: String,
    pub raw_frontmatter: serde_json::Value,
    pub bundled_files: Vec<String>,
    pub compatibility: SkillCompatibility,
    pub compatibility_signals: Vec<String>,
    pub symlinked: bool,
    pub plugin_slug: Option<String>,
}

/// Stable id derived from the canonical SKILL.md path. Truncated SHA-256 hex
/// (16 chars) — short enough for log lines, long enough to avoid collisions
/// across the small skill universe a single user has installed.
pub fn skill_id_for_path(file_path: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(file_path.as_bytes());
    let digest = hasher.finalize();
    digest
        .iter()
        .take(8)
        .map(|b| format!("{:02x}", b))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skill_id_is_stable_and_deterministic() {
        let a = skill_id_for_path("/home/user/.claude/skills/foo/SKILL.md");
        let b = skill_id_for_path("/home/user/.claude/skills/foo/SKILL.md");
        assert_eq!(a, b);
        assert_eq!(a.len(), 16);
    }

    #[test]
    fn skill_id_differs_per_path() {
        let a = skill_id_for_path("/a/SKILL.md");
        let b = skill_id_for_path("/b/SKILL.md");
        assert_ne!(a, b);
    }

    #[test]
    fn provider_serializes_lowercase() {
        let p = SkillProvider::Claude;
        let json = serde_json::to_string(&p).unwrap();
        assert_eq!(json, "\"claude\"");
    }

    #[test]
    fn compatibility_serializes_kebab_case() {
        let c = SkillCompatibility::SoftWarn;
        let json = serde_json::to_string(&c).unwrap();
        assert_eq!(json, "\"soft-warn\"");
    }

    #[test]
    fn skill_serializes_camel_case_keys() {
        let skill = Skill {
            id: "abc".to_string(),
            name: "demo".to_string(),
            description: None,
            provider: SkillProvider::Claude,
            scope: SkillScope::User,
            skill_dir: "/x".to_string(),
            file_path: "/x/SKILL.md".to_string(),
            body: String::new(),
            raw_frontmatter: serde_json::Value::Null,
            bundled_files: vec![],
            compatibility: SkillCompatibility::Compatible,
            compatibility_signals: vec![],
            symlinked: false,
            plugin_slug: None,
        };
        let json = serde_json::to_value(&skill).unwrap();
        assert!(json.get("skillDir").is_some());
        assert!(json.get("filePath").is_some());
        assert!(json.get("rawFrontmatter").is_some());
        assert!(json.get("bundledFiles").is_some());
        assert!(json.get("compatibilitySignals").is_some());
        assert!(json.get("pluginSlug").is_some());
    }
}
