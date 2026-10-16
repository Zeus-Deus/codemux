use super::*;
use std::fs;

fn write_plugin(root: &Path, id: &str, body: &str) -> PathBuf {
    let dir = root.join(id);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(ENTRY_FILE), body).unwrap();
    dir
}

#[test]
fn ids_are_single_lowercase_segments() {
    for ok in ["hello", "a", "my-plugin-2", &"x".repeat(64)] {
        assert!(valid_id(ok), "{ok}");
    }
    for bad in [
        "", "Hello", "../x", "a/b", "a\\b", ".hidden", "a.b", "a b", "ä", &"x".repeat(65),
    ] {
        assert!(!valid_id(bad), "{bad}");
    }
}

#[test]
fn list_sorts_skips_and_flags() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    write_plugin(root, "zeta", "export default {}");
    write_plugin(root, "alpha", "export default {}");
    write_plugin(root, "Bad_Name", "export default {}");
    write_plugin(root, ".staging-alpha", "export default {}");
    fs::create_dir_all(root.join("no-entry")).unwrap();
    fs::write(root.join("loose.js"), "x").unwrap();

    let entries = list(root).unwrap();
    let ids: Vec<_> = entries.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(ids, ["Bad_Name", "alpha", "zeta"]);
    assert!(entries[0].error.as_deref().unwrap().contains("not a valid plugin id"));
    assert!(entries[1].error.is_none() && entries[1].marker.is_none());
}

#[test]
fn list_of_missing_root_is_empty() {
    let tmp = tempfile::tempdir().unwrap();
    assert!(list(&tmp.path().join("absent")).unwrap().is_empty());
}

#[test]
fn marker_is_parsed_and_bad_marker_is_an_error_row() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = write_plugin(tmp.path(), "good", "x");
    fs::write(
        dir.join(MARKER_FILE),
        r#"{"source":"git","repo":"https://github.com/o/r","sha":"0123456789012345678901234567890123456789","tier":"unverified","installedAt":"2026-10-16T00:00:00Z"}"#,
    )
    .unwrap();
    let bad = write_plugin(tmp.path(), "bad", "x");
    fs::write(bad.join(MARKER_FILE), "{not json").unwrap();
    let entries = list(tmp.path()).unwrap();
    let good = entries.iter().find(|e| e.id == "good").unwrap();
    assert_eq!(good.marker.as_ref().unwrap().tier, "unverified");
    let bad = entries.iter().find(|e| e.id == "bad").unwrap();
    assert!(bad.error.as_deref().unwrap().contains("marker"));
}

#[test]
fn read_source_reads_whole_file_and_refuses_bad_ids() {
    let tmp = tempfile::tempdir().unwrap();
    write_plugin(tmp.path(), "hello", "export default { id: 'hello' }");
    assert_eq!(
        read_source(tmp.path(), "hello").unwrap(),
        "export default { id: 'hello' }"
    );
    for bad in ["../hello", "hello/../hello", "", "HELLO"] {
        assert!(read_source(tmp.path(), bad).is_err(), "{bad}");
    }
    assert!(read_source(tmp.path(), "absent").is_err());
}

#[test]
fn read_source_refuses_oversize_instead_of_truncating() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("big");
    fs::create_dir_all(&dir).unwrap();
    let file = fs::File::create(dir.join(ENTRY_FILE)).unwrap();
    file.set_len(MAX_SOURCE_BYTES + 1).unwrap();
    let error = read_source(tmp.path(), "big").unwrap_err();
    assert!(error.contains("limit"), "{error}");
}

#[cfg(unix)]
#[test]
fn entry_symlink_escaping_the_folder_is_refused() {
    let tmp = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("secret.js"), "secret").unwrap();
    let dir = tmp.path().join("sneaky");
    fs::create_dir_all(&dir).unwrap();
    std::os::unix::fs::symlink(outside.path().join("secret.js"), dir.join(ENTRY_FILE)).unwrap();
    let error = read_source(tmp.path(), "sneaky").unwrap_err();
    assert!(error.contains("outside"), "{error}");
}

#[cfg(unix)]
#[test]
fn linked_plugin_folder_is_readable_and_removal_unlinks_only() {
    let tmp = tempfile::tempdir().unwrap();
    let checkout = tempfile::tempdir().unwrap();
    fs::write(checkout.path().join(ENTRY_FILE), "export default {}").unwrap();
    std::os::unix::fs::symlink(checkout.path(), tmp.path().join("linked")).unwrap();
    assert_eq!(read_source(tmp.path(), "linked").unwrap(), "export default {}");

    paths::remove_plugin_dir(tmp.path(), "linked").unwrap();
    assert!(!tmp.path().join("linked").exists());
    // The link target survives.
    assert!(checkout.path().join(ENTRY_FILE).is_file());
}

#[test]
fn remove_refuses_traversal_and_removes_folder() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("plugins");
    fs::create_dir_all(&root).unwrap();
    write_plugin(&root, "gone", "x");
    fs::write(tmp.path().join("victim"), "keep").unwrap();
    for bad in ["..", "../victim", "gone/..", ""] {
        assert!(paths::remove_plugin_dir(&root, bad).is_err(), "{bad}");
    }
    assert!(tmp.path().join("victim").is_file());
    paths::remove_plugin_dir(&root, "gone").unwrap();
    assert!(!root.join("gone").exists());
}

#[test]
fn watcher_maps_paths_to_folders() {
    let root = Path::new("/p/plugins");
    assert_eq!(watch::changed_folder(root, root), Some(None));
    assert_eq!(
        watch::changed_folder(root, &root.join("hello/plugin.js")),
        Some(Some("hello".into()))
    );
    assert_eq!(watch::changed_folder(root, &root.join(".staging-hello/plugin.js")), None);
    assert_eq!(watch::changed_folder(root, Path::new("/elsewhere/x")), None);
}

#[test]
fn watcher_reports_a_saved_plugin_file() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("plugins");
    fs::create_dir_all(root.join("hello")).unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let _watcher = watch::start(root.clone(), move |changes| {
        let _ = tx.send(changes);
    })
    .unwrap();
    std::thread::sleep(std::time::Duration::from_millis(100));
    fs::write(root.join("hello").join(ENTRY_FILE), "x").unwrap();
    let changes = rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
    assert!(changes.iter().any(|c| c.id.as_deref() == Some("hello")), "{changes:?}");
}
