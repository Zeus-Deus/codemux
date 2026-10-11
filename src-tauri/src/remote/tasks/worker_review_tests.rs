use super::*;
use crate::agent_provider::{claude::{ClaudeAgentProvider, ClaudeProviderConfig}, ApprovalDecision, ProviderKind};
use serde_json::{json, Value};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

async fn claude_fixture(dir: &tempfile::TempDir, script: Value, extra_env: &str) -> Arc<dyn AgentProvider> {
    let helper = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/debug/fake_claude_sidecar");
    assert!(helper.is_file());
    let path = dir.path().join("review-claude-script.json");
    std::fs::write(&path, serde_json::to_vec(&script).unwrap()).unwrap();
    let wrapper = dir.path().join("review-sidecar");
    std::fs::write(&wrapper, format!("#!/bin/sh\nexport FAKE_CLAUDE_SIDECAR_SCRIPT='{}'\nexport FAKE_CLAUDE_SIDECAR_CAPTURE='{}'\n{}\nexec '{}' \"$@\"\n", path.display(), dir.path().join("claude-calls.jsonl").display(), extra_env, helper.display())).unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o700)).unwrap();
    Arc::new(ClaudeAgentProvider::new(ClaudeProviderConfig {
        sidecar_binary: Some(wrapper), claude_binary: Some("/fixture/claude".into()), ..Default::default()
    }).await.unwrap())
}
fn claude_task(dir: &tempfile::TempDir) -> (Arc<TaskStore>, String, String) {
    let store = Arc::new(TaskStore::open(dir.path()).unwrap());
    let mut request = super::super::tests::request();
    request.provider = ProviderKind::Claude;
    request.permission_mode = "default".into();
    request.workspace_path = dir.path().to_string_lossy().into();
    store.admit(&request).unwrap();
    let thread = store.snapshot(&request.id).unwrap().child_thread_id;
    (store, request.id, thread)
}
#[tokio::test]
async fn r1_native_claude_before_ack_approval_keeps_original_empty_id() {
    let dir = tempfile::TempDir::new().unwrap();
    let (store, id, thread) = claude_task(&dir);
    let provider = claude_fixture(&dir, json!([
        {"after":"send-turn","emit":"notification","method":"request-opened","params":{"threadId":thread,"requestId":"early-A","toolName":"Bash","toolInput":{"command":"fixture"},"kind":"command"}},
        {"after":"respond-to-request","emit":"notification","method":"sdk-message","params":{"threadId":thread,"message":{"type":"assistant","message":{"content":[{"type":"text","text":"approved final"}]}}}},
        {"after":"respond-to-request","delay_ms":10,"emit":"notification","method":"sdk-message","params":{"threadId":thread,"message":{"type":"result","subtype":"success"}}}
    ]), "export FAKE_CLAUDE_SEND_ACK_DELAY_MS=250").await;
    let worker = tokio::spawn({let store=store.clone();let id=id.clone(); async move {execute(store,&id,provider,"fixture-workspace".into()).await}});
    // Wait on the actual observed callback, not an assumed startup duration.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    let observed = loop {
        let snapshot = store.snapshot(&id).unwrap();
        if !snapshot.pending_requests.is_empty()
            || snapshot.status.is_terminal()
            || tokio::time::Instant::now() >= deadline
        {
            break snapshot;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    };
    let pending = observed.pending_requests.clone();
    if pending.is_empty() {store.cancel(&id).unwrap();} else {
        store.respond(&id,&super::super::RespondRequest {original_request:Some(pending[0].clone()),request_id:"early-A".into(), decision:ApprovalDecision::Allow { updated_input: None, updated_permissions: None }}).unwrap();
    }
    let outcome = tokio::time::timeout(Duration::from_secs(15),worker).await.unwrap().unwrap();
    assert_eq!(pending.len(),1,"genuine before-ack native callback must be pending: {observed:?}; calls: {:?}",std::fs::read_to_string(dir.path().join("claude-calls.jsonl")));
    assert!(outcome.is_ok(),"{outcome:?}");
    let read=store.read(&id,0).unwrap();
    assert_eq!(read.task.status,TaskStatus::Completed);
    assert_eq!(read.task.result.as_deref(),Some("approved final"));
    let approval=read.events.iter().find(|e|e.event["type"]=="request_opened").unwrap();
    assert_eq!(approval.event["turn_id"],"","journal must not fabricate native event IDs");
}
#[tokio::test]
async fn r1_native_claude_before_ack_result_and_completion_bind_fresh_run() {
    let dir=tempfile::TempDir::new().unwrap();
    let (store,id,thread)=claude_task(&dir);
    let provider=claude_fixture(&dir,json!([
        {"after":"send-turn","emit":"notification","method":"sdk-message","params":{"threadId":thread,"message":{"type":"assistant","message":{"content":[{"type":"text","text":"early final"}]}}}},
        {"after":"send-turn","delay_ms":10,"emit":"notification","method":"sdk-message","params":{"threadId":thread,"message":{"type":"result","subtype":"success"}}}
    ]),"export FAKE_CLAUDE_SEND_ACK_DELAY_MS=250").await;
    let result=execute(store.clone(),&id,provider,"fixture-workspace".into()).await;
    assert!(result.is_ok(),"{result:?}");
    let read=store.read(&id,0).unwrap();
    assert_eq!(read.task.result.as_deref(),Some("early final"));
    assert_eq!(read.task.status,TaskStatus::Completed);
    for event in read.events.iter().filter(|e|matches!(e.event["type"].as_str(),Some("item_completed"|"turn_completed"))) {assert_eq!(event.event["turn_id"],"");}
}
#[tokio::test]
async fn r3_native_claude_async_session_error_after_ack_is_interrupted_without_replay() {
    let dir = tempfile::TempDir::new().unwrap();
    let (store, id, thread) = claude_task(&dir);
    let gate = dir.path().join("release-session-error");
    let provider = claude_fixture(&dir, json!([
        {"after":"send-turn","wait_for":gate,"emit":"notification","method":"session-error","params":{"threadId":thread,"error":"iterator failed after possible tool effects"}}
    ]), "").await;
    let worker = tokio::spawn({ let store=store.clone(); let id=id.clone(); let provider=provider.clone(); async move {execute(store,&id,provider,"fixture-workspace".into()).await} });
    tokio::time::timeout(Duration::from_secs(5), async {
        while store.snapshot(&id).unwrap().turn_id.is_none() { tokio::time::sleep(Duration::from_millis(5)).await; }
    }).await.unwrap();
    assert_eq!(store.snapshot(&id).unwrap().status, TaskStatus::Running);
    std::fs::write(gate, "ack was consumed").unwrap();
    let result = tokio::time::timeout(Duration::from_secs(15),worker).await.unwrap().unwrap();
    assert!(result.is_err());
    assert!(provider.list_sessions().await.unwrap().is_empty());
    let read = store.read(&id,0).unwrap();
    assert!(!read.events.iter().any(|e| e.event["type"]=="turn_completed"), "no correlated terminal receipt exists");
    assert_eq!(read.task.status, TaskStatus::Interrupted, "session failure is not proof of native rejection");
    let reopened = Arc::new(TaskStore::open(dir.path()).unwrap());
    assert!(execute(reopened.clone(),&id,provider,"fixture-workspace".into()).await.is_err());
    assert_eq!(reopened.snapshot(&id).unwrap().status, TaskStatus::Interrupted);
    let calls = std::fs::read_to_string(dir.path().join("claude-calls.jsonl")).unwrap();
    assert_eq!(calls.lines().filter(|l|serde_json::from_str::<Value>(l).unwrap()["method"]=="send-turn").count(),1);
}

#[tokio::test]
async fn r3_native_claude_correlated_failure_remains_failed() {
    let dir = tempfile::TempDir::new().unwrap();
    let (store,id,thread) = claude_task(&dir);
    let provider = claude_fixture(&dir,json!([
        {"after":"send-turn","emit":"notification","method":"sdk-message","params":{"threadId":thread,"message":{"type":"result","subtype":"error_during_execution","is_error":true,"errors":["correlated error"]}}}
    ]), "").await;
    assert!(execute(store.clone(),&id,provider,"fixture-workspace".into()).await.is_err());
    assert_eq!(store.snapshot(&id).unwrap().status,TaskStatus::Failed);
}

#[tokio::test]
async fn r3_pre_native_checkout_failure_remains_failed() {
    let dir = tempfile::TempDir::new().unwrap();
    let (store,id,_thread) = claude_task(&dir);
    let provider = claude_fixture(&dir,json!([]), "").await;
    let checkout = crate::remote::tasks::checkout::CheckoutIdentity::capture("bound-workspace".into(),dir.path()).unwrap();
    let request = store.snapshot(&id).unwrap().request;
    // A fresh bound admission exercises the actual pre-native identity check.
    let bound_dir = tempfile::TempDir::new().unwrap();
    let store = Arc::new(TaskStore::open(bound_dir.path()).unwrap());
    store.admit_bound(&request,Some(&checkout)).unwrap();
    assert!(execute(store.clone(),&id,provider,"wrong-workspace".into()).await.is_err());
    assert_eq!(store.snapshot(&id).unwrap().status,TaskStatus::Failed);
    assert!(!dir.path().join("claude-calls.jsonl").exists());
}

#[tokio::test]
async fn r3_native_malformed_send_ack_is_interrupted_without_replay() {
    let dir = tempfile::TempDir::new().unwrap();
    let (store, id) = super::tests::task(&dir);
    let provider = super::tests::fixture(&dir, json!([]));
    let proxy = dir.path().join("wire-proxy.py");
    let original = std::fs::read_to_string(&proxy).unwrap();
    std::fs::write(&proxy, original.replace(
        "capture('from_provider',line);sys.stdout.write(line);sys.stdout.flush()",
        "capture('from_provider',line)\n        value=json.loads(line)\n        if 'turn' in value.get('result',{}):\n            open(os.path.join(os.path.dirname(os.environ['WIRE_CAPTURE']),'accepted-write'),'w').write('accepted native turn')\n            value['result']={}\n            line=json.dumps(value)+'\\n'\n        sys.stdout.write(line);sys.stdout.flush()"
    )).unwrap();
    let outcome = execute(store.clone(), &id, provider.clone(), "fixture-workspace".into()).await;
    assert!(outcome.is_err());
    assert!(dir.path().join("accepted-write").exists(), "actual native acknowledgement was observed before it was corrupted");
    assert_eq!(store.snapshot(&id).unwrap().status, TaskStatus::Interrupted, "dispatched malformed ack must remain uncertain, not a definite failure");
    assert!(provider.list_sessions().await.unwrap().is_empty());
    assert!(execute(store.clone(), &id, provider, "fixture-workspace".into()).await.is_err());
    let trace = std::fs::read_to_string(dir.path().join("trace.jsonl")).unwrap();
    assert_eq!(trace.lines().filter(|l| serde_json::from_str::<Value>(l).unwrap()["method"] == "turn/start").count(), 1);
}


#[tokio::test]
async fn r3_native_inner_rpc_timeout_retains_dispatched_uncertainty() {
    let dir=tempfile::TempDir::new().unwrap();let (store,id)=super::tests::task(&dir);
    let provider=super::tests::fixture(&dir,json!([]));let proxy=dir.path().join("wire-proxy.py");
    let original=std::fs::read_to_string(&proxy).unwrap();
    std::fs::write(&proxy,original.replace("capture('from_provider',line);sys.stdout.write(line);sys.stdout.flush()","capture('from_provider',line)\n        value=json.loads(line)\n        if 'turn' in value.get('result',{}):\n            open(os.path.join(os.path.dirname(os.environ['WIRE_CAPTURE']),'accepted-write'),'w').write('accepted native turn')\n            continue\n        sys.stdout.write(line);sys.stdout.flush()")).unwrap();
    let result=tokio::time::timeout(Duration::from_secs(30),execute(store.clone(),&id,provider.clone(),"fixture-workspace".into())).await.unwrap();
    assert!(result.is_err());assert!(dir.path().join("accepted-write").exists());
    let snapshot=store.snapshot(&id).unwrap();
    assert_eq!(snapshot.status,TaskStatus::Interrupted);assert!(snapshot.turn_id.is_none());
    assert!(provider.list_sessions().await.unwrap().is_empty());
    assert!(execute(store,&id,provider,"fixture-workspace".into()).await.is_err());
}

#[tokio::test]
async fn r4_native_cancel_between_approval_callbacks_never_dispatches_second() {
    let dir = tempfile::TempDir::new().unwrap();
    let (store, id, thread) = claude_task(&dir);
    let provider = claude_fixture(&dir, json!([
        {"after":"send-turn","emit":"notification","method":"request-opened","params":{"threadId":thread,"requestId":"A","toolName":"Bash","toolInput":{},"kind":"command"}},
        {"after":"send-turn","emit":"notification","method":"request-opened","params":{"threadId":thread,"requestId":"B","toolName":"Bash","toolInput":{},"kind":"command"}}
    ]), "").await;
    let proxy = dir.path().join("approval-proxy.py");
    std::fs::write(&proxy, r#"import subprocess,sys,threading,json,time,os
child=subprocess.Popen(sys.argv[1:],stdin=subprocess.PIPE,stdout=subprocess.PIPE,text=True,bufsize=1)
first=None
root=os.path.dirname(os.environ['FAKE_CLAUDE_SIDECAR_CAPTURE'])
def output():
    for line in child.stdout:
        value=json.loads(line)
        if first is not None and value.get('id')==first:
            while not os.path.exists(os.path.join(root,'release-A')): time.sleep(.01)
        sys.stdout.write(line);sys.stdout.flush()
    os._exit(child.wait())
threading.Thread(target=output,daemon=True).start()
for line in sys.stdin:
    value=json.loads(line)
    if value.get('method')=='respond-to-request' and value['params']['requestId']=='A':
        first=value['id'];open(os.path.join(root,'entered-A'),'w').write('native callback invoked')
    child.stdin.write(line);child.stdin.flush()
child.stdin.close();child.wait()
"#).unwrap();
    let wrapper = dir.path().join("review-sidecar");
    let original = std::fs::read_to_string(&wrapper).unwrap();
    std::fs::write(&wrapper, original.replace("exec '", &format!("exec '{}' '{}' '", which::which("python3").unwrap().display(), proxy.display()))).unwrap();
    let worker = tokio::spawn({let store=store.clone(); let id=id.clone(); async move {execute(store,&id,provider,"fixture-workspace".into()).await}});
    tokio::time::timeout(Duration::from_secs(5), async {
        while store.snapshot(&id).unwrap().pending_requests.len()!=2 {tokio::time::sleep(Duration::from_millis(10)).await;}
    }).await.unwrap();
    for request_id in ["A","B"] {store.respond(&id, &super::super::RespondRequest {original_request:store.snapshot(&id).unwrap().pending_requests.into_iter().find(|r|r.request_id==request_id),request_id:request_id.into(),decision:ApprovalDecision::Allow {updated_input:None,updated_permissions:None}}).unwrap();}
    tokio::time::timeout(Duration::from_secs(5), async {
        while !dir.path().join("entered-A").exists() {tokio::time::sleep(Duration::from_millis(10)).await;}
    }).await.unwrap();
    store.cancel(&id).unwrap();
    assert_eq!(store.snapshot(&id).unwrap().status,TaskStatus::Stopping);
    std::fs::write(dir.path().join("release-A"),"continue").unwrap();
    let result = tokio::time::timeout(Duration::from_secs(10),worker).await.unwrap().unwrap();
    assert!(result.is_ok(),"{result:?}");
    let calls = std::fs::read_to_string(dir.path().join("claude-calls.jsonl")).unwrap();
    let callbacks: Vec<Value> = calls.lines().map(|l|serde_json::from_str::<Value>(l).unwrap()).filter(|v|v["method"]=="respond-to-request").collect();
    assert_eq!(callbacks.len(),1,"durable Stop must dominate callback B after awaited callback A: {callbacks:?}");
    assert_eq!(store.snapshot(&id).unwrap().status,TaskStatus::Cancelled);
}

#[test]
fn r1_claude_empty_correlation_rejects_unrelated_threads_turns_and_subagents() {
    let dir=tempfile::TempDir::new().unwrap();
    let (store,id,thread)=claude_task(&dir);
    let event=|thread:&str,turn:&str,subagent_id:Option<String>|ProviderRuntimeEvent::ItemCompleted {thread_id:ThreadId(thread.into()),turn_id:TurnId(turn.into()),item:CompletedItem::AssistantText{text:"foreign".into()},subagent_id};
    record(&store,&id,&event(&thread,"",None)).unwrap();
    assert!(store.snapshot(&id).unwrap().result.is_none(),"no dispatched turn");
    store.update(&id,|t|t.turn_id=Some("accepted".into())).unwrap();
    record(&store,&id,&event("foreign-thread","",None)).unwrap();
    record(&store,&id,&event(&thread,"foreign-turn",None)).unwrap();
    record(&store,&id,&event(&thread,"",Some("subagent".into()))).unwrap();
    assert!(store.snapshot(&id).unwrap().result.is_none());
    record(&store,&id,&event(&thread,"",None)).unwrap();
    assert_eq!(store.snapshot(&id).unwrap().result.as_deref(),Some("foreign"));
}
