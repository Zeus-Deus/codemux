// Skills sync — provider + scope detection from a filesystem path.
//
// The sync engine syncs **user-authored skills only**, never plugin or
// project skills. This module is the source of truth for "is this
// path syncable?" — the push pipeline calls `detect_skill_path` and
// skips anything that comes back `is_syncable=false`.
//
// Locked decision: the canonical local destination after pull is
// always `~/.codemux/skills/<name>/SKILL.md`, regardless of which
// provider's path the skill originally came from. The original
// provider stays as plaintext metadata server-side and in the local
// mapping table, but the file lives at one well-known place on the
// receiving device. `destination_path_after_pull` enforces this.

use std::path::{Path, PathBuf};

/// Outcome of a path classification.
///
/// `provider` is the lowercase provider tag the wire format uses
/// (mirrors `crate::skills::SkillProvider::serialize` output). It is
/// preserved verbatim across the sync boundary so a skill that
/// originated in Claude on one device shows up tagged as `"claude"`
/// on every other device, even after the file ends up at the
/// canonical `~/.codemux/skills/` location.
///
/// `scope` mirrors `crate::skills::SkillScope` and is currently
/// always `"user"` for syncable paths. The `"project"` scope is
/// reserved for Step 10.5 (project-scoped sync); v1 leaves the field
/// in place so the schema doesn't need a breaking migration when
/// 10.5 lands.
///
/// `is_syncable=false` is the kill switch — the push pipeline drops
/// these paths silently. It covers plugin skills, project skills (for
/// now), and anything that doesn't match a known provider layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillPathInfo {
    pub provider: String,
    pub scope: String,
    pub is_syncable: bool,
}

/// Classify a SKILL.md path. Returns `None` if the path doesn't fit
/// any known skill layout (not under any provider's user-skill
/// directory and not in a project's `.codemux/skills/`). `None` is
/// **not** the same as `is_syncable=false`: a `None` path is unknown
/// and gets dropped by the caller, while a `Some(_, false)` path is
/// recognized but deliberately excluded from sync.
pub fn detect_skill_path(path: &Path, home: &Path) -> Option<SkillPathInfo> {
    detect_with_project_root(path, home, None)
}

/// Variant that lets the caller indicate which directory is the
/// active project root. Useful for tests and for when the sync
/// engine's owner already knows the workspace folder.
pub fn detect_with_project_root(
    path: &Path,
    home: &Path,
    project_root: Option<&Path>,
) -> Option<SkillPathInfo> {
    // Each known user-scope provider has a fixed directory under
    // `$HOME`. The path is syncable iff it's underneath one of those
    // AND the relative remainder looks like `<name>/SKILL.md` (the
    // standard skill layout).
    for (subdir, provider) in USER_SCOPE_PROVIDERS {
        let root = home.join(subdir);
        if let Ok(rel) = path.strip_prefix(&root) {
            if is_skill_md_layout(rel) {
                return Some(SkillPathInfo {
                    provider: (*provider).to_string(),
                    scope: "user".into(),
                    is_syncable: true,
                });
            }
        }
    }

    // Project-scope: deferred to Step 10.5. Recognize the layout but
    // mark non-syncable so the v1 pipeline skips cleanly.
    if let Some(root) = project_root {
        for subdir in PROJECT_SCOPE_SUBDIRS {
            let project_skills_root = root.join(subdir);
            if let Ok(rel) = path.strip_prefix(&project_skills_root) {
                if is_skill_md_layout(rel) {
                    return Some(SkillPathInfo {
                        provider: provider_from_project_subdir(subdir).to_string(),
                        scope: "project".into(),
                        is_syncable: false,
                    });
                }
            }
        }
    }

    // Plugin skills live wherever the host installed them — we can't
    // enumerate them by path prefix the way we do for user/project.
    // The sync engine's caller already filters by enumeration source
    // (only `enumerate_scan_paths().user_paths` are walked for sync),
    // so reaching this branch means the path is genuinely unknown to
    // us. Return `None` so the caller drops it.
    None
}

/// Where a freshly-pulled skill lives on disk. Always
/// `~/.codemux/skills/<name>/SKILL.md`, regardless of which
/// provider's path the skill originated from. The pull pipeline
/// uses this to write decrypted skills; the file watcher's
/// "currently-writing" guard uses it to avoid an echo loop.
pub fn destination_path_after_pull(name: &str, home: &Path) -> PathBuf {
    home.join(".codemux/skills").join(name).join("SKILL.md")
}

// ────────────────────────────────────────────────────────────────
// Internal lookup tables.

/// User-scope skill roots. Order matters only for diagnostic
/// messages — paths are mutually disjoint by construction.
const USER_SCOPE_PROVIDERS: &[(&str, &str)] = &[
    (".codemux/skills", "codemux"),
    (".claude/skills", "claude"),
    (".codex/skills", "codex"),
    (".opencode/skills", "opencode"),
];

/// Subdirectories searched relative to a project root. Each one
/// corresponds to the same provider as the user-scope variant —
/// `.claude/skills` under home is Claude-user; `.claude/skills`
/// under a project root is Claude-project.
const PROJECT_SCOPE_SUBDIRS: &[&str] = &[
    ".codemux/skills",
    ".claude/skills",
    ".codex/skills",
    ".opencode/skills",
];

fn provider_from_project_subdir(subdir: &str) -> &'static str {
    match subdir {
        ".codemux/skills" => "codemux",
        ".claude/skills" => "claude",
        ".codex/skills" => "codex",
        ".opencode/skills" => "opencode",
        _ => "unknown",
    }
}

/// True when the relative path looks like `<name>/SKILL.md`. Allows
/// any non-empty single path component as the name; rejects deeper
/// nesting (the skill layout is exactly one directory deep) and
/// non-SKILL.md filenames.
fn is_skill_md_layout(rel: &Path) -> bool {
    let mut comps = rel.components();
    let _name = match comps.next() {
        Some(std::path::Component::Normal(n)) if !n.is_empty() => n,
        _ => return false,
    };
    let last = match comps.next() {
        Some(std::path::Component::Normal(n)) => n,
        _ => return false,
    };
    if comps.next().is_some() {
        return false;
    }
    last == "SKILL.md"
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fake_home() -> PathBuf {
        PathBuf::from("/home/zeus")
    }

    // ── User-scope detection ───────────────────────────────────

    #[test]
    fn codemux_user_path_is_syncable() {
        let home = fake_home();
        let p = home.join(".codemux/skills/my-skill/SKILL.md");
        let info = detect_skill_path(&p, &home).unwrap();
        assert_eq!(info.provider, "codemux");
        assert_eq!(info.scope, "user");
        assert!(info.is_syncable);
    }

    #[test]
    fn claude_user_path_is_syncable() {
        let home = fake_home();
        let p = home.join(".claude/skills/foo/SKILL.md");
        let info = detect_skill_path(&p, &home).unwrap();
        assert_eq!(info.provider, "claude");
        assert!(info.is_syncable);
    }

    #[test]
    fn codex_user_path_is_syncable() {
        let home = fake_home();
        let p = home.join(".codex/skills/bar/SKILL.md");
        let info = detect_skill_path(&p, &home).unwrap();
        assert_eq!(info.provider, "codex");
        assert!(info.is_syncable);
    }

    #[test]
    fn opencode_user_path_is_syncable() {
        let home = fake_home();
        let p = home.join(".opencode/skills/baz/SKILL.md");
        let info = detect_skill_path(&p, &home).unwrap();
        assert_eq!(info.provider, "opencode");
        assert!(info.is_syncable);
    }

    // ── Project-scope detection (recognized but not syncable in v1) ──

    #[test]
    fn project_codemux_path_recognized_but_not_syncable() {
        let home = fake_home();
        let project = PathBuf::from("/repo/myproj");
        let p = project.join(".codemux/skills/proj-skill/SKILL.md");
        let info = detect_with_project_root(&p, &home, Some(&project)).unwrap();
        assert_eq!(info.provider, "codemux");
        assert_eq!(info.scope, "project");
        assert!(!info.is_syncable, "project skills are not syncable in v1");
    }

    #[test]
    fn project_claude_path_recognized_but_not_syncable() {
        let home = fake_home();
        let project = PathBuf::from("/repo/myproj");
        let p = project.join(".claude/skills/x/SKILL.md");
        let info = detect_with_project_root(&p, &home, Some(&project)).unwrap();
        assert_eq!(info.provider, "claude");
        assert_eq!(info.scope, "project");
        assert!(!info.is_syncable);
    }

    // ── Unknown / unsyncable paths ─────────────────────────────

    #[test]
    fn plugin_skill_path_returns_none() {
        let home = fake_home();
        let p = PathBuf::from("/usr/share/somecli-plugin/skills/p/SKILL.md");
        assert!(detect_skill_path(&p, &home).is_none());
    }

    #[test]
    fn arbitrary_path_returns_none() {
        let home = fake_home();
        let p = PathBuf::from("/tmp/random/file.txt");
        assert!(detect_skill_path(&p, &home).is_none());
    }

    #[test]
    fn home_root_returns_none() {
        let home = fake_home();
        assert!(detect_skill_path(&home, &home).is_none());
    }

    // ── Layout edge cases ──────────────────────────────────────

    #[test]
    fn deeply_nested_under_skills_dir_returns_none() {
        let home = fake_home();
        // `<name>/sub/SKILL.md` is two levels deep — not the standard
        // single-directory skill layout, so reject. Stage 7's parser
        // also expects this layout.
        let p = home.join(".codemux/skills/foo/sub/SKILL.md");
        assert!(detect_skill_path(&p, &home).is_none());
    }

    #[test]
    fn non_skill_md_filename_returns_none() {
        let home = fake_home();
        let p = home.join(".codemux/skills/foo/README.md");
        assert!(detect_skill_path(&p, &home).is_none());
    }

    #[test]
    fn skill_md_directly_in_skills_dir_returns_none() {
        // ~/.codemux/skills/SKILL.md (no name dir) — malformed
        // skill, not syncable.
        let home = fake_home();
        let p = home.join(".codemux/skills/SKILL.md");
        assert!(detect_skill_path(&p, &home).is_none());
    }

    // ── Destination path ───────────────────────────────────────

    #[test]
    fn destination_is_canonical_codemux_skills_root() {
        let home = fake_home();
        let dest = destination_path_after_pull("my-skill", &home);
        assert_eq!(dest, home.join(".codemux/skills/my-skill/SKILL.md"));
    }

    #[test]
    fn destination_does_not_use_origin_provider() {
        // Even if the original skill came from Claude, the
        // destination on the receiving device is always under
        // ~/.codemux/skills/. Origin survives only as metadata.
        let home = fake_home();
        let dest = destination_path_after_pull("from-claude", &home);
        assert!(dest.starts_with(home.join(".codemux/skills")));
        assert!(!dest.starts_with(home.join(".claude/skills")));
    }
}
