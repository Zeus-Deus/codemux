// Walks a single skill base directory and returns every well-formed skill
// inside it. Errors at the per-skill level (malformed YAML, unreadable
// SKILL.md) are logged and the offending skill is dropped — one bad skill
// never breaks the scan.

use std::path::Path;

use super::{
    compatibility::classify_compatibility,
    parser::{extract_name_description, parse_skill_file, validate_skill_name},
    paths::is_codex_system_dir,
    skill_id_for_path, Skill, SkillAvailability, SkillInvocationKind, SkillProjection,
    SkillProvenance, SkillProvider, SkillScope,
};

pub fn scan_directory(
    base_dir: &Path,
    provider: SkillProvider,
    scope: SkillScope,
    plugin_slug: Option<String>,
) -> Vec<Skill> {
    let mut skills: Vec<Skill> = Vec::new();

    let entries = match std::fs::read_dir(base_dir) {
        Ok(e) => e,
        Err(_) => return skills, // Missing directory is normal — silent skip.
    };

    for entry in entries.flatten() {
        let path = entry.path();

        // Skip non-directories at the top level — Agent Skills convention is
        // one directory per skill, even for "flat" single-file skills.
        let metadata = match entry.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        if !metadata.is_dir() && !metadata.file_type().is_symlink() {
            continue;
        }

        let dir_name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };

        // Skip the codex `.system/` subdirectory of `~/.codex/skills/` —
        // built-in Codex skills, not user content.
        if is_codex_system_dir(base_dir, &dir_name) {
            continue;
        }

        let skill_md = path.join("SKILL.md");
        if !skill_md.exists() {
            continue;
        }

        match build_skill(
            &skill_md,
            &path,
            &dir_name,
            provider,
            scope,
            plugin_slug.clone(),
        ) {
            Ok(skill) => skills.push(skill),
            Err(err) => {
                eprintln!("skills: skipped {} — {}", skill_md.display(), err);
            }
        }
    }

    skills
}

fn build_skill(
    skill_md_path: &Path,
    skill_dir: &Path,
    dir_name: &str,
    provider: SkillProvider,
    scope: SkillScope,
    plugin_slug: Option<String>,
) -> Result<Skill, String> {
    // Resolve symlinks. The omarchy skill on disk is a symlink to a target
    // outside `~/.claude/skills/`; we want sibling-file resolution and the
    // file_path field to point at the real location.
    let canonical_md = skill_md_path
        .canonicalize()
        .map_err(|e| format!("canonicalize SKILL.md failed: {e}"))?;
    let canonical_dir = canonical_md
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| skill_dir.to_path_buf());

    // Detect whether the SKILL.md file or any ancestor directory is a
    // symlink. The earlier implementation compared `canonical_md`
    // against `skill_md_path` directly, but Windows's `canonicalize`
    // returns `\\?\C:\…`-prefixed paths even when nothing is
    // symlinked, which made every Windows path look symlinked. Walk
    // ancestors with `symlink_metadata` instead so neither the
    // verbatim prefix nor case-fold differences pollute the answer.
    let symlinked = {
        let mut found = false;
        let mut cursor: &Path = skill_md_path;
        loop {
            if let Ok(md) = std::fs::symlink_metadata(cursor) {
                if md.file_type().is_symlink() {
                    found = true;
                    break;
                }
            }
            match cursor.parent() {
                Some(parent) if !parent.as_os_str().is_empty() => cursor = parent,
                _ => break,
            }
        }
        found
    };

    let content =
        std::fs::read_to_string(&canonical_md).map_err(|e| format!("read SKILL.md failed: {e}"))?;

    let parsed = parse_skill_file(&content)?;

    let (name, description) = extract_name_description(&parsed.frontmatter, dir_name);
    // Keep invalid definitions visible in Settings so an upgrade does not
    // silently make an existing row disappear. They never reach the popup or
    // turn resolver: every projection is explicitly unavailable.
    let validation_error = validate_skill_name(&name).err();

    let bundled_files = enumerate_bundled_files(&canonical_dir);

    let projections = [
        SkillProvider::Claude,
        SkillProvider::Codex,
        SkillProvider::Opencode,
    ]
    .into_iter()
    .map(|target| {
        if let Some(error) = validation_error.as_deref() {
            return SkillProjection {
                target_provider: target,
                availability: SkillAvailability::Unavailable,
                compatibility: super::SkillCompatibility::HardWarn,
                reasons: vec![format!("Invalid skill name: {error}")],
                invocation: SkillInvocationKind::None,
            };
        }
        let (compatibility, reasons) =
            classify_compatibility(&parsed.body, &parsed.frontmatter, provider, target);
        let native = provider == target && provider != SkillProvider::Codemux;
        SkillProjection {
            target_provider: target,
            availability: if native {
                SkillAvailability::Native
            } else {
                SkillAvailability::ExplicitPortable
            },
            compatibility,
            reasons,
            invocation: if target == SkillProvider::Codex {
                SkillInvocationKind::CodexSkillItem
            } else if native && target == SkillProvider::Claude {
                SkillInvocationKind::NativeCommand
            } else {
                SkillInvocationKind::PromptPrefix
            },
        }
    })
    .collect::<Vec<_>>();
    let claude_projection = projections
        .iter()
        .find(|projection| projection.target_provider == SkillProvider::Claude)
        .expect("Claude projection is always computed");
    let compatibility = claude_projection.compatibility;
    let signals = claude_projection.reasons.clone();

    let file_path = canonical_md.display().to_string();
    let id = skill_id_for_path(&file_path);

    Ok(Skill {
        preference_id: id.clone(),
        id,
        name,
        description,
        provider,
        scope,
        skill_dir: canonical_dir.display().to_string(),
        file_path,
        body: parsed.body,
        raw_frontmatter: parsed.frontmatter,
        bundled_files,
        compatibility,
        compatibility_signals: signals,
        symlinked,
        plugin_slug,
        provenance: SkillProvenance::Filesystem,
        readable: true,
        source_enabled: validation_error.is_none(),
        validation_error,
        projections,
    })
}

fn enumerate_bundled_files(skill_dir: &Path) -> Vec<String> {
    let entries = match std::fs::read_dir(skill_dir) {
        Ok(e) => e,
        Err(_) => return Vec::new(),
    };
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|name| name != "SKILL.md")
        .collect();
    names.sort();
    names
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::SkillCompatibility;
    use std::fs;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn write_skill(base: &Path, name: &str, body: &str) -> PathBuf {
        let dir = base.join(name);
        fs::create_dir_all(&dir).unwrap();
        let md = dir.join("SKILL.md");
        fs::write(&md, body).unwrap();
        md
    }

    #[test]
    fn flat_single_file_skill_extracted() {
        let tmp = TempDir::new().unwrap();
        write_skill(
            tmp.path(),
            "demo",
            "---\nname: demo\ndescription: A demo\n---\nBody.\n",
        );
        let skills = scan_directory(tmp.path(), SkillProvider::Claude, SkillScope::User, None);
        assert_eq!(skills.len(), 1);
        let s = &skills[0];
        assert_eq!(s.name, "demo");
        assert_eq!(s.description.as_deref(), Some("A demo"));
        assert_eq!(s.scope, SkillScope::User);
        assert_eq!(s.provider, SkillProvider::Claude);
        assert!(s.bundled_files.is_empty());
        assert!(!s.symlinked);
        assert_eq!(s.body, "Body.\n");
    }

    #[test]
    fn invalid_name_remains_visible_but_is_not_invocable() {
        let tmp = TempDir::new().unwrap();
        write_skill(
            tmp.path(),
            "legacy",
            "---\nname: Legacy_Skill\ndescription: Old name\n---\nBody.\n",
        );
        let skills = scan_directory(tmp.path(), SkillProvider::Claude, SkillScope::User, None);
        assert_eq!(skills.len(), 1);
        let skill = &skills[0];
        assert!(skill.validation_error.is_some());
        assert!(!skill.source_enabled);
        assert!(skill.projections.iter().all(|projection| {
            projection.availability == SkillAvailability::Unavailable
                && projection.invocation == SkillInvocationKind::None
        }));
    }

    #[test]
    fn bundled_files_listed_excluding_skill_md() {
        let tmp = TempDir::new().unwrap();
        write_skill(
            tmp.path(),
            "bundle",
            "---\nname: bundle\ndescription: x\n---\n",
        );
        let bundle_dir = tmp.path().join("bundle");
        fs::create_dir_all(bundle_dir.join("references")).unwrap();
        fs::create_dir_all(bundle_dir.join("scripts")).unwrap();
        fs::write(bundle_dir.join("LICENSE"), "MIT").unwrap();

        let skills = scan_directory(tmp.path(), SkillProvider::Claude, SkillScope::User, None);
        assert_eq!(skills.len(), 1);
        let mut bundled = skills[0].bundled_files.clone();
        bundled.sort();
        assert_eq!(bundled, vec!["LICENSE", "references", "scripts"]);
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_skill_resolves_path_and_flags_symlinked() {
        use std::os::unix::fs::symlink;

        let target_root = TempDir::new().unwrap();
        let real_dir = target_root.path().join("omarchy-real");
        fs::create_dir_all(&real_dir).unwrap();
        fs::write(
            real_dir.join("SKILL.md"),
            "---\nname: omarchy\ndescription: y\n---\nBody.",
        )
        .unwrap();

        let scan_root = TempDir::new().unwrap();
        symlink(&real_dir, scan_root.path().join("omarchy")).unwrap();

        let skills = scan_directory(
            scan_root.path(),
            SkillProvider::Claude,
            SkillScope::User,
            None,
        );
        assert_eq!(skills.len(), 1);
        let s = &skills[0];
        assert!(s.symlinked, "expected symlinked=true for symlink target");
        assert!(
            s.file_path.contains("omarchy-real"),
            "file_path should be the resolved target, got {}",
            s.file_path
        );
    }

    #[test]
    fn missing_frontmatter_falls_back_to_dir_name() {
        let tmp = TempDir::new().unwrap();
        write_skill(
            tmp.path(),
            "fallback-name",
            "Just a body, no frontmatter.\n",
        );
        let skills = scan_directory(tmp.path(), SkillProvider::Claude, SkillScope::User, None);
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "fallback-name");
        assert!(skills[0].description.is_none());
    }

    #[test]
    fn malformed_yaml_skill_is_dropped() {
        let tmp = TempDir::new().unwrap();
        write_skill(
            tmp.path(),
            "good",
            "---\nname: good\ndescription: ok\n---\nBody.",
        );
        write_skill(
            tmp.path(),
            "bad",
            "---\nname: bad\n: : :\n  - bad indent\n     more bad\n---\nBody.",
        );
        let skills = scan_directory(tmp.path(), SkillProvider::Claude, SkillScope::User, None);
        // The bad one is dropped, the good one remains.
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "good");
    }

    #[test]
    fn empty_directory_returns_empty_vec() {
        let tmp = TempDir::new().unwrap();
        let skills = scan_directory(tmp.path(), SkillProvider::Claude, SkillScope::User, None);
        assert!(skills.is_empty());
    }

    #[test]
    fn directory_without_skill_md_skipped() {
        let tmp = TempDir::new().unwrap();
        fs::create_dir_all(tmp.path().join("not-a-skill")).unwrap();
        fs::write(tmp.path().join("not-a-skill/README.md"), "hi").unwrap();
        let skills = scan_directory(tmp.path(), SkillProvider::Claude, SkillScope::User, None);
        assert!(skills.is_empty());
    }

    #[test]
    fn nonexistent_base_dir_returns_empty_vec_silently() {
        let path = PathBuf::from("/this/path/does/not/exist/probably");
        let skills = scan_directory(&path, SkillProvider::Claude, SkillScope::User, None);
        assert!(skills.is_empty());
    }

    #[test]
    fn plugin_slug_propagates_to_skill() {
        let tmp = TempDir::new().unwrap();
        write_skill(
            tmp.path(),
            "demo",
            "---\nname: demo\ndescription: x\n---\nBody.",
        );
        let skills = scan_directory(
            tmp.path(),
            SkillProvider::Claude,
            SkillScope::Plugin,
            Some("frontend-design".to_string()),
        );
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].plugin_slug.as_deref(), Some("frontend-design"));
        assert_eq!(skills[0].scope, SkillScope::Plugin);
    }

    #[test]
    fn codex_system_subdir_is_skipped() {
        // Synthesize the codex layout: tmp/.codex/skills/imagegen and
        // tmp/.codex/skills/.system/built-in.
        let tmp = TempDir::new().unwrap();
        let skills_dir = tmp.path().join(".codex").join("skills");
        fs::create_dir_all(&skills_dir).unwrap();

        write_skill(
            &skills_dir,
            "imagegen",
            "---\nname: imagegen\ndescription: y\n---",
        );
        let system_dir = skills_dir.join(".system");
        fs::create_dir_all(&system_dir).unwrap();
        write_skill(
            &system_dir,
            "built-in",
            "---\nname: built-in\ndescription: y\n---",
        );

        let skills = scan_directory(&skills_dir, SkillProvider::Codex, SkillScope::User, None);
        // Only `imagegen` discovered — `.system/` was skipped.
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "imagegen");
    }

    #[test]
    fn id_is_stable_across_scans_for_same_skill() {
        let tmp = TempDir::new().unwrap();
        write_skill(
            tmp.path(),
            "demo",
            "---\nname: demo\ndescription: x\n---\nBody.",
        );
        let a = scan_directory(tmp.path(), SkillProvider::Claude, SkillScope::User, None);
        let b = scan_directory(tmp.path(), SkillProvider::Claude, SkillScope::User, None);
        assert_eq!(a[0].id, b[0].id);
    }

    #[test]
    fn compatibility_classified_at_scan_time() {
        let tmp = TempDir::new().unwrap();
        write_skill(
            tmp.path(),
            "uses-bash",
            "---\nname: uses-bash\ndescription: x\n---\nRun:\n```bash\nls\n```\n",
        );
        let skills = scan_directory(tmp.path(), SkillProvider::Claude, SkillScope::User, None);
        assert_eq!(skills[0].compatibility, SkillCompatibility::SoftWarn);
        assert!(!skills[0].compatibility_signals.is_empty());
    }
}
