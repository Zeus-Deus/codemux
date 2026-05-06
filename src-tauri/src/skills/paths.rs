// Skill scan-path enumeration.
//
// Encodes the locked decisions from Stage 7 research: which directories
// belong to which provider, which scope each path falls into, and how the
// plugin pool gates on the `include_plugins` toggle.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use super::SkillProvider;

pub struct ScanPaths {
    pub user_paths: Vec<(PathBuf, SkillProvider)>,
    pub project_paths: Vec<(PathBuf, SkillProvider)>,
    pub plugin_paths: Vec<(PathBuf, String)>,
}

/// Build the full list of paths the scanner should walk. Existence is NOT
/// checked here for the user/project entries — the scanner is responsible
/// for skipping missing directories silently. Plugin paths ARE filtered
/// for existence because they're glob-derived and might match nothing.
///
/// `home` lets tests inject a fake home; production callers pass `None`
/// and we use `dirs::home_dir()`.
pub fn enumerate_scan_paths(
    project_root: Option<&Path>,
    include_plugins: bool,
) -> ScanPaths {
    enumerate_scan_paths_with_home(project_root, include_plugins, dirs::home_dir().as_deref())
}

pub fn enumerate_scan_paths_with_home(
    project_root: Option<&Path>,
    include_plugins: bool,
    home: Option<&Path>,
) -> ScanPaths {
    let mut user_paths: Vec<(PathBuf, SkillProvider)> = Vec::new();
    let mut project_paths: Vec<(PathBuf, SkillProvider)> = Vec::new();
    let mut plugin_paths: Vec<(PathBuf, String)> = Vec::new();

    if let Some(home) = home {
        // Claude user-wide.
        user_paths.push((home.join(".claude").join("skills"), SkillProvider::Claude));

        // Codex: legacy `~/.codex/skills/` plus OpenAI's newer
        // `$HOME/.agents/skills/`. The scanner is responsible for
        // excluding the `.system/` subdirectory of `~/.codex/skills/`
        // (built-in skills, not user content).
        user_paths.push((home.join(".codex").join("skills"), SkillProvider::Codex));
        user_paths.push((home.join(".agents").join("skills"), SkillProvider::Codex));

        // OpenCode: dedupe the two known conventions if they resolve to
        // the same directory (e.g., one is a symlink to the other).
        let oc_a = home.join(".opencode").join("skills");
        let oc_b = home.join(".config").join("opencode").join("skills");
        let mut opencode_seen: HashSet<PathBuf> = HashSet::new();
        for path in [oc_a, oc_b] {
            let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
            if opencode_seen.insert(canonical) {
                user_paths.push((path, SkillProvider::Opencode));
            }
        }

        // Codemux's neutral location.
        user_paths.push((home.join(".codemux").join("skills"), SkillProvider::Codemux));

        // Plugin-bundled Claude skills. The locked path globs are
        // `~/.claude/plugins/marketplaces/*/plugins/*/skills`
        // and `~/.claude/plugins/external_plugins/*/skills`.
        if include_plugins {
            plugin_paths.extend(enumerate_claude_plugin_paths(home));
        }
    }

    if let Some(root) = project_root {
        project_paths.push((root.join(".claude").join("skills"), SkillProvider::Claude));
        project_paths.push((root.join(".codex").join("skills"), SkillProvider::Codex));
        project_paths.push((root.join(".opencode").join("skills"), SkillProvider::Opencode));
        project_paths.push((root.join(".codemux").join("skills"), SkillProvider::Codemux));
    }

    ScanPaths {
        user_paths,
        project_paths,
        plugin_paths,
    }
}

/// Whether the codex `.system/` subdirectory should be skipped during a
/// scan of `~/.codex/skills/`. Centralized so tests can pin this rule.
pub fn is_codex_system_dir(parent: &Path, child_name: &str) -> bool {
    if child_name != ".system" {
        return false;
    }
    parent
        .file_name()
        .and_then(|n| n.to_str())
        .map(|n| n == "skills")
        .unwrap_or(false)
        && parent
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .map(|n| n == ".codex")
            .unwrap_or(false)
}

fn enumerate_claude_plugin_paths(home: &Path) -> Vec<(PathBuf, String)> {
    let mut out: Vec<(PathBuf, String)> = Vec::new();

    // `~/.claude/plugins/marketplaces/*/plugins/*/skills`
    let marketplaces_glob = home
        .join(".claude")
        .join("plugins")
        .join("marketplaces")
        .join("*")
        .join("plugins")
        .join("*")
        .join("skills");
    out.extend(glob_skills_dirs(&marketplaces_glob, |path| {
        path.parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .map(ToString::to_string)
    }));

    // `~/.claude/plugins/external_plugins/*/skills`
    let external_glob = home
        .join(".claude")
        .join("plugins")
        .join("external_plugins")
        .join("*")
        .join("skills");
    out.extend(glob_skills_dirs(&external_glob, |path| {
        path.parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .map(ToString::to_string)
    }));

    out
}

fn glob_skills_dirs<F>(pattern: &Path, slug: F) -> Vec<(PathBuf, String)>
where
    F: Fn(&Path) -> Option<String>,
{
    let pattern_str = match pattern.to_str() {
        Some(s) => s,
        None => return Vec::new(),
    };
    let entries = match glob::glob(pattern_str) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };
    entries
        .filter_map(|res| res.ok())
        .filter(|path| path.is_dir())
        .map(|path| {
            let s = slug(&path).unwrap_or_else(|| "unknown".to_string());
            (path, s)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn fake_home() -> TempDir {
        TempDir::new().unwrap()
    }

    #[test]
    fn includes_all_user_paths_for_a_home() {
        let home = fake_home();
        let paths = enumerate_scan_paths_with_home(None, false, Some(home.path()));
        let providers: Vec<_> = paths.user_paths.iter().map(|(_, p)| *p).collect();
        assert!(providers.contains(&SkillProvider::Claude));
        assert!(providers.contains(&SkillProvider::Codex));
        assert!(providers.contains(&SkillProvider::Opencode));
        assert!(providers.contains(&SkillProvider::Codemux));
    }

    #[test]
    fn includes_both_codex_paths() {
        let home = fake_home();
        let paths = enumerate_scan_paths_with_home(None, false, Some(home.path()));
        let codex_paths: Vec<_> = paths
            .user_paths
            .iter()
            .filter(|(_, p)| matches!(p, SkillProvider::Codex))
            .map(|(p, _)| p.clone())
            .collect();
        assert_eq!(codex_paths.len(), 2);
        assert!(codex_paths
            .iter()
            .any(|p| p.ends_with(".codex/skills")));
        assert!(codex_paths
            .iter()
            .any(|p| p.ends_with(".agents/skills")));
    }

    #[test]
    fn project_paths_only_when_root_given() {
        let home = fake_home();
        let no_project = enumerate_scan_paths_with_home(None, false, Some(home.path()));
        assert!(no_project.project_paths.is_empty());

        let project_dir = TempDir::new().unwrap();
        let with_project = enumerate_scan_paths_with_home(Some(project_dir.path()), false, Some(home.path()));
        assert_eq!(with_project.project_paths.len(), 4);
    }

    #[test]
    fn plugin_paths_excluded_when_include_plugins_false() {
        let home = fake_home();
        // Even if the directories exist, include_plugins=false suppresses them.
        fs::create_dir_all(
            home.path()
                .join(".claude/plugins/marketplaces/example/plugins/foo/skills"),
        )
        .unwrap();
        let paths = enumerate_scan_paths_with_home(None, false, Some(home.path()));
        assert!(paths.plugin_paths.is_empty());
    }

    #[test]
    fn plugin_paths_globbed_when_include_plugins_true() {
        let home = fake_home();
        fs::create_dir_all(
            home.path()
                .join(".claude/plugins/marketplaces/m1/plugins/p1/skills"),
        )
        .unwrap();
        fs::create_dir_all(
            home.path()
                .join(".claude/plugins/marketplaces/m1/plugins/p2/skills"),
        )
        .unwrap();
        fs::create_dir_all(
            home.path()
                .join(".claude/plugins/external_plugins/discord/skills"),
        )
        .unwrap();

        let paths = enumerate_scan_paths_with_home(None, true, Some(home.path()));
        let slugs: Vec<_> = paths.plugin_paths.iter().map(|(_, s)| s.clone()).collect();
        assert!(slugs.contains(&"p1".to_string()));
        assert!(slugs.contains(&"p2".to_string()));
        assert!(slugs.contains(&"discord".to_string()));
    }

    #[test]
    fn is_codex_system_dir_recognizes_correct_path() {
        let parent = PathBuf::from("/home/user/.codex/skills");
        assert!(is_codex_system_dir(&parent, ".system"));
        assert!(!is_codex_system_dir(&parent, "imagegen"));

        // Different parent layout — must not match.
        let other = PathBuf::from("/home/user/.claude/skills");
        assert!(!is_codex_system_dir(&other, ".system"));
    }

    #[test]
    fn opencode_dedupes_when_paths_resolve_to_same_dir() {
        // We can't easily symlink-test on every filesystem, but we can at
        // least verify that two distinct paths both make it in when there
        // is no symlink relationship.
        let home = fake_home();
        let paths = enumerate_scan_paths_with_home(None, false, Some(home.path()));
        let opencode: Vec<_> = paths
            .user_paths
            .iter()
            .filter(|(_, p)| matches!(p, SkillProvider::Opencode))
            .collect();
        assert_eq!(opencode.len(), 2);
    }

    #[cfg(unix)]
    #[test]
    fn opencode_dedupes_via_symlink() {
        use std::os::unix::fs::symlink;

        let home = fake_home();
        let real = home.path().join(".opencode").join("skills");
        fs::create_dir_all(&real).unwrap();
        let link_parent = home.path().join(".config").join("opencode");
        fs::create_dir_all(&link_parent).unwrap();
        symlink(&real, link_parent.join("skills")).unwrap();

        let paths = enumerate_scan_paths_with_home(None, false, Some(home.path()));
        let opencode: Vec<_> = paths
            .user_paths
            .iter()
            .filter(|(_, p)| matches!(p, SkillProvider::Opencode))
            .collect();
        // Symlink resolves to the same target — only one entry survives.
        assert_eq!(opencode.len(), 1);
    }
}
