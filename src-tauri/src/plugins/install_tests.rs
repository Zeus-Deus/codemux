//! Installer tests against a local bare repository (no network).
use super::catalog::Catalog;
use super::install::{self, check_subdir, parse_repo, scan_meta, Existing, Git};
use super::{list, InstallMarker, ENTRY_FILE, MARKER_FILE};
use std::path::{Path, PathBuf};
use std::process::Command;

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .current_dir(dir)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .args(["-c", "user.name=t", "-c", "user.email=t@example.invalid", "-c", "commit.gpgsign=false"])
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A bare repo at `<tmp>/origin.git` whose history has two commits of a
/// plugin in `codemux/`, plus traps the installer must not copy.
struct Fixture {
    _tmp: tempfile::TempDir,
    url: String,
    first: String,
    second: String,
    root: PathBuf,
}

fn fixture() -> Fixture {
    let tmp = tempfile::tempdir().unwrap();
    let work = tmp.path().join("work");
    std::fs::create_dir_all(work.join("codemux/nested")).unwrap();
    git(&work, &["init", "-q", "-b", "main"]);
    let plugin = |v: &str| {
        format!("export default {{ id: \"hello\", name: \"Hello\", description: \"Says hello\", version: \"{v}\", register() {{}} }};\n")
    };
    std::fs::write(work.join("codemux").join(ENTRY_FILE), plugin("1.0.0")).unwrap();
    std::fs::write(work.join("codemux/README.md"), "readme").unwrap();
    std::fs::write(work.join("codemux/nested/deep.js"), "nested").unwrap();
    // A marker in the repo must never be trusted or copied.
    std::fs::write(work.join("codemux").join(MARKER_FILE), r#"{"source":"catalog","repo":"x","sha":"x","tier":"official","installedAt":"x"}"#).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink("/etc/passwd", work.join("codemux/leak")).unwrap();
    std::fs::write(work.join("package.json"), r#"{"scripts":{"postinstall":"touch PWNED"}}"#).unwrap();
    git(&work, &["add", "-A"]);
    git(&work, &["commit", "-q", "-m", "one"]);
    let first = git(&work, &["rev-parse", "HEAD"]);
    std::fs::write(work.join("codemux").join(ENTRY_FILE), plugin("2.0.0")).unwrap();
    git(&work, &["commit", "-q", "-am", "two"]);
    let second = git(&work, &["rev-parse", "HEAD"]);
    let origin = tmp.path().join("origin.git");
    git(tmp.path(), &["clone", "-q", "--bare", work.to_str().unwrap(), origin.to_str().unwrap()]);
    // Allow fetching the first (non-tip) commit by id from the file remote.
    git(&origin, &["config", "uploadpack.allowAnySHA1InWant", "true"]);
    let root = tmp.path().join("plugins");
    std::fs::create_dir_all(&root).unwrap();
    Fixture { url: format!("file://{}", origin.display()), first, second, root, _tmp: tmp }
}

fn test_git() -> Git {
    Git::with(which::which("git").unwrap(), true).unwrap()
}

fn marker(sha: &str) -> InstallMarker {
    InstallMarker {
        source: "git".into(),
        repo: "https://github.com/owner/hello".into(),
        sha: sha.into(),
        catalog_id: None,
        tier: "unverified".into(),
        installed_at: install::now_rfc3339(),
        subdir: Some("codemux".into()),
        git_ref: None,
        name: Some("Hello".into()),
        description: None,
    }
}

#[test]
fn installs_a_pinned_commit_from_a_bare_repo_then_updates_and_removes() {
    let f = fixture();
    // Install the older commit by sha.
    let fetched = install::fetch_plugin(&test_git(), &f.url, None, Some(&f.first), None).unwrap();
    assert_eq!(fetched.sha, f.first);
    assert_eq!(fetched.subdir.as_deref(), Some("codemux"), "found codemux/plugin.js");
    assert_eq!(fetched.meta.id.as_deref(), Some("hello"));
    assert_eq!(fetched.meta.version.as_deref(), Some("1.0.0"));
    let names: Vec<&str> = fetched.files.iter().map(|(n, _)| n.as_str()).collect();
    assert!(names.contains(&ENTRY_FILE) && names.contains(&"README.md"), "{names:?}");
    assert!(!names.contains(&MARKER_FILE), "a repository marker is never copied");
    assert!(!names.contains(&"leak"), "symlinks are never copied");
    assert!(!names.iter().any(|n| n.contains("deep")), "sub-folders are not copied");
    install::write_install(&f.root, "hello", &fetched.files, &marker(&fetched.sha)).unwrap();

    let entries = list(&f.root).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].marker.as_ref().unwrap().sha, f.first);
    assert_eq!(entries[0].marker.as_ref().unwrap().tier, "unverified");
    assert!(!f.root.join("hello/leak").exists());
    assert!(!f.root.join("PWNED").exists() && !f.root.join("hello/PWNED").exists(), "no repo scripts ran");

    // Update to the branch tip.
    let newer = install::fetch_plugin(&test_git(), &f.url, None, Some("main"), Some("codemux")).unwrap();
    assert_eq!(newer.sha, f.second);
    assert_eq!(newer.git_ref.as_deref(), Some("main"));
    assert!(matches!(install::existing(&f.root, "hello").unwrap(), Existing::Installed(_)));
    install::write_install(&f.root, "hello", &newer.files, &marker(&newer.sha)).unwrap();
    let source = std::fs::read_to_string(f.root.join("hello").join(ENTRY_FILE)).unwrap();
    assert!(source.contains("2.0.0"));
    assert_eq!(list(&f.root).unwrap()[0].marker.as_ref().unwrap().sha, f.second);
    assert!(
        std::fs::read_dir(&f.root).unwrap().flatten().all(|e| !e.file_name().to_string_lossy().starts_with('.')),
        "no staging or backup folders left"
    );

    super::paths::remove_plugin_dir(&f.root, "hello").unwrap();
    assert!(list(&f.root).unwrap().is_empty());
}

#[test]
fn a_tree_url_resolves_the_ref_and_folder() {
    let f = fixture();
    let fetched = install::fetch_plugin(&test_git(), &f.url, Some("main/codemux"), None, None).unwrap();
    assert_eq!(fetched.sha, f.second);
    assert_eq!(fetched.subdir.as_deref(), Some("codemux"));
    let err = install::fetch_plugin(&test_git(), &f.url, Some("nope/codemux"), None, None).err().unwrap();
    assert!(err.contains("No branch or tag"), "{err}");
}

#[test]
fn a_local_or_linked_folder_is_never_replaced() {
    let f = fixture();
    std::fs::create_dir_all(f.root.join("hello")).unwrap();
    std::fs::write(f.root.join("hello").join(ENTRY_FILE), "mine").unwrap();
    let fetched = install::fetch_plugin(&test_git(), &f.url, None, Some(&f.second), None).unwrap();
    let err = install::write_install(&f.root, "hello", &fetched.files, &marker(&fetched.sha)).unwrap_err();
    assert!(err.contains("Local plugin folder"), "{err}");
    assert_eq!(std::fs::read_to_string(f.root.join("hello").join(ENTRY_FILE)).unwrap(), "mine");
    #[cfg(unix)]
    {
        let elsewhere = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(elsewhere.path(), f.root.join("linked")).unwrap();
        let err = install::write_install(&f.root, "linked", &fetched.files, &marker(&fetched.sha)).unwrap_err();
        assert!(err.contains("linked folder"), "{err}");
    }
}

#[test]
fn ids_and_paths_refuse_traversal_and_symlinks() {
    let f = fixture();
    for bad in ["../evil", "a/b", "..", ".hidden", "Evil", "a\\b", ""] {
        assert!(install::write_install(&f.root, bad, &[], &marker(&f.first)).is_err(), "{bad}");
        assert!(super::paths::remove_plugin_dir(&f.root, bad).is_err(), "{bad}");
    }
    for bad in ["../x", "/etc", "a/../b", "a\\b", "./a", "a//b"] {
        assert!(check_subdir(bad).is_err(), "{bad}");
    }
    // A file name with a separator from a hostile tree is dropped, not written.
    install::write_install(
        &f.root,
        "safe",
        &[("plugin.js".into(), b"ok".to_vec()), ("../escape.js".into(), b"x".to_vec())],
        &marker(&f.first),
    )
    .unwrap();
    assert!(!f.root.join("escape.js").exists());
    assert!(!f.root.parent().unwrap().join("escape.js").exists());
    #[cfg(unix)]
    {
        // Removing a symlinked folder unlinks it and leaves the target alone.
        let target = tempfile::tempdir().unwrap();
        std::fs::write(target.path().join(ENTRY_FILE), "keep").unwrap();
        std::os::unix::fs::symlink(target.path(), f.root.join("link")).unwrap();
        super::paths::remove_plugin_dir(&f.root, "link").unwrap();
        assert!(target.path().join(ENTRY_FILE).exists());
    }
}

#[test]
fn interrupted_installs_are_recovered() {
    let f = fixture();
    std::fs::create_dir_all(f.root.join(".staging-hello.1")).unwrap();
    std::fs::create_dir_all(f.root.join(".backup-gone.2")).unwrap();
    std::fs::write(f.root.join(".backup-gone.2").join(ENTRY_FILE), "restored").unwrap();
    install::recover(&f.root);
    assert!(!f.root.join(".staging-hello.1").exists());
    assert_eq!(std::fs::read_to_string(f.root.join("gone").join(ENTRY_FILE)).unwrap(), "restored");
}

#[test]
fn github_urls_are_the_only_accepted_source() {
    let ok = parse_repo("https://github.com/owner/repo.git").unwrap();
    assert_eq!(ok.repo, "https://github.com/owner/repo");
    let tree = parse_repo("https://github.com/owner/repo/tree/main/codemux").unwrap();
    assert_eq!(tree.tree.as_deref(), Some("main/codemux"));
    for bad in [
        "http://github.com/o/r",
        "https://gitlab.com/o/r",
        "https://user:pw@github.com/o/r",
        "https://github.com:8443/o/r",
        "https://github.com/o",
        "https://github.com/o/r?x=1",
        "https://github.com/o/r/blob/main/x",
        "https://github.com/../r",
        "file:///tmp/r",
        "git@github.com:o/r.git",
    ] {
        assert!(parse_repo(bad).is_err(), "{bad}");
    }
}

#[test]
fn metadata_is_read_statically() {
    let m = scan_meta("const x = { id: 'nope' };\nexport default { id: \"hello\", name: \"Hello\", version: '1.2.3', register(){} };");
    assert_eq!(m.id.as_deref(), Some("hello"));
    assert_eq!(m.name.as_deref(), Some("Hello"));
    assert_eq!(m.version.as_deref(), Some("1.2.3"));
    assert_eq!(scan_meta("export default { id: \"Bad Id\" }").id, None);
}

#[test]
fn the_catalog_validates_and_blocks() {
    let sha = "a".repeat(40);
    let json = format!(
        r#"{{"schemaVersion":1,"revision":3,"plugins":[{{"id":"hello","name":"Hello","tier":"unverified","repo":"https://github.com/o/r","sha":"{sha}"}}],
            "blocked":[{{"sha":"{bad}","reason":"leaked a token"}},{{"id":"evil","reason":"malware"}}]}}"#,
        bad = "b".repeat(40)
    );
    let catalog = Catalog::parse(json.as_bytes()).unwrap();
    assert_eq!(catalog.entry("hello").unwrap().sha, sha);
    assert_eq!(catalog.blocked_reason("evil", None).as_deref(), Some("malware"));
    assert_eq!(catalog.blocked_reason("hello", Some(&"b".repeat(40))).as_deref(), Some("leaked a token"));
    assert_eq!(catalog.blocked_reason("hello", Some(&sha)), None);
    for bad in [
        r#"{"schemaVersion":2,"revision":1}"#.to_string(),
        format!(r#"{{"schemaVersion":1,"revision":1,"plugins":[{{"id":"x","name":"x","tier":"gold","repo":"https://github.com/o/r","sha":"{sha}"}}]}}"#),
        r#"{"schemaVersion":1,"revision":1,"plugins":[{"id":"x","name":"x","tier":"official","repo":"https://github.com/o/r","sha":"main"}]}"#.to_string(),
        format!(r#"{{"schemaVersion":1,"revision":1,"plugins":[{{"id":"x","name":"x","tier":"official","repo":"https://evil.example/o/r","sha":"{sha}"}}]}}"#),
        r#"{"schemaVersion":1,"revision":1,"blocked":[{"reason":"no target"}]}"#.to_string(),
    ] {
        assert!(Catalog::parse(bad.as_bytes()).is_err(), "{bad}");
    }
    // The bundled catalog shipped with the app is valid.
    let _ = super::catalog::bundled();
}

#[tokio::test]
async fn a_failed_catalog_fetch_keeps_the_previous_catalog() {
    let cache = super::catalog::CatalogCache::default();
    let good = br#"{"schemaVersion":1,"revision":5,"plugins":[],"blocked":[{"id":"evil","reason":"r"}]}"#.to_vec();
    *cache.fetcher.lock().unwrap() = Some(Box::new(move || Ok(good.clone())));
    let view = cache.get(true).await;
    assert_eq!(view.catalog.revision, 5);
    assert!(view.fetched_at.is_some());
    *cache.fetcher.lock().unwrap() = Some(Box::new(|| Err("offline".into())));
    let view = cache.get(true).await;
    assert_eq!(view.catalog.revision, 5, "kept");
    assert_eq!(view.error.as_deref(), Some("offline"));
    let older = br#"{"schemaVersion":1,"revision":2,"plugins":[]}"#.to_vec();
    *cache.fetcher.lock().unwrap() = Some(Box::new(move || Ok(older.clone())));
    assert_eq!(cache.get(true).await.catalog.revision, 5, "never rolls back");
}

#[test]
fn a_late_catalog_result_never_undoes_a_newer_one() {
    // Two refreshes started from revision 1. The newer response (revision 3,
    // which adds a block) lands first; the older one (revision 2) and a
    // failing one land after it. Both are merged against what is cached NOW.
    let cache = super::catalog::CatalogCache::default();
    let parse = |json: &str| Catalog::parse(json.as_bytes());
    cache.accept(parse(r#"{"schemaVersion":1,"revision":1,"plugins":[]}"#));
    cache.accept(parse(
        r#"{"schemaVersion":1,"revision":3,"plugins":[],"blocked":[{"id":"evil","reason":"malware"}]}"#,
    ));
    cache.accept(parse(r#"{"schemaVersion":1,"revision":2,"plugins":[]}"#));
    let view = cache.current();
    assert_eq!(view.catalog.revision, 3);
    assert_eq!(view.catalog.blocked_reason("evil", None).as_deref(), Some("malware"));
    cache.accept(Err("offline".into()));
    let view = cache.current();
    assert_eq!(view.catalog.revision, 3, "a failure keeps the newest catalog");
    assert_eq!(view.catalog.blocked_reason("evil", None).as_deref(), Some("malware"));
    assert_eq!(view.error.as_deref(), Some("offline"));
}
