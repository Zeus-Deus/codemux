//! Native adapters against owned, token-free JSON-RPC processes. No SDK/model
//! turn is run by these tests; the fixture is a temporary Python script.
#![cfg(target_os = "linux")]

use async_trait::async_trait;
use codemux_lib::agent_provider::{
    claude::{ClaudeAgentProvider, ClaudeProviderConfig},
    codex::{protocol::ClientInfo, CodexAgentProvider, CodexProviderConfig},
    managed::{register_session, ManagedSession, ManagedTool, ManagedToolHandler},
    AgentProvider, ProviderRuntimeEvent, SendTurnInput, StartSessionInput, ThreadId, TurnStatus,
};
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use tempfile::TempDir;

#[derive(Default)]
struct CapturedTools {
    calls: Mutex<Vec<Value>>,
    evidence: Mutex<Option<Value>>,
}
#[async_trait]
impl ManagedToolHandler for CapturedTools {
    fn tools(&self) -> Vec<ManagedTool> {
        vec![ManagedTool {
            name: "workflow_submit_result".into(),
            description: "fake result".into(),
            input_schema: json!({"type":"object","properties":{"output":{"type":"object"}},"required":["output"]}),
        }]
    }
    async fn call(&self, name: &str, args: Value) -> Result<Value, String> {
        self.calls
            .lock()
            .unwrap()
            .push(json!({"name":name,"arguments":args}));
        Ok(json!({"accepted":true}))
    }
    fn record_runtime(&self, evidence: Value) -> Result<(), String> {
        *self.evidence.lock().unwrap() = Some(evidence);
        Ok(())
    }
    async fn quiesce(&self) -> Result<(), String> {
        Ok(())
    }
}

const FAKE: &str = r#"#!/usr/bin/env python3
import json,os,sys,time
mode=os.environ['FAKE_MANAGED_MODE'];capture=os.environ['FAKE_MANAGED_CAPTURE'];thread='fake-thread'
features={}
for arg in sys.argv[1:]:
 if arg.startswith('features.') and arg.endswith('=false'):features[arg[9:-6]]=False
def emit(value):print(json.dumps(value),flush=True)
def reply(i,value):emit({'jsonrpc':'2.0','id':i,'result':value})
def note(method,params):emit({'jsonrpc':'2.0','method':method,'params':params})
def completed():
 if mode.startswith('claude'):
  note('sdk-message',{'threadId':thread,'message':{'type':'result','subtype':'success','duration_ms':1,'num_turns':1,'total_cost_usd':0.001,'usage':{'input_tokens':17,'output_tokens':3}}})
 else:
  note('thread/tokenUsage/updated',{'threadId':thread,'tokenUsage':{'total':{'inputTokens':17,'outputTokens':3,'totalTokens':20},'last':{'inputTokens':17,'outputTokens':3,'totalTokens':20}}})
  note('turn/completed',{'threadId':thread,'turn':{'id':'fake-turn','status':'completed'}})
for line in sys.stdin:
 msg=json.loads(line)
 with open(capture,'a') as f:f.write(json.dumps(msg)+'\n')
 method=msg.get('method');params=msg.get('params') or {};i=msg.get('id')
 if method is None:
  if i=='blocked':
   if mode.startswith('claude'):emit({'id':'owned','method':'mcp-tool-call','params':{'name':'workflow_submit_result','arguments':{'output':{'ok':True}}}})
   else:emit({'id':'owned','method':'item/tool/call','params':{'threadId':thread,'turnId':'fake-turn','callId':'owned','tool':'workflow_submit_result','arguments':{'output':{'ok':True}}}})
  elif i=='owned':completed()
  continue
 if i is None:continue
 if method=='config/read':reply(i,{'config':{'features':features,'web_search':'disabled','mcp_servers':{}}})
 elif method=='experimentalFeature/list':reply(i,{'data':[{'name':k,'enabled':k=='unified_exec'} for k in features]})
 elif method=='account/read':reply(i,{'account':None if mode=='codex-unauth' else {'type':'apiKey'},'requiresOpenaiAuth':True})
 elif method=='start-session':
  thread=params['threadId']
  if mode=='claude-not-ready':emit({'jsonrpc':'2.0','id':i,'error':{'code':-32000,'message':'managed native tool catalog incomplete'}})
  else:reply(i,{'threadId':thread,'pathToClaudeCodeExecutable':params['pathToClaudeCodeExecutable']})
 elif method=='thread/start':reply(i,{'thread':{'id':thread,'environments':[]},'approvalPolicy':'never','sandbox':{'type':'readOnly'}})
 elif method in ('send-turn','turn/start'):
  if mode=='claude-early':completed();time.sleep(0.04)
  reply(i,{'turn':{'id':'fake-turn'}} if method=='turn/start' else {'turnStarted':True})
  if mode!='claude-early':
   if mode.startswith('claude'):emit({'id':'blocked','method':'mcp-tool-call','params':{'name':'mcp__global__dangerous','arguments':{}}})
   else:emit({'id':'blocked','method':'item/tool/call','params':{'threadId':thread,'turnId':'fake-turn','callId':'blocked','tool':'mcp__global__dangerous','arguments':{}}})
 elif method=='stop-session':reply(i,{'alreadyClosed':False})
 else:reply(i,{})
"#;

fn fixture() -> (TempDir, PathBuf, PathBuf) {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let binary = dir.path().join("fake-native.py");
    std::fs::write(&binary, FAKE).unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o755)).unwrap();
    let capture = dir.path().join("capture.jsonl");
    (dir, binary, capture)
}
fn start(id: ThreadId, dir: &TempDir, capture: &PathBuf, mode: &str) -> StartSessionInput {
    StartSessionInput {
        thread_id: id,
        cwd: dir.path().into(),
        model: None,
        resume_cursor: None,
        fresh_session: true,
        permission_mode: None,
        effort: None,
        context_window: None,
        fast_mode: false,
        additional_directories: vec![],
        env: Some(HashMap::from([
            ("FAKE_MANAGED_MODE".into(), mode.into()),
            (
                "FAKE_MANAGED_CAPTURE".into(),
                capture.to_string_lossy().into(),
            ),
        ])),
        workspace_id: None,
        extra: json!({"maxBudgetUsd":0.2,"maxTurns":6}),
        recorded_usage_baseline: None,
    }
}
fn send(id: ThreadId) -> SendTurnInput {
    SendTurnInput {
        thread_id: id,
        text: "fixture only".into(),
        display_text: None,
        images: vec![],
        skill_invocations: vec![],
        model_override: None,
        effort_override: None,
        permission_mode_override: None,
        client_nonce: None,
        turn_checkpoint: None,
    }
}
fn transcript(path: &PathBuf) -> Vec<Value> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}
async fn exercise(mode: &str) {
    let (dir, binary, capture) = fixture();
    let id = ThreadId(uuid::Uuid::new_v4().to_string());
    let tools = Arc::new(CapturedTools::default());
    let guard = register_session(
        id.clone(),
        ManagedSession {
            handler: tools.clone(),
            read_only: true,
        },
    )
    .unwrap();
    let provider: Arc<dyn AgentProvider> = if mode.starts_with("claude") {
        Arc::new(
            ClaudeAgentProvider::new(ClaudeProviderConfig {
                sidecar_binary: Some(binary),
                claude_binary: Some("/fake/never-executed-claude".into()),
                event_channel_capacity: 1024,
                mcp_registry: None,
            })
            .await
            .unwrap(),
        )
    } else {
        Arc::new(CodexAgentProvider::new(CodexProviderConfig {
            codex_binary: binary,
            codex_home: None,
            event_channel_capacity: 1024,
            client_info: ClientInfo {
                name: "fixture".into(),
                title: "fixture".into(),
                version: "0".into(),
            },
            mcp_registry: None,
        }))
    };
    let mut events = provider.managed_event_stream(&id);
    provider
        .start_session(start(id.clone(), &dir, &capture, mode))
        .await
        .unwrap();
    let turn = provider.send_turn(send(id.clone())).await.unwrap().turn_id;
    let completed = tokio::time::timeout(Duration::from_secs(3), async {
        while let Some(event) = events.next().await {
            if let ProviderRuntimeEvent::TurnCompleted {
                turn_id, status, ..
            } = event
            {
                return (turn_id, status);
            }
        }
        panic!("fixture stream closed before completion")
    })
    .await;
    // Stop the owned fake even when a regression prevents completion.
    provider.stop_managed_session(id.clone()).await.unwrap();
    assert!(!provider.has_session(&id).await);
    let (completed_turn, status) = completed.expect("adapter lost/stamped the wrong completion");
    assert_eq!(completed_turn, turn);
    assert!(matches!(status, TurnStatus::Success));
    let captured = transcript(&capture);
    if mode != "claude-early" {
        assert_eq!(
            tools.calls.lock().unwrap().as_slice(),
            &[json!({"name":"workflow_submit_result","arguments":{"output":{"ok":true}}})]
        );
        let blocked = captured
            .iter()
            .find(|entry| entry.get("id") == Some(&json!("blocked")))
            .unwrap();
        let allowed = captured
            .iter()
            .find(|entry| entry.get("id") == Some(&json!("owned")))
            .unwrap();
        if mode.starts_with("claude") {
            assert_eq!(blocked["result"]["isError"], true);
            assert_eq!(allowed["result"]["isError"], false);
        } else {
            assert_eq!(blocked["result"]["success"], false);
            assert_eq!(allowed["result"]["success"], true);
        }
    }
    if mode.starts_with("claude") {
        let options = captured
            .iter()
            .find(|entry| entry["method"] == "start-session")
            .unwrap();
        assert_eq!(options["params"]["managed"], true);
        assert_eq!(options["params"]["maxBudgetUsd"], 0.2);
        assert_eq!(options["params"]["maxTurns"], 6);
        assert_eq!(
            options["params"]["mcpTools"][0]["prefixedName"],
            "workflow_submit_result"
        );
    } else {
        for method in ["thread/start", "turn/start"] {
            let request = captured
                .iter()
                .find(|entry| entry["method"] == method)
                .unwrap();
            assert_eq!(request["params"]["environments"], json!([]));
        }
    }
    drop(guard);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn managed_native_claude_tool_wire_and_terminal_turn_identity() {
    exercise("claude").await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn managed_native_claude_completion_before_ack_preserves_identity() {
    exercise("claude-early").await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn managed_native_codex_tool_wire_and_no_environment_requests() {
    exercise("codex").await;
}

#[tokio::test]
async fn managed_native_codex_unauthenticated_start_is_rejected_and_stopped() {
    let (dir, binary, capture) = fixture();
    let id = ThreadId(uuid::Uuid::new_v4().to_string());
    let tools = Arc::new(CapturedTools::default());
    let _guard = register_session(
        id.clone(),
        ManagedSession {
            handler: tools.clone(),
            read_only: true,
        },
    )
    .unwrap();
    let provider = CodexAgentProvider::new(CodexProviderConfig {
        codex_binary: binary,
        codex_home: None,
        event_channel_capacity: 1024,
        client_info: ClientInfo {
            name: "fixture".into(),
            title: "fixture".into(),
            version: "0".into(),
        },
        mcp_registry: None,
    });
    let error = provider
        .start_session(start(id.clone(), &dir, &capture, "codex-unauth"))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("managed-start-rejected:"));
    assert!(!provider.has_session(&id).await);
    assert!(!transcript(&capture)
        .iter()
        .any(|entry| entry["method"] == "thread/start" || entry["method"] == "turn/start"));
    let evidence = tools.evidence.lock().unwrap().clone().unwrap();
    let pid = evidence["pid"].as_u64().unwrap();
    assert!(
        !PathBuf::from(format!("/proc/{pid}")).exists(),
        "owned unauthenticated fake remained alive"
    );
}

#[tokio::test]
async fn managed_native_claude_readiness_rejection_proves_owned_process_stopped() {
    let (dir, binary, capture) = fixture();
    let id = ThreadId(uuid::Uuid::new_v4().to_string());
    let tools = Arc::new(CapturedTools::default());
    let _guard = register_session(
        id.clone(),
        ManagedSession {
            handler: tools.clone(),
            read_only: true,
        },
    )
    .unwrap();
    let provider = ClaudeAgentProvider::new(ClaudeProviderConfig {
        sidecar_binary: Some(binary),
        claude_binary: Some("/fake/never-executed-claude".into()),
        event_channel_capacity: 1024,
        mcp_registry: None,
    })
    .await
    .unwrap();
    let error = provider
        .start_session(start(id.clone(), &dir, &capture, "claude-not-ready"))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("managed-start-rejected:"));
    assert!(!provider.has_session(&id).await);
    assert!(!transcript(&capture)
        .iter()
        .any(|entry| entry["method"] == "send-turn"));
    let evidence = tools.evidence.lock().unwrap().clone().unwrap();
    let pid = evidence["pid"].as_u64().unwrap();
    assert!(
        !PathBuf::from(format!("/proc/{pid}")).exists(),
        "owned fake with rejected tool catalog remained alive"
    );
}
