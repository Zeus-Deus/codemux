use super::journal::Journal;
use rusqlite::OptionalExtension;
#[test]
fn validation_journal_metadata_bounds_preserve_existing_decisions_and_grant_revocation() {
    let dir = tempfile::TempDir::new().unwrap();
    let store = Journal::open(dir.path()).unwrap();
    let (parent, mut grant, input) = fixture();
    store.put_grant(&parent.workspace_id, &grant).unwrap();
    let task = store.admit(&parent, &grant, "checkout", &input).unwrap();
    let caps = crate::remote::tasks::TaskCapabilities { protocol_version: 1, providers: vec![], workspaces: vec![] };
    for id in 0..128 { store.cache_host(id, &caps).unwrap(); }
    let host_refused = store.cache_host(128, &caps).is_err();
    store.cache_host(0, &caps).unwrap();
    let response = crate::remote::tasks::RespondRequest { original_request:Some(crate::remote::tasks::RemoteApproval {request_id:"existing".into(),request_kind:"tool".into(),payload:serde_json::Value::Null}), request_id: "existing".into(), decision: crate::agent_provider::ApprovalDecision::Deny { message: "fixture".into() } };
    store.update(&task.id,|t|{let mut remote=remote(&task,TaskStatus::AwaitingApproval).task;remote.pending_requests=vec![response.original_request.clone().unwrap()];t.remote=Some(remote);}).unwrap();
    assert!(store.claim_response(&task.id, &response).unwrap());
    {
        let conn = store.conn.lock().unwrap();
        conn.execute("WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<255) INSERT INTO response_outcomes(task,request,decision,state) SELECT ?1,'request-'||x,?2,'accepted' FROM n", rusqlite::params![task.id, serde_json::to_string(&response.decision).unwrap()]).unwrap();
        conn.execute_batch("WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<127) INSERT INTO grants(id,scope,data) SELECT 'grant-'||x,'retained','{}' FROM n").unwrap();
    }
    let mut fresh = response.clone(); fresh.request_id = "overflow".into();
    fresh.original_request.as_mut().unwrap().request_id = fresh.request_id.clone();
    store.update(&task.id, |t| t.remote.as_mut().unwrap().pending_requests.push(fresh.original_request.clone().unwrap())).unwrap();
    assert_eq!(fresh.original_request.as_ref().unwrap().request_id, fresh.request_id);
    assert!(store.task(&task.id).unwrap().remote.unwrap().pending_requests.contains(fresh.original_request.as_ref().unwrap()));
    let rows = || {
        let conn = store.conn.lock().unwrap();
        let mut statement = conn.prepare("SELECT request,decision,state,original_request FROM response_outcomes WHERE task=?1 ORDER BY request").unwrap();
        statement.query_map([&task.id], |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,Option<String>>(3)?))).unwrap().map(Result::unwrap).collect::<Vec<_>>()
    };
    let before = rows();
    assert_eq!(before.len(), 256);
    let refusal = store.claim_response(&task.id, &fresh).unwrap_err();
    assert_eq!(refusal, "Delegation approval retention/admission capacity reached; prior decisions remain immutable", "fixture must actually reach durable response quota, not identity rejection");
    assert_eq!(rows(), before, "quota refusal changed durable decision/outcome rows");
    assert!(!store.claim_response(&task.id, &response).unwrap(), "capacity caused an uncertain decision to replay");
    assert_eq!(rows(), before, "exact uncertain retry changed immutable durable decisions");
    grant.enabled = false; store.put_grant(&parent.workspace_id, &grant).unwrap();
    let mut extra = grant.clone(); extra.id = "overflow-grant".into();
    assert!(store.put_grant(&parent.workspace_id, &extra).is_err());
    assert!(host_refused, "actual host metadata owner grew without bound");
}

#[test]
fn validation_journal_retention_backpressure_keeps_exact_retry_and_cached_history() {
    let dir = tempfile::TempDir::new().unwrap();
    let store = Journal::open(dir.path()).unwrap();
    let (parent, grant, input) = fixture();
    store.put_grant(&parent.workspace_id, &grant).unwrap();
    let task = store.admit(&parent, &grant, "checkout", &input).unwrap();
    store.apply_remote(&task.id, &remote(&task, TaskStatus::Running)).unwrap();
    {
        let conn = store.conn.lock().unwrap();
        conn.execute_batch("WITH RECURSIVE n(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<2047) INSERT INTO identities(parent,nonce,input,id) SELECT 'retained', 'nonce-'||x, 'original', 'retained-'||x FROM n;").unwrap();
    }
    let mut fresh = input.clone(); fresh.client_request_id = "over-capacity".into();
    let refusal = store.admit(&parent, &grant, "checkout", &fresh);
    assert!(refusal.is_err(), "actual durable admission grew beyond retained identity bound");
    assert_eq!(store.admit(&parent, &grant, "checkout", &input).unwrap().id, task.id, "quota broke exact retry identity");
    assert_eq!(store.read(&task.id, 0).unwrap().events.len(), 1, "backpressure silently deleted history");
}
#[test]
fn validation_journal_event_budget_refuses_page_without_advancing_or_losing_cache() {
    let dir = tempfile::TempDir::new().unwrap();
    let store = Journal::open(dir.path()).unwrap();
    let (parent, grant, input) = fixture();
    store.put_grant(&parent.workspace_id, &grant).unwrap();
    let task = store.admit(&parent, &grant, "checkout", &input).unwrap();
    let page = remote(&task, TaskStatus::Running);
    store.apply_remote(&task.id, &page).unwrap();
    {
        let conn = store.conn.lock().unwrap();
        conn.execute("WITH RECURSIVE n(x) AS (SELECT 2 UNION ALL SELECT x+1 FROM n WHERE x<10000) INSERT INTO events(task,sequence,data) SELECT ?1,x,'{}' FROM n", [&task.id]).unwrap();
        conn.execute("UPDATE tasks SET cursor=10000 WHERE id=?1", [&task.id]).unwrap();
    }
    let mut next = page.clone(); next.events[0].sequence = 10001; next.next_cursor = 10001;
    assert!(store.apply_remote_after(&task.id, &next, 10000).is_err(), "event retention grew without bound");
    assert_eq!(store.follow_intent(&task.id).unwrap().1, 10000);
    assert!(store.read(&task.id, 0).unwrap().has_more);
    assert_eq!(store.read(&task.id, 9999).unwrap().events.len(), 1);
}

#[test]
fn validation_remote_page_integrity_rejects_gaps_cursor_jumps_and_nonprogress() {
    let mut accepted = Vec::new();
    for fault in ["gap", "cursor-jump", "empty-more", "foreign-event", "child-rebind", "stale", "stale-snapshot", "request-id"] {
        let dir = tempfile::TempDir::new().unwrap();
        let store = Journal::open(dir.path()).unwrap();
        let (parent, grant, input) = fixture();
        store.put_grant(&parent.workspace_id, &grant).unwrap();
        let task = store.admit(&parent, &grant, "checkout", &input).unwrap();
        let mut page = remote(&task, TaskStatus::Running);
        if matches!(fault, "child-rebind" | "stale" | "stale-snapshot") { store.apply_remote(&task.id, &page).unwrap(); }
        match fault {
            "gap" => { page.events[0].sequence = 2; page.next_cursor = 2; },
            "cursor-jump" => page.next_cursor = 3,
            "empty-more" => { page.events.clear(); page.next_cursor = 0; page.has_more = true; },
            "foreign-event" => page.events[0].event = serde_json::json!({"thread_id":"foreign-child"}),
            "child-rebind" => page.task.child_thread_id = "replacement-child".into(),
            "stale" => { page.events.clear(); page.next_cursor = 0; },
            "stale-snapshot" => page.task.updated_at = "2000-01-01T00:00:00Z".into(),
            "request-id" => page.task.id = "foreign-task".into(),
            _ => unreachable!(),
        }
        let before = store.read(&task.id, 0).unwrap();
        if store.apply_remote(&task.id, &page).is_ok() { accepted.push(fault); }
        else {
            let after = store.read(&task.id, 0).unwrap();
            assert_eq!(serde_json::to_value(before).unwrap(), serde_json::to_value(after).unwrap(), "rejected page changed durable state: {fault}");
        }
    }
    assert!(accepted.is_empty(), "malformed/stale pages committed by actual journal: {accepted:?}");
}

#[test]
fn ngi3_modern_no_attempt_is_durable_before_effects_and_survives_recovery() {
    for presentation in ["cancel-before-remote", "cancel-no-result", "held", "suppressed"] {
        let dir = tempfile::TempDir::new().unwrap();
        let journal = Journal::open(dir.path()).unwrap();
        let (parent, grant, input) = fixture();
        journal.put_grant(&parent.workspace_id, &grant).unwrap();
        let task = journal.admit(&parent, &grant, "checkout", &input).unwrap();
        let admitted_state: Option<String> = journal.conn.lock().unwrap().query_row(
            "SELECT state FROM delivery_outcomes WHERE task=?1", [&task.id], |r| r.get(0),
        ).optional().unwrap();
        assert_eq!(admitted_state.as_deref(), Some("not_attempted"), "admission lacks durable no-attempt evidence before any effect");
        if presentation != "cancel-before-remote" {
            let mut page = remote(&task, if presentation == "cancel-no-result" { TaskStatus::Running } else { TaskStatus::Completed });
            if presentation == "cancel-no-result" { page.task.result = None; }
            journal.apply_remote(&task.id, &page).unwrap();
        }
        match presentation {
            "cancel-before-remote" | "cancel-no-result" => { journal.cancel(&task.id).unwrap(); },
            "held" => { journal.update(&task.id, |t| t.wake_state = WakeState::Held).unwrap(); },
            "suppressed" => journal.suppress_parent(&parent.thread_id, false).unwrap(),
            _ => unreachable!(),
        }
        drop(journal);
        let recovered = Journal::open(dir.path()).unwrap();
        recovered.recover_deliveries().unwrap();
        recovered.recover_deliveries().unwrap();
        assert_eq!(recovered.delivery_outcome(&task.id).unwrap(), DeliveryOutcome::NotAttempted, "modern {presentation} became historical uncertainty");
        assert_eq!(recovered.admit(&parent, &grant, "checkout", &input).unwrap().id, task.id);
        if presentation.starts_with("cancel") {
            assert!(recovered.explicit_delivery(&task.id).is_err());
            assert!(recovered.claim_delivery(&task.id).unwrap().is_none());
            assert_eq!(recovered.delivery_outcome(&task.id).unwrap(), DeliveryOutcome::NotAttempted);
        } else {
            recovered.explicit_delivery(&task.id).unwrap();
            let attempt = recovered.claim_delivery(&task.id).unwrap().unwrap();
            assert_eq!(recovered.delivery_outcome(&task.id).unwrap(), DeliveryOutcome::InFlight);
            assert_eq!(recovered.delivery_attempt(&task.id).unwrap(), attempt);
            assert!(recovered.claim_delivery(&task.id).unwrap().is_none());
        }
    }
}

#[test]
fn final_dispatch_legacy_held_delivery_is_unknown_after_recovery() {
    let dir = tempfile::TempDir::new().unwrap();
    let journal = Journal::open(dir.path()).unwrap();
    let (parent, grant, input) = fixture();
    journal.put_grant(&parent.workspace_id, &grant).unwrap();
    let task = journal.admit(&parent, &grant, "checkout", &input).unwrap();
    journal
        .apply_remote(&task.id, &remote(&task, TaskStatus::Completed))
        .unwrap();
    journal
        .update(&task.id, |t| t.wake_state = WakeState::Held)
        .unwrap();
    // Reproduce genuinely old storage, not a freshly admitted modern marker.
    journal.conn.lock().unwrap().execute("DELETE FROM delivery_outcomes WHERE task=?1", [&task.id]).unwrap();
    drop(journal);
    let recovered = Journal::open(dir.path()).unwrap();
    recovered.recover_deliveries().unwrap();
    assert!(
        recovered.explicit_delivery(&task.id).is_err(),
        "legacy held result without byte/ack evidence became replayable"
    );
}

#[test]
fn ngi3_legacy_missing_outcomes_remain_unknown_without_replay_or_backfill() {
    for wake in [WakeState::Held, WakeState::Suppressed, WakeState::Delivering] {
        let dir = tempfile::TempDir::new().unwrap();
        let journal = Journal::open(dir.path()).unwrap();
        let (parent, grant, input) = fixture();
        journal.put_grant(&parent.workspace_id, &grant).unwrap();
        let task = journal.admit(&parent, &grant, "checkout", &input).unwrap();
        journal.conn.lock().unwrap().execute("DELETE FROM delivery_outcomes WHERE task=?1", [&task.id]).unwrap();
        journal.update(&task.id, |t| { t.wake_state = wake; t.remote = None; t.cancel_requested = wake == WakeState::Suppressed; }).unwrap();
        // An exact legacy identity retry must not fabricate modern provenance.
        assert_eq!(journal.admit(&parent, &grant, "checkout", &input).unwrap().id, task.id);
        assert!(!journal.conn.lock().unwrap().query_row("SELECT EXISTS(SELECT 1 FROM delivery_outcomes WHERE task=?1)", [&task.id], |r| r.get::<_, bool>(0)).unwrap());
        drop(journal);
        let recovered = Journal::open(dir.path()).unwrap();
        recovered.recover_deliveries().unwrap();
        assert_eq!(recovered.delivery_outcome(&task.id).unwrap(), DeliveryOutcome::Unknown);
        let attempt: String = recovered.conn.lock().unwrap().query_row("SELECT attempt FROM delivery_outcomes WHERE task=?1", [&task.id], |r| r.get(0)).unwrap();
        recovered.recover_deliveries().unwrap();
        assert_eq!(recovered.conn.lock().unwrap().query_row("SELECT attempt FROM delivery_outcomes WHERE task=?1", [&task.id], |r| r.get::<_, String>(0)).unwrap(), attempt);
        assert!(recovered.explicit_delivery(&task.id).is_err());
        recovered.update(&task.id, |t| { t.cancel_requested = false; t.wake_state = WakeState::Pending; }).unwrap();
        assert!(recovered.claim_delivery(&task.id).is_err(), "legacy missing outcome became replayable from presentation alone");
    }
}

#[test]
fn ngi3_reopen_preserves_byte_ack_outcomes_and_only_before_send_retry() {
    for state in ["accepted", "unknown", "before_send", "in_flight"] {
        for cancel in [false, true] {
            let dir = tempfile::TempDir::new().unwrap();
            let journal = Journal::open(dir.path()).unwrap();
            let (parent, grant, input) = fixture();
            journal.put_grant(&parent.workspace_id, &grant).unwrap();
            let task = journal.admit(&parent, &grant, "checkout", &input).unwrap();
            journal.apply_remote(&task.id, &remote(&task, TaskStatus::Completed)).unwrap();
            let attempt = journal.claim_delivery(&task.id).unwrap().unwrap();
            if matches!(state, "accepted" | "unknown") {
                let mut write = || std::task::Poll::Ready(Ok(1));
                assert!(journal.poll_wake(&task.id, &attempt, |_, _| Ok(()), &mut write).unwrap().is_ready());
                assert_eq!(journal.delivery_outcome(&task.id).unwrap(), DeliveryOutcome::Unknown);
                if state == "accepted" { journal.accept_delivery(&task.id, &attempt).unwrap(); }
            }
            if state != "in_flight" { journal.finish_delivery(&task.id, &attempt).unwrap(); }
            journal.update(&task.id, |t| { t.wake_state = WakeState::Held; if state != "before_send" { t.remote = None; } }).unwrap();
            if cancel { journal.cancel(&task.id).unwrap(); }
            drop(journal);
            let recovered = Journal::open(dir.path()).unwrap();
            recovered.recover_deliveries().unwrap();
            recovered.recover_deliveries().unwrap();
            let expected = match state { "accepted" => DeliveryOutcome::Accepted, "before_send" => DeliveryOutcome::BeforeSend, _ => DeliveryOutcome::Unknown };
            assert_eq!(recovered.delivery_outcome(&task.id).unwrap(), expected, "{state}/{cancel}: cancellation/no result changed byte/ACK evidence");
            assert_eq!(recovered.admit(&parent, &grant, "checkout", &input).unwrap().id, task.id);
            assert_eq!(recovered.delivery_outcome(&task.id).unwrap(), expected, "exact identity retry rewrote provenance");
            if state == "before_send" && !cancel {
                recovered.explicit_delivery(&task.id).unwrap();
                let retry = recovered.claim_delivery(&task.id).unwrap().unwrap();
                assert_ne!(retry, attempt);
                assert_eq!(recovered.follow_intent(&task.id).unwrap().0.prompt, task.prompt);
                assert_eq!(recovered.delivery_attempt(&task.id).unwrap(), retry);
            } else {
                assert!(recovered.explicit_delivery(&task.id).is_err());
                recovered.update(&task.id, |t| t.wake_state = WakeState::Pending).unwrap();
                let claim = recovered.claim_delivery(&task.id);
                assert!(if cancel { claim.unwrap().is_none() } else { claim.is_err() }, "{state}/{cancel}: uncertain or cancelled delivery was replayed");
                assert_eq!(recovered.delivery_outcome(&task.id).unwrap(), expected);
            }
        }
    }
}

#[tokio::test]
async fn l3_real_before_send_failure_allows_only_identical_durable_decision_retry() {
    use super::transport::{Operation, SshTransport, TaskTransport, TransportError};
    let dir = tempfile::TempDir::new().unwrap();
    let journal = Journal::open(dir.path()).unwrap();
    let (parent, grant, input) = fixture();
    journal.put_grant(&parent.workspace_id, &grant).unwrap();
    let task = journal.admit(&parent, &grant, "checkout", &input).unwrap();
    let decision = crate::remote::tasks::RespondRequest {
        original_request:Some(crate::remote::tasks::RemoteApproval {request_id:"approval".into(),request_kind:"tool".into(),payload:serde_json::Value::Null}),
        request_id: "approval".into(),
        decision: crate::agent_provider::ApprovalDecision::Deny {
            message: "No".into(),
        },
    };
    journal.update(&task.id,|t|{let mut remote=remote(&task,TaskStatus::AwaitingApproval).task;remote.pending_requests=vec![decision.original_request.clone().unwrap()];t.remote=Some(remote);}).unwrap();
    assert!(journal.claim_response(&task.id, &decision).unwrap());
    let outcome=SshTransport.call_outcome("-invalid-target",Operation::Respond{id:task.id.clone()},Some(serde_json::json!({"request_id":"approval","decision":{"kind":"deny","message":"No"}}))).await;
    assert!(matches!(outcome, Err(TransportError::BeforeSend(_))));
    journal
        .finish_response(&task.id, "approval", &outcome)
        .unwrap();
    let reopened = Journal::open(dir.path()).unwrap();
    assert!(
        reopened.claim_response(&task.id, &decision).unwrap(),
        "definite before-send failure permanently consumed the approval"
    );
    let conflicting = crate::remote::tasks::RespondRequest {
        original_request:decision.original_request.clone(),
        request_id: "approval".into(),
        decision: crate::agent_provider::ApprovalDecision::Allow {
            updated_input: None,
            updated_permissions: None,
        },
    };
    assert!(reopened.claim_response(&task.id, &conflicting).is_err());
    reopened
        .finish_response(
            &task.id,
            "approval",
            &Err(TransportError::Unknown("accepted, reply lost".into())),
        )
        .unwrap();
    assert!(
        !reopened.claim_response(&task.id, &decision).unwrap(),
        "unknown callback outcome must reconcile, not replay"
    );
}

#[test]
fn durable_admission_rechecks_grant_revocation_under_the_journal_write_fence() {
    let dir = tempfile::TempDir::new().unwrap();
    let store = Journal::open(dir.path()).unwrap();
    let (parent, original, input) = fixture();
    store.put_grant(&parent.workspace_id, &original).unwrap();
    let mut revoked = original.clone();
    revoked.enabled = false;
    store.put_grant(&parent.workspace_id, &revoked).unwrap();
    let result = store.admit(&parent, &original, "checkout", &input);
    assert!(
        result.is_err(),
        "journal admitted the stale enabled grant after durable revocation: {result:?}"
    );
    assert!(store.tasks().unwrap().is_empty());
}

#[tokio::test]
async fn conflicting_pane_and_rebound_parent_cannot_borrow_original_grant() {
    use tauri::Manager;
    let app = tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    let state = crate::state::AppStateStore::default();
    let a = state.snapshot().active_workspace_id.0;
    let pane_a = state
        .create_agent_chat_pane(
            &a,
            Some(ProviderKind::Codex),
            None,
            None,
            Some("scope-parent".into()),
        )
        .unwrap();
    let b = state
        .create_workspace_at_path(std::path::PathBuf::from("/synthetic/other-workspace"))
        .0;
    let pane_b = state
        .create_agent_chat_pane(
            &b,
            Some(ProviderKind::Codex),
            None,
            None,
            Some("scope-parent".into()),
        )
        .unwrap();
    let db = crate::database::DatabaseStore::new_in_memory();
    db.upsert_agent_chat_session("scope-parent", &a, Some("/synthetic/parent"), "codex")
        .unwrap();
    db.update_agent_chat_session_config(
        "scope-parent",
        &crate::database::AgentChatSessionConfig {
            permission_mode: crate::database::AgentChatSessionConfig::set("workspace-write"),
            ..Default::default()
        },
    )
    .unwrap();
    let host = db.insert_host("Fixture", "fixture-host").unwrap();
    let dir = tempfile::TempDir::new().unwrap();
    let store = std::sync::Arc::new(Journal::open(dir.path()).unwrap());
    let (mut parent, mut grant, input) = fixture();
    parent.thread_id = "scope-parent".into();
    parent.workspace_id = a.clone();
    grant.host_id = host.id;
    store.put_grant(&a, &grant).unwrap();
    let task = store.admit(&parent, &grant, "checkout", &input).unwrap();
    store
        .apply_remote(&task.id, &remote(&task, TaskStatus::Completed))
        .unwrap();
    store
        .update(&task.id, |t| t.wake_state = WakeState::Delivering)
        .unwrap();
    app.manage(state);
    app.manage(db);
    app.manage(store.clone());
    assert!(super::app::parent_for_pane(app.handle(), &pane_a.0).is_ok());
    let foreign = super::app::invoke(
        app.handle(),
        "delegation_grants",
        serde_json::json!({"paneId":pane_b.0}),
    )
    .await;
    assert!(
        foreign.is_err(),
        "conflicting pane borrowed another workspace's canonical parent and grant: {foreign:?}"
    );
    app.state::<crate::database::DatabaseStore>()
        .upsert_agent_chat_session(
            "scope-parent",
            &b,
            Some("/synthetic/other-workspace"),
            "codex",
        )
        .unwrap();
    let guard = super::app::guard_wake(
        app.handle(),
        "scope-parent",
        Some(&format!("delegation:{}", task.id)),
    );
    assert!(
        guard.is_err(),
        "rebound parent accepted original workspace's result: {guard:?}"
    );
    store
        .update(&task.id, |t| t.wake_state = WakeState::Pending)
        .unwrap();
    super::coordinator::wake(app.handle(), &task.id)
        .await
        .unwrap();
    assert_eq!(store.task(&task.id).unwrap().wake_state, WakeState::Held);
}

#[test]
fn terminal_remote_pages_are_followed_until_the_real_tail_is_cached() {
    let dir = tempfile::TempDir::new().unwrap();
    let store = Journal::open(dir.path()).unwrap();
    let (parent, grant, input) = fixture();
    store.put_grant(&parent.workspace_id, &grant).unwrap();
    let task = store.admit(&parent, &grant, "checkout", &input).unwrap();
    let mut first = remote(&task, TaskStatus::Completed);
    first.has_more = true;
    store.apply_remote(&task.id, &first).unwrap();
    assert!(
        store.tail_pending(&task.id).unwrap(),
        "terminal state cannot drop genuine remote tail events"
    );
    let mut tail = first.clone();
    tail.has_more = false;
    tail.events[0].sequence = 2;
    tail.next_cursor = 2;
    store.apply_remote_after(&task.id, &tail, 1).unwrap();
    assert!(!store.tail_pending(&task.id).unwrap());
    assert_eq!(store.read(&task.id, 0).unwrap().events.len(), 2);
}
#[test]
fn stop_is_durable_and_unconfirmed_until_native_acknowledgement() {
    let dir = tempfile::TempDir::new().unwrap();
    let store = Journal::open(dir.path()).unwrap();
    let (parent, grant, input) = fixture();
    store.put_grant(&parent.workspace_id, &grant).unwrap();
    let task = store.admit(&parent, &grant, "checkout", &input).unwrap();
    store.cancel(&task.id).unwrap();
    let running = remote(&task, TaskStatus::Running);
    store.apply_remote(&task.id, &running).unwrap();
    let current = store.task(&task.id).unwrap();
    assert_eq!(
        current.status,
        TaskStatus::Stopping,
        "a read of a still-running remote is not stop acknowledgement"
    );
    assert_eq!(current.wake_state, WakeState::Suppressed);
    drop(store);
    let store = Journal::open(dir.path()).unwrap();
    assert!(store.task(&task.id).unwrap().cancel_requested);
    let mut ack = running;
    ack.task.status = TaskStatus::Cancelled;
    ack.task.cancel_requested = true;
    store.apply_remote(&task.id, &ack).unwrap();
    assert_eq!(store.task(&task.id).unwrap().status, TaskStatus::Cancelled);
}

#[test]
fn ssh_command_is_constant_except_validated_uuid_and_decimal_cursor() {
    use super::transport::*;
    assert_eq!(
        remote_command(&Operation::Capabilities).unwrap(),
        "codemux-remote task capabilities"
    );
    let id = uuid::Uuid::new_v4().to_string();
    assert_eq!(
        remote_command(&Operation::Read {
            id: id.clone(),
            cursor: 12
        })
        .unwrap(),
        format!("codemux-remote task read --id {id} --after 12 --wait-ms 15000")
    );
    assert!(remote_command(&Operation::Cancel {
        id: "x;touch /fixture/leak".into()
    })
    .is_err());
    assert!(remote_command(&Operation::Read { id, cursor: -1 }).is_err());
}
#[tokio::test]
async fn native_commands_are_dispatched_against_real_pane_and_db_ownership() {
    use tauri::Manager;
    let app = tauri::test::mock_builder()
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    let state = crate::state::AppStateStore::default();
    let ws = state.snapshot().active_workspace_id.0;
    let pane = state
        .create_agent_chat_pane(
            &ws,
            Some(ProviderKind::Codex),
            None,
            None,
            Some("parent".into()),
        )
        .unwrap();
    let db = crate::database::DatabaseStore::new_in_memory();
    db.upsert_agent_chat_session("parent", &ws, Some("/fixture"), "codex")
        .unwrap();
    app.manage(state);
    app.manage(db);
    let dir = tempfile::TempDir::new().unwrap();
    app.manage(std::sync::Arc::new(Journal::open(dir.path()).unwrap()));
    let result = super::app::invoke(
        app.handle(),
        "delegation_list",
        serde_json::json!({"paneId":pane.0}),
    )
    .await;
    assert_eq!(
        result.unwrap(),
        serde_json::json!([]),
        "native UI commands must use canonical pane ownership"
    );
    assert!(super::app::invoke(
        app.handle(),
        "delegation_list",
        serde_json::json!({"paneId":"missing","parent_thread_id":"parent"})
    )
    .await
    .is_err());
}

use super::*;
use crate::agent_provider::ProviderKind;
use crate::remote::tasks::{TaskEvent, TaskRead as RemoteRead, TaskSnapshot, TaskStatus};
pub(super) fn fixture() -> (Parent, Grant, DelegateTaskInput) {
    (
        Parent {
            thread_id: "parent".into(),
            provider: ProviderKind::Codex,
            workspace_id: "workspace".into(),
            label: "Parent".into(),
            permission_mode: "workspace-write".into(),
            event_id: Some(7),
        },
        Grant {
            id: "grant".into(),
            host_id: 4,
            host_name: "Fixture host".into(),
            ssh_target: "fixture-host".into(),
            workspace_path: "/fixture/checkout".into(),
            workspace_name: "Checkout".into(),
            provider: ProviderKind::Codex,
            permission_mode: "workspace-write".into(),
            enabled: true,
            created_at: "fixture".into(),
        },
        DelegateTaskInput {
            target_id: "grant".into(),
            prompt: "Self-contained task".into(),
            title: None,
            model: None,
            effort: None,
            client_request_id: "request".into(),
        },
    )
}
pub(super) fn remote(task: &LocalTask, status: TaskStatus) -> RemoteRead {
    RemoteRead {
        approval_receipts: vec![],
        task: TaskSnapshot {
            id: task.id.clone(),
            request: task.request(None),
            child_thread_id: "specific-child".into(),
            provider_session_id: Some("native-session".into()),
            turn_id: Some("specific-turn".into()),
            status,
            activity: None,
            result: status.is_terminal().then(|| "specific final output".into()),
            error: None,
            created_at: task.created_at.clone(),
            updated_at: task.updated_at.clone(),
            cancel_requested: false,
            pending_requests: vec![],
        },
        events: vec![TaskEvent {
            sequence: 1,
            event: serde_json::json!({"kind":"fixture-native-event"}),
        }],
        next_cursor: 1,
        has_more: false,
    }
}
#[test]
fn journal_caches_real_result_and_cursor_and_refuses_terminal_mutation() {
    let dir = tempfile::TempDir::new().unwrap();
    let store = Journal::open(dir.path()).unwrap();
    let (parent, grant, input) = fixture();
    store.put_grant(&parent.workspace_id, &grant).unwrap();
    let task = store
        .admit(&parent, &grant, "registered-workspace", &input)
        .unwrap();
    let read = remote(&task, TaskStatus::Completed);
    store.apply_remote(&task.id, &read).unwrap();
    let cached = store.read(&task.id, 0).unwrap();
    assert_eq!(
        cached.task.status,
        TaskStatus::Completed,
        "SSH result must be persisted"
    );
    assert_eq!(cached.events.len(), 1);
    assert_eq!(cached.next_cursor, 1);
    assert_eq!(store.follow_intent(&task.id).unwrap().1, 1);
    assert!(store.follow_intent(&task.id).unwrap().2);
    let mut changed = read.clone();
    changed.task.result = Some("replacement output".into());
    assert!(store.apply_remote(&task.id, &changed).is_err());
    assert_eq!(
        store.task(&task.id).unwrap().remote.unwrap().result,
        read.task.result
    );
}
#[test]
fn restart_delivery_ambiguity_is_held_instead_of_resent() {
    let dir = tempfile::TempDir::new().unwrap();
    let store = Journal::open(dir.path()).unwrap();
    let (parent, grant, input) = fixture();
    store.put_grant(&parent.workspace_id, &grant).unwrap();
    let task = store.admit(&parent, &grant, "checkout", &input).unwrap();
    store
        .update(&task.id, |t| t.wake_state = WakeState::Delivering)
        .unwrap();
    drop(store);
    let store = Journal::open(dir.path()).unwrap();
    store.recover_deliveries().unwrap();
    assert_eq!(store.task(&task.id).unwrap().wake_state, WakeState::Held);
}
#[test]
fn ceilings_allow_known_native_modes_but_never_unknown_or_escalation() {
    use ProviderKind::*;
    assert!(permissions::permits(
        Codex,
        "workspace-write",
        Claude,
        "default"
    ));
    assert!(permissions::permits(Claude, "plan", Codex, "read-only"));
    assert!(!permissions::permits(Codex, "read-only", Claude, "default"));
    assert!(!permissions::permits(
        Codex,
        "workspace-write",
        Codex,
        "danger-full-access"
    ));
    assert!(!permissions::permits(
        Claude,
        "dontAsk",
        Codex,
        "workspace-write"
    ));
    assert!(!permissions::permits(Hermes, "unknown", Codex, "read-only"));
    assert!(!permissions::permits(
        Codex,
        "danger-full-access",
        Hermes,
        "yolo"
    ));
}

#[test]
fn durable_identity_survives_reopen_and_conflicting_retry_is_rejected() {
    let dir = tempfile::TempDir::new().unwrap();
    let journal = Journal::open(dir.path()).unwrap();
    let admitted = journal.reserve("parent", "client-request", "self-contained prompt");
    assert!(
        admitted.is_ok(),
        "must reserve durable identity before SSH: {admitted:?}"
    );
    let id = admitted.unwrap();
    drop(journal);
    let reopened = Journal::open(dir.path()).unwrap();
    assert_eq!(
        reopened
            .reserve("parent", "client-request", "self-contained prompt")
            .unwrap(),
        id
    );
    assert!(reopened
        .reserve("parent", "client-request", "different prompt")
        .is_err());
    assert_ne!(
        reopened
            .reserve(
                "different-parent",
                "client-request",
                "self-contained prompt"
            )
            .unwrap(),
        id
    );
}
