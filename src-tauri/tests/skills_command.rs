//! Integration test for the cross-provider skills discovery pipeline.
//!
//! Exercises the same code path as the `list_skills` Tauri command —
//! enumerate paths → scan each base dir → collect into a flat list — but
//! against a synthetic home/project fixture so the test is hermetic.
//!
//! Stage 1 of Step 7. Stage 2+ wires this into the frontend.

use std::fs;
use std::path::Path;

use codemux_lib::skills::{
    paths::enumerate_scan_paths_with_home, scanner::scan_directory, Skill, SkillCompatibility,
    SkillProvider, SkillScope,
};
use tempfile::TempDir;

/// Mirrors the body of `commands::skills::list_skills`, but lets us inject
/// a fake home directory so we don't read the developer's real
/// `~/.claude/skills/` during the test.
fn collect_skills(
    home: &Path,
    project_root: Option<&Path>,
    include_plugins: bool,
) -> Vec<Skill> {
    let paths = enumerate_scan_paths_with_home(project_root, include_plugins, Some(home));
    let mut all_skills: Vec<Skill> = Vec::new();
    for (path, provider) in paths.user_paths {
        all_skills.extend(scan_directory(&path, provider, SkillScope::User, None));
    }
    for (path, provider) in paths.project_paths {
        all_skills.extend(scan_directory(&path, provider, SkillScope::Project, None));
    }
    for (path, plugin_slug) in paths.plugin_paths {
        all_skills.extend(scan_directory(
            &path,
            SkillProvider::Claude,
            SkillScope::Plugin,
            Some(plugin_slug),
        ));
    }
    all_skills
}

fn write_skill(base: &Path, name: &str, body: &str) {
    let dir = base.join(name);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("SKILL.md"), body).unwrap();
}

#[test]
fn discovers_skills_across_all_provider_directories() {
    let home = TempDir::new().unwrap();
    let project = TempDir::new().unwrap();

    // Claude user-wide.
    let claude_user = home.path().join(".claude").join("skills");
    fs::create_dir_all(&claude_user).unwrap();
    write_skill(
        &claude_user,
        "release-codemux",
        "---\nname: codemux-release\ndescription: Release a new Codemux version\n---\nRun `gh release create`.\n",
    );

    // Codex user-wide; the .system/ subdir should be excluded.
    let codex_user = home.path().join(".codex").join("skills");
    fs::create_dir_all(&codex_user).unwrap();
    write_skill(
        &codex_user,
        "user-codex-skill",
        "---\nname: user-codex\ndescription: A user-installed codex skill\n---\nBody.",
    );
    let codex_system = codex_user.join(".system");
    fs::create_dir_all(&codex_system).unwrap();
    write_skill(
        &codex_system,
        "imagegen",
        "---\nname: imagegen\ndescription: Built-in\n---\nBody.",
    );

    // Codemux user-wide — pure prompt skill, should be `compatible`.
    let codemux_user = home.path().join(".codemux").join("skills");
    fs::create_dir_all(&codemux_user).unwrap();
    write_skill(
        &codemux_user,
        "post-generator",
        "---\nname: post-generator\ndescription: Pure prompt heuristics\n---\nWrite engaging copy.",
    );

    // Project Claude skill.
    let project_claude = project.path().join(".claude").join("skills");
    fs::create_dir_all(&project_claude).unwrap();
    write_skill(
        &project_claude,
        "project-only",
        "---\nname: project-only\ndescription: Project-scoped skill\n---\nBody.",
    );

    // Plugin-bundled Claude skill — under marketplaces/<m>/plugins/<p>/skills/.
    let plugin_skills = home
        .path()
        .join(".claude")
        .join("plugins")
        .join("marketplaces")
        .join("official")
        .join("plugins")
        .join("frontend-design")
        .join("skills");
    fs::create_dir_all(&plugin_skills).unwrap();
    write_skill(
        &plugin_skills,
        "frontend-design",
        "---\nname: frontend-design\ndescription: Aesthetic guidance\nlicense: MIT\n---\nBody.",
    );

    let skills = collect_skills(home.path(), Some(project.path()), true);

    let names: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();

    // User-wide skills present from each provider that has fixtures.
    assert!(names.contains(&"codemux-release"), "missing claude/user skill: {names:?}");
    assert!(names.contains(&"user-codex"), "missing codex/user skill: {names:?}");
    assert!(names.contains(&"post-generator"), "missing codemux/user skill: {names:?}");

    // Project-scoped Claude skill.
    assert!(names.contains(&"project-only"), "missing project skill: {names:?}");

    // Plugin-bundled skill arrived with scope=Plugin and slug populated.
    let plugin = skills
        .iter()
        .find(|s| s.name == "frontend-design")
        .expect("plugin skill missing");
    assert_eq!(plugin.scope, SkillScope::Plugin);
    assert_eq!(plugin.plugin_slug.as_deref(), Some("frontend-design"));

    // Codex `.system/` skill MUST NOT appear — excluded as built-in.
    assert!(
        !names.contains(&"imagegen"),
        ".system/ subdir should be excluded but found: {names:?}"
    );
}

#[test]
fn include_plugins_false_hides_plugin_pool() {
    let home = TempDir::new().unwrap();

    let plugin_skills = home
        .path()
        .join(".claude")
        .join("plugins")
        .join("marketplaces")
        .join("m")
        .join("plugins")
        .join("p")
        .join("skills");
    fs::create_dir_all(&plugin_skills).unwrap();
    write_skill(
        &plugin_skills,
        "should-be-hidden",
        "---\nname: should-be-hidden\ndescription: x\n---\nBody.",
    );

    let with_plugins = collect_skills(home.path(), None, true);
    let without_plugins = collect_skills(home.path(), None, false);

    assert!(with_plugins.iter().any(|s| s.name == "should-be-hidden"));
    assert!(without_plugins.iter().all(|s| s.name != "should-be-hidden"));
}

#[test]
fn empty_home_returns_empty_list() {
    let home = TempDir::new().unwrap();
    let skills = collect_skills(home.path(), None, true);
    assert!(skills.is_empty());
}

#[test]
fn compatibility_classified_per_skill() {
    let home = TempDir::new().unwrap();
    let claude_user = home.path().join(".claude").join("skills");
    fs::create_dir_all(&claude_user).unwrap();

    write_skill(
        &claude_user,
        "pure",
        "---\nname: pure\ndescription: prompt only\n---\nUse calm aesthetics and clear language.",
    );
    write_skill(
        &claude_user,
        "uses-bash",
        "---\nname: uses-bash\ndescription: shell skill\n---\nRun:\n```bash\nls\n```\n",
    );
    write_skill(
        &claude_user,
        "needs-mcp",
        "---\nname: needs-mcp\ndescription: mcp skill\n---\nCall mcp__foo__bar to do the thing.",
    );

    let skills = collect_skills(home.path(), None, false);

    let pure = skills.iter().find(|s| s.name == "pure").unwrap();
    assert_eq!(pure.compatibility, SkillCompatibility::Compatible);

    let bash = skills.iter().find(|s| s.name == "uses-bash").unwrap();
    assert_eq!(bash.compatibility, SkillCompatibility::SoftWarn);

    let mcp = skills.iter().find(|s| s.name == "needs-mcp").unwrap();
    assert_eq!(mcp.compatibility, SkillCompatibility::HardWarn);
}

#[cfg(unix)]
#[test]
fn symlinked_skill_resolves_and_is_flagged() {
    use std::os::unix::fs::symlink;

    let home = TempDir::new().unwrap();

    // Real skill lives outside the scan tree.
    let outside = TempDir::new().unwrap();
    let real_dir = outside.path().join("omarchy-real");
    fs::create_dir_all(&real_dir).unwrap();
    fs::write(
        real_dir.join("SKILL.md"),
        "---\nname: omarchy\ndescription: Linux desktop\n---\nBody.",
    )
    .unwrap();

    // Symlink it into ~/.claude/skills/omarchy.
    let claude_user = home.path().join(".claude").join("skills");
    fs::create_dir_all(&claude_user).unwrap();
    symlink(&real_dir, claude_user.join("omarchy")).unwrap();

    let skills = collect_skills(home.path(), None, false);
    let omarchy = skills.iter().find(|s| s.name == "omarchy").expect("omarchy missing");
    assert!(omarchy.symlinked, "expected symlinked=true");
    assert!(
        omarchy.file_path.contains("omarchy-real"),
        "file_path should be the resolved target, got {}",
        omarchy.file_path
    );
}
