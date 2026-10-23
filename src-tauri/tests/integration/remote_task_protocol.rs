//! This target is deliberately portable: no Unix receiver imports outside the identity test.
use codemux_lib::remote_task_protocol::*;
use serde::{de::DeserializeOwned, Serialize};
use serde_json::{json, Value};

fn roundtrip<T: DeserializeOwned + Serialize>(wire: Value) -> T {
    let value: T = serde_json::from_value(wire.clone()).unwrap();
    assert_eq!(serde_json::to_value(&value).unwrap(), wire);
    value
}
fn launch_wire() -> Value {
    json!({"id":"opaque/task:01","parent_thread_id":"parent/thread:01","parent_label":"Parent",
        "workspace_path":"/remote/checkout","prompt":"exact\ninput","provider":"codex",
        "model":null,"permission_mode":"default","effort":null})
}
fn approval_wire() -> Value {
    json!({"request_id":"opaque/request:01","request_kind":"user-input",
        "payload":{"questions":[{"question":"Original?","options":[]}],"nested":[null,true,1,"exact"]}})
}
fn snapshot_wire() -> Value {
    json!({"id":"opaque/task:01","request":launch_wire(),"child_thread_id":"child/thread:01",
        "provider_session_id":"session:01","turn_id":"turn:01","status":"awaiting_approval",
        "activity":"Waiting","result":null,"error":null,"created_at":"created","updated_at":"updated",
        "cancel_requested":false,"pending_requests":[approval_wire()]})
}

#[test]
fn portable_client_public_type_contract() {
    use codemux_lib::delegation::{journal::Journal, types::LocalTask};
    let _: fn(&Journal, i64) -> Result<TaskCapabilities, String> = Journal::host_info;
    let _: fn(&Journal, i64, &TaskCapabilities) -> Result<(), String> = Journal::cache_host;
    fn projection(task: &LocalTask) -> (TaskStatus, Option<&TaskSnapshot>) {
        (task.status, task.remote.as_ref())
    }
    let _: fn(&LocalTask) -> (TaskStatus, Option<&TaskSnapshot>) = projection;
}

#[test]
fn launch_schema_preserves_one_shot_identity_and_strict_fields() {
    let request: LaunchRequest = roundtrip(launch_wire());
    assert_eq!(request.id, "opaque/task:01");
    assert_eq!(request.prompt, "exact\ninput");
    let mut extra = launch_wire();
    extra["unexpected"] = json!(true);
    assert!(serde_json::from_value::<LaunchRequest>(extra).is_err());
}

#[test]
fn response_schema_preserves_complete_original_consent() {
    let wire = json!({"original_request":approval_wire(),"request_id":"opaque/request:01",
        "decision":{"decision":"allow","updated_input":{"answers":{"Original?":"Exact immutable answer"}},
        "updated_permissions":null}});
    let response: RespondRequest = roundtrip(wire.clone());
    let original = response.original_request.unwrap();
    assert_eq!(original.request_id, response.request_id);
    assert_eq!(original.request_kind, "user-input");
    assert_eq!(original.payload, approval_wire()["payload"]);
    let mut extra = wire;
    extra["unexpected"] = json!(true);
    assert!(serde_json::from_value::<RespondRequest>(extra).is_err());
    let mut approval = approval_wire();
    approval["unexpected"] = json!(true);
    assert!(serde_json::from_value::<RemoteApproval>(approval).is_err());
}

#[test]
fn status_enum_representation_and_terminal_classification_are_unchanged() {
    for (status, name, terminal) in [
        (TaskStatus::Starting, "starting", false),
        (TaskStatus::Running, "running", false),
        (TaskStatus::AwaitingApproval, "awaiting_approval", false),
        (TaskStatus::Stopping, "stopping", false),
        (TaskStatus::Completed, "completed", true),
        (TaskStatus::Failed, "failed", true),
        (TaskStatus::Cancelled, "cancelled", true),
        (TaskStatus::Interrupted, "interrupted", true),
    ] {
        assert_eq!(roundtrip::<TaskStatus>(json!(name)), status);
        assert_eq!(status.is_terminal(), terminal);
    }
}

#[test]
fn task_page_schema_preserves_correlation_cursor_and_receipt_default() {
    let wire = json!({"approval_receipts":[],"task":snapshot_wire(),
        "events":[{"sequence":7,"event":{"type":"native","turn_id":"turn:01"}}],
        "next_cursor":7,"has_more":true});
    let read: TaskRead = roundtrip(wire.clone());
    assert_eq!(read.task.turn_id.as_deref(), Some("turn:01"));
    assert_eq!(read.events[0].sequence, read.next_cursor);
    let mut legacy = wire;
    legacy.as_object_mut().unwrap().remove("approval_receipts");
    assert!(serde_json::from_value::<TaskRead>(legacy).unwrap().approval_receipts.is_empty());
}

#[test]
fn cancel_receipt_retains_untagged_task_and_absence_fence() {
    let absent = json!({"id":"opaque/task:01","absent_and_fenced":true});
    assert!(matches!(roundtrip::<CancelReceipt>(absent.clone()), CancelReceipt::Absent(_)));
    let task = json!({"approval_receipts":[],"task":snapshot_wire(),"events":[],"next_cursor":0,"has_more":false});
    assert!(matches!(roundtrip::<CancelReceipt>(task), CancelReceipt::Task(_)));
    let mut extra = absent;
    extra["unexpected"] = json!(true);
    assert!(serde_json::from_value::<CancelFence>(extra).is_err());
}

#[test]
fn capabilities_preserve_workspace_schema_and_legacy_optional_defaults() {
    let workspace = json!({"id":"workspace:01","name":"Checkout","path":"/remote/checkout",
        "branch":"feature","project_root":null,"project_uid":null,"project_name":null,"kind":null,
        "repo_remote":null,"default_branch":null,"created_at":"created","updated_at":"updated",
        "owner_id":null,"origin_host_id":"host:01","notes":null});
    let wire = json!({"protocol_version":1,"providers":[{"provider":"claude","capabilities":null,"error":"unavailable"}],
        "workspaces":[workspace.clone()]});
    let caps: TaskCapabilities = roundtrip(wire);
    assert_eq!(caps.workspaces[0].id, "workspace:01");
    let mut legacy = workspace.clone();
    for field in ["project_uid", "project_name", "kind", "repo_remote", "default_branch"] {
        legacy.as_object_mut().unwrap().remove(field);
    }
    let decoded: Workspace = serde_json::from_value(legacy).unwrap();
    assert_eq!(serde_json::to_value(decoded).unwrap(), workspace);
}

#[test]
fn approval_receipt_preserves_decision_variants_and_missing_identity() {
    for decision in [
        json!({"decision":"provider_option","option_id":"opaque/option:01"}),
        json!({"decision":"allow","updated_input":null,"updated_permissions":null}),
        json!({"decision":"allow_for_session"}),
        json!({"decision":"deny","message":"exact denial"}),
        json!({"decision":"cancel"}),
    ] {
        let wire = json!({"original_request":approval_wire(),"request_id":"opaque/request:01",
            "decision":decision,"state":"accepted"});
        let _: ApprovalReceipt = roundtrip(wire.clone());
        let mut legacy = wire;
        legacy.as_object_mut().unwrap().remove("original_request");
        assert!(serde_json::from_value::<ApprovalReceipt>(legacy).unwrap().original_request.is_none());
    }
    let legacy: RespondRequest = serde_json::from_value(json!({"request_id":"opaque/request:01",
        "decision":{"decision":"cancel"}})).unwrap();
    assert!(legacy.original_request.is_none(), "missing legacy proof must not be reconstructed");
}

#[cfg(unix)]
#[test]
fn unix_receiver_reexports_have_the_same_type_identity() {
    fn task(value: LaunchRequest) -> codemux_lib::remote::tasks::LaunchRequest { value }
    fn caps(value: TaskCapabilities) -> codemux_lib::remote::tasks::TaskCapabilities { value }
    fn workspace(value: Workspace) -> codemux_lib::remote::workspace::Workspace { value }
    let _: fn(LaunchRequest) -> LaunchRequest = task;
    let _: fn(TaskCapabilities) -> TaskCapabilities = caps;
    let _: fn(Workspace) -> Workspace = workspace;
}
