use super::{RemoteApproval, TaskSnapshot, TaskStatus, TaskStore};
use crate::agent_provider::{
    AgentProvider, CompletedItem, ProviderRuntimeEvent, SendTurnInput, SessionStatus,
    StartSessionInput, ThreadId, TurnId, TurnStatus,
};
use futures_util::StreamExt;
use std::sync::Arc;
use std::time::Duration;

#[cfg(test)]
pub(super) async fn execute(
    store: Arc<TaskStore>,
    id: &str,
    provider: Arc<dyn AgentProvider>,
    workspace_id: String,
) -> Result<(), String> {
    let _lease = store.claim(id)?;
    execute_claimed(store, id, provider, workspace_id).await
}

pub(super) async fn execute_claimed(
    store: Arc<TaskStore>,
    id: &str,
    provider: Arc<dyn AgentProvider>,
    workspace_id: String,
) -> Result<(), String> {
    let task = store.snapshot(id)?;
    let thread = ThreadId(task.child_thread_id.clone());
    // Subscribe before either native operation. Drain concurrently even while
    // RPC startup/send awaits, so early completed/approval events are not lost.
    let mut stream = provider.event_stream();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let pump = tokio::spawn(async move {
        while let Some(event) = stream.next().await {
            if tx.send(event).is_err() {
                break;
            }
        }
    });
    let mut startup_attempted = false;
    let mut bound_checkout = None;
    let operation:Result<(TaskStatus,Option<String>),String>=async {
        if store.snapshot(id)?.cancel_requested {return Ok((TaskStatus::Cancelled,None));}
        let request=&task.request;
        let identity=store.checkout(id)?;
        if let Some(identity)=identity {
            if identity.workspace_id!=workspace_id {return Err("Native workspace identity differs from admission".into());}
            bound_checkout=Some(identity.open_registered(store.state_root(),std::path::Path::new(&request.workspace_path))?);
        } else if !cfg!(test) {
            return Err("Task has no validated checkout identity; legacy admission must not execute".into());
        }
        let cwd=bound_checkout.as_ref().map(|b|b.cwd()).unwrap_or_else(||request.workspace_path.clone().into());
        startup_attempted=true;
        let session=tokio::time::timeout(Duration::from_secs(60),provider.start_session(StartSessionInput {
            thread_id:thread.clone(),cwd,model:request.model.clone(),resume_cursor:None,fresh_session:true,
            permission_mode:Some(request.permission_mode.clone()),effort:request.effort.clone(),context_window:None,fast_mode:false,
            additional_directories:vec![],env:None,workspace_id:Some(workspace_id),extra:serde_json::Value::Null,recorded_usage_baseline:None,
        })).await.map_err(|_|"native session startup timed out; execution is uncertain".to_string())?.map_err(|e|e.to_string())?;
        if let Some(bound)=bound_checkout.as_mut() {bound.release_registry()?;}
        store.update(id,|task| {task.provider_session_id=Some(session.session_id.0);task.activity=Some("Native session configured".into());})?;
        if store.snapshot(id)?.cancel_requested {return Ok((TaskStatus::Cancelled,None));}
        let turn=tokio::time::timeout(Duration::from_secs(60),provider.send_turn(SendTurnInput {
            thread_id:thread.clone(),text:request.prompt.clone(),display_text:None,images:vec![],skill_invocations:vec![],
            model_override:request.model.clone(),effort_override:request.effort.clone(),permission_mode_override:Some(request.permission_mode.clone()),
            client_nonce:Some(request.id.clone()),turn_checkpoint:None,dispatch_guard:None,
        })).await.map_err(|_|"native send acknowledgement timed out; execution is uncertain".to_string())?.map_err(|e|e.to_string())?;
        if turn.queued_id.is_some() || turn.steered || turn.turn_id.0.is_empty() {return Err("fresh task did not receive a unique native turn; never replay it".into());}
        store.update(id,|task| {task.turn_id=Some(turn.turn_id.0.clone());task.status=TaskStatus::Running;task.activity=Some("Native agent running".into());})?;
        let mut ticker=tokio::time::interval(Duration::from_millis(100));
        loop {
            tokio::select! {
                event=rx.recv() => {
                    let event=event.ok_or("native event stream closed before turn completion")?;
                    if let Some(end)=record(&store,id,&event)? {return Ok(end);}
                }
                _=ticker.tick() => {
                    let snapshot=store.snapshot(id)?;
                    if snapshot.cancel_requested {return Ok((TaskStatus::Cancelled,None));}
                    while let Some(response) = store.take_responses(id)?.into_iter().next() {
                        // Native callbacks are process-local. Each durable response
                        // is attempted once; transport ambiguity is never retried.
                        let callback=tokio::time::timeout(Duration::from_secs(15),provider.respond_to_request(thread.clone(),crate::agent_provider::RequestId(response.request_id.clone()),response.decision));
                        let cancelled=async {
                            loop {
                                if store.snapshot(id)?.cancel_requested {return Ok::<(),String>(());}
                                tokio::time::sleep(Duration::from_millis(20)).await;
                            }
                        };
                        let delivered=tokio::select! {
                            biased;
                            stop=cancelled => {stop?;return Ok((TaskStatus::Cancelled,None));}
                            result=callback => result,
                        };
                        if store.snapshot(id)?.cancel_requested {return Ok((TaskStatus::Cancelled,None));}
                        store.finish_response_attempt(id,&response.request_id,matches!(&delivered,Ok(Ok(()))))?;
                        match delivered {
                            Ok(Ok(())) => store.update(id,|task| {
                                resolve_approval(task, &response.request_id);
                            })?,
                            Ok(Err(error)) => store.update(id,|task| {task.error=Some(format!("Native approval response failed: {error}"));task.activity=Some("Approval delivery failed; stop this task if the callback is stale".into());})?,
                            Err(_) => store.update(id,|task| {task.error=Some("Native approval acknowledgement timed out; delivery is uncertain and will not be replayed".into());})?,
                        }
                    }
                }
            }
        }
    }.await;
    let (mut status, mut error) = match operation {
        Ok(end) => end,
        Err(error) => (
            // ProviderError erases the RPC transport outcome. Once a native
            // startup/send was invoked, conservatively retain uncertainty for
            // every uncorrelated failure (including malformed/late acks).
            // Successful teardown cannot disprove earlier checkout side effects.
            if startup_attempted {
                TaskStatus::Interrupted
            } else {
                TaskStatus::Failed
            },
            Some(error),
        ),
    };
    // Never expose Cancelled (or Completed) while an owned provider is still
    // alive. Ok from stop_session acknowledges teardown of its own subprocess.
    let stop = if startup_attempted {
        tokio::time::timeout(Duration::from_secs(15), provider.stop_session(thread)).await
    } else {
        Ok(Ok(()))
    };
    match stop {
        Ok(Ok(())) => {}
        Ok(Err(stop)) => {
            status = TaskStatus::Interrupted;
            error = Some(format!(
                "Native stop was not acknowledged: {stop}; earlier outcome: {error:?}"
            ));
        }
        Err(_) => {
            status = TaskStatus::Interrupted;
            error = Some(
                "Native stop acknowledgement timed out; remote execution may still be active"
                    .into(),
            );
        }
    }
    // Include final lifecycle/response events already observed during teardown.
    tokio::task::yield_now().await;
    let mut drain_error = None;
    while let Ok(event) = rx.try_recv() {
        if let Err(failure) = record(&store, id, &event) {
            drain_error = Some(failure);
            break;
        }
    }
    // Cleanup must run even when the journal cannot accept the final event.
    pump.abort();
    let _ = pump.await;
    if let Some(failure) = drain_error {
        status = TaskStatus::Interrupted;
        error = Some(failure);
    }
    store.update(id, |task| {
        task.status = if task.cancel_requested && status != TaskStatus::Interrupted {
            TaskStatus::Cancelled
        } else {
            status
        };
        task.error = error.clone();
        task.activity = None;
        task.pending_requests.clear();
        if task.status != TaskStatus::Completed {
            task.result = None;
        }
    })?;
    match error {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

fn resolve_approval(task: &mut TaskSnapshot, request_id: &str) {
    if task.cancel_requested || task.status.is_terminal() || task.status == TaskStatus::Stopping {
        return;
    }
    let before = task.pending_requests.len();
    task.pending_requests.retain(|r| r.request_id != request_id);
    if task.pending_requests.len() == before {
        return;
    }
    task.status = if task.pending_requests.is_empty() {
        if task.activity.as_deref() == Some("Waiting for native approval") {
            task.activity = Some("Native agent running".into());
        }
        TaskStatus::Running
    } else {
        TaskStatus::AwaitingApproval
    };
}

fn record(
    store: &TaskStore,
    id: &str,
    event: &ProviderRuntimeEvent,
) -> Result<Option<(TaskStatus, Option<String>)>, String> {
    let value = serde_json::to_value(event).map_err(|e| e.to_string())?;
    let snapshot = store.snapshot(id)?;
    if value
        .get("thread_id")
        .and_then(|v| v.as_str())
        .is_some_and(|thread| thread != snapshot.child_thread_id)
    {
        return Ok(None);
    }
    let mut end = None;
    store.append(id, &value, |task| {
        end = apply_event(task, event);
    })?;
    Ok(end)
}
fn apply_event(
    task: &mut TaskSnapshot,
    event: &ProviderRuntimeEvent,
) -> Option<(TaskStatus, Option<String>)> {
    // This worker owns one fresh session and dispatches exactly one turn.
    // Claude may emit callbacks and its final result before the send ack,
    // stamping an empty native turn ID. The pump preserves those events until
    // the single accepted turn is known; never rewrite their original IDs.
    let is_turn = |turn: &TurnId| {
        task.turn_id.as_deref() == Some(turn.0.as_str())
            || (task.request.provider == crate::agent_provider::ProviderKind::Claude
                && turn.0.is_empty()
                && task.turn_id.is_some())
    };
    match event {
        ProviderRuntimeEvent::SessionConfigured {
            provider_session_id,
            ..
        } => task.provider_session_id = Some(provider_session_id.0.clone()),
        ProviderRuntimeEvent::ItemCompleted {
            turn_id,
            item: CompletedItem::AssistantText { text },
            subagent_id: None,
            ..
        } if is_turn(turn_id) => task.result = Some(text.clone()),
        ProviderRuntimeEvent::ItemCompleted {
            turn_id,
            item: CompletedItem::ToolUse { tool_name, .. },
            subagent_id: None,
            ..
        } if is_turn(turn_id) => task.activity = Some(format!("Using {tool_name}")),
        ProviderRuntimeEvent::TurnCompleted {
            turn_id, status, ..
        } if is_turn(turn_id) => {
            return Some(match status {
                TurnStatus::Success => (TaskStatus::Completed, None),
                TurnStatus::Error { subtype, message } => (
                    if subtype == crate::agent_provider::CHILD_EXITED_SUBTYPE {
                        TaskStatus::Interrupted
                    } else {
                        TaskStatus::Failed
                    },
                    Some(message.clone()),
                ),
                TurnStatus::MaxTurns => (
                    TaskStatus::Failed,
                    Some("Native provider reached its turn limit".into()),
                ),
                TurnStatus::MaxBudget => (
                    TaskStatus::Failed,
                    Some("Native provider reached its budget limit".into()),
                ),
            });
        }
        ProviderRuntimeEvent::RequestOpened {
            turn_id,
            request_id,
            request_kind,
            payload,
            ..
        } if is_turn(turn_id) => {
            if !task
                .pending_requests
                .iter()
                .any(|r| r.request_id == request_id.0)
            {
                task.pending_requests.push(RemoteApproval {
                    request_id: request_id.0.clone(),
                    request_kind: request_kind.clone(),
                    payload: payload.clone(),
                });
            }
            task.status = TaskStatus::AwaitingApproval;
            task.activity = Some("Waiting for native approval".into());
        }
        ProviderRuntimeEvent::RequestResolved { request_id, .. } => {
            resolve_approval(task, &request_id.0);
        }
        ProviderRuntimeEvent::RequestResponseFailed {
            request_id,
            message,
            ..
        } => {
            task.pending_requests
                .retain(|r| r.request_id != request_id.0);
            task.error = Some(message.clone());
        }
        ProviderRuntimeEvent::SessionStateChanged {
            status: SessionStatus::Error { message },
            ..
        } => {
            // A session-only error has no native turn outcome. This branch is
            // consumed only after startup/send; teardown cannot disprove effects.
            return Some((TaskStatus::Interrupted, Some(message.clone())))
        },
        ProviderRuntimeEvent::SessionStateChanged {
            status: SessionStatus::Closed,
            ..
        } => {
            return Some((
                TaskStatus::Interrupted,
                Some("Native session closed before this turn completed".into()),
            ))
        }
        ProviderRuntimeEvent::RuntimeWarning { message, .. } => {
            task.activity = Some(message.clone())
        }
        _ => {}
    }
    None
}

#[cfg(test)]
#[path = "worker_review_tests.rs"]
mod worker_review_tests;

#[cfg(test)]
pub(super) mod tests {
    use super::super::TaskStatus;
    use super::*;
    use crate::agent_provider::{
        codex::{CodexAgentProvider, CodexProviderConfig},
        ProviderKind,
    };
    use serde_json::json;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    pub(in crate::remote::tasks) fn fixture(
        dir: &tempfile::TempDir,
        script: serde_json::Value,
    ) -> Arc<dyn AgentProvider> {
        let helper =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/debug/fake_codex_app_server");
        assert!(
            helper.is_file(),
            "build the existing test-fixtures fake_codex_app_server helper first"
        );
        let script_path = dir.path().join("script.json");
        std::fs::write(&script_path, serde_json::to_vec(&script).unwrap()).unwrap();
        let proxy = dir.path().join("wire-proxy.py");
        std::fs::write(&proxy,r#"import subprocess,sys,threading,os,json
child=subprocess.Popen(sys.argv[1:],stdin=subprocess.PIPE,stdout=subprocess.PIPE,text=True,bufsize=1)
def capture(direction,line):
    with open(os.environ['WIRE_CAPTURE'],'a') as f: f.write(json.dumps({'direction':direction,'message':json.loads(line)})+'\n')
def output():
    for line in child.stdout:
        capture('from_provider',line);sys.stdout.write(line);sys.stdout.flush()
    os._exit(child.wait())
threading.Thread(target=output,daemon=True).start()
for line in sys.stdin:
    capture('to_provider',line);child.stdin.write(line);child.stdin.flush()
child.stdin.close();child.wait()
"#).unwrap();
        let wrapper = dir.path().join("codex-fixture");
        std::fs::write(&wrapper,format!("#!/bin/sh\nexport FAKE_CODEX_SCRIPT='{}'\nexport FAKE_CODEX_TRACE='{}'\nexport WIRE_CAPTURE='{}'\nexec '{}' '{}' '{}' \"$@\"\n",script_path.display(),dir.path().join("trace.jsonl").display(),dir.path().join("wire.jsonl").display(),which::which("python3").unwrap().display(),proxy.display(),helper.display())).unwrap();
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o700)).unwrap();
        Arc::new(CodexAgentProvider::new(CodexProviderConfig {
            codex_binary: wrapper,
            ..Default::default()
        }))
    }
    pub(super) fn task(dir: &tempfile::TempDir) -> (Arc<TaskStore>, String) {
        let store = Arc::new(TaskStore::open(dir.path()).unwrap());
        let mut request = super::super::tests::request();
        request.provider = ProviderKind::Codex;
        request.workspace_path = dir.path().to_string_lossy().into();
        store.admit(&request).unwrap();
        (store, request.id)
    }
    pub(super) async fn wait_status(
        store: &TaskStore,
        id: &str,
        status: TaskStatus,
    ) -> TaskSnapshot {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let snapshot = store.snapshot(id).unwrap();
                if snapshot.status == status {
                    return snapshot;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("native fixture task did not reach the expected state")
    }
    #[tokio::test]
    async fn live_cancel_waits_for_native_session_shutdown_and_never_replays() {
        let dir = tempfile::TempDir::new().unwrap();
        let (store, id) = task(&dir);
        let provider = fixture(&dir, json!([]));
        let worker = tokio::spawn({
            let store = store.clone();
            let id = id.clone();
            let provider = provider.clone();
            async move { execute(store, &id, provider, "fixture-workspace".into()).await }
        });
        wait_status(&store, &id, TaskStatus::Running).await;
        store.cancel(&id).unwrap();
        assert_eq!(store.snapshot(&id).unwrap().status, TaskStatus::Stopping);
        assert!(tokio::time::timeout(Duration::from_secs(5), worker)
            .await
            .unwrap()
            .unwrap()
            .is_ok());
        assert_eq!(store.snapshot(&id).unwrap().status, TaskStatus::Cancelled);
        assert!(
            provider.list_sessions().await.unwrap().is_empty(),
            "native owned process/session was reaped before Cancelled"
        );
        assert!(
            execute(store.clone(), &id, provider, "fixture-workspace".into())
                .await
                .is_err()
        );
    }
    #[tokio::test]
    async fn ngi2_native_callback_retires_only_last_approval_wait() {
        use crate::agent_provider::ApprovalDecision;
        for progress in [false, true] {
            let dir = tempfile::TempDir::new().unwrap();
            let (store, id) = task(&dir);
            let provider = fixture(&dir, json!([
                {"after":"turn/start","emit":"server_request","method":"item/commandExecution/requestApproval","params":{"threadId":"c-1","turnId":"t-1","command":"first original command"}},
                {"after":"turn/start","emit":"server_request","method":"item/commandExecution/requestApproval","params":{"threadId":"c-1","turnId":"t-1","command":"second original command"}}
            ]));
            let worker = tokio::spawn({
                let store = store.clone(); let id = id.clone();
                async move { execute(store, &id, provider, "fixture-workspace".into()).await }
            });
            tokio::time::timeout(Duration::from_secs(5), async {
                while store.snapshot(&id).unwrap().pending_requests.len() != 2 {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            }).await.unwrap();
            let requests = store.snapshot(&id).unwrap().pending_requests;
            if progress {
                record(&store, &id, &ProviderRuntimeEvent::RuntimeWarning {
                    thread_id: Some(ThreadId(store.snapshot(&id).unwrap().child_thread_id)),
                    message: "Newer native progress".into(),
                    original_payload: None,
                }).unwrap();
            }
            let mut settled = Vec::new();
            for (index, request) in requests.iter().enumerate() {
                let response = super::super::RespondRequest {
                    original_request: Some(request.clone()), request_id: request.request_id.clone(),
                    decision: ApprovalDecision::Deny { message: "Fixture denies this exact command".into() },
                };
                store.respond(&id, &response).unwrap();
                store.respond(&id, &response).unwrap();
                tokio::time::timeout(Duration::from_secs(5), async {
                    while store.snapshot(&id).unwrap().pending_requests.len() != 1 - index {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                }).await.unwrap();
                settled.push(store.snapshot(&id).unwrap());
            }
            store.cancel(&id).unwrap();
            assert!(tokio::time::timeout(Duration::from_secs(5), worker).await.unwrap().unwrap().is_ok());
            let wire = std::fs::read_to_string(dir.path().join("wire.jsonl")).unwrap();
            assert_eq!(wire.lines().filter(|l| serde_json::from_str::<serde_json::Value>(l).unwrap()["message"]["result"]["decision"] == "deny").count(), 2, "exact duplicate enqueues must not replay callbacks");
            assert_eq!(settled[0].status, TaskStatus::AwaitingApproval);
            assert_eq!(settled[0].activity.as_deref(), Some(if progress { "Newer native progress" } else { "Waiting for native approval" }));
            assert_eq!(settled[1].status, TaskStatus::Running);
            assert_eq!(settled[1].activity.as_deref(), Some(if progress { "Newer native progress" } else { "Native agent running" }), "successful last callback left stale wait activity");
        }
    }
    #[test]
    fn ngi2_resolved_event_preserves_progress_and_stop_terminal_guards() {
        use crate::agent_provider::{ApprovalDecision, RequestId};
        for guard in ["live", "progress", "cancel", "stopping", "terminal"] {
            let dir = tempfile::TempDir::new().unwrap();
            let (store, id) = task(&dir);
            store.update(&id, |t| { t.status = TaskStatus::Running; t.turn_id = Some("turn".into()); }).unwrap();
            let thread = ThreadId(store.snapshot(&id).unwrap().child_thread_id);
            for request_id in ["A", "B"] {
                record(&store, &id, &ProviderRuntimeEvent::RequestOpened {
                    thread_id: thread.clone(), turn_id: TurnId("turn".into()),
                    request_id: RequestId(request_id.into()), request_kind: "command".into(),
                    payload: json!({"command":"original", "request":request_id}),
                    tool_use_id: None, subagent_id: None,
                }).unwrap();
            }
            let resolved = |request_id: &str| ProviderRuntimeEvent::RequestResolved {
                thread_id: thread.clone(), request_id: RequestId(request_id.into()),
                decision: ApprovalDecision::Deny { message: "fixture".into() },
            };
            record(&store, &id, &resolved("A")).unwrap();
            assert_eq!(store.snapshot(&id).unwrap().pending_requests.len(), 1);
            assert_eq!(store.snapshot(&id).unwrap().activity.as_deref(), Some("Waiting for native approval"));
            if guard == "progress" {
                record(&store, &id, &ProviderRuntimeEvent::RuntimeWarning {
                    thread_id: Some(thread.clone()), message: "Newer progress".into(), original_payload: None,
                }).unwrap();
            }
            if guard == "cancel" { store.cancel(&id).unwrap(); }
            if guard == "stopping" { store.update(&id, |t| t.status = TaskStatus::Stopping).unwrap(); }
            if guard == "terminal" { store.update(&id, |t| t.status = TaskStatus::Completed).unwrap(); }
            let before = store.snapshot(&id).unwrap();
            record(&store, &id, &resolved("B")).unwrap();
            let after = store.snapshot(&id).unwrap();
            if matches!(guard, "cancel" | "stopping" | "terminal") {
                assert_eq!(after.status, before.status, "late resolution revived {guard}");
                assert_eq!(after.pending_requests, before.pending_requests, "late resolution rewrote {guard}");
                assert_eq!(after.activity, before.activity, "late resolution rewrote {guard} activity");
            } else {
                assert_eq!(after.status, TaskStatus::Running);
                assert!(after.pending_requests.is_empty());
                assert_eq!(after.activity.as_deref(), Some(if guard == "progress" { "Newer progress" } else { "Native agent running" }));
                record(&store, &id, &resolved("B")).unwrap();
                record(&store, &id, &resolved("foreign")).unwrap();
                assert_eq!(store.snapshot(&id).unwrap().activity, after.activity, "duplicate/unrelated resolution rewrote progress");
            }
        }
    }
    #[tokio::test]
    async fn genuine_native_approval_deny_is_delivered_and_journalled_once() {
        use crate::agent_provider::ApprovalDecision;
        let dir = tempfile::TempDir::new().unwrap();
        let (store, id) = task(&dir);
        let provider = fixture(
            &dir,
            json!([
                {"after":"turn/start","emit":"server_request","method":"item/commandExecution/requestApproval","params":{"threadId":"c-1","turnId":"t-1","command":"fixture command"}}
            ]),
        );
        let worker = tokio::spawn({
            let store = store.clone();
            let id = id.clone();
            async move { execute(store, &id, provider, "fixture-workspace".into()).await }
        });
        let snapshot = wait_status(&store, &id, TaskStatus::AwaitingApproval).await;
        let request_id = snapshot.pending_requests[0].request_id.clone();
        let response = super::super::RespondRequest {
            original_request:Some(snapshot.pending_requests[0].clone()),
            request_id: request_id.clone(),
            decision: ApprovalDecision::Deny {
                message: "fixture denied".into(),
            },
        };
        store.respond(&id, &response).unwrap();
        assert!(store.respond(&id, &response).is_ok());
        wait_status(&store, &id, TaskStatus::Running).await;
        // Codex's adapter acknowledges the actual JSON-RPC write but does not
        // emit RequestResolved; do not invent a canonical event to fill that gap.
        let wire = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let wire = std::fs::read_to_string(dir.path().join("wire.jsonl")).unwrap();
                if wire.lines().any(|l| {
                    serde_json::from_str::<serde_json::Value>(l).unwrap()["message"]["result"]
                        ["decision"]
                        == "deny"
                }) {
                    return wire;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("denial did not reach the native fixture's wire");
        assert_eq!(
            wire.lines()
                .filter(
                    |l| serde_json::from_str::<serde_json::Value>(l).unwrap()["message"]["result"]
                        ["decision"]
                        == "deny"
                )
                .count(),
            1
        );
        assert!(store
            .read(&id, 0)
            .unwrap()
            .events
            .iter()
            .any(|e| e.event["type"] == "request_opened"));
        store.cancel(&id).unwrap();
        assert!(tokio::time::timeout(Duration::from_secs(5), worker)
            .await
            .unwrap()
            .unwrap()
            .is_ok());
    }
    #[tokio::test]
    async fn native_child_exit_is_interrupted_and_never_reports_completion() {
        let dir = tempfile::TempDir::new().unwrap();
        let (store, id) = task(&dir);
        let provider = fixture(&dir, json!([]));
        // The fixture wrapper is task-owned. Exit only this child after its
        // accepted send to exercise the actual adapter's crash watchdog.
        let wrapper = dir.path().join("codex-fixture");
        let original = std::fs::read_to_string(&wrapper).unwrap();
        std::fs::write(
            &wrapper,
            original.replace("exec '", "export FAKE_CODEX_EXIT_AFTER=turn/start\nexec '"),
        )
        .unwrap();
        let result = tokio::time::timeout(
            Duration::from_secs(120),
            execute(store.clone(), &id, provider, "fixture-workspace".into()),
        )
        .await
        .unwrap();
        assert!(result.is_err());
        let task = store.snapshot(&id).unwrap();
        assert_eq!(task.status, TaskStatus::Interrupted);
        assert!(task.result.is_none());
        assert!(task.error.is_some());
        assert!(!store.admit(&task.request).unwrap());
    }
    #[test]
    fn final_result_ignores_other_turns_threads_and_subagent_text() {
        let dir = tempfile::TempDir::new().unwrap();
        let (store, id) = task(&dir);
        store
            .update(&id, |t| t.turn_id = Some("run-turn".into()))
            .unwrap();
        let thread = ThreadId(store.snapshot(&id).unwrap().child_thread_id);
        let event = |thread_id: ThreadId, turn: &str, subagent_id: Option<String>, text: &str| {
            ProviderRuntimeEvent::ItemCompleted {
                thread_id,
                turn_id: TurnId(turn.into()),
                item: CompletedItem::AssistantText { text: text.into() },
                subagent_id,
            }
        };
        record(
            &store,
            &id,
            &event(thread.clone(), "older-turn", None, "old history"),
        )
        .unwrap();
        record(
            &store,
            &id,
            &event(
                ThreadId("someone-else".into()),
                "run-turn",
                None,
                "different session",
            ),
        )
        .unwrap();
        record(
            &store,
            &id,
            &event(
                thread.clone(),
                "run-turn",
                Some("child".into()),
                "subagent output",
            ),
        )
        .unwrap();
        assert!(store.snapshot(&id).unwrap().result.is_none());
        record(
            &store,
            &id,
            &event(thread, "run-turn", None, "this run final"),
        )
        .unwrap();
        assert_eq!(
            store.snapshot(&id).unwrap().result.as_deref(),
            Some("this run final")
        );
    }
    #[tokio::test]
    async fn codex_native_run_persists_final_text_session_turn_and_real_events() {
        let dir = tempfile::TempDir::new().unwrap();
        let (store, id) = task(&dir);
        let provider = fixture(
            &dir,
            json!([
                {"after":"turn/start","emit":"notification","method":"item/completed","params":{"threadId":"c-1","turnId":"t-1","item":{"type":"agentMessage","id":"a-1","text":"native fixture final"}}},
                {"after":"turn/start","delay_ms":30,"emit":"notification","method":"turn/completed","params":{"threadId":"c-1","turnId":"t-1","status":"succeeded"}}
            ]),
        );
        let outcome = tokio::time::timeout(
            std::time::Duration::from_secs(120),
            execute(store.clone(), &id, provider, "fixture-workspace".into()),
        )
        .await
        .unwrap();
        assert!(
            outcome.is_ok(),
            "real Codex AgentProvider must execute through the journal: {outcome:?}"
        );
        let read = store.read(&id, 0).unwrap();
        assert_eq!(read.task.status, TaskStatus::Completed);
        assert_eq!(read.task.result.as_deref(), Some("native fixture final"));
        assert_eq!(read.task.provider_session_id.as_deref(), Some("c-1"));
        assert_eq!(read.task.turn_id.as_deref(), Some("t-1"));
        assert!(read
            .events
            .iter()
            .any(|e| e.event["type"] == "turn_completed"));
        let calls = std::fs::read_to_string(dir.path().join("trace.jsonl")).unwrap();
        let sends: Vec<_> = calls
            .lines()
            .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap())
            .filter(|v| v["method"] == "turn/start")
            .collect();
        assert_eq!(sends.len(), 1);
        let start: serde_json::Value = calls
            .lines()
            .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap())
            .find(|v| v["method"] == "thread/start")
            .unwrap();
        assert_eq!(start["params"]["sandbox"], "read-only");
        assert_eq!(start["params"]["approvalPolicy"], "untrusted");
    }
    #[tokio::test]
    async fn cancel_before_start_is_acknowledged_without_sending_a_turn() {
        let dir = tempfile::TempDir::new().unwrap();
        let (store, id) = task(&dir);
        store.cancel(&id).unwrap();
        let outcome = execute(
            store.clone(),
            &id,
            fixture(&dir, json!([])),
            "fixture-workspace".into(),
        )
        .await;
        assert!(
            outcome.is_ok(),
            "persisted cancellation needs native stop acknowledgement"
        );
        assert_eq!(store.snapshot(&id).unwrap().status, TaskStatus::Cancelled);
        assert!(
            !dir.path().join("trace.jsonl").exists(),
            "prestart cancellation never dispatches native work"
        );
    }
    #[tokio::test]
    async fn claude_native_adapter_executes_one_fresh_run_and_reaps_sidecar() {
        use crate::agent_provider::claude::{ClaudeAgentProvider, ClaudeProviderConfig};
        let dir = tempfile::TempDir::new().unwrap();
        let store = Arc::new(TaskStore::open(dir.path()).unwrap());
        let mut request = super::super::tests::request();
        request.provider = ProviderKind::Claude;
        request.workspace_path = dir.path().to_string_lossy().into();
        request.permission_mode = "default".into();
        store.admit(&request).unwrap();
        let thread = store.snapshot(&request.id).unwrap().child_thread_id;
        let script = json!([
            {"after":"send-turn","delay_ms":5,"emit":"notification","method":"sdk-message","params":{"threadId":thread,"message":{"type":"system","subtype":"init","session_id":"claude-fixture-session"}}},
            {"after":"send-turn","delay_ms":5,"emit":"notification","method":"sdk-message","params":{"threadId":thread,"message":{"type":"assistant","message":{"content":[{"type":"text","text":"Claude native fixture final"}]}}}},
            {"after":"send-turn","delay_ms":5,"emit":"notification","method":"sdk-message","params":{"threadId":thread,"message":{"type":"result","subtype":"success","duration_ms":15,"num_turns":1}}}
        ]);
        let helper =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/debug/fake_claude_sidecar");
        assert!(
            helper.is_file(),
            "build the existing fake_claude_sidecar test helper first"
        );
        let path = dir.path().join("claude-script.json");
        std::fs::write(&path, serde_json::to_vec(&script).unwrap()).unwrap();
        let wrapper = dir.path().join("sidecar");
        std::fs::write(&wrapper,format!("#!/bin/sh\nexport FAKE_CLAUDE_SIDECAR_SCRIPT='{}'\nexport FAKE_CLAUDE_SIDECAR_CAPTURE='{}'\nexec '{}' \"$@\"\n",path.display(),dir.path().join("claude-calls.jsonl").display(),helper.display())).unwrap();
        std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o700)).unwrap();
        let provider: Arc<dyn AgentProvider> = Arc::new(
            ClaudeAgentProvider::new(ClaudeProviderConfig {
                sidecar_binary: Some(wrapper),
                claude_binary: Some("/fixture/claude".into()),
                ..Default::default()
            })
            .await
            .unwrap(),
        );
        let result = tokio::time::timeout(
            Duration::from_secs(120),
            execute(
                store.clone(),
                &request.id,
                provider.clone(),
                "fixture-workspace".into(),
            ),
        )
        .await
        .unwrap();
        assert!(result.is_ok(), "{result:?}");
        let read = store.read(&request.id, 0).unwrap();
        assert_eq!(read.task.status, TaskStatus::Completed);
        assert_eq!(
            read.task.result.as_deref(),
            Some("Claude native fixture final")
        );
        assert_eq!(
            read.task.provider_session_id.as_deref(),
            Some("claude-fixture-session")
        );
        assert!(read.task.turn_id.as_ref().is_some_and(|id| !id.is_empty()));
        assert!(read
            .events
            .iter()
            .any(|e| e.event["type"] == "turn_completed"));
        assert!(provider.list_sessions().await.unwrap().is_empty());
        let call: serde_json::Value =
            std::fs::read_to_string(dir.path().join("claude-calls.jsonl"))
                .unwrap()
                .lines()
                .next()
                .map(|l| serde_json::from_str(l).unwrap())
                .unwrap();
        assert_eq!(call["params"]["permissionMode"], "default");
        assert!(call["params"].get("resume").is_none() || call["params"]["resume"].is_null());
    }
}
