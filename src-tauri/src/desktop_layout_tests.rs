#![cfg(unix)]

use super::{core_startup_plugin, AppMode};
use std::fs;

#[test]
fn desktop_rejects_an_unreadable_layout_before_setup() {
    if crate::test_isolation::isolated(
        "desktop_layout_tests::desktop_rejects_an_unreadable_layout_before_setup",
    ) {
        return;
    }

    let root = crate::test_isolation::isolated_root().unwrap();
    let layout = root.join("layout.json");
    let original = b"{\"workspaces\":[";
    fs::write(&layout, original).unwrap();

    // If startup proceeds unexpectedly, keep browser cleanup in the fixture.
    let fixture_pid = crate::agent_browser::session_pid_path("layout-test").unwrap();
    assert!(fixture_pid.starts_with(&root));
    fs::write(fixture_pid, b"invalid fixture PID").unwrap();

    let app = tauri::test::mock_builder()
        .plugin(core_startup_plugin(AppMode::Gui, None))
        .setup(|_| panic!("desktop setup ran with an unreadable layout"))
        .build(tauri::test::mock_context(tauri::test::noop_assets()));
    let Err(error) = app else {
        panic!("desktop startup accepted an unreadable layout");
    };

    assert!(
        error.to_string().contains("Failed to decode workspace layout"),
        "{error}"
    );
    assert_eq!(fs::read(layout).unwrap(), original);
}
