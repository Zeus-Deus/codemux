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
    // Serve like GitHub: any commit by id (the first is not a tip), and
    // partial fetches, so a `--filter` git ignored would fail the tests.
    git(&origin, &["config", "uploadpack.allowAnySHA1InWant", "true"]);
    git(&origin, &["config", "uploadpack.allowFilter", "true"]);
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
fn only_commits_on_a_branch_or_tag_of_the_repository_install() {
    let f = fixture();
    let origin = PathBuf::from(f.url.strip_prefix("file://").unwrap());
    let work = origin.parent().unwrap().join("work");
    let origin = origin.to_str().unwrap();
    let commit = |version: &str| {
        std::fs::write(work.join("codemux").join(ENTRY_FILE), format!("export default {{ id: \"hello\", version: \"{version}\" }};\n")).unwrap();
        git(&work, &["commit", "-q", "-am", version]);
        git(&work, &["rev-parse", "HEAD"])
    };

    // Reachable only through an annotated tag, one commit behind its tip.
    git(&work, &["checkout", "-q", "-b", "release", &f.first]);
    let tagged_parent = commit("1.1.0");
    commit("1.2.0");
    git(&work, &["tag", "-a", "v1.2.0", "-m", "v1.2.0"]);
    git(&work, &["push", "-q", origin, "refs/tags/v1.2.0"]);

    // What GitHub serves through a parent's URL but no branch or tag holds:
    // a pull request head, and a commit whose only ref is gone (a fork's).
    git(&work, &["checkout", "-q", "-b", "fork", &f.second]);
    let pull = commit("6.6.6");
    git(&work, &["push", "-q", origin, "HEAD:refs/pull/1/head"]);
    let fork = commit("6.6.7");
    git(&work, &["push", "-q", origin, "HEAD:refs/heads/gone"]);
    git(Path::new(origin), &["update-ref", "-d", "refs/heads/gone"]);

    for (tree, want) in [
        (Some(format!("{fork}/codemux")), None),
        (None, Some(fork.clone())),
        (None, Some(pull.clone())),
        (None, Some("refs/pull/1/head".to_string())),
    ] {
        let err = install::fetch_plugin(&test_git(), &f.url, tree.as_deref(), want.as_deref(), None)
            .err()
            .unwrap_or_else(|| panic!("{tree:?} {want:?} installed"));
        assert!(err.contains("isn't on any branch or tag") && err.contains("fork"), "{err}");
    }

    // Branch tips, older branch commits and tagged history still install.
    for (want, sha) in [
        ("main", &f.second),
        (f.second.as_str(), &f.second),
        (f.first.as_str(), &f.first),
        ("v1.2.0", &git(&work, &["rev-parse", "v1.2.0^{commit}"])),
        (tagged_parent.as_str(), &tagged_parent),
    ] {
        let fetched = install::fetch_plugin(&test_git(), &f.url, None, Some(want), None).unwrap();
        assert_eq!(&fetched.sha, sha, "{want}");
    }
    let fetched = install::fetch_plugin(&test_git(), &f.url, Some(&format!("{tagged_parent}/codemux")), None, None).unwrap();
    assert_eq!(fetched.sha, tagged_parent);
}

/// Commits `count` more versions on the fixture's work tree and returns
/// their ids, oldest first.
fn commits(work: &Path, count: usize) -> Vec<String> {
    (0..count)
        .map(|n| {
            std::fs::write(work.join("codemux/README.md"), format!("readme {n}")).unwrap();
            git(work, &["commit", "-q", "-am", &format!("c{n}")]);
            git(work, &["rev-parse", "HEAD"])
        })
        .collect()
}

#[test]
fn provenance_search_is_bounded_and_says_when_it_gave_up() {
    let f = fixture();
    let origin = PathBuf::from(f.url.strip_prefix("file://").unwrap());
    let work = origin.parent().unwrap().join("work");
    let check = |sha: &str, depths: &[u32]| {
        let git = test_git();
        git.check_on_branch_or_tag_within(&f.url, sha, depths)
    };
    let push = |refspec: &str| git(&work, &["push", "-q", "-f", origin.to_str().unwrap(), refspec]);

    // On a branch, eight commits below its tip: found once the search
    // reaches it, and refused with the budget message before then.
    let main = commits(&work, 8);
    push("main");
    check(&f.second, &[2, 4, 16]).unwrap();
    let err = check(&f.second, &[2, 4]).unwrap_err();
    assert!(err.contains("Could not confirm") && err.contains("within the last 4 commits"), "{err}");

    // Reachable only through an annotated tag (no branch ever held it).
    git(&work, &["checkout", "-q", "-b", "release", &f.first]);
    let release = commits(&work, 3);
    git(&work, &["tag", "-a", "v9", "-m", "v9"]);
    push("refs/tags/v9");
    check(&release[0], &[1, 3, 16]).unwrap();
    check(&release[2], &[1]).unwrap();

    // On no branch or tag: every history is complete within the search, so
    // it is refused as unreachable, not as out of budget.
    git(&work, &["checkout", "-q", "-b", "stray", &main[7]]);
    let stray = commits(&work, 1);
    push("HEAD:refs/heads/stray");
    git(&origin, &["update-ref", "-d", "refs/heads/stray"]);
    let err = check(&stray[0], &[2, 100]).unwrap_err();
    assert!(err.contains("isn't on any branch or tag"), "{err}");
    // A search too short to see everything can't call it unreachable.
    let err = check(&stray[0], &[2]).unwrap_err();
    assert!(err.contains("within the last 2 commits"), "{err}");
}

#[test]
fn provenance_checks_the_default_branch_before_large_unrelated_refs_and_caps_all_packs() {
    let f = fixture();
    let origin = PathBuf::from(f.url.strip_prefix("file://").unwrap());
    let work = origin.parent().unwrap().join("work");
    let origin = origin.to_str().unwrap();

    // An unrelated branch has a large, hard-to-compress commit object. A
    // wildcard fetch would download it even when main proves the pin.
    git(&work, &["checkout", "-q", "-b", "zlarge"]);
    let mut seed = 1u64;
    let alphabet = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
    let message: String = (0..3_000_000)
        .map(|_| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            alphabet[(seed as usize) % alphabet.len()] as char
        })
        .collect();
    let message_file = work.parent().unwrap().join("large-message");
    std::fs::write(&message_file, message).unwrap();
    git(&work, &["commit", "-q", "--allow-empty", "-F", message_file.to_str().unwrap()]);
    git(&work, &["push", "-q", origin, "zlarge"]);

    let verifier = test_git();
    verifier.check_on_branch_or_tag_with_budget(&f.url, &f.first, &[2], 1024 * 1024).unwrap();

    // A deleted ref leaves a fetchable object, but no named ref proves it.
    // The large branch must exhaust the shared disk budget and fail closed.
    git(&work, &["checkout", "-q", "-b", "stray", "main"]);
    let stray = commits(&work, 1).pop().unwrap();
    git(&work, &["push", "-q", origin, "stray"]);
    git(Path::new(origin), &["update-ref", "-d", "refs/heads/stray"]);
    let error = verifier.check_on_branch_or_tag_with_budget(&f.url, &stray, &[1, 2], 1024 * 1024).unwrap_err();
    assert!(error.contains("Could not confirm commit") && error.contains("download exceeded 1 MiB"), "{error}");
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
fn plugin_install_identity_is_checked_before_writing() {
    let git = marker(&"a".repeat(40));
    for original in [
        git.clone(),
        InstallMarker { source: "catalog".into(), catalog_id: Some("hello".into()), ..git },
    ] {
        let root = tempfile::tempdir().unwrap();
        let files = vec![(ENTRY_FILE.into(), b"original".to_vec())];
        install::write_install(root.path(), "hello", &files, &original).unwrap();
        let moved = InstallMarker { repo: "https://github.com/fork/hello".into(), ..original.clone() };
        let other_source = InstallMarker {
            source: if original.source == "git" { "catalog" } else { "git" }.into(),
            catalog_id: Some("hello".into()),
            ..original.clone()
        };
        let other_plugin = match original.source.as_str() {
            "catalog" => InstallMarker { catalog_id: Some("other".into()), ..original.clone() },
            _ => moved.clone(),
        };
        for replacement in [other_source, other_plugin] {
            let error = install::write_install(
                root.path(),
                "hello",
                &[(ENTRY_FILE.into(), b"replacement".to_vec())],
                &replacement,
            )
            .expect_err("an unrelated install must not replace the installed plugin");
            assert!(
                error.contains("this install is from") && error.contains("Remove it first"),
                "{error}"
            );
            assert_eq!(std::fs::read(root.path().join("hello").join(ENTRY_FILE)).unwrap(), b"original");
            assert_eq!(super::read_marker(&root.path().join("hello")).unwrap(), Some(original.clone()));
            assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1, "no staging or backup folders");
        }
        // A catalog entry whose repository moved is still an update.
        if original.source == "catalog" {
            install::write_install(root.path(), "hello", &files, &moved).unwrap();
            assert_eq!(super::read_marker(&root.path().join("hello")).unwrap(), Some(moved));
        }
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
