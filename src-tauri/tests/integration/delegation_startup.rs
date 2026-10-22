#[test]
fn delegation_wire_owner_is_portable_without_opening_the_receiver() {
    let root = include_str!("../../src/lib.rs").replace("\r\n", "\n");
    assert!(
        root.contains("#[path = \"remote/tasks/types.rs\"]\npub mod remote_task_protocol;"),
        "remote task wire DTOs need an unconditional portable owner"
    );
    for module in ["remote", "pty_daemon", "ssh"] {
        assert!(root.contains(&format!("#[cfg(unix)]\npub mod {module};")));
    }
    let receiver = include_str!("../../src/remote/tasks/mod.rs").replace("\r\n", "\n");
    assert!(receiver.contains("pub use crate::remote_task_protocol::*;"));
    assert!(!receiver.contains("mod types;"), "do not compile a second wire owner");
    let workspace = include_str!("../../src/remote/workspace.rs").replace("\r\n", "\n");
    assert!(workspace.contains("pub use crate::remote_task_protocol::Workspace;"));
    assert!(!workspace.contains("pub struct Workspace {"));
    let wire = include_str!("../../src/remote/tasks/types.rs").replace("\r\n", "\n");
    assert!(!wire.contains("crate::remote::"));
    assert!(!wire.contains("super::workspace::"));
    for client in [
        include_str!("../../src/delegation/app.rs").replace("\r\n", "\n"),
        include_str!("../../src/delegation/commands.rs").replace("\r\n", "\n"),
        include_str!("../../src/delegation/coordinator.rs").replace("\r\n", "\n"),
        include_str!("../../src/delegation/journal.rs").replace("\r\n", "\n"),
        include_str!("../../src/delegation/types.rs").replace("\r\n", "\n"),
    ] {
        assert!(!client.contains("remote::tasks::"), "portable client must not depend on Unix receiver");
    }
    let delegation = include_str!("../../src/delegation/mod.rs").replace("\r\n", "\n");
    assert!(delegation.contains("pub mod app;\n"));
    assert!(delegation.contains("pub mod commands;\n"));
    assert!(delegation.contains("pub mod coordinator;\n"));
    assert!(!delegation.contains("#[cfg(unix)]"), "do not hide runtime clients on Windows");
}

#[test]
fn delegation_recovery_starts_after_canonical_workspace_restoration() {
    let source = include_str!("../../src/lib.rs").replace("\r\n", "\n");
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
