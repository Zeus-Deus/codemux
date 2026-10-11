use super::*;
use std::{sync::Arc, time::Duration};
use tauri::Manager;
use crate::{agent_provider::ProviderKind, database::DatabaseStore, state::AppStateStore};

fn setup(root: &std::path::Path, name: &str) -> (tauri::App<tauri::test::MockRuntime>, Arc<journal::Journal>, LocalTask, String, Grant, i64) {
    let app = tauri::test::mock_app();
    let state = AppStateStore::default();
    let ws = state.snapshot().active_workspace_id.0;
    let pane = state.create_agent_chat_pane(&ws,Some(ProviderKind::Codex),None,None,Some(name.into())).unwrap();
    let db = DatabaseStore::new_in_memory();
    db.upsert_agent_chat_session(name,&ws,Some("/synthetic/parent"),"codex").unwrap();
    db.update_agent_chat_session_config(name,&crate::database::AgentChatSessionConfig {permission_mode:crate::database::AgentChatSessionConfig::set("workspace-write"),..Default::default()}).unwrap();
    let host = db.insert_host("Fixture", "fixture-host").unwrap();
    let store = Arc::new(journal::Journal::open(root).unwrap());
    let (mut parent,mut grant,input)=tests::fixture();
    parent.thread_id=name.into();parent.workspace_id=ws.clone();grant.host_id=host.id;
    store.put_grant(&ws,&grant).unwrap();
    let task=store.admit(&parent,&grant,"checkout",&input).unwrap();
    app.manage(state);app.manage(db);app.manage(store.clone());
    (app,store,task,pane.0,grant,host.id)
}

#[test]
fn contract_native_delivery_projection_uses_ledger_and_current_authority() {
    let dir=tempfile::TempDir::new().unwrap();
    let (app,store,task,pane,_,_)=setup(dir.path(),"contract-delivery-projection");
    store.apply_remote(&task.id,&tests::remote(&task,crate::remote::tasks::TaskStatus::Completed)).unwrap();
    for (wake,state,cancel,result,eligible) in [
        (WakeState::Held,"before_send",false,true,true),
        (WakeState::Suppressed,"before_send",false,true,true),
        (WakeState::Held,"accepted",false,true,false),
        (WakeState::Suppressed,"unknown",false,true,false),
        (WakeState::Held,"before_send",true,true,false),
        (WakeState::Held,"before_send",false,false,false),
    ] {
        store.update(&task.id,|t|{t.wake_state=wake;t.cancel_requested=cancel;if !result {t.remote=None;}}).unwrap();
        store.conn.lock().unwrap().execute("INSERT OR REPLACE INTO delivery_outcomes(task,attempt,state) VALUES(?1,'fixture',?2)",rusqlite::params![task.id,state]).unwrap();
        let args=serde_json::json!({"paneId":pane,"taskId":task.id});
        let read=tauri::async_runtime::block_on(app::invoke(app.handle(),"delegation_read",args.clone())).unwrap();
        assert_eq!(read["task"]["delivery"]["eligible"],serde_json::json!(eligible),"delivery projection missing/wrong: {wake:?}/{state}/{cancel}/{result}");
        assert_eq!(read["task"]["delivery"]["outcome"],state);
        let list=tauri::async_runtime::block_on(app::invoke(app.handle(),"delegation_list",args)).unwrap();
        assert_eq!(list[0]["delivery"],read["task"]["delivery"]);
    }
    store.update(&task.id,|t|{t.cancel_requested=false;t.remote=Some(tests::remote(&task,crate::remote::tasks::TaskStatus::Completed).task);}).unwrap();
    store.conn.lock().unwrap().execute("UPDATE delivery_outcomes SET state='in_flight' WHERE task=?1",[&task.id]).unwrap();
    let reopened=journal::Journal::open(dir.path()).unwrap(); reopened.recover_deliveries().unwrap();
    let read=tauri::async_runtime::block_on(app::invoke(app.handle(),"delegation_read",serde_json::json!({"paneId":pane,"taskId":task.id}))).unwrap();
    assert_eq!(read["task"]["delivery"]["outcome"],"unknown","restart projection lost immutable uncertainty");
    assert_eq!(read["task"]["delivery"]["eligible"],false);
    store.conn.lock().unwrap().execute("UPDATE delivery_outcomes SET state='before_send' WHERE task=?1",[&task.id]).unwrap();
    store.disable_grant(&task.parent_workspace_id,&store.grant_for_task(&task.id).unwrap().id).unwrap();
    let read=tauri::async_runtime::block_on(app::invoke(app.handle(),"delegation_read",serde_json::json!({"paneId":pane,"taskId":task.id}))).unwrap();
    assert_eq!(read["task"]["delivery"]["reason"],"authority_unavailable","projection ignored current grant authority");
    assert_eq!(read["task"]["delivery"]["eligible"],false);
}
#[test]
fn ui_recovery_response_projection_retains_immutable_decision_and_fail_closed_outcomes() {
    let dir=tempfile::TempDir::new().unwrap();
    let (app,store,task,pane,_,_)=setup(dir.path(),"ui-recovery-response");
    let mut remote=tests::remote(&task,crate::remote::tasks::TaskStatus::AwaitingApproval);
    remote.task.pending_requests.push(crate::remote::tasks::RemoteApproval {request_id:"question".into(),request_kind:"user-input".into(),payload:serde_json::json!({"questions":[{"question":"Answer?","options":[]}]})});
    store.apply_remote(&task.id,&remote).unwrap();
    let read=||tauri::async_runtime::block_on(app::invoke(app.handle(),"delegation_read",serde_json::json!({"paneId":pane,"taskId":task.id}))).unwrap();
    assert_eq!(read()["task"]["responses"][0]["outcome"],"not_attempted","native cannot qualify a preclaim retry");
    assert_eq!(read()["task"]["responses"][0]["eligible"],true);
    let response=crate::remote::tasks::RespondRequest {original_request:Some(remote.task.pending_requests[0].clone()),request_id:"question".into(),decision:serde_json::from_value(serde_json::json!({"decision":"allow","updated_input":{"questions":[{"question":"Answer?","options":[]}],"answers":{"Answer?":"Exact immutable answer"}}})).unwrap()};
    assert!(store.claim_response(&task.id,&response).unwrap());
    for (state,eligible) in [("in_flight",false),("before_send",true),("accepted",false),("unknown",false)] {
        store.conn.lock().unwrap().execute("UPDATE response_outcomes SET state=?2 WHERE task=?1",rusqlite::params![task.id,state]).unwrap();
        let projected=read();let projection=&projected["task"]["responses"][0];
        assert_eq!(projection["outcome"],state);assert_eq!(projection["eligible"],eligible);
        assert_eq!(projection["decision"],serde_json::to_value(&response.decision).unwrap());
        let list=tauri::async_runtime::block_on(app::invoke(app.handle(),"delegation_list",serde_json::json!({"paneId":pane}))).unwrap();
        assert_eq!(list[0]["responses"],projected["task"]["responses"]);
        let mut changed=response.clone();changed.decision=crate::agent_provider::ApprovalDecision::Deny {message:"Conflicting".into()};
        assert!(store.claim_response(&task.id,&changed).is_err());
        if !eligible {assert!(!store.claim_response(&task.id,&response).unwrap());}
    }
    store.conn.lock().unwrap().execute("UPDATE response_outcomes SET state='before_send' WHERE task=?1",[&task.id]).unwrap();
    for changed in [false,true] {
        store.update(&task.id,|task|{task.cancel_requested=!changed;task.remote=Some(remote.task.clone());if changed {task.remote.as_mut().unwrap().pending_requests[0].payload["hint"]=serde_json::json!("Changed original payload");}}).unwrap();
        let projected=read();assert_eq!(projected["task"]["responses"][0]["eligible"],false);
        assert_eq!(projected["task"]["responses"][0]["reason"],if changed {"payload_changed"} else {"cancelled"});
    }
    store.update(&task.id,|task|{task.cancel_requested=false;task.remote=Some(remote.task.clone());}).unwrap();
    store.disable_grant(&task.parent_workspace_id,&store.grant_for_task(&task.id).unwrap().id).unwrap();
    assert_eq!(read()["task"]["responses"][0]["reason"],"authority_unavailable");
    assert!(tauri::async_runtime::block_on(app::invoke(app.handle(),"delegation_read",serde_json::json!({"paneId":"foreign-pane","taskId":task.id}))).is_err());
    store.conn.lock().unwrap().execute("DELETE FROM response_outcomes WHERE task=?1",[&task.id]).unwrap();
    assert_eq!(read()["task"]["responses"][0]["outcome"],"unknown","legacy response became replayable");
    assert_eq!(read()["task"]["responses"][0]["eligible"],false);
}
#[tokio::test]
async fn consent_boundary_original_request_survives_reopen_and_all_decisions() {
    for (kind,decision) in [("command_execution",serde_json::json!({"decision":"allow"})),("user-input",serde_json::json!({"decision":"deny","message":"No"})),("user-input",serde_json::json!({"decision":"cancel"})),("user-input",serde_json::json!({"decision":"allow","updated_input":{"questions":[{"question":"Original?","options":[]}],"nested":{"all":[null,true,1,"exact"]},"answers":{"Original?":"Exact answer"}}}))] {
        for change in ["unchanged","payload","kind","legacy"] {
            let dir=tempfile::TempDir::new().unwrap();let root=dir.path();
            let (app,store,task,pane,_,_)=setup(&root.join("journal"),"consent-original");
            let receiver=crate::remote::tasks::TaskStore::open(&root.join("receiver")).unwrap();receiver.admit(&task.request(None)).unwrap();
            let request=crate::remote::tasks::RemoteApproval {request_id:"opaque/request:01".into(),request_kind:kind.into(),payload:serde_json::json!({"questions":[{"question":"Original?","options":[]}],"nested":{"all":[null,true,1,"exact"]}})};
            receiver.update(&task.id,|t|{t.status=crate::remote::tasks::TaskStatus::AwaitingApproval;t.pending_requests=vec![request.clone()];}).unwrap();
            store.apply_remote(&task.id,&receiver.read(&task.id,0).unwrap()).unwrap();
            let channel=Arc::new(RecoveryCli {cli:PreparedCli {root:root.into(),entered:Default::default(),release:Default::default()},before_send:true,sends:Default::default(),reads:Default::default()});
            app.manage(channel.clone() as Arc<dyn transport::TaskTransport>);
            let args=serde_json::json!({"paneId":pane,"taskId":task.id,"requestId":request.request_id,"originalRequest":request,"decision":decision});
            let first=app::invoke(app.handle(),"delegation_respond",args.clone()).await.unwrap();assert_eq!(first["responses"][0]["outcome"],"before_send");
            if change=="payload" {store.update(&task.id,|t|t.remote.as_mut().unwrap().pending_requests[0].payload["nested"]["all"][3]=serde_json::json!("changed")).unwrap();}
            if change=="kind" {store.update(&task.id,|t|t.remote.as_mut().unwrap().pending_requests[0].request_kind="other-kind".into()).unwrap();}
            if change=="legacy" { // Simulate the exact pre-migration schema, never recapture pending data.
                let conn=store.conn.lock().unwrap();conn.execute_batch("ALTER TABLE response_outcomes RENAME TO bound_responses; CREATE TABLE response_outcomes(task TEXT NOT NULL, request TEXT NOT NULL, decision TEXT NOT NULL, state TEXT NOT NULL, PRIMARY KEY(task,request)); INSERT INTO response_outcomes SELECT task,request,decision,state FROM bound_responses; DROP TABLE bound_responses;").unwrap();
            }
            let restored=app.state::<AppStateStore>().snapshot();drop(app);drop(store);
            let app=tauri::test::mock_app();let state=AppStateStore::default();state.replace_snapshot(restored);
            let db=DatabaseStore::new_in_memory();db.upsert_agent_chat_session("consent-original",&task.parent_workspace_id,Some("/synthetic/parent"),"codex").unwrap();
            db.update_agent_chat_session_config("consent-original",&crate::database::AgentChatSessionConfig {permission_mode:crate::database::AgentChatSessionConfig::set("workspace-write"),..Default::default()}).unwrap();
            assert_eq!(db.insert_host("Fixture","fixture-host").unwrap().id,task.target_host_id);
            let reopened=Arc::new(journal::Journal::open(&root.join("journal")).unwrap());app.manage(state);app.manage(db);app.manage(reopened.clone());app.manage(channel.clone() as Arc<dyn transport::TaskTransport>);
            let read=app::invoke(app.handle(),"delegation_read",serde_json::json!({"paneId":pane,"taskId":task.id})).await.unwrap();
            assert_eq!(read["task"]["responses"][0]["eligible"],change=="unchanged","{kind}/{decision}/{change}: recreated owner recaptured changed/unproven consent");
            if change=="unchanged" {
                app::invoke(app.handle(),"delegation_respond",args.clone()).await.unwrap();
                assert_eq!(receiver.take_responses(&task.id).unwrap().len(),1);
                app::invoke(app.handle(),"delegation_respond",args).await.unwrap();assert!(receiver.take_responses(&task.id).unwrap().is_empty());
            } else {
                assert!(app::invoke(app.handle(),"delegation_respond",args).await.is_err(),"changed/unproven consent dispatched");
                assert_eq!(channel.sends.load(std::sync::atomic::Ordering::SeqCst),1);
                assert!(receiver.read(&task.id,0).unwrap().approval_receipts.is_empty());
                if change=="legacy" {assert_eq!(read["task"]["responses"][0]["outcome"],"unknown");}
            }
        }
    }
}
#[tokio::test]
async fn consent_boundary_initial_display_and_actual_prepare_recheck_full_request() {
    for change in ["prepare-payload","prepare-kind","missing","initial-payload"] {
        let dir=tempfile::TempDir::new().unwrap();let root=dir.path();
        let (app,store,task,pane,_,_)=setup(&root.join("journal"),"consent-prepare");
        let receiver=crate::remote::tasks::TaskStore::open(&root.join("receiver")).unwrap();receiver.admit(&task.request(None)).unwrap();
        let request=crate::remote::tasks::RemoteApproval {request_id:"original".into(),request_kind:"command_execution".into(),payload:serde_json::json!({"command":"original","extra":{"flag":false}})};
        receiver.update(&task.id,|t|{t.status=crate::remote::tasks::TaskStatus::AwaitingApproval;t.pending_requests=vec![request.clone()];}).unwrap();store.apply_remote(&task.id,&receiver.read(&task.id,0).unwrap()).unwrap();
        let channel=Arc::new(PreparedCli {root:root.into(),entered:Default::default(),release:Default::default()});app.manage(channel.clone() as Arc<dyn transport::TaskTransport>);
        let mut args=serde_json::json!({"paneId":pane,"taskId":task.id,"requestId":request.request_id,"originalRequest":request,"decision":{"decision":"allow"}});
        if change=="missing" {args.as_object_mut().unwrap().remove("originalRequest");}
        if change=="initial-payload" {store.update(&task.id,|t|t.remote.as_mut().unwrap().pending_requests[0].payload["command"]=serde_json::json!("replacement")).unwrap();}
        if change.starts_with("prepare-") {
            let h=app.handle().clone();let work=tokio::spawn(async move {app::invoke(&h,"delegation_respond",args).await});
            tokio::time::timeout(Duration::from_secs(5),channel.entered.notified()).await.unwrap();
            store.update(&task.id,|t|{let r=&mut t.remote.as_mut().unwrap().pending_requests[0];if change=="prepare-kind" {r.request_kind="user-input".into();} else {r.payload["extra"]["flag"]=serde_json::json!(true);}}).unwrap();
            channel.release.notify_one();work.await.unwrap().unwrap();
            assert!(!root.join("spawned").exists(),"full consent was not checked after async prepare: {change}");
        } else {
            channel.release.notify_one();let result=app::invoke(app.handle(),"delegation_respond",args).await;
            assert!(result.is_err(),"initial unconfirmed/changed displayed request was admitted: {change}");
        }
        assert!(receiver.read(&task.id,0).unwrap().approval_receipts.is_empty());
    }
}
#[tokio::test]
async fn consent_target_projection_matches_original_admitted_tuple() {
    for attempted in [false,true] { for field in ["checkout","host","ssh","provider","mode","cosmetic"] {
        let dir=tempfile::TempDir::new().unwrap();let root=dir.path();
        let (app,store,task,pane,mut grant,host)=setup(&root.join("journal"),"consent-target");
        let receiver=crate::remote::tasks::TaskStore::open(&root.join("receiver")).unwrap();receiver.admit(&task.request(None)).unwrap();
        let request=crate::remote::tasks::RemoteApproval {request_id:"target".into(),request_kind:"command".into(),payload:serde_json::json!({"cmd":"original"})};
        receiver.update(&task.id,|t|{t.status=crate::remote::tasks::TaskStatus::AwaitingApproval;t.pending_requests=vec![request.clone()];}).unwrap();store.apply_remote(&task.id,&receiver.read(&task.id,0).unwrap()).unwrap();
        let channel=Arc::new(RecoveryCli {cli:PreparedCli {root:root.into(),entered:Default::default(),release:Default::default()},before_send:true,sends:Default::default(),reads:Default::default()});app.manage(channel.clone() as Arc<dyn transport::TaskTransport>);
        let args=serde_json::json!({"paneId":pane,"taskId":task.id,"requestId":request.request_id,"originalRequest":request,"decision":{"decision":"deny","message":"Exact"}});
        if attempted {let before=app::invoke(app.handle(),"delegation_respond",args.clone()).await.unwrap();assert_eq!(before["responses"][0]["outcome"],"before_send");}
        match field {
            "checkout"=>grant.workspace_path="/synthetic/other-checkout".into(),
            "host"=>grant.host_id=app.state::<DatabaseStore>().insert_host("Other","fixture-host").unwrap().id,
            "ssh"=>{grant.ssh_target="other-fixture-host".into();app.state::<DatabaseStore>().update_host(host,"Fixture",&grant.ssh_target).unwrap();},
            "provider"=>{grant.provider=ProviderKind::Claude;grant.permission_mode="default".into();},
            "mode"=>grant.permission_mode="read-only".into(),
            "cosmetic"=>{grant.host_name="Renamed host".into();grant.workspace_name="Renamed checkout".into();},
            _=>unreachable!(),
        }
        store.put_grant(&task.parent_workspace_id,&grant).unwrap();
        let read_args=serde_json::json!({"paneId":pane,"taskId":task.id});
        let read=app::invoke(app.handle(),"delegation_read",read_args.clone()).await.unwrap();
        assert_eq!(read["task"]["responses"][0]["eligible"],field=="cosmetic","new response projection used current rather than original tuple: {field}");
        if field=="cosmetic" {app::invoke(app.handle(),"delegation_respond",args.clone()).await.unwrap();} else {
            assert!(app::invoke(app.handle(),"delegation_respond",args).await.is_err());assert_eq!(channel.sends.load(std::sync::atomic::Ordering::SeqCst),usize::from(attempted));
        }
        receiver.update(&task.id,|t|t.status=crate::remote::tasks::TaskStatus::Completed).unwrap();
        store.apply_remote(&task.id,&receiver.read(&task.id,0).unwrap()).unwrap();
        store.update(&task.id,|t|t.wake_state=WakeState::Held).unwrap();
        store.conn.lock().unwrap().execute("INSERT OR REPLACE INTO delivery_outcomes(task,attempt,state) VALUES(?1,'fixture','before_send')",[&task.id]).unwrap();
        let read=app::invoke(app.handle(),"delegation_read",read_args).await.unwrap();
        assert_eq!(read["task"]["delivery"]["eligible"],field=="cosmetic","sibling delivery projection ignored original tuple: {field}");
        if field!="cosmetic" {assert!(app::invoke(app.handle(),"delegation_deliver",serde_json::json!({"paneId":pane,"taskId":task.id})).await.is_err());}
    }}
}
#[test]
fn contract_oversized_legacy_grant_revoke_disables_and_cancels_without_quota_bypass() {
    let dir=tempfile::TempDir::new().unwrap();
    let (app,store,task,pane,mut grant,host)=setup(dir.path(),"contract-legacy-revoke");
    grant.host_name="x".repeat(20000);
    app.state::<DatabaseStore>().update_host(host,&grant.host_name,&grant.ssh_target).unwrap();
    store.conn.lock().unwrap().execute("UPDATE grants SET data=?2 WHERE id=?1",rusqlite::params![grant.id,serde_json::to_string(&grant).unwrap()]).unwrap();
    assert!(store.put_grant(&task.parent_workspace_id,&grant).is_err(),"enabled grant size bound was lifted");
    assert!(store.disable_grant("foreign-workspace",&grant.id).is_err(),"disable-only transaction crossed grant scope");
    assert!(store.grant_for_task(&task.id).unwrap().enabled);
    store.apply_remote(&task.id,&tests::remote(&task,crate::remote::tasks::TaskStatus::Completed)).unwrap();
    let result=tauri::async_runtime::block_on(app::invoke(app.handle(),"delegation_revoke",serde_json::json!({"paneId":pane,"targetId":grant.id})));
    assert!(result.is_ok(),"actual revoke cannot disable oversized retained metadata: {result:?}");
    let retained=store.grant_for_task(&task.id).unwrap();
    assert!(!retained.enabled);
    assert_eq!(retained.host_name,grant.host_name,"disable changed retained metadata");
    assert!(store.task(&task.id).unwrap().cancel_requested);
    assert!(app::guard_wake(app.handle(),&task.parent_thread_id,Some(&format!("delegation:{}",task.id))).is_err());
}
#[test]
fn contract_idle_native_reads_never_emit_renderer_invalidation() {
    use tauri::Listener;
    let dir = tempfile::TempDir::new().unwrap();
    let (app, store, task, pane, _, _) = setup(dir.path(), "contract-idle-reads");
    let events = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = events.clone();
    let payloads=Arc::new(std::sync::Mutex::new(Vec::<serde_json::Value>::new())); let captured=payloads.clone();
    app.listen("delegation-changed", move |event| { count.fetch_add(1, std::sync::atomic::Ordering::SeqCst); captured.lock().unwrap().push(serde_json::from_str(event.payload()).unwrap()); });
    tauri::async_runtime::block_on(async {
        for command in ["delegation_list", "delegation_grants", "delegation_read"] {
            app::invoke(app.handle(), command, serde_json::json!({"paneId":pane,"taskId":task.id})).await.unwrap();
        }
    });
    assert_eq!(events.load(std::sync::atomic::Ordering::SeqCst), 0, "idle native reads invalidate the renderer and feed its list loop");
    assert_eq!(store.task(&task.id).unwrap().status, task.status);
    let before=tauri::async_runtime::block_on(app::invoke(app.handle(),"delegation_list",serde_json::json!({"paneId":pane}))).unwrap();
    tauri::async_runtime::block_on(app::invoke(app.handle(),"delegation_cancel",serde_json::json!({"paneId":pane,"taskId":task.id}))).unwrap();
    assert_eq!(events.load(std::sync::atomic::Ordering::SeqCst),1,"a durable mutation must invalidate the renderer once");
    let after=tauri::async_runtime::block_on(app::invoke(app.handle(),"delegation_list",serde_json::json!({"paneId":pane}))).unwrap();
    assert_eq!(events.load(std::sync::atomic::Ordering::SeqCst),1,"readback invalidated its own mutation notification");
    if let Ok(path)=std::env::var("CODEMUX_DELEGATION_CONTRACT_CAPTURE") {
        let captured=payloads.lock().unwrap().clone();
        std::fs::write(path,serde_json::to_vec(&serde_json::json!({"pane":pane,"thread":task.parent_thread_id,"idle_list":before,"idle_events":&captured[..0],"mutation_events":&captured,"after_list":after,"after_events":&captured[1..]})).unwrap()).unwrap();
    }
}

#[test]
fn s4_native_canonical_mutation_is_fenced_through_actual_write_syscall() {
    for action in ["database", "pane", "host", "permission", "ordered-pane-db"] {
        let dir=tempfile::TempDir::new().unwrap();
        let thread=format!("atomic-canonical-{action}");
        let (app,store,task,pane,_grant,host_id)=setup(dir.path(),&thread);
        let _authority=authority::NativeAuthority::new(&thread);
        store.apply_remote(&task.id,&tests::remote(&task,crate::remote::tasks::TaskStatus::Completed)).unwrap();
        store.claim_delivery(&task.id).unwrap().unwrap();
        let guard=app::dispatch_guard(app.handle(),&thread,Some(&format!("delegation:{}",task.id))).unwrap();
        let (start_tx,start_rx)=std::sync::mpsc::channel();
        let (done_tx,done_rx)=std::sync::mpsc::channel();
        let h=app.handle().clone(); let t=thread.clone();
        let mutation=std::thread::spawn(move || {
            start_rx.recv().unwrap();
            let db=h.state::<DatabaseStore>();
            let state=h.state::<AppStateStore>();
            match action {
                "database"=>db.upsert_agent_chat_session(&t,"foreign-workspace",None,"codex").unwrap(),
                "pane"=>{state.claim_agent_chat_pane(&pane,ProviderKind::Codex,"replacement",Some(&t)).unwrap();},
                "ordered-pane-db"=>{state.claim_agent_chat_pane_checked(&pane,ProviderKind::Codex,"replacement",Some(&t),|id| {let _=db.get_agent_chat_session(id);Ok(())}).unwrap();},
                "host"=>{db.update_host(host_id,"Fixture","foreign-host").unwrap();},
                "permission"=>db.update_agent_chat_session_config(&t,&crate::database::AgentChatSessionConfig {permission_mode:crate::database::AgentChatSessionConfig::set("read-only"),..Default::default()}).unwrap(),
                _=>unreachable!(),
            }
            done_tx.send(()).unwrap();
        });
        let (mut writer,mut reader)=std::os::unix::net::UnixStream::pair().unwrap();
        let mut blocked=false;
        let poll=guard.poll_first(&mut || {
            start_tx.send(()).unwrap();
            blocked=matches!(done_rx.recv_timeout(Duration::from_millis(120)),Err(std::sync::mpsc::RecvTimeoutError::Timeout));
            std::task::Poll::Ready(std::io::Write::write(&mut writer,b"actual first bytes"))
        }).unwrap();
        mutation.join().unwrap();
        assert!(matches!(poll,std::task::Poll::Ready(Ok(18))));
        let mut bytes=[0;18];std::io::Read::read_exact(&mut reader,&mut bytes).unwrap();
        assert_eq!(&bytes,b"actual first bytes");
        assert!(blocked,"{action} canonical mutation completed after validation but before the actual syscall");
    }
}


struct PreparedCli {
    root: std::path::PathBuf,
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}
#[async_trait::async_trait]
impl transport::TaskTransport for PreparedCli {
    async fn call(&self,_target:&str,op:transport::Operation,input:Option<serde_json::Value>)->Result<serde_json::Value,String> {self.perform(op,input,None).await.map_err(|e|e.to_string())}
    async fn call_guarded(&self,_target:&str,op:transport::Operation,input:Option<serde_json::Value>,guard:Arc<dyn crate::json_rpc_child::dispatch::DispatchGuard>)->Result<serde_json::Value,transport::TransportError> {self.perform(op,input,Some(guard)).await}
}
impl PreparedCli {
    async fn perform(&self,op:transport::Operation,input:Option<serde_json::Value>,guard:Option<Arc<dyn crate::json_rpc_child::dispatch::DispatchGuard>>)->Result<serde_json::Value,transport::TransportError> {
        self.entered.notify_one();self.release.notified().await;
        let binary=std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/debug/codemux-remote");
        let launcher=self.root.join("cli-launcher");
        std::fs::write(&launcher,format!("#!/bin/sh\nprintf 'spawned\\n' >> '{}'\nexec '{}' \"$@\"\n",self.root.join("spawned").display(),binary.display())).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&launcher,std::fs::Permissions::from_mode(0o700)).unwrap();
        let mut cmd=tokio::process::Command::new(launcher);
        let action=match &op {transport::Operation::Launch=>"launch",transport::Operation::Respond {..}=>"respond",transport::Operation::Read {..}=>"read",_=>panic!("fixture only writes or reconciles")};
        cmd.args(["task",action,"--state-dir"]).arg(self.root.join("receiver"));
        match op { transport::Operation::Respond{id} => {cmd.args(["--id",&id]);}, transport::Operation::Read{id,cursor} => {cmd.args(["--id",&id,"--after",&cursor.to_string()]);}, _ => {} }
        cmd.env("HOME",self.root.join("home")).env("XDG_CONFIG_HOME",self.root.join("home/config")).env("XDG_DATA_HOME",self.root.join("home/data")).env("CODEX_HOME",self.root.join("home/codex")).env("PATH","/usr/bin:/bin").env("CODEMUX_CLAUDE_SIDECAR_PATH",self.root.join("absent-sidecar"));
        cmd.stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::piped()).kill_on_drop(true);
        let bytes=serde_json::to_vec(&input.unwrap_or(serde_json::Value::Null)).unwrap();
        transport::dispatch_command(cmd,bytes,guard).await
    }
}
#[tokio::test]
async fn contract_user_input_refuses_bare_allow_before_receiver_callback() {
    let dir=tempfile::TempDir::new().unwrap(); let root=dir.path();
    let (app,store,task,pane,_,_)=setup(&root.join("journal"),"contract-question");
    let receiver=crate::remote::tasks::TaskStore::open(&root.join("receiver")).unwrap(); receiver.admit(&task.request(None)).unwrap();
    receiver.update(&task.id,|t|{t.status=crate::remote::tasks::TaskStatus::AwaitingApproval;t.pending_requests.push(crate::remote::tasks::RemoteApproval {request_id:"question".into(),request_kind:"user-input".into(),payload:serde_json::json!({"questions":[{"question":"Which route?","options":[{"label":"Safe"}]}]})});}).unwrap();
    store.apply_remote(&task.id,&receiver.read(&task.id,0).unwrap()).unwrap();
    let channel=Arc::new(PreparedCli {root:root.into(),entered:Default::default(),release:Default::default()}); channel.release.notify_one();
    app.manage(channel.clone() as Arc<dyn transport::TaskTransport>);
    let result=app::invoke(app.handle(),"delegation_respond",serde_json::json!({"paneId":pane,"taskId":task.id,"requestId":"question","originalRequest":receiver.snapshot(&task.id).unwrap().pending_requests[0],"decision":{"decision":"allow"}})).await;
    assert!(result.is_err(),"native accepted unanswered AskUserQuestion: {result:?}");
    assert_eq!(receiver.read(&task.id,0).unwrap().approval_receipts.len(),0);
    let original=receiver.snapshot(&task.id).unwrap().pending_requests[0].payload.clone();
    let mut updated=original.clone(); updated["answers"]=serde_json::json!({"Which route?":"Safe"});
    let decision=serde_json::json!({"decision":"allow","updated_input":updated});
    let args=serde_json::json!({"paneId":pane,"taskId":task.id,"requestId":"question","originalRequest":receiver.snapshot(&task.id).unwrap().pending_requests[0],"decision":decision});
    app::invoke(app.handle(),"delegation_respond",args.clone()).await.unwrap();
    let callbacks=receiver.take_responses(&task.id).unwrap();
    assert_eq!(callbacks.len(),1);
    // Native ApprovalDecision serialization includes its optional null field.
    let expected=serde_json::to_value(serde_json::from_value::<crate::agent_provider::ApprovalDecision>(decision).unwrap()).unwrap();
    assert_eq!(serde_json::to_value(&callbacks[0].decision).unwrap(),expected,"answer-bearing native receiver callback claim changed payload");
    channel.release.notify_one();
    app::invoke(app.handle(),"delegation_respond",args).await.unwrap();
    assert!(receiver.take_responses(&task.id).unwrap().is_empty(),"reconciliation replayed the answer callback");
}
struct RecoveryCli {
    cli: PreparedCli,
    before_send: bool,
    sends: std::sync::atomic::AtomicUsize,
    reads: std::sync::atomic::AtomicUsize,
}
#[async_trait::async_trait]
impl transport::TaskTransport for RecoveryCli {
    async fn call(&self,_target:&str,op:transport::Operation,input:Option<serde_json::Value>)->Result<serde_json::Value,String> {
        self.reads.fetch_add(1,std::sync::atomic::Ordering::SeqCst);
        self.cli.release.notify_one();self.cli.perform(op,input,None).await.map_err(|e|e.to_string())
    }
    async fn call_guarded(&self,_target:&str,op:transport::Operation,input:Option<serde_json::Value>,guard:Arc<dyn crate::json_rpc_child::dispatch::DispatchGuard>)->Result<serde_json::Value,transport::TransportError> {
        let attempt=self.sends.fetch_add(1,std::sync::atomic::Ordering::SeqCst);
        if attempt==0 && self.before_send {return Err(transport::TransportError::BeforeSend("Fixture refused before any spawn/write".into()));}
        self.cli.release.notify_one();let read=self.cli.perform(op,input,Some(guard)).await?;
        if attempt==0 {return Err(transport::TransportError::Unknown("Receiver accepted, acknowledgement lost".into()));}
        Ok(read)
    }
}
#[tokio::test]
async fn ui_recovery_actual_native_response_before_send_retry_and_unknown_read_only() {
    let mut captures=Vec::new();
    for before_send in [true,false] {
        let dir=tempfile::TempDir::new().unwrap();let root=dir.path();
        let (app,store,task,pane,_,_)=setup(&root.join("journal"),if before_send {"ui-recovery-before"} else {"ui-recovery-unknown"});
        let receiver=crate::remote::tasks::TaskStore::open(&root.join("receiver")).unwrap();receiver.admit(&task.request(None)).unwrap();
        let payload=serde_json::json!({"questions":[{"question":"Exact answer?","options":[]}],"hint":"Original native payload"});
        receiver.update(&task.id,|t|{t.status=crate::remote::tasks::TaskStatus::AwaitingApproval;t.pending_requests.push(crate::remote::tasks::RemoteApproval {request_id:"recovery".into(),request_kind:"user-input".into(),payload:payload.clone()});}).unwrap();
        store.apply_remote(&task.id,&receiver.read(&task.id,0).unwrap()).unwrap();
        let channel=Arc::new(RecoveryCli {cli:PreparedCli {root:root.into(),entered:Default::default(),release:Default::default()},before_send,sends:Default::default(),reads:Default::default()});
        app.manage(channel.clone() as Arc<dyn transport::TaskTransport>);
        let mut updated=payload;updated["answers"]=serde_json::json!({"Exact answer?":"Native immutable answer"});
        let decision=serde_json::json!({"decision":"allow","updated_input":updated});
        let args=serde_json::json!({"paneId":pane,"taskId":task.id,"requestId":"recovery","originalRequest":receiver.snapshot(&task.id).unwrap().pending_requests[0],"decision":decision});
        let first=app::invoke(app.handle(),"delegation_respond",args.clone()).await.unwrap();
        assert!(first["connection_error"].is_string(),"Ok task must expose the failed response diagnostic");
        assert_eq!(first["responses"][0]["outcome"],if before_send {"before_send"} else {"unknown"});
        assert_eq!(first["responses"][0]["eligible"],before_send);
        let first_read=app::invoke(app.handle(),"delegation_read",serde_json::json!({"paneId":pane,"taskId":task.id})).await.unwrap();
        assert_eq!(first_read["task"]["responses"],first["responses"],"local qualified read lost immutable decision/outcome");
        app::invoke(app.handle(),"delegation_respond",args.clone()).await.unwrap();
        let claimed=receiver.take_responses(&task.id).unwrap();assert_eq!(claimed.len(),1);
        assert_eq!(serde_json::to_value(&claimed[0].decision).unwrap(),first["responses"][0]["decision"]);
        app::invoke(app.handle(),"delegation_respond",args.clone()).await.unwrap();
        assert!(receiver.take_responses(&task.id).unwrap().is_empty(),"accepted/unknown reconciliation replayed callback");
        assert_eq!(channel.sends.load(std::sync::atomic::Ordering::SeqCst),if before_send {2} else {1});
        let after_read=app::invoke(app.handle(),"delegation_read",serde_json::json!({"paneId":pane,"taskId":task.id})).await.unwrap();
        assert_eq!(after_read["task"]["responses"][0]["outcome"],"accepted");
        assert_eq!(after_read["task"]["responses"][0]["eligible"],false);
        captures.push(serde_json::json!({"before_send":before_send,"pane":pane,"thread":task.parent_thread_id,"first":first,"first_read":first_read,"after_read":after_read,"args":args,"sends":channel.sends.load(std::sync::atomic::Ordering::SeqCst),"reads":channel.reads.load(std::sync::atomic::Ordering::SeqCst)}));
    }
    if let Ok(path)=std::env::var("CODEMUX_UI_RECOVERY_CAPTURE") {std::fs::write(path,serde_json::to_vec_pretty(&captures).unwrap()).unwrap();}
}
#[tokio::test]
async fn contract_manual_respond_shutdown_joins_actual_cli_and_rejects_postclose() {
    for drop_caller in [false,true] { owned_respond_case(drop_caller).await; }
}
async fn owned_respond_case(drop_caller:bool) {
    let dir=tempfile::TempDir::new().unwrap(); let root=dir.path();
    let (app,store,task,pane,_,_)=setup(&root.join("journal"),"contract-owned-respond");
    let receiver=crate::remote::tasks::TaskStore::open(&root.join("receiver")).unwrap();
    receiver.admit(&task.request(None)).unwrap();
    receiver.update(&task.id,|t|{t.status=crate::remote::tasks::TaskStatus::AwaitingApproval;t.pending_requests.push(crate::remote::tasks::RemoteApproval {request_id:"approval".into(),request_kind:"permission".into(),payload:serde_json::Value::Null});}).unwrap();
    store.apply_remote(&task.id,&receiver.read(&task.id,0).unwrap()).unwrap();
    let channel=Arc::new(PreparedCli {root:root.into(),entered:Default::default(),release:Default::default()});
    let coordinator=coordinator::Coordinator::new(channel.clone()); app.manage(coordinator.clone());
    app.manage(channel.clone() as Arc<dyn transport::TaskTransport>);
    let gate=Arc::new(crate::json_rpc_child::TestWriteGate {partial:false,entered:Default::default(),released:Default::default(),waker:Default::default()});
    transport::TEST_TRANSPORT_WRITES.lock().unwrap().insert(root.join("cli-launcher"),gate.clone());
    let args=serde_json::json!({"paneId":pane,"taskId":task.id,"requestId":"approval","originalRequest":{"request_id":"approval","request_kind":"permission","payload":null},"decision":{"decision":"deny","message":"fixture"}});
    let h=app.handle().clone(); let a=args.clone();
    let responding=tokio::spawn(async move {app::invoke(&h,"delegation_respond",a).await});
    tokio::time::timeout(Duration::from_secs(5),channel.entered.notified()).await.unwrap();
    channel.release.notify_one();
    tokio::time::timeout(Duration::from_secs(5),gate.entered.notified()).await.unwrap();
    let c=coordinator.clone(); let mut closing=tokio::spawn(async move {c.shutdown().await});
    if drop_caller { responding.abort(); }
    let waited=tokio::time::timeout(Duration::from_millis(100),&mut closing).await.is_err();
    gate.release(); if drop_caller {let _=responding.await;} else {responding.await.unwrap().unwrap();}
    if waited { closing.await.unwrap().unwrap(); }
    transport::TEST_TRANSPORT_WRITES.lock().unwrap().remove(&root.join("cli-launcher"));
    let pid=transport::TEST_TRANSPORT_SPAWNED.lock().unwrap().remove(&root.join("cli-launcher")).unwrap();
    channel.release.notify_one();
    let postclose=app::invoke(app.handle(),"delegation_respond",args).await;
    assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists(),"shutdown returned before actual CLI reap");
    assert!(waited,"desktop Respond escaped shutdown ownership while native transport was Pending");
    assert!(postclose.is_err(),"Respond was admitted after closure");
    assert_eq!(receiver.read(&task.id,0).unwrap().approval_receipts.len(),1);
    assert_eq!(coordinator.owner_counts(),(0,0,false));
}
#[tokio::test]
async fn contract_manual_respond_actual_admission_is_bounded_and_survives_all_rpc_drops() {
    let dir=tempfile::TempDir::new().unwrap(); let root=dir.path();
    let (app,store,task,pane,_,_)=setup(&root.join("journal"),"contract-respond-capacity");
    let receiver=crate::remote::tasks::TaskStore::open(&root.join("receiver")).unwrap(); receiver.admit(&task.request(None)).unwrap();
    receiver.update(&task.id,|t|{t.status=crate::remote::tasks::TaskStatus::AwaitingApproval;t.pending_requests=(0..9).map(|n|crate::remote::tasks::RemoteApproval {request_id:format!("approval-{n}"),request_kind:"permission".into(),payload:serde_json::Value::Null}).collect();}).unwrap();
    store.apply_remote(&task.id,&receiver.read(&task.id,0).unwrap()).unwrap();
    let channel=Arc::new(PreparedCli {root:root.into(),entered:Default::default(),release:Default::default()});
    let coordinator=coordinator::Coordinator::new(channel.clone()); app.manage(coordinator.clone()); app.manage(channel.clone() as Arc<dyn transport::TaskTransport>);
    let mut callers=Vec::new();
    for n in 0..8 {
        let h=app.handle().clone(); let args=serde_json::json!({"paneId":pane,"taskId":task.id,"requestId":format!("approval-{n}"),"originalRequest":{"request_id":format!("approval-{n}"),"request_kind":"permission","payload":null},"decision":{"decision":"deny","message":"fixture"}});
        callers.push(tokio::spawn(async move {app::invoke(&h,"delegation_respond",args).await}));
    }
    tokio::time::timeout(Duration::from_secs(5),async {
        loop {let claimed:i64=store.conn.lock().unwrap().query_row("SELECT COUNT(*) FROM response_outcomes WHERE state='in_flight'",[],|r|r.get(0)).unwrap(); if claimed==8 {break;} tokio::task::yield_now().await;}
    }).await.unwrap();
    assert_eq!(coordinator.owner_counts().1,8,"manual effects did not consume the production step quota");
    let ninth=app::invoke(app.handle(),"delegation_respond",serde_json::json!({"paneId":pane,"taskId":task.id,"requestId":"approval-8","originalRequest":{"request_id":"approval-8","request_kind":"permission","payload":null},"decision":{"decision":"deny","message":"fixture"}})).await;
    assert!(ninth.is_err(),"ninth desktop effect escaped bounded admission");
    for caller in callers {caller.abort();let _=caller.await;}
    let c=coordinator.clone(); let mut closing=tokio::spawn(async move {c.shutdown().await});
    assert!(tokio::time::timeout(Duration::from_millis(100),&mut closing).await.is_err(),"RPC drops released real effects before durable outcome/reap");
    for n in 1..=8 {
        channel.release.notify_one();
        tokio::time::timeout(Duration::from_secs(5),async {loop {let accepted:i64=store.conn.lock().unwrap().query_row("SELECT COUNT(*) FROM response_outcomes WHERE state='accepted'",[],|r|r.get(0)).unwrap();if accepted>=n {break;}tokio::task::yield_now().await;}}).await.unwrap();
    }
    closing.await.unwrap().unwrap();
    assert_eq!(receiver.read(&task.id,0).unwrap().approval_receipts.len(),8);
    assert_eq!(coordinator.owner_counts(),(0,0,false));
}
#[tokio::test]
async fn s4_launch_after_async_transport_prepare_refuses_stale_authority() {
    for action in ["database","pane","cancel","revoke","host","permission"] {prepared_case(false,action).await;}
}
#[tokio::test]
async fn s4_respond_after_async_transport_prepare_refuses_stale_authority() {
    for action in ["database","pane","cancel","revoke","host","permission"] {prepared_case(true,action).await;}
}
#[tokio::test]
async fn s4_transport_pending_first_poll_rechecks_canonical_and_cancel_revoke() {
    for respond in [false,true] {for action in ["pending-database","pending-pane","pending-host","pending-permission","pending-cancel","pending-revoke"] {prepared_case(respond,action).await;}}
}
#[tokio::test]
async fn s4_transport_real_respond_positive_control_enqueues_exact_receipt() {
    prepared_case(true,"unchanged").await;
}
async fn prepared_case(respond:bool,action:&str) {
    let dir=tempfile::TempDir::new().unwrap();let root=dir.path();
    let thread=format!("prepared-{respond}-{action}");
    let (app,store,task,pane,mut grant,host_id)=setup(&root.join("journal"),&thread);
    let receiver=crate::remote::tasks::TaskStore::open(&root.join("receiver")).unwrap();
    if respond {
        receiver.admit(&task.request(None)).unwrap();
        receiver.update(&task.id,|t| {t.status=crate::remote::tasks::TaskStatus::AwaitingApproval;t.pending_requests.push(crate::remote::tasks::RemoteApproval {request_id:"approval".into(),request_kind:"command_execution".into(),payload:serde_json::Value::Null});}).unwrap();
        store.apply_remote(&task.id,&receiver.read(&task.id,0).unwrap()).unwrap();
    }
    let channel=Arc::new(PreparedCli {root:root.into(),entered:Default::default(),release:Default::default()});
    let pending=action.starts_with("pending-");
    let write_gate=Arc::new(crate::json_rpc_child::TestWriteGate {partial:false,entered:Default::default(),released:Default::default(),waker:Default::default()});
    if pending {transport::TEST_TRANSPORT_WRITES.lock().unwrap().insert(root.join("cli-launcher"),write_gate.clone());}
    app.manage(channel.clone() as Arc<dyn transport::TaskTransport>);
    let h=app.handle().clone();let id=task.id.clone();let p=pane.clone();let c=channel.clone();
    let work=tokio::spawn(async move {
        if respond {app::invoke(&h,"delegation_respond",serde_json::json!({"paneId":p,"taskId":id,"requestId":"approval","originalRequest":{"request_id":"approval","request_kind":"command_execution","payload":null},"decision":{"decision":"deny","message":"fixture"}})).await.map(|_|())}
        else {coordinator::Coordinator::new(c).step(&h,&id).await.map(|_|())}
    });
    tokio::time::timeout(Duration::from_secs(5),channel.entered.notified()).await.unwrap();
    if pending {
        channel.release.notify_one();
        tokio::time::timeout(Duration::from_secs(5),write_gate.entered.notified()).await.unwrap();
    }
    match action.strip_prefix("pending-").unwrap_or(action) {
        "database"=>app.state::<DatabaseStore>().upsert_agent_chat_session(&thread,"foreign-workspace",None,"codex").unwrap(),
        "pane"=>{app.state::<AppStateStore>().claim_agent_chat_pane(&pane,ProviderKind::Codex,"other-thread",Some(&thread)).unwrap();},
        "cancel"=>{store.cancel(&task.id).unwrap();},
        "revoke"=>{grant.enabled=false;store.put_grant(&task.parent_workspace_id,&grant).unwrap();},
        "host"=>{app.state::<DatabaseStore>().update_host(host_id,"Fixture","foreign-host").unwrap();},
        "permission"=>app.state::<DatabaseStore>().update_agent_chat_session_config(&thread,&crate::database::AgentChatSessionConfig {permission_mode:crate::database::AgentChatSessionConfig::set("read-only"),..Default::default()}).unwrap(),
        "unchanged"=>{},
        _=>unreachable!(),
    }
    if pending && !matches!(action,"pending-cancel"|"pending-revoke") {write_gate.release();}
    if !pending {channel.release.notify_one();}
    let _=tokio::time::timeout(Duration::from_secs(10),work).await.unwrap().unwrap();
    transport::TEST_TRANSPORT_WRITES.lock().unwrap().remove(&root.join("cli-launcher"));
    if pending {
        let pid=transport::TEST_TRANSPORT_SPAWNED.lock().unwrap().remove(&root.join("cli-launcher")).expect("production spawn executed before Pending");
        assert!(!std::path::Path::new(&format!("/proc/{pid}")).exists(),"guard refusal returned before owned transport reap");
    } else {
        assert_eq!(std::fs::read_to_string(root.join("spawned")).unwrap_or_default().lines().count(),usize::from(action=="unchanged"),"{respond}/{action} passed stale transport admission after async preparation");
    }
    let conn=store.conn.lock().unwrap();
    let ledger:String=if respond {conn.query_row("SELECT state FROM response_outcomes WHERE task=?1 AND request='approval'",[&task.id],|r|r.get(0)).unwrap()} else {conn.query_row("SELECT state FROM launch_outcomes WHERE task=?1",[&task.id],|r|r.get(0)).unwrap()};
    assert_eq!(ledger,if action=="unchanged" {"accepted"} else if pending {"unknown"} else {"before_send"},"phase classification must remain causal");
    if respond {assert_eq!(receiver.read(&task.id,0).unwrap().approval_receipts.len(),usize::from(action=="unchanged"));}
}


#[derive(Debug)]
struct CheckedSyscall {
    guard:Arc<dyn crate::json_rpc_child::dispatch::DispatchGuard>,
    skip:std::sync::atomic::AtomicUsize,
    start:std::sync::Mutex<Option<std::sync::mpsc::Sender<()>>>,
    done:std::sync::Mutex<std::sync::mpsc::Receiver<()>>,
    blocked:std::sync::atomic::AtomicBool,
    positive:std::sync::atomic::AtomicBool,
}
impl crate::json_rpc_child::dispatch::DispatchGuard for CheckedSyscall {
    fn register(&self,w:&std::task::Waker) {self.guard.register(w);}
    fn poll_first(&self,write:&mut dyn FnMut()->crate::json_rpc_child::dispatch::WritePoll)->Result<crate::json_rpc_child::dispatch::WritePoll,String> {
        self.guard.poll_first(&mut || {
            use std::sync::atomic::Ordering::SeqCst;
            if self.skip.load(SeqCst)==0 {
                if let Some(start)=self.start.lock().unwrap().take() {
                    start.send(()).unwrap();
                    self.blocked.store(matches!(self.done.lock().unwrap().recv_timeout(Duration::from_millis(120)),Err(std::sync::mpsc::RecvTimeoutError::Timeout)),SeqCst);
                }
            }
            if self.skip.load(SeqCst)>0 {self.skip.fetch_sub(1,SeqCst);}
            let poll=write();
            if matches!(poll,std::task::Poll::Ready(Ok(n)) if n>0) {self.positive.store(true,SeqCst);}
            poll
        })
    }
}
#[tokio::test]
async fn s4_actual_native_and_transport_stdin_keep_canonical_leases_through_positive_write() {
    use std::sync::atomic::Ordering::SeqCst;
    for native in [true,false] {
        for action in ["database","ordered-pane-db","host","permission","cancel","revoke"] {
            let dir=tempfile::TempDir::new().unwrap();let root=dir.path();
            let thread=format!("syscall-owner-{native}-{action}");
            let (app,store,task,pane,mut grant,host_id)=setup(&root.join("journal"),&thread);
            let _authority=authority::NativeAuthority::new(&thread);
            let inner=if native {
                store.apply_remote(&task.id,&tests::remote(&task,crate::remote::tasks::TaskStatus::Completed)).unwrap();
                store.claim_delivery(&task.id).unwrap().unwrap();
                app::dispatch_guard(app.handle(),&thread,Some(&format!("delegation:{}",task.id))).unwrap()
            } else {
                store.mark_attempted(&task.id).unwrap();
                app::remote_dispatch_guard(app.handle(),&store,&task,None)
            };
            let (start_tx,start_rx)=std::sync::mpsc::channel();let (done_tx,done_rx)=std::sync::mpsc::channel();
            let h=app.handle().clone();let s=store.clone();let t=thread.clone();let id=task.id.clone();let ws=task.parent_workspace_id.clone();
            let mutation=std::thread::spawn(move || {
                start_rx.recv_timeout(Duration::from_secs(10)).unwrap();
                match action {
                    "database"=>h.state::<DatabaseStore>().upsert_agent_chat_session(&t,"foreign-workspace",None,"codex").unwrap(),
                    "ordered-pane-db"=>{h.state::<AppStateStore>().claim_agent_chat_pane_checked(&pane,ProviderKind::Codex,"replacement",Some(&t),|id| {let _=h.state::<DatabaseStore>().get_agent_chat_session(id);Ok(())}).unwrap();},
                    "host"=>{h.state::<DatabaseStore>().update_host(host_id,"Fixture","foreign-host").unwrap();},
                    "permission"=>h.state::<DatabaseStore>().update_agent_chat_session_config(&t,&crate::database::AgentChatSessionConfig {permission_mode:crate::database::AgentChatSessionConfig::set("read-only"),..Default::default()}).unwrap(),
                    "cancel"=>{s.cancel(&id).unwrap();},
                    "revoke"=>{grant.enabled=false;s.put_grant(&ws,&grant).unwrap();},
                    _=>unreachable!(),
                }
                done_tx.send(()).unwrap();
            });
            let fenced=Arc::new(CheckedSyscall {guard:inner,skip:std::sync::atomic::AtomicUsize::new(if native {1} else {2}),start:std::sync::Mutex::new(Some(start_tx)),done:std::sync::Mutex::new(done_rx),blocked:Default::default(),positive:Default::default()});
            if native {
                let child=crate::json_rpc_child::JsonRpcChild::spawn(crate::json_rpc_child::SpawnConfig {program:std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("target/debug/fake_claude_sidecar"),args:vec![],env:Default::default(),cwd:Some(root.into()),default_timeout:Duration::from_secs(5)}).await.unwrap();
                let response=child.request_guarded("ping",serde_json::json!({"owner":"native"}),&*fenced).await;
                child.shutdown().await.unwrap();
                assert!(response.is_ok(),"real native pipe did not finish its admitted frame: {response:?}");
            } else {
                let mut command=tokio::process::Command::new("/usr/bin/python3");
                command.args(["-I","-c","import sys,json; data=json.load(sys.stdin); print(json.dumps(data))"]);
                let response=transport::dispatch_command(command,b"{\"owner\":\"transport\"}".to_vec(),Some(fenced.clone())).await;
                assert!(response.is_ok(),"actual shared transport owner did not finish admitted frame: {response:?}");
            }
            mutation.join().unwrap();
            assert!(fenced.blocked.load(SeqCst),"{native}/{action} mutated between final validation and actual positive ChildStdin poll");
            assert!(fenced.positive.load(SeqCst),"must observe actual positive ChildStdin write, not a Pending-only schedule");
        }
    }
}
