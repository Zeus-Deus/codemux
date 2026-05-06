//! Smoke test against the developer's real ~/.claude/skills/ to surface
//! frontmatter or compatibility classifier issues on real-world data.
//!
//! Ignored by default so CI doesn't depend on the host's skill collection.
//! Run with: `cargo test --test skills_real_disk_smoke -- --ignored --nocapture`

use codemux_lib::skills::{paths::enumerate_scan_paths, scanner::scan_directory, SkillScope};

#[test]
#[ignore]
fn scan_real_home_and_print_summary() {
    let paths = enumerate_scan_paths(None, true);

    let mut total = 0usize;
    let mut compatible = 0usize;
    let mut soft = 0usize;
    let mut hard = 0usize;
    let mut by_provider: std::collections::BTreeMap<String, usize> =
        std::collections::BTreeMap::new();

    println!("\n=== USER PATHS ===");
    for (path, provider) in &paths.user_paths {
        let skills = scan_directory(path, *provider, SkillScope::User, None);
        if skills.is_empty() && !path.exists() {
            continue;
        }
        println!(
            "  {} ({:?}) -> {} skill(s)",
            path.display(),
            provider,
            skills.len()
        );
        for s in &skills {
            *by_provider.entry(format!("{:?}/user", s.provider)).or_insert(0) += 1;
            total += 1;
            match s.compatibility {
                codemux_lib::skills::SkillCompatibility::Compatible => compatible += 1,
                codemux_lib::skills::SkillCompatibility::SoftWarn => soft += 1,
                codemux_lib::skills::SkillCompatibility::HardWarn => hard += 1,
            }
            println!(
                "    - {:24}  [{:?}]  signals={:?}",
                s.name, s.compatibility, s.compatibility_signals
            );
        }
    }

    println!("\n=== PLUGIN PATHS ===");
    let mut plugin_count = 0;
    for (path, slug) in &paths.plugin_paths {
        let skills = scan_directory(
            path,
            codemux_lib::skills::SkillProvider::Claude,
            SkillScope::Plugin,
            Some(slug.clone()),
        );
        plugin_count += skills.len();
        for s in &skills {
            *by_provider.entry(format!("{:?}/plugin", s.provider)).or_insert(0) += 1;
            total += 1;
            match s.compatibility {
                codemux_lib::skills::SkillCompatibility::Compatible => compatible += 1,
                codemux_lib::skills::SkillCompatibility::SoftWarn => soft += 1,
                codemux_lib::skills::SkillCompatibility::HardWarn => hard += 1,
            }
        }
    }
    println!("  total plugin skills discovered: {plugin_count}");

    println!("\n=== SUMMARY ===");
    println!("  total: {total}");
    println!("  compatible: {compatible}");
    println!("  soft-warn:  {soft}");
    println!("  hard-warn:  {hard}");
    println!("  by provider/scope: {by_provider:#?}");
}
