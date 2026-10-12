use super::*;
use async_trait::async_trait;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio::sync::watch;

fn task(id: &str) -> TaskSpec {
    TaskSpec {
        id: id.into(),
        title: id.into(),
        prompt: "Token-free fixture".into(),
        dependencies: vec![],
        route_id: None,
        access: TaskAccess::ReadOnly,
        scope: vec![],
        output_schema: None,
        required: true,
    }
}
fn spec(tasks: Vec<TaskSpec>) -> RunSpec {
    RunSpec {
        workspace_id: "fixture".into(),
        title: "Fixture workflow".into(),
        goal: "Test scheduling without any real model".into(),
        mode: RunMode::DryRun,
        allow_writes: false,
        routes: vec![RouteSpec {
            id: "fixture-a".into(),
            provider: "fake".into(),
            model: None,
            effort: None,
        }],
        limits: WorkflowLimits::default(),
        tasks,
        script: None,
    }
}
fn fixture_service(cap: usize) -> (tempfile::TempDir, WorkflowService) {
    let dir = tempfile::tempdir().unwrap();
    let service = WorkflowService::open(dir.path().join("workflow.sqlite"), cap).unwrap();
    (dir, service)
}
fn finish(service: &WorkflowService, dispatch: &Dispatch) {
    service
        .finish_attempt(dispatch, ExecutionReport::success(json!({"ok":true})))
        .unwrap();
}
async fn await_status(service: &WorkflowService, id: &str, status: RunStatus) -> RunSnapshot {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let run = service.snapshot(id).unwrap();
            if run.status == status {
                return run;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("workflow did not reach expected state")
}

#[test]
fn workflow_journal_budget_bounds_results_errors_and_pending_intents() {
    let (_directory, service) = fixture_service(1);
    let run = service
        .create(spec(vec![task("a")]), "journal-budget")
        .unwrap();
    let payload = json!({"request":1});
    let result = json!({"answer":42});
    service
        .journal_begin(&run.id, "last-result", &payload)
        .unwrap();
    service
        .with_connection(|connection| {
            connection
                .execute(
                    "UPDATE workflow_journal_counters SET retained_bytes=?2 WHERE run_id=?1",
                    params![run.id, 32 * 1024 * 1024 - result.to_string().len()],
                )
                .map_err(|e| e.to_string())?;
            Ok(())
        })
        .unwrap();
    assert_eq!(
        service
            .journal_commit(&run.id, "last-result", &payload, &result)
            .unwrap(),
        result
    );
    assert_eq!(
        service
            .journal_commit(&run.id, "last-result", &payload, &result)
            .unwrap(),
        result
    );
    let calls = AtomicUsize::new(0);
    assert!(service
        .read_or_execute(&run.id, "excess-intent", &payload, || {
            calls.fetch_add(1, Ordering::SeqCst);
            Ok(Value::Null)
        })
        .unwrap_err()
        .contains("journal byte budget"));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(service
        .journal_get(&run.id, "excess-intent", &payload)
        .unwrap()
        .is_none());

    let pending = service
        .create(spec(vec![task("a")]), "journal-pending")
        .unwrap();
    service
        .journal_begin(&pending.id, "pending", &payload)
        .unwrap();
    service
        .with_connection(|connection| {
            connection
                .execute(
                    "UPDATE workflow_journal_counters SET retained_bytes=?2 WHERE run_id=?1",
                    params![pending.id, 32 * 1024 * 1024],
                )
                .map_err(|e| e.to_string())?;
            Ok(())
        })
        .unwrap();
    assert!(service
        .journal_commit(&pending.id, "pending", &payload, &result)
        .unwrap_err()
        .contains("journal byte budget"));
    assert!(service
        .journal_get(&pending.id, "pending", &payload)
        .unwrap()
        .is_none());
    assert!(service
        .read_or_execute(&pending.id, "pending", &payload, || panic!(
            "Uncertain intent must not execute again"
        ))
        .unwrap_err()
        .contains("unknown"));

    let errors = service
        .create(spec(vec![task("a")]), "journal-errors")
        .unwrap();
    let error = service
        .read_or_execute(&errors.id, "error", &payload, || Err("🦀".repeat(3000)))
        .unwrap_err();
    assert_eq!(error.len(), 8192);
    assert_eq!(
        service
            .read_or_execute(&errors.id, "error", &payload, || panic!(
                "Recorded error must replay"
            ))
            .unwrap_err(),
        error
    );
    service.with_connection(|connection| {
        let counter:u64=connection.query_row("SELECT retained_bytes FROM workflow_journal_counters WHERE run_id=?1",[&errors.id],|row|row.get(0)).map_err(|e|e.to_string())?;
        let actual:u64=connection.query_row("SELECT SUM(length(CAST(command_id AS BLOB))+length(CAST(payload AS BLOB))+COALESCE(length(CAST(result AS BLOB)),0)+COALESCE(length(CAST(error AS BLOB)),0)) FROM workflow_journal WHERE run_id=?1",[&errors.id],|row|row.get(0)).map_err(|e|e.to_string())?;
        assert_eq!(counter,actual);
        Ok(())
    }).unwrap();
}

#[test]
fn workflow_retained_operation_budget_rolls_back_graph_and_preserves_replay() {
    let (_directory, service) = fixture_service(1);
    let run = service.create(spec(vec![task("a")]), "operations").unwrap();
    let mut replacement = task("a");
    replacement.prompt = "A revised task".into();
    let key = "last-permitted";
    let encoded = json!({"task_id":"a","spec":replacement}).to_string();
    let bytes = encoded.len() + key.len();
    service
        .with_connection(|connection| {
            connection
                .execute(
                    "UPDATE workflow_operation_counters SET retained_bytes=?2 WHERE run_id=?1",
                    params![run.id, 32 * 1024 * 1024 - bytes],
                )
                .map_err(|e| e.to_string())?;
            Ok(())
        })
        .unwrap();
    let accepted = service
        .replace(&run.id, "a", replacement.clone(), key)
        .unwrap();
    assert!(service
        .replace(&run.id, "a", task("a"), "over-budget")
        .unwrap_err()
        .contains("operation byte budget"));
    let unchanged = service.snapshot(&run.id).unwrap();
    assert_eq!(unchanged.revision, accepted.revision);
    assert_eq!(unchanged.tasks[0].generation, accepted.tasks[0].generation);
    assert_eq!(unchanged.tasks[0].spec, replacement);
    assert_eq!(
        service
            .replace(&run.id, "a", replacement, key)
            .unwrap()
            .revision,
        accepted.revision
    );
    service.with_connection(|connection| {
        connection.execute("UPDATE workflow_operation_counters SET retained_bytes=0,operation_count=10000 WHERE run_id=?1",[&run.id]).map_err(|e|e.to_string())?;
        Ok(())
    }).unwrap();
    assert!(service
        .add_tasks(&run.id, vec![task("b")], None, "excess-count")
        .unwrap_err()
        .contains("operation count"));
    assert_eq!(
        service.snapshot(&run.id).unwrap().revision,
        accepted.revision
    );
    assert_eq!(service.snapshot(&run.id).unwrap().tasks.len(), 1);
}

#[test]
fn workflow_operation_counter_migration_preserves_old_idempotency_records() {
    let (directory, service) = fixture_service(1);
    let run = service
        .create(spec(vec![task("a")]), "legacy-operations")
        .unwrap();
    service
        .add_tasks(&run.id, vec![task("b")], None, "legacy-add")
        .unwrap();
    let payload = json!({"text":"é🦀"});
    let result = json!({"saved":"é🦀"});
    service
        .read_or_execute(&run.id, "legacy-journal", &payload, || Ok(result.clone()))
        .unwrap();
    service
        .with_connection(|connection| {
            connection
                .execute_batch(
                    "DROP TABLE workflow_operation_counters; DROP TABLE workflow_journal_counters",
                )
                .map_err(|e| e.to_string())
        })
        .unwrap();
    drop(service);
    let migrated = WorkflowService::open(directory.path().join("workflow.sqlite"), 1).unwrap();
    migrated.with_connection(|connection| {
        let counters:(u64,u64)=connection.query_row("SELECT retained_bytes,operation_count FROM workflow_operation_counters WHERE run_id=?1",[&run.id],|row|Ok((row.get(0)?,row.get(1)?))).map_err(|e|e.to_string())?;
        let actual:(u64,u64)=connection.query_row("SELECT SUM(length(CAST(payload AS BLOB))+length(CAST(key AS BLOB))),COUNT(*) FROM workflow_operations WHERE run_id=?1",[&run.id],|row|Ok((row.get(0)?,row.get(1)?))).map_err(|e|e.to_string())?;
        assert_eq!(counters,actual);
        assert_eq!(counters.1,2);
        let journal:(u64,u64)=connection.query_row("SELECT retained_bytes,entry_count FROM workflow_journal_counters WHERE run_id=?1",[&run.id],|row|Ok((row.get(0)?,row.get(1)?))).map_err(|e|e.to_string())?;
        assert_eq!(journal,("legacy-journal".len() as u64+payload.to_string().len() as u64+result.to_string().len() as u64,1));
        Ok(())
    }).unwrap();
    assert_eq!(
        migrated
            .add_tasks(&run.id, vec![task("b")], None, "legacy-add")
            .unwrap()
            .tasks
            .len(),
        2
    );
    assert_eq!(
        migrated
            .read_or_execute(&run.id, "legacy-journal", &payload, || panic!(
                "Completed legacy journal must replay"
            ))
            .unwrap(),
        result
    );
}

#[test]
fn workflow_create_and_dynamic_graph_are_atomic_and_idempotent() {
    let (_dir, service) = fixture_service(4);
    let run = service.create(spec(vec![task("a")]), "create").unwrap();
    assert_eq!(
        service.create(spec(vec![task("a")]), "create").unwrap().id,
        run.id
    );
    assert!(service.create(spec(vec![task("b")]), "create").is_err());
    let mut b = task("b");
    b.dependencies = vec!["missing".into()];
    assert!(service
        .add_tasks(&run.id, vec![b], None, "invalid")
        .is_err());
    assert_eq!(service.snapshot(&run.id).unwrap().tasks.len(), 1);
    let added = service
        .add_tasks(&run.id, vec![task("b")], Some("a"), "add")
        .unwrap();
    assert_eq!(added.tasks[1].depth, 1);
    assert_eq!(
        service
            .add_tasks(&run.id, vec![task("b")], Some("a"), "add")
            .unwrap()
            .tasks
            .len(),
        2
    );
    let mut a = task("a");
    a.dependencies = vec!["b".into()];
    let mut b = task("b");
    b.dependencies = vec!["a".into()];
    assert!(service
        .create(spec(vec![a, b]), "cycle")
        .unwrap_err()
        .contains("cycle"));
}

#[test]
fn workflow_recovery_pauses_pending_script_before_any_new_admission() {
    let (directory, service) = fixture_service(1);
    let mut input = spec(vec![task("not-started")]);
    input.script = Some(ScriptSpec {
        source: "return await agent('Inspect', {id:'dynamic'});".into(),
        args: json!({"pinned":true}),
        api_version: 1,
    });
    let run = service.create(input, "pending-script").unwrap();
    assert_eq!(run.script.as_ref().unwrap().status, ScriptStatus::Pending);
    drop(service);
    let recovered = WorkflowService::open(directory.path().join("workflow.sqlite"), 1).unwrap();
    let paused = recovered.snapshot(&run.id).unwrap();
    assert_eq!(paused.status, RunStatus::Paused);
    assert_eq!(paused.script.as_ref().unwrap().status, ScriptStatus::Paused);
    assert_eq!(paused.spec.script, run.spec.script);
    assert!(paused.tasks.iter().all(|task| task.attempts.is_empty()));
    assert!(recovered.claim_next().unwrap().is_none());
    let resumed = recovered.resume(&run.id).unwrap();
    assert_eq!(resumed.script.unwrap().status, ScriptStatus::Pending);
    assert!(recovered.claim_next().unwrap().is_some());
}

#[test]
fn workflow_preparation_is_atomic_and_replay_does_not_resume_paused_runs() {
    let (_dir, service) = fixture_service(1);
    let input = spec(vec![task("a")]);
    let (run, fresh) = service.create_paused(input.clone(), "prepare").unwrap();
    assert!(fresh);
    assert_eq!(run.status, RunStatus::Paused);
    assert!(service.claim_next().unwrap().is_none());
    let (same, fresh) = service.create_paused(input.clone(), "prepare").unwrap();
    assert!(!fresh);
    assert_eq!(same.id, run.id);
    assert_eq!(same.status, RunStatus::Paused);
    service.resume(&run.id).unwrap();
    let dispatch = service.claim_next().unwrap().unwrap();
    finish(&service, &dispatch);
    let (completed, fresh) = service.create_paused(input, "prepare").unwrap();
    assert!(!fresh);
    assert_eq!(completed.status, RunStatus::Completed);
}

#[test]
fn workflow_admission_enforces_global_run_route_and_write_scopes() {
    let (_dir, service) = fixture_service(2);
    service.set_route_capacity("fixture-a", 1).unwrap();
    let a = service
        .create(spec(vec![task("a"), task("b")]), "a")
        .unwrap();
    let first = service.claim_next().unwrap().unwrap();
    assert_eq!(first.run_id, a.id);
    assert!(service.claim_next().unwrap().is_none());
    let mut second = spec(vec![task("c")]);
    second.routes[0].id = "fixture-b".into();
    service.create(second, "b").unwrap();
    let other = service.claim_next().unwrap().unwrap();
    assert!(service.claim_next().unwrap().is_none());
    finish(&service, &first);
    finish(&service, &other);
    assert_eq!(service.claim_next().unwrap().unwrap().task_id, "b");
    let (_dir, service) = fixture_service(4);
    let mut writer = task("writer");
    writer.access = TaskAccess::Write;
    writer.scope = vec!["src".into()];
    let mut reader = task("reader");
    reader.scope = vec!["src/main.rs".into()];
    let mut unrelated = task("unrelated");
    unrelated.scope = vec!["docs".into()];
    let mut input = spec(vec![writer, reader, unrelated]);
    input.allow_writes = true;
    service.create(input, "scopes").unwrap();
    let writer = service.claim_next().unwrap().unwrap();
    let unrelated = service.claim_next().unwrap().unwrap();
    assert_eq!(unrelated.task_id, "unrelated");
    assert!(service.claim_next().unwrap().is_none());
    finish(&service, &writer);
    assert_eq!(service.claim_next().unwrap().unwrap().task_id, "reader");
}

#[test]
fn workflow_provider_quota_cannot_be_bypassed_by_minting_route_ids() {
    let (_dir, service) = fixture_service(4);
    service.set_provider_capacity("fake", 1).unwrap();
    let a = service.create(spec(vec![task("a")]), "provider-a").unwrap();
    let mut input = spec(vec![task("b")]);
    input.routes[0].id = "different-id-same-account".into();
    let b = service.create(input, "provider-b").unwrap();
    let dispatch = service.claim_next().unwrap().unwrap();
    assert!(dispatch.run_id == a.id || dispatch.run_id == b.id);
    assert!(service.claim_next().unwrap().is_none());
    finish(&service, &dispatch);
    assert!(service.claim_next().unwrap().is_some());
}

#[test]
fn workflow_active_index_excludes_completed_history_and_history_is_bounded() {
    let (_dir, service) = fixture_service(1);
    for n in 0..5 {
        let mut input = spec(vec![]);
        input.workspace_id = format!("workspace-{}", n % 2);
        service.create(input, &format!("history-{n}")).unwrap();
    }
    assert!(service
        .with_connection(|c| store::list_active(c))
        .unwrap()
        .is_empty());
    assert_eq!(
        service.list_recent(Some("workspace-0"), 2).unwrap().len(),
        2
    );
    assert_eq!(
        service.list_recent(Some("workspace-1"), 10).unwrap().len(),
        2
    );
    assert!(service.list_recent(None, 1001).is_err());
}

#[test]
fn workflow_history_summaries_do_not_load_or_return_task_payloads() {
    let (_dir, service) = fixture_service(1);
    let run = service.create(spec(vec![task("a")]), "summaries").unwrap();
    let dispatch = service.claim_next().unwrap().unwrap();
    finish(&service, &dispatch);
    let rows = service.list_summaries("fixture", 10).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].id, run.id);
    assert_eq!(rows[0].status, RunStatus::Completed);
    assert_eq!(rows[0].spec.title, "Fixture workflow");
    let value = serde_json::to_value(&rows[0]).unwrap();
    assert!(value.get("tasks").is_none());
    assert!(value["spec"].get("goal").is_none());
    service
        .with_connection(|c| {
            c.execute(
                "UPDATE workflow_runs SET snapshot='not valid JSON' WHERE id=?1",
                [&run.id],
            )
            .map(|_| ())
            .map_err(|e| e.to_string())
        })
        .unwrap();
    assert_eq!(service.list_summaries("fixture", 10).unwrap().len(), 1);
    assert!(service.snapshot(&run.id).is_err());
    assert!(service.list_summaries("elsewhere", 10).unwrap().is_empty());
}

#[test]
fn workflow_message_budget_is_shared_by_root_and_scoped_authority() {
    let (_dir, service) = fixture_service(1);
    let run = service
        .create(
            spec((0..5).map(|n| task(&format!("task-{n}"))).collect()),
            "messages",
        )
        .unwrap();
    let dispatch = service.claim_next().unwrap().unwrap();
    service
        .with_connection(|connection| {
            let mut snapshot = store::load(connection, &run.id)?;
            for task in &mut snapshot.tasks[1..] {
                task.messages = vec!["x".repeat(8192); 128];
            }
            snapshot.tasks[4].messages[127].pop();
            store::save(connection, &snapshot, "fixture_messages")
        })
        .unwrap();
    let accepted = service.message(&run.id, "task-0", "x").unwrap();
    assert!(service
        .message(&run.id, "task-0", "x")
        .unwrap_err()
        .contains("byte budget"));
    assert!(service
        .message_as(&dispatch, "task-0", "x")
        .unwrap_err()
        .contains("byte budget"));
    let unchanged = service.snapshot(&run.id).unwrap();
    assert_eq!(unchanged.revision, accepted.revision);
    assert_eq!(unchanged.tasks[0].messages, vec!["x"]);
}

#[test]
fn workflow_scoped_mutations_fence_cancelled_workers_and_permission_escalation() {
    let (_dir, service) = fixture_service(2);
    let run = service
        .create(spec(vec![task("parent"), task("sibling")]), "create")
        .unwrap();
    let dispatch = service.claim_next().unwrap().unwrap();
    let mut write = task("write");
    write.access = TaskAccess::Write;
    write.scope = vec!["src".into()];
    assert!(service
        .add_tasks_as(&dispatch, vec![write], "write")
        .is_err());
    service
        .add_tasks_as(&dispatch, vec![task("child")], "child")
        .unwrap();
    assert!(service
        .message_as(&dispatch, "sibling", "unauthorized")
        .is_err());
    assert!(service.retire_as(&dispatch, "sibling").is_err());
    service.message_as(&dispatch, "child", "guidance").unwrap();
    service.cancel(&run.id).unwrap();
    assert!(service
        .add_tasks_as(&dispatch, vec![task("late")], "late")
        .is_err());
    assert!(service.authorize_attempt(&dispatch).is_err());
    assert_eq!(
        service.snapshot(&run.id).unwrap().status,
        RunStatus::Stopping
    );
    service
        .finish_attempt(&dispatch, ExecutionReport::success(json!("late success")))
        .unwrap();
    assert_eq!(
        service.snapshot(&run.id).unwrap().tasks[0].status,
        TaskStatus::Cancelled
    );
}

#[test]
fn workflow_delegation_inherits_scope_and_refuses_widening_or_excess_depth() {
    let (_dir, service) = fixture_service(1);
    let mut parent = task("parent");
    parent.scope = vec!["src".into()];
    let mut input = spec(vec![parent]);
    input.limits.max_depth = 1;
    let run = service.create(input, "scope-depth").unwrap();
    let dispatch = service.claim_next().unwrap().unwrap();
    let added = service
        .add_tasks_as(&dispatch, vec![task("child")], "child")
        .unwrap();
    assert_eq!(added.tasks[1].spec.scope, vec!["src"]);
    let mut wide = task("wide");
    wide.scope = vec!["docs".into()];
    assert!(service.add_tasks_as(&dispatch, vec![wide], "wide").is_err());
    assert!(service
        .add_tasks(&run.id, vec![task("grandchild")], Some("child"), "deep")
        .is_err());
}

#[test]
fn workflow_pause_stops_admission_while_admitted_attempts_can_finish() {
    let (_dir, service) = fixture_service(1);
    let run = service
        .create(spec(vec![task("a"), task("b")]), "pause")
        .unwrap();
    let first = service.claim_next().unwrap().unwrap();
    service.pause(&run.id).unwrap();
    assert!(service.authorize_attempt(&first).is_ok());
    assert!(service.claim_next().unwrap().is_none());
    finish(&service, &first);
    assert_eq!(service.snapshot(&run.id).unwrap().status, RunStatus::Paused);
    service.resume(&run.id).unwrap();
    assert_eq!(service.claim_next().unwrap().unwrap().task_id, "b");
}

#[test]
fn workflow_resumed_parent_can_replace_failed_child_and_yield_again() {
    let (_dir, service) = fixture_service(1);
    let run = service
        .create(spec(vec![task("parent")]), "repair")
        .unwrap();
    let parent = service.claim_next().unwrap().unwrap();
    service
        .add_tasks_as(&parent, vec![task("child")], "spawn")
        .unwrap();
    service
        .finish_attempt(&parent, ExecutionReport::waiting(vec!["child".into()]))
        .unwrap();
    let child = service.claim_next().unwrap().unwrap();
    service
        .finish_attempt(&child, ExecutionReport::failed("fixture child failure"))
        .unwrap();
    let parent = service.claim_next().unwrap().unwrap();
    assert_eq!(parent.task_id, "parent");
    assert_eq!(parent.dependency_results[0].1["status"], "failed");
    service.replace_as(&parent, "child", task("child")).unwrap();
    service
        .finish_attempt(&parent, ExecutionReport::waiting(vec!["child".into()]))
        .unwrap();
    let child = service.claim_next().unwrap().unwrap();
    assert_eq!(child.generation, 2);
    finish(&service, &child);
    let parent = service.claim_next().unwrap().unwrap();
    finish(&service, &parent);
    assert_eq!(
        service.snapshot(&run.id).unwrap().status,
        RunStatus::Completed
    );
}

#[test]
fn workflow_artifact_acceptance_lock_fences_concurrent_replacement() {
    let (_dir, service) = fixture_service(1);
    let run = service
        .create(spec(vec![task("a")]), "artifact-lease")
        .unwrap();
    let dispatch = service.claim_next().unwrap().unwrap();
    finish(&service, &dispatch);
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let release = Arc::new(std::sync::Barrier::new(2));
    let application = std::thread::spawn({
        let service = service.clone();
        let dispatch = dispatch.clone();
        let release = release.clone();
        move || {
            service.with_accepted_attempt(
                &dispatch.run_id,
                &dispatch.task_id,
                &dispatch.attempt_id,
                dispatch.generation,
                |task, _| {
                    entered_tx.send(()).unwrap();
                    release.wait();
                    Ok(task.generation)
                },
            )
        }
    });
    entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let replacement = std::thread::spawn({
        let service = service.clone();
        let id = run.id.clone();
        move || {
            let value = service.replace(&id, "a", task("a"), "new-generation");
            done_tx.send(value).unwrap();
        }
    });
    assert!(done_rx.try_recv().is_err());
    release.wait();
    assert_eq!(application.join().unwrap().unwrap(), 1);
    let replaced = done_rx
        .recv_timeout(Duration::from_secs(2))
        .unwrap()
        .unwrap();
    replacement.join().unwrap();
    assert_eq!(replaced.tasks[0].generation, 2);
    assert!(service
        .with_accepted_attempt(
            &dispatch.run_id,
            &dispatch.task_id,
            &dispatch.attempt_id,
            1,
            |_, _| Ok(())
        )
        .is_err());
}

#[test]
fn workflow_retired_success_preserves_evidence_but_revokes_application() {
    let (_dir, service) = fixture_service(1);
    let run = service
        .create(spec(vec![task("a")]), "retire-evidence")
        .unwrap();
    let dispatch = service.claim_next().unwrap().unwrap();
    finish(&service, &dispatch);
    let retired = service.retire(&run.id, "a").unwrap();
    assert!(retired.tasks[0].retired);
    assert_eq!(retired.tasks[0].status, TaskStatus::Succeeded);
    assert!(retired.tasks[0].result.is_some());
    assert!(service
        .with_accepted_attempt(
            &run.id,
            "a",
            &dispatch.attempt_id,
            dispatch.generation,
            |_, _| -> Result<(), String> { panic!("retired artifact must never apply") }
        )
        .is_err());
}

#[test]
fn workflow_cancel_keeps_required_outcome_while_retire_removes_future_work() {
    let (_dir, service) = fixture_service(1);
    let cancelled = service
        .create(spec(vec![task("required")]), "cancel-required")
        .unwrap();
    let cancelled = service.cancel_task(&cancelled.id, "required").unwrap();
    assert_eq!(cancelled.status, RunStatus::Failed);
    assert!(!cancelled.tasks[0].retired);
    service.retry(&cancelled.id, "required").unwrap();
    let dispatch = service.claim_next().unwrap().unwrap();
    finish(&service, &dispatch);
    let retired = service
        .create(spec(vec![task("obsolete")]), "retire-obsolete")
        .unwrap();
    let retired = service.retire(&retired.id, "obsolete").unwrap();
    assert_eq!(retired.status, RunStatus::Completed);
    assert!(retired.tasks[0].retired);
    assert_eq!(retired.tasks[0].status, TaskStatus::Cancelled);
    assert!(retired.tasks[0].result.is_none());
    let mut dependent = task("dependent");
    dependent.dependencies = vec!["withdrawn".into()];
    let run = service
        .create(
            spec(vec![task("withdrawn"), dependent]),
            "required-dependency",
        )
        .unwrap();
    let run = service.retire(&run.id, "withdrawn").unwrap();
    assert_eq!(run.status, RunStatus::Failed);
    assert_eq!(run.tasks[1].status, TaskStatus::Blocked);
}

#[test]
fn workflow_scope_and_schema_validation_fail_closed() {
    let (_dir, service) = fixture_service(4);
    for scope in [
        "../secret",
        "/etc",
        ".git/config",
        "src/../../secret",
        "C:/secret",
        "src\\secret",
        "src//x",
        "src/./x",
    ] {
        let mut t = task("a");
        t.scope = vec![scope.into()];
        assert!(service.create(spec(vec![t]), scope).is_err());
    }
    for schema in [
        json!({"$ref":"https://example.com/schema"}),
        json!({"$ref":"#"}),
        json!({"type":"nonsense"}),
    ] {
        let mut t = task("a");
        t.output_schema = Some(schema);
        assert!(service.create(spec(vec![t]), "bad-schema").is_err());
    }
    let mut t = task("a");
    t.access = TaskAccess::Write;
    t.scope = vec!["src".into()];
    assert!(service.create(spec(vec![t]), "unauthorized-write").is_err());
}

#[test]
fn workflow_recovery_never_retries_unknown_and_preserves_admission_and_usage() {
    let (dir, service) = fixture_service(1);
    let mut input = spec(vec![task("a"), task("b")]);
    input.mode = RunMode::Live;
    input.limits.token_budget = Some(5000);
    let run = service.create(input, "create").unwrap();
    let dispatch = service.claim_next().unwrap().unwrap();
    service.record_external_ref(&run.id, &dispatch.attempt_id, json!({"phase":"starting","provider":"fake"})).unwrap();
    assert_eq!(
        service.snapshot(&run.id).unwrap().usage.reserved_tokens,
        4096
    );
    drop(service);
    let recovered = WorkflowService::open(dir.path().join("workflow.sqlite"), 1).unwrap();
    let snapshot = recovered.snapshot(&run.id).unwrap();
    assert_eq!(snapshot.status, RunStatus::Unknown);
    assert_eq!(
        snapshot.tasks[0].current_attempt.as_ref().unwrap().id,
        dispatch.attempt_id
    );
    assert!(recovered.claim_next().unwrap().is_none());
    assert!(recovered.retry(&run.id, "a").is_err());
    assert!(recovered
        .finish_attempt(&dispatch, ExecutionReport::success(json!(true)))
        .is_err());
    let mut report = ExecutionReport::success(json!(true));
    report.usage.total_tokens = 37;
    recovered.reconcile_attempt(&dispatch, report).unwrap();
    assert_eq!(
        recovered.snapshot(&run.id).unwrap().usage.reserved_tokens,
        0
    );
    assert_eq!(recovered.snapshot(&run.id).unwrap().usage.total_tokens, 37);
    assert_eq!(recovered.claim_next().unwrap().unwrap().task_id, "b");
}

#[test]
fn workflow_retired_success_blocks_queued_consumer_before_admission() {
    let (_directory, service) = fixture_service(1);
    let prerequisite = task("prerequisite");
    let mut consumer = task("consumer");
    consumer.dependencies = vec![prerequisite.id.clone()];
    let mut input = spec(vec![prerequisite, consumer]);
    input.mode = RunMode::Live;
    let run = service.create(input, "retired-input").unwrap();
    let first = service.claim_next().unwrap().unwrap();
    finish(&service, &first);
    service.retire(&run.id, "prerequisite").unwrap();
    assert!(service.claim_next().unwrap().is_none());
    let run = service.snapshot(&run.id).unwrap();
    let consumer = run.tasks.iter().find(|task| task.spec.id == "consumer").unwrap();
    assert_eq!(consumer.status, TaskStatus::Blocked);
    assert!(consumer.attempts.is_empty());
    assert_eq!(run.usage.reserved_tokens, 0);
    assert_eq!(run.tasks.iter().map(|task| task.attempts.len()).sum::<usize>(), 1);
}

#[test]
fn workflow_recovery_fenced_prelaunch_releases_holds_and_requires_explicit_retry() {
    for stage in ["dispatching", "running", "stopping"] {
        let (directory, service) = fixture_service(1);
        let mut input = spec(vec![task("a"), task("b")]);
        input.mode = RunMode::Live;
        let run = service.create(input, stage).unwrap();
        let old = service.claim_next().unwrap().unwrap();
        if stage != "dispatching" { service.mark_running(&old).unwrap(); }
        if stage == "stopping" { service.cancel_task(&run.id, "a").unwrap(); }
        assert_eq!(service.snapshot(&run.id).unwrap().usage.reserved_tokens, 4096);
        // No driver or provider was started. Reopen only after the previous
        // service has been dropped, mirroring the exclusive owner handoff.
        drop(service);
        let recovered = WorkflowService::open(directory.path().join("workflow.sqlite"), 1).unwrap();
        let snapshot = recovered.snapshot(&run.id).unwrap();
        assert!(snapshot.pause_requested);
        assert_eq!(snapshot.status, RunStatus::Paused);
        assert_eq!(snapshot.usage.reserved_tokens, 0);
        let attempt = snapshot.tasks[0].current_attempt.as_ref().unwrap();
        assert_eq!(attempt.status, if stage == "stopping" { AttemptStatus::Cancelled } else { AttemptStatus::Failed });
        assert!(attempt.finished_at_ms.is_some());
        assert!(attempt.external_ref.is_none() && attempt.output.is_none() && attempt.artifacts.is_empty());
        assert_eq!(attempt.usage.cost_usd, Some(0.0));
        assert_eq!(attempt.usage.total_tokens, 0);
        assert!(!attempt.usage.tokens_unknown && !attempt.usage.cost_unknown);
        assert!(recovered.authorize_attempt(&old).is_err());
        assert!(recovered.finish_attempt(&old, ExecutionReport::success(json!({"late":true}))).is_err());
        assert!(recovered.claim_next().unwrap().is_none());
        let other = recovered.create(spec(vec![task("other")]), "capacity-released").unwrap();
        let available = recovered.claim_next().unwrap().unwrap();
        assert_eq!(available.run_id, other.id);
        finish(&recovered, &available);
        recovered.retry(&run.id, "a").unwrap();
        assert!(recovered.claim_next().unwrap().is_none());
        recovered.resume(&run.id).unwrap();
        let fresh = recovered.claim_next().unwrap().unwrap();
        assert_eq!(fresh.task_id, "a");
        assert_ne!(fresh.attempt_id, old.attempt_id);
        assert!(recovered.authorize_attempt(&old).is_err());
        assert!(recovered.finish_attempt(&old, ExecutionReport::success(json!({"late":true}))).is_err());
    }
}

#[test]
fn workflow_recovery_never_infers_zero_for_legacy_or_existing_unknown_attempts() {
    for stage in ["legacy", "unknown"] {
        let (directory, service) = fixture_service(1);
        let mut input = spec(vec![task("a")]);
        input.mode = RunMode::Live;
        let run = service.create(input, stage).unwrap();
        let dispatch = service.claim_next().unwrap().unwrap();
        if stage == "legacy" {
            let mut value = serde_json::to_value(service.snapshot(&run.id).unwrap()).unwrap();
            value["tasks"][0]["attempts"][0].as_object_mut().unwrap().remove("external_execution_fenced");
            value["tasks"][0]["current_attempt"].as_object_mut().unwrap().remove("external_execution_fenced");
            let legacy: RunSnapshot = serde_json::from_value(value).unwrap();
            assert!(!legacy.tasks[0].current_attempt.as_ref().unwrap().external_execution_fenced);
            service.with_connection(|connection| store::save(connection, &legacy, "legacy-fixture")).unwrap();
        } else {
            service.finish_attempt(&dispatch, ExecutionReport::unknown("Uncertain driver outcome")).unwrap();
        }
        drop(service);
        let recovered = WorkflowService::open(directory.path().join("workflow.sqlite"), 1).unwrap();
        let snapshot = recovered.snapshot(&run.id).unwrap();
        assert_eq!(snapshot.status, RunStatus::Unknown);
        assert_eq!(snapshot.usage.reserved_tokens, 4096);
        assert_eq!(snapshot.tasks[0].current_attempt.as_ref().unwrap().status, AttemptStatus::Unknown);
        assert!(recovered.retry(&run.id, "a").is_err());
        recovered.create(spec(vec![task("other")]), "capacity-held").unwrap();
        assert!(recovered.claim_next().unwrap().is_none());
    }
}

#[test]
fn workflow_unknown_report_and_cancel_retain_resources_until_proven_quiescent() {
    let (_dir, service) = fixture_service(1);
    let run = service
        .create(spec(vec![task("a"), task("b")]), "create")
        .unwrap();
    let dispatch = service.claim_next().unwrap().unwrap();
    service.cancel(&run.id).unwrap();
    service
        .finish_attempt(
            &dispatch,
            ExecutionReport::unknown("native stop unconfirmed"),
        )
        .unwrap();
    assert_eq!(
        service.snapshot(&run.id).unwrap().status,
        RunStatus::Unknown
    );
    assert!(service.claim_next().unwrap().is_none());
    service
        .reconcile_attempt(&dispatch, ExecutionReport::cancelled())
        .unwrap();
    assert_eq!(
        service.snapshot(&run.id).unwrap().status,
        RunStatus::Cancelled
    );
}

#[test]
fn workflow_invalid_results_do_not_erase_billable_usage_or_unknown_holds() {
    let (_dir, service) = fixture_service(1);
    let mut t = task("a");
    t.output_schema = Some(json!({"type":"integer"}));
    service.create(spec(vec![t]), "create").unwrap();
    let dispatch = service.claim_next().unwrap().unwrap();
    let mut report = ExecutionReport::success(json!("invalid"));
    report.usage.total_tokens = 99;
    let run = service.finish_attempt(&dispatch, report).unwrap();
    assert_eq!(run.status, RunStatus::Failed);
    assert_eq!(run.usage.total_tokens, 99);
    service.retry(&run.id, "a").unwrap();
    let dispatch = service.claim_next().unwrap().unwrap();
    let mut report = ExecutionReport::unknown("may still run");
    report.output = Some(json!("x".repeat(2_000_000)));
    let run = service.finish_attempt(&dispatch, report).unwrap();
    assert_eq!(run.status, RunStatus::Unknown);
    assert!(service.claim_next().unwrap().is_none());
}

#[test]
fn workflow_replace_invalidates_dependents_without_rerunning_siblings() {
    let (_dir, service) = fixture_service(1);
    let mut b = task("b");
    b.dependencies = vec!["a".into()];
    let run = service
        .create(spec(vec![task("a"), b, task("sibling")]), "create")
        .unwrap();
    while let Some(dispatch) = service.claim_next().unwrap() {
        finish(&service, &dispatch);
    }
    assert_eq!(
        service.snapshot(&run.id).unwrap().status,
        RunStatus::Completed
    );
    let mut replacement = task("a");
    replacement.prompt = "new generation".into();
    let replaced = service
        .replace(&run.id, "a", replacement, "replace")
        .unwrap();
    assert_eq!(replaced.tasks[0].generation, 2);
    assert_eq!(replaced.tasks[1].generation, 2);
    assert_eq!(replaced.tasks[2].generation, 1);
    assert_eq!(replaced.tasks[2].status, TaskStatus::Succeeded);
    assert_eq!(replaced.tasks[2].attempts.len(), 1);
    assert_eq!(service.claim_next().unwrap().unwrap().task_id, "a");
}

#[test]
fn workflow_replacing_awaited_child_revokes_completed_coordinator_result() {
    let (dir, service) = fixture_service(1);
    let run = service
        .create(spec(vec![task("parent"), task("sibling")]), "awaited-input")
        .unwrap();
    let parent = service.claim_next().unwrap().unwrap();
    service
        .add_tasks_as(&parent, vec![task("child")], "spawn")
        .unwrap();
    service
        .finish_attempt(&parent, ExecutionReport::waiting(vec!["child".into()]))
        .unwrap();
    while let Some(dispatch) = service.claim_next().unwrap() {
        finish(&service, &dispatch);
    }
    let completed = service.snapshot(&run.id).unwrap();
    let parent = completed
        .tasks
        .iter()
        .find(|t| t.spec.id == "parent")
        .unwrap();
    let accepted = parent.current_attempt.as_ref().unwrap().clone();
    assert_eq!(parent.status, TaskStatus::Succeeded);
    let service = WorkflowService::open(dir.path().join("workflow.sqlite"), 1).unwrap();
    service
        .replace(&run.id, "child", task("child"), "replace-child")
        .unwrap();
    let updated = service.snapshot(&run.id).unwrap();
    let parent = updated
        .tasks
        .iter()
        .find(|t| t.spec.id == "parent")
        .unwrap();
    assert_eq!(parent.status, TaskStatus::Queued);
    assert_eq!(parent.generation, 2);
    assert!(parent.result.is_none());
    assert!(service
        .with_accepted_attempt(&run.id, "parent", &accepted.id, 1, |_, _| Ok(()))
        .is_err());
    let sibling = updated
        .tasks
        .iter()
        .find(|t| t.spec.id == "sibling")
        .unwrap();
    assert_eq!(sibling.status, TaskStatus::Succeeded);
    assert_eq!(sibling.generation, 1);
}

#[tokio::test]
async fn workflow_ready_child_observation_fences_active_and_completed_consumers() {
    let (_dir, service) = fixture_service(2);
    let run = service
        .create(spec(vec![task("parent")]), "ready-input")
        .unwrap();
    let parent = service.claim_next().unwrap().unwrap();
    service.mark_running(&parent).unwrap();
    service
        .add_tasks_as(&parent, vec![task("child")], "spawn")
        .unwrap();
    let child = service.claim_next().unwrap().unwrap();
    finish(&service, &child);
    let tools = tools::graph_tools(parent.clone(), service.clone());
    let observed = tools
        .call("workflow_wait", json!({"task_ids":["child"]}))
        .await
        .unwrap();
    assert_eq!(observed["state"], "ready");
    let revision = service.snapshot(&run.id).unwrap().revision;
    tools
        .call("workflow_wait", json!({"task_ids":["child"]}))
        .await
        .unwrap();
    assert_eq!(service.snapshot(&run.id).unwrap().revision, revision);
    assert!(service
        .replace(&run.id, "child", task("child"), "replace-active-input")
        .is_err());
    finish(&service, &parent);
    service.retire(&run.id, "child").unwrap();
    assert!(service
        .with_accepted_attempt(&run.id, "parent", &parent.attempt_id, 1, |_, _| Ok(()))
        .is_err());
}

#[tokio::test]
async fn workflow_retiring_observed_child_rejects_late_coordinator_success() {
    let (_dir, service) = fixture_service(2);
    let run = service
        .create(spec(vec![task("parent")]), "retired-input")
        .unwrap();
    let parent = service.claim_next().unwrap().unwrap();
    service.mark_running(&parent).unwrap();
    service
        .add_tasks_as(&parent, vec![task("child")], "spawn")
        .unwrap();
    let child = service.claim_next().unwrap().unwrap();
    finish(&service, &child);
    let tools = tools::graph_tools(parent.clone(), service.clone());
    tools
        .call("workflow_wait", json!({"task_ids":["child"]}))
        .await
        .unwrap();
    service.retire(&run.id, "child").unwrap();
    let finished = service
        .finish_attempt(&parent, ExecutionReport::success(json!({"ok":true})))
        .unwrap();
    let parent = finished
        .tasks
        .iter()
        .find(|task| task.spec.id == "parent")
        .unwrap();
    assert_eq!(parent.status, TaskStatus::Failed);
    assert!(parent.result.is_none());
    assert!(parent.error.as_deref().unwrap().contains("revoked"));
}

#[test]
fn workflow_failure_propagates_and_retry_repairs_reverse_ordered_graph() {
    let (_dir, service) = fixture_service(1);
    let mut b = task("b");
    b.dependencies = vec!["a".into()];
    let mut c = task("c");
    c.dependencies = vec!["b".into()];
    let run = service
        .create(spec(vec![c, b, task("a")]), "create")
        .unwrap();
    let a = service.claim_next().unwrap().unwrap();
    let failed = service
        .finish_attempt(&a, ExecutionReport::failed("fixture"))
        .unwrap();
    assert_eq!(failed.status, RunStatus::Failed);
    assert_eq!(failed.tasks[0].status, TaskStatus::Blocked);
    assert_eq!(failed.tasks[1].status, TaskStatus::Blocked);
    service.retry(&run.id, "a").unwrap();
    let a = service.claim_next().unwrap().unwrap();
    finish(&service, &a);
    assert_eq!(service.claim_next().unwrap().unwrap().task_id, "b");
}

#[test]
fn workflow_budget_limits_attempts_and_accounts_inflight_token_overshoot() {
    let (_dir, service) = fixture_service(1);
    let mut input = spec(vec![task("a"), task("b")]);
    input.mode = RunMode::Live;
    input.limits.token_budget = Some(20);
    let run = service.create(input, "budget").unwrap();
    let dispatch = service.claim_next().unwrap().unwrap();
    assert_eq!(service.snapshot(&run.id).unwrap().usage.reserved_tokens, 20);
    let mut report = ExecutionReport::success(json!(true));
    report.usage.total_tokens = 30;
    service.finish_attempt(&dispatch, report).unwrap();
    assert!(service.claim_next().unwrap().is_none());
    let run = service.snapshot(&run.id).unwrap();
    assert_eq!(run.status, RunStatus::Failed);
    assert_eq!(run.usage.total_tokens, 30);
    let mut input = spec(vec![task("c")]);
    input.limits.max_attempts = 1;
    let run = service.create(input, "attempts").unwrap();
    let dispatch = service.claim_next().unwrap().unwrap();
    service
        .finish_attempt(&dispatch, ExecutionReport::failed("fixture"))
        .unwrap();
    service.retry(&run.id, "c").unwrap();
    assert!(service.claim_next().unwrap().is_none());
    assert_eq!(
        service.snapshot(&run.id).unwrap().tasks[0].attempts.len(),
        1
    );
}

#[test]
fn workflow_missing_usage_consumes_an_explicit_conservative_estimate() {
    let (_dir, service) = fixture_service(1);
    let mut input = spec(vec![task("a"), task("b")]);
    input.mode = RunMode::Live;
    input.limits.token_budget = Some(20);
    let run = service.create(input, "missing-usage").unwrap();
    let dispatch = service.claim_next().unwrap().unwrap();
    service
        .finish_attempt(&dispatch, ExecutionReport::cancelled())
        .unwrap();
    let run = service.snapshot(&run.id).unwrap();
    assert_eq!(run.usage.total_tokens, 0);
    assert_eq!(run.usage.estimated_tokens, 20);
    assert!(run.usage.tokens_unknown);
    assert_eq!(run.usage.reserved_tokens, 0);
    assert!(service.claim_next().unwrap().is_none());
}

#[test]
fn workflow_aggregate_output_budget_rejects_evidence_without_losing_quiescence_or_usage() {
    let (_dir, service) = fixture_service(1);
    let mut input = spec(vec![task("a"), task("b")]);
    input.mode = RunMode::Live;
    input.limits.max_output_bytes = 16 * 1024 * 1024;
    input.script = Some(ScriptSpec {
        source: "return {}".into(),
        args: Value::Null,
        api_version: 1,
    });
    let run = service.create(input, "aggregate-outputs").unwrap();
    let first = service.claim_next().unwrap().unwrap();
    let mut report = ExecutionReport::success(json!("x".repeat(8 * 1024 * 1024)));
    report.usage.total_tokens = 100;
    report.usage.tokens_unknown = false;
    service.finish_attempt(&first, report).unwrap();
    let second = service.claim_next().unwrap().unwrap();
    let mut report = ExecutionReport::success(json!("y".repeat(8 * 1024 * 1024)));
    report.usage.total_tokens = 200;
    report.usage.tokens_unknown = false;
    let bounded = service.finish_attempt(&second, report).unwrap();
    assert_eq!(bounded.tasks[1].status, TaskStatus::Failed);
    assert!(bounded.tasks[1]
        .error
        .as_ref()
        .unwrap()
        .contains("retained output budget"));
    assert!(bounded.tasks[1]
        .current_attempt
        .as_ref()
        .unwrap()
        .output
        .is_none());
    assert_eq!(bounded.usage.total_tokens, 300);
    assert_eq!(bounded.usage.reserved_tokens, 0);
    assert!(service
        .set_script_status(
            &run.id,
            ScriptStatus::Completed,
            Some(json!("z".repeat(12 * 1024 * 1024))),
            None
        )
        .is_err());
    let mut independent = spec(vec![task("c")]);
    independent.workspace_id = "another-workspace".into();
    service.create(independent, "after-budget").unwrap();
    assert_eq!(service.claim_next().unwrap().unwrap().task_id, "c");
}

#[test]
fn workflow_failed_script_cannot_be_reported_complete_by_retrying_its_children() {
    let (_dir, service) = fixture_service(1);
    let mut input = spec(vec![task("a")]);
    input.script = Some(ScriptSpec {
        source: "throw Error('fixture')".into(),
        args: Value::Null,
        api_version: 1,
    });
    let run = service.create(input, "script-failure").unwrap();
    service
        .set_script_status(&run.id, ScriptStatus::Failed, None, Some("fixture".into()))
        .unwrap();
    assert!(service.retry(&run.id, "a").is_err());
    assert_eq!(service.snapshot(&run.id).unwrap().status, RunStatus::Failed);
    assert!(service.claim_next().unwrap().is_none());
}

#[test]
fn workflow_reconciliation_preserves_previously_observed_usage() {
    let (_dir, service) = fixture_service(1);
    let mut input = spec(vec![task("a")]);
    input.mode = RunMode::Live;
    let run = service.create(input, "unknown-accounting").unwrap();
    let dispatch = service.claim_next().unwrap().unwrap();
    let mut unknown = ExecutionReport::unknown("process stop unconfirmed");
    unknown.usage.total_tokens = 100;
    unknown.usage.input_tokens = 60;
    unknown.usage.output_tokens = 40;
    unknown.usage.cost_usd = Some(0.01);
    service.finish_attempt(&dispatch, unknown).unwrap();
    let resolved = service
        .reconcile_attempt(&dispatch, ExecutionReport::cancelled())
        .unwrap();
    assert_eq!(resolved.usage.total_tokens, 100);
    assert_eq!(resolved.usage.estimated_tokens, 3996);
    assert_eq!(resolved.usage.cost_usd, Some(0.01));
    assert!(resolved.usage.tokens_unknown);
    assert!(service
        .reconcile_attempt(&dispatch, ExecutionReport::cancelled())
        .is_err());
    assert_eq!(service.snapshot(&run.id).unwrap().usage.total_tokens, 100);
}

#[test]
fn workflow_wall_time_revokes_authority_but_keeps_active_holds() {
    let (_dir, service) = fixture_service(1);
    let run = service.create(spec(vec![task("a")]), "create").unwrap();
    let dispatch = service.claim_next().unwrap().unwrap();
    service.pause(&run.id).unwrap();
    service
        .with_connection(|c| {
            let mut run = store::load(c, &run.id)?;
            run.created_at_ms = now_ms() - 4_000_000;
            store::save(c, &run, "fixture")
        })
        .unwrap();
    assert!(service.claim_next().unwrap().is_none());
    assert!(service.cancellation_requested(&dispatch));
    assert_eq!(
        service.snapshot(&run.id).unwrap().status,
        RunStatus::Stopping
    );
    service
        .finish_attempt(&dispatch, ExecutionReport::cancelled())
        .unwrap();
    assert_eq!(service.snapshot(&run.id).unwrap().status, RunStatus::Failed);
}

#[test]
fn workflow_script_keeps_empty_run_open_and_can_repair_failed_tasks() {
    let (_dir, service) = fixture_service(1);
    let mut input = spec(vec![]);
    input.script = Some(ScriptSpec {
        source: "return 1".into(),
        args: json!({}),
        api_version: 1,
    });
    let run = service.create(input, "script").unwrap();
    assert_eq!(run.status, RunStatus::Running);
    service
        .set_script_status(&run.id, ScriptStatus::Running, None, None)
        .unwrap();
    service
        .add_tasks(&run.id, vec![task("a")], None, "add")
        .unwrap();
    let dispatch = service.claim_next().unwrap().unwrap();
    let failed = service
        .finish_attempt(&dispatch, ExecutionReport::failed("repairable"))
        .unwrap();
    assert_eq!(failed.status, RunStatus::Running);
    service.pause(&run.id).unwrap();
    assert!(service.claim_next().unwrap().is_none());
    assert_eq!(service.resume(&run.id).unwrap().status, RunStatus::Running);
    service
        .set_script_status(&run.id, ScriptStatus::Running, None, None)
        .unwrap();
    service.retry(&run.id, "a").unwrap();
    let dispatch = service.claim_next().unwrap().unwrap();
    finish(&service, &dispatch);
    assert_eq!(
        service.snapshot(&run.id).unwrap().status,
        RunStatus::Running
    );
    assert_eq!(
        service
            .set_script_status(&run.id, ScriptStatus::Completed, Some(json!(1)), None)
            .unwrap()
            .status,
        RunStatus::Completed
    );
}

#[test]
fn workflow_journal_replays_exact_observations_and_atomically_claims_closures() {
    let (dir, service) = fixture_service(1);
    let run = service.create(spec(vec![task("a")]), "create").unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let worker = std::thread::spawn({
        let service = service.clone();
        let calls = calls.clone();
        let barrier = barrier.clone();
        let id = run.id.clone();
        move || {
            service.read_or_execute(&id, "observe", &json!({"a":1}), || {
                calls.fetch_add(1, Ordering::SeqCst);
                barrier.wait();
                std::thread::sleep(Duration::from_millis(30));
                Ok(json!({"revision":1}))
            })
        }
    });
    barrier.wait();
    assert!(service
        .read_or_execute(&run.id, "observe", &json!({"a":1}), || panic!(
            "must never duplicate"
        ))
        .is_err());
    assert_eq!(worker.join().unwrap().unwrap(), json!({"revision":1}));
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(service
        .journal_get(&run.id, "observe", &json!({"a":2}))
        .is_err());
    service
        .journal_begin(&run.id, "unknown", &json!({}))
        .unwrap();
    assert!(service
        .read_or_execute(&run.id, "unknown", &json!({}), || panic!("must not rerun"))
        .is_err());
    drop(service);
    let recovered = WorkflowService::open(dir.path().join("workflow.sqlite"), 1).unwrap();
    assert_eq!(
        recovered
            .read_or_execute(&run.id, "observe", &json!({"a":1}), || panic!(
                "must replay"
            ))
            .unwrap(),
        json!({"revision":1})
    );
}

#[derive(Default)]
struct NestedDriver {
    calls: AtomicUsize,
}
#[async_trait]
impl WorkflowDriver for NestedDriver {
    async fn execute(
        &self,
        dispatch: Dispatch,
        service: WorkflowService,
        _cancel: watch::Receiver<bool>,
    ) -> ExecutionReport {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if dispatch.task_id == "parent" && dispatch.dependency_results.is_empty() {
            service
                .add_tasks_as(&dispatch, vec![task("child")], "spawn-child")
                .unwrap();
            ExecutionReport::waiting(vec!["child".into()])
        } else {
            ExecutionReport::success(json!({"dependencies":dispatch.dependency_results}))
        }
    }
}

#[tokio::test]
async fn workflow_actual_driver_supports_nested_delegation_with_capacity_one() {
    let (_dir, service) = fixture_service(1);
    let run = service
        .create(spec(vec![task("parent")]), "create")
        .unwrap();
    let driver = Arc::new(NestedDriver::default());
    let handle = tokio::spawn({
        let service = service.clone();
        let driver = driver.clone();
        async move { service.run_driver(driver).await }
    });
    let completed = await_status(&service, &run.id, RunStatus::Completed).await;
    assert_eq!(driver.calls.load(Ordering::SeqCst), 3);
    assert_eq!(completed.tasks[0].attempts.len(), 2);
    assert_eq!(completed.tasks[1].depth, 1);
    assert_eq!(completed.usage.total_tokens, 0);
    handle.abort();
    let _ = handle.await;
}

#[derive(Default)]
struct CountingDriver {
    active: AtomicUsize,
    peak: AtomicUsize,
    calls: AtomicUsize,
}

struct OwnedStressDriver(Option<tokio::task::JoinHandle<Result<(), String>>>);

impl OwnedStressDriver {
    async fn stop(mut self) {
        if let Some(handle) = self.0.take() {
            handle.abort();
            let _ = handle.await;
        }
    }
}

impl Drop for OwnedStressDriver {
    fn drop(&mut self) {
        if let Some(handle) = &self.0 {
            handle.abort();
        }
    }
}

struct PanickingDriver;
#[async_trait]
impl WorkflowDriver for PanickingDriver {
    async fn execute(
        &self,
        _dispatch: Dispatch,
        _service: WorkflowService,
        _cancel: watch::Receiver<bool>,
    ) -> ExecutionReport {
        panic!("deliberate fake executor panic")
    }
}

#[tokio::test]
async fn workflow_driver_panic_persists_unknown_without_releasing_capacity() {
    let (_dir, service) = fixture_service(1);
    let run = service
        .create(spec(vec![task("a"), task("queued")]), "panic")
        .unwrap();
    let (stop, receiver) = watch::channel(false);
    let handle = tokio::spawn({
        let service = service.clone();
        async move {
            service
                .run_driver_until(Arc::new(PanickingDriver), receiver)
                .await
        }
    });
    let unknown = await_status(&service, &run.id, RunStatus::Unknown).await;
    assert_eq!(unknown.tasks[0].attempts.len(), 1);
    assert_eq!(unknown.tasks[1].attempts.len(), 0);
    assert!(service.claim_next().unwrap().is_none());
    stop.send(true).unwrap();
    handle.await.unwrap().unwrap();
}
#[async_trait]
impl WorkflowDriver for CountingDriver {
    async fn execute(
        &self,
        dispatch: Dispatch,
        service: WorkflowService,
        cancel: watch::Receiver<bool>,
    ) -> ExecutionReport {
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        struct ActiveCount<'a>(&'a AtomicUsize);
        impl Drop for ActiveCount<'_> {
            fn drop(&mut self) {
                self.0.fetch_sub(1, Ordering::SeqCst);
            }
        }
        let _active = ActiveCount(&self.active);
        self.peak.fetch_max(active, Ordering::SeqCst);
        self.calls.fetch_add(1, Ordering::SeqCst);
        DryRunDriver {
            delay: Duration::from_millis(1),
            fail_tasks: HashSet::new(),
        }
        .execute(dispatch, service, cancel)
        .await
    }
}

#[tokio::test]
async fn workflow_actual_scheduler_stress_hundreds_of_tasks_without_model_tokens() {
    let (_dir, service) = fixture_service(4);
    let mut input = spec((0..256).map(|n| task(&format!("task-{n}"))).collect());
    input.limits.concurrency = 0;
    let run = service.create(input, "stress").unwrap();
    assert_eq!(run.resolved_limits.concurrency, 4);
    let driver = Arc::new(CountingDriver::default());
    let mut events = service.subscribe();
    let owned = OwnedStressDriver(Some(tokio::spawn({
        let service = service.clone();
        let driver = driver.clone();
        async move { service.run_driver(driver).await }
    })));
    // Full Windows CI shares its runner with thousands of other tests and
    // persists three transitions per task. Observe coalesced changes rather
    // than repeatedly decoding this large snapshot on a 5ms polling loop.
    let deadline = Duration::from_secs(if cfg!(windows) { 120 } else { 30 });
    let completed = tokio::time::timeout(deadline, async {
        loop {
            let snapshot = service.snapshot(&run.id).unwrap();
            if snapshot.status == RunStatus::Completed {
                break snapshot;
            }
            match events.recv().await {
                Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(error) => panic!("stress observation stream closed: {error}"),
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
            while events.try_recv().is_ok() {}
        }
    })
    .await
    .expect("256-task scheduler did not complete within its stress-fixture deadline");
    assert_eq!(driver.calls.load(Ordering::SeqCst), 256);
    assert!(driver.peak.load(Ordering::SeqCst) <= 4);
    assert!(driver.peak.load(Ordering::SeqCst) > 1);
    assert!(completed.tasks.iter().all(|task| task.attempts.len() == 1));
    assert_eq!(completed.usage.total_tokens, 0);
    assert!(!completed.usage.tokens_unknown);
    assert_eq!(completed.usage.cost_usd, Some(0.0));
    assert!(!completed.usage.cost_unknown);
    owned.stop().await;
    assert_eq!(driver.active.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn workflow_actual_driver_cancels_and_reports_dry_run_failures() {
    let (_dir, service) = fixture_service(2);
    let run = service
        .create(spec(vec![task("a"), task("b")]), "cancel")
        .unwrap();
    let handle = tokio::spawn({
        let service = service.clone();
        async move {
            service
                .run_driver(Arc::new(DryRunDriver {
                    delay: Duration::from_secs(10),
                    fail_tasks: HashSet::new(),
                }))
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if service
                .snapshot(&run.id)
                .unwrap()
                .tasks
                .iter()
                .any(|t| t.status == TaskStatus::Running)
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    service.cancel(&run.id).unwrap();
    let cancelled = await_status(&service, &run.id, RunStatus::Cancelled).await;
    assert_eq!(cancelled.usage.total_tokens, 0);
    handle.abort();
    let _ = handle.await;
    let run = service
        .create(spec(vec![task("failure")]), "failure")
        .unwrap();
    let handle = tokio::spawn({
        let service = service.clone();
        async move {
            service
                .run_driver(Arc::new(DryRunDriver {
                    delay: Duration::ZERO,
                    fail_tasks: HashSet::from(["failure".into()]),
                }))
                .await
        }
    });
    let failed = await_status(&service, &run.id, RunStatus::Failed).await;
    assert!(failed.tasks[0].error.as_ref().unwrap().contains("no model"));
    handle.abort();
    let _ = handle.await;
}

#[tokio::test]
async fn workflow_driver_shutdown_owns_cancel_until_quiescent_report() {
    let (_dir, service) = fixture_service(1);
    let run = service
        .create(spec(vec![task("a"), task("queued")]), "shutdown")
        .unwrap();
    let (stop, receiver) = watch::channel(false);
    let handle = tokio::spawn({
        let service = service.clone();
        async move {
            service
                .run_driver_until(
                    Arc::new(DryRunDriver {
                        delay: Duration::from_secs(30),
                        fail_tasks: HashSet::new(),
                    }),
                    receiver,
                )
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if service.snapshot(&run.id).unwrap().tasks[0].status == TaskStatus::Running {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    stop.send(true).unwrap();
    tokio::time::timeout(Duration::from_secs(2), handle)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let stopped = service.snapshot(&run.id).unwrap();
    assert_eq!(stopped.tasks[0].status, TaskStatus::Cancelled);
    assert_eq!(stopped.tasks[1].status, TaskStatus::Queued);
    assert_eq!(stopped.status, RunStatus::Paused);
    assert!(service.claim_next().unwrap().is_none());
}

#[tokio::test]
async fn workflow_shutdown_before_driver_start_cannot_be_overwritten() {
    let (_dir, service) = fixture_service(1);
    let run = service
        .create(spec(vec![task("a")]), "shutdown-before-start")
        .unwrap();
    let driver = Arc::new(CountingDriver::default());
    service.shutdown_signal().unwrap();
    service.run_driver(driver.clone()).await.unwrap();
    assert_eq!(driver.calls.load(Ordering::SeqCst), 0);
    assert_eq!(service.snapshot(&run.id).unwrap().status, RunStatus::Paused);
}

#[test]
fn workflow_shutdown_checkpoint_failure_revokes_admission_and_callback_authority() {
    let (_dir, service) = fixture_service(2);
    let mut run_spec = spec(vec![task("a"), task("queued")]);
    run_spec.mode = RunMode::Live;
    let run = service.create(run_spec, "failed-checkpoint").unwrap();
    let dispatch = service.claim_next().unwrap().unwrap();
    service.mark_running(&dispatch).unwrap();
    let child = task("child");
    service
        .add_tasks_as(&dispatch, vec![child.clone()], "child-add")
        .unwrap();
    service
        .with_connection(|connection| {
            connection
                .execute_batch(
                    "CREATE TEMP TRIGGER fail_shutdown_checkpoint BEFORE UPDATE ON workflow_runs
                     BEGIN SELECT RAISE(ABORT, 'injected shutdown checkpoint failure'); END;",
                )
                .map_err(|error| error.to_string())
        })
        .unwrap();

    let error = service.shutdown_signal().unwrap_err();
    assert!(
        error.contains("injected shutdown checkpoint failure"),
        "{error}"
    );
    let unchanged = service.snapshot(&run.id).unwrap();
    assert_eq!(unchanged.status, RunStatus::Running);
    assert!(
        !unchanged.tasks[0]
            .current_attempt
            .as_ref()
            .unwrap()
            .cancel_requested
    );
    assert_eq!(unchanged.usage.reserved_tokens, 4096);
    assert!(service.claim_next().unwrap().is_none());
    assert!(service
        .authorize_attempt(&dispatch)
        .unwrap_err()
        .contains("shutting down"));
    // Replaying an already committed graph operation must recheck authority.
    assert!(service
        .add_tasks_as(&dispatch, vec![child], "child-add")
        .unwrap_err()
        .contains("shutting down"));
    assert!(service
        .observe_successful_dependencies(&dispatch, &[("child".into(), 1)])
        .unwrap_err()
        .contains("shutting down"));

    service
        .with_connection(|connection| {
            connection
                .execute_batch("DROP TRIGGER fail_shutdown_checkpoint")
                .map_err(|error| error.to_string())
        })
        .unwrap();
    let held = service.snapshot(&run.id).unwrap();
    assert_eq!(held.usage.reserved_tokens, 4096);
    assert_eq!(held.tasks[0].attempts.len(), 1);
    assert!(held.tasks.iter().skip(1).all(|task| task.attempts.is_empty()));
    assert!(service.claim_next().unwrap().is_none());
}

#[test]
fn workflow_branch_stopping_preserves_wall_time_limit_for_active_sibling() {
    let (_dir, service) = fixture_service(2);
    let mut run_spec = spec(vec![task("branch"), task("sibling"), task("queued")]);
    run_spec.mode = RunMode::Live;
    let run = service.create(run_spec, "stopping-deadline").unwrap();
    let branch = service.claim_next().unwrap().unwrap();
    let sibling = service.claim_next().unwrap().unwrap();
    service.mark_running(&branch).unwrap();
    service.mark_running(&sibling).unwrap();
    service.cancel_task(&run.id, "branch").unwrap();
    assert_eq!(service.snapshot(&run.id).unwrap().status, RunStatus::Stopping);
    service.authorize_attempt(&sibling).unwrap();

    service
        .with_connection(|connection| {
            let mut snapshot = store::load(connection, &run.id)?;
            snapshot.created_at_ms = now_ms() - snapshot.resolved_limits.wall_time_ms as i64 - 1;
            store::save(connection, &snapshot, "elapsed-deadline-fixture")
        })
        .unwrap();
    assert!(service.claim_next().unwrap().is_none());
    let expired = service.snapshot(&run.id).unwrap();
    assert_eq!(expired.error.as_deref(), Some("Workflow wall-time limit reached"));
    assert_eq!(expired.status, RunStatus::Stopping);
    assert!(service.cancellation_requested(&sibling));
    assert!(service.authorize_attempt(&sibling).is_err());
    assert_eq!(expired.usage.reserved_tokens, 8192);
    assert!(expired.tasks[2].attempts.is_empty());
}

#[test]
fn workflow_retired_waited_child_blocks_yielded_consumer_before_continuation() {
    let (_dir, service) = fixture_service(1);
    let mut run_spec = spec(vec![task("parent")]);
    run_spec.mode = RunMode::Live;
    let run = service.create(run_spec, "retired-waited-child").unwrap();
    let parent = service.claim_next().unwrap().unwrap();
    service.mark_running(&parent).unwrap();
    service
        .add_tasks_as(&parent, vec![task("child")], "child-add")
        .unwrap();
    service
        .finish_attempt(&parent, ExecutionReport::waiting(vec!["child".into()]))
        .unwrap();
    let child = service.claim_next().unwrap().unwrap();
    assert_eq!(child.task_id, "child");
    finish(&service, &child);
    let ready = service.snapshot(&run.id).unwrap();
    assert_eq!(ready.tasks[0].status, TaskStatus::Queued);
    assert_eq!(ready.tasks[0].attempts.len(), 1);

    service.retire(&run.id, "child").unwrap();
    assert!(service.claim_next().unwrap().is_none());
    let blocked = service.snapshot(&run.id).unwrap();
    assert_eq!(blocked.tasks[0].status, TaskStatus::Blocked);
    assert_eq!(blocked.tasks[0].attempts.len(), 1);
    assert_eq!(
        blocked.tasks[0].current_attempt.as_ref().unwrap().id,
        parent.attempt_id
    );
    assert!(blocked.tasks[0].result.is_none());
    assert!(blocked.tasks[0].accepted_dependencies.is_empty());
    assert_eq!(blocked.usage.reserved_tokens, 0);
    assert_eq!(
        blocked
            .tasks
            .iter()
            .map(|task| task.attempts.len())
            .sum::<usize>(),
        2
    );
}

#[test]
fn workflow_revision_events_are_durable_and_broadcast() {
    let (_dir, service) = fixture_service(1);
    let mut events = service.subscribe();
    let run = service.create(spec(vec![task("a")]), "events").unwrap();
    assert_eq!(events.try_recv().unwrap().run_id, run.id);
    service.message(&run.id, "a", "hello").unwrap();
    let event = events.try_recv().unwrap();
    assert_eq!(event.kind, "message");
    assert_eq!(event.revision, 2);
    let durable = service.events(0).unwrap();
    assert_eq!(durable.len(), 2);
    assert!(durable[1].sequence > durable[0].sequence);
}
