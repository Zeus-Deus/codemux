//! Token-free sidecar admission, captured callbacks and process-group teardown.
#![cfg(target_os = "linux")]

use async_trait::async_trait;
use codemux_lib::agent_provider::{managed::{ManagedSession, ManagedTool, ManagedToolHandler},
    managed_bridge::ManagedBridgeSession, ProviderKind, ProviderRuntimeEvent, SendTurnInput,
    StartSessionInput, ThreadId, TurnStatus};
use codemux_lib::json_rpc_child::SpawnConfig;
use serde_json::{json, Value};
use std::{collections::HashMap, sync::{Arc, Mutex}, time::Duration};
use tokio::sync::broadcast;

#[derive(Default)]
struct Captured { calls: Mutex<Vec<Value>>, evidence: Mutex<Option<Value>> }
#[async_trait]
impl ManagedToolHandler for Captured {
    fn tools(&self) -> Vec<ManagedTool> { vec![ManagedTool { name: "workflow_result".into(), description: "Report".into(), input_schema: json!({"type":"object"}) }] }
    async fn call(&self, name: &str, arguments: Value) -> Result<Value, String> {
        self.calls.lock().unwrap().push(json!({"name":name,"arguments":arguments}));
        Ok(json!({"authority":"captured-host-attempt"}))
    }
    fn record_runtime(&self, evidence: Value) -> Result<(), String> { *self.evidence.lock().unwrap() = Some(evidence); Ok(()) }
    async fn quiesce(&self) -> Result<(), String> { Ok(()) }
}

const SIDECAR: &str = r#"
import json,sys
mode=sys.argv[1];turn=None
def emit(x): print(json.dumps(dict(jsonrpc='2.0',**x)),flush=True)
for line in sys.stdin:
 m=json.loads(line); method=m.get('method'); i=m.get('id'); p=m.get('params',{})
 if method=='initialize':
  emit({'id':'early','method':'managed/tool_call','params':{'name':'workflow_result','arguments':{'phase':'before-ready'}}})
  early=json.loads(sys.stdin.readline())
  if 'error' not in early: raise RuntimeError('Host admitted a pre-ready call')
  emit({'id':i,'result':{'protocolVersion':1,'provider':'cursor','adapterVersion':'unreviewed' if mode=='wrong-version' else 'cursor-sdk-1.0.37','sessionId':'fresh','tools':[t['name'] for t in p['tools']],'isolation':{'nativeTools':[],'nativeFanout':False,'ambientConfig':False}}})
 elif method=='startTurn':
  turn=p['turnId'];emit({'id':i,'result':{}})
  emit({'id':'tool','method':'managed/tool_call','params':{'name':'shell' if mode=='forged-tool' else 'workflow_result','arguments':{'run_id':'forged-run','attempt_id':'forged-attempt'}}})
 elif i=='tool':
  emit({'method':'complete','params':{'turnId':'forged-turn' if mode=='wrong-turn' else turn,'status':'error' if 'error' in m else 'success','usage':{'durationMs':1,'numTurns':1}}})
 elif method=='cancel':emit({'id':i,'result':{}})
"#;

fn start(thread_id: ThreadId, cwd: &std::path::Path) -> StartSessionInput {
    StartSessionInput { thread_id, cwd: cwd.into(), model: None, resume_cursor: None,
        fresh_session: true, permission_mode: None, effort: None, context_window: None,
        fast_mode: false, additional_directories: vec![], env: None, workspace_id: None,
        extra: json!({}), recorded_usage_baseline: None }
}
fn send(thread_id: ThreadId) -> SendTurnInput {
    SendTurnInput { thread_id, text: "Fixture only".into(), display_text: None, images: vec![],
        skill_invocations: vec![], model_override: None, effort_override: None,
        permission_mode_override: None, client_nonce: None, turn_checkpoint: None }
}

async fn spawn(mode: &str, captured: Arc<Captured>) -> Result<(Arc<ManagedBridgeSession>, broadcast::Receiver<ProviderRuntimeEvent>), codemux_lib::agent_provider::ProviderError> {
    let cwd = tempfile::tempdir().unwrap();
    let thread = ThreadId(format!("bridge-{}", uuid::Uuid::new_v4()));
    let (tx, rx) = broadcast::channel(32);
    let session = ManagedBridgeSession::spawn(start(thread, cwd.path()), ProviderKind::Cursor, "cursor-sdk-1.0.37",
        Arc::new(ManagedSession { handler: captured, read_only: true }),
        SpawnConfig { program: "python3".into(), args: vec!["-u".into(), "-c".into(), SIDECAR.into(), mode.into()],
            env: HashMap::new(), cwd: Some(cwd.path().into()), default_timeout: Duration::from_secs(5) }, json!({}), tx).await?;
    Ok((session, rx))
}

#[tokio::test]
async fn managed_bridge_captures_callbacks_and_verifies_group_stop() {
    let captured = Arc::new(Captured::default());
    let (session, mut events) = spawn("valid", Arc::clone(&captured)).await.unwrap();
    session.send_turn(send(session.thread_id.clone())).await.unwrap();
    let terminal = tokio::time::timeout(Duration::from_secs(5), async {
        loop { if let ProviderRuntimeEvent::TurnCompleted { status, usage, .. } = events.recv().await.unwrap() { break (status, usage); } }
    }).await.unwrap();
    assert!(matches!(terminal.0, TurnStatus::Success));
    assert_eq!(terminal.1.unwrap().total_cost_usd, None, "unknown native usage is never $0");
    assert_eq!(captured.calls.lock().unwrap().as_slice(), &[json!({"name":"workflow_result","arguments":{"run_id":"forged-run","attempt_id":"forged-attempt"}})]);
    session.shutdown_managed().await.unwrap();
    let pid = captured.evidence.lock().unwrap().as_ref().unwrap()["pid"].as_u64().unwrap() as i32;
    assert_ne!(unsafe { libc::kill(-pid, 0) }, 0);
    assert_eq!(std::io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH));
}

#[tokio::test]
async fn managed_bridge_rejects_unreviewed_version_before_prompt() {
    let captured = Arc::new(Captured::default());
    assert!(spawn("wrong-version", Arc::clone(&captured)).await.is_err());
    assert!(captured.calls.lock().unwrap().is_empty());
    let pid = captured.evidence.lock().unwrap().as_ref().unwrap()["pid"].as_u64().unwrap() as i32;
    assert_ne!(unsafe { libc::kill(-pid, 0) }, 0);
    assert_eq!(std::io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH));
}

#[tokio::test]
async fn managed_bridge_denies_unowned_tools_and_wrong_turn_notifications() {
    for mode in ["forged-tool", "wrong-turn"] {
        let captured = Arc::new(Captured::default());
        let (session, mut events) = spawn(mode, Arc::clone(&captured)).await.unwrap();
        session.send_turn(send(session.thread_id.clone())).await.unwrap();
        let status = tokio::time::timeout(Duration::from_secs(5), async {
            loop { if let ProviderRuntimeEvent::TurnCompleted { status, .. } = events.recv().await.unwrap() { break status; } }
        }).await.unwrap();
        assert!(matches!(status, TurnStatus::Error { .. }));
        if mode == "forged-tool" { assert!(captured.calls.lock().unwrap().is_empty()); }
        session.shutdown_managed().await.unwrap();
    }
}
