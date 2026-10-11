#[test]
fn delegation_recovery_starts_after_canonical_workspace_restoration() {
    let source = include_str!("../src/lib.rs");
    let restore = source
        .find("state.recover_local_import_layout(&db)")
        .expect("canonical layout recovery must remain in app setup");
    let coordinator = source
        .find("delegation::coordinator::install(app.handle())")
        .expect("delegation coordinator must be installed in app setup");
    assert!(
        coordinator > restore,
        "Delegation recovery must not hold saved results before parent workspaces are restored"
    );
}
