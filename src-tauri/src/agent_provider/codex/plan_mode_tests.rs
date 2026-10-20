use super::super::{CodexAgentProvider, CodexProviderConfig};
use super::*;
use crate::agent_provider::AgentProvider;
use std::os::unix::fs::PermissionsExt;

/// Fake `codex app-server` that records every client message and completes
/// each turn as soon as it starts.
fn fixture_binary(root: &std::path::Path) -> PathBuf {
    let source = r#"#!/usr/bin/python3
import sys,json
from pathlib import Path
root=Path(__ROOT__)
if '--version' in sys.argv:
 print('codex-cli 0.162.1');sys.exit(0)
def output(v): print(json.dumps(v),flush=True)
count=0
for line in sys.stdin:
 m=json.loads(line);method=m.get('method')
 with (root/'wire.jsonl').open('a') as f: f.write(json.dumps(m)+'\n')
 if 'id' not in m or method is None: continue
 if method=='account/read': result={'account':None,'requiresOpenaiAuth':False}
 elif method=='thread/start': result={'thread':{'id':'fixture-thread'},'model':'fixture-model'}
 elif method=='thread/resume':
  result={'thread':{'id':'fixture-thread'},'model':'fixture-model'}
  # Older servers do not report the resumed thread's mode.
  reported='default' if (root/'reports-default').exists() else 'plan'
  if not (root/'older-server').exists(): result['collaborationMode']={'mode':reported,'settings':{'model':'fixture-model'}}
 elif method=='thread/read': result={'thread':{'id':'fixture-thread','turns':[]}}
 elif method=='turn/start':
  count+=1;result={'turn':{'id':'turn-'+str(count),'status':'inProgress'}}
 else: result={}
 output({'id':m['id'],'result':result})
 if method=='turn/start':
  turn={'id':'turn-'+str(count),'status':'completed'}
  output({'method':'turn/started','params':{'threadId':'fixture-thread','turn':turn}})
  if (root/'waiting-tools').exists():
   output({'id':701,'method':'item/tool/call','params':{'threadId':'fixture-thread','turnId':turn['id'],'callId':'wait-call','tool':'codemux_mcp__codemux__thread_wait','arguments':{'thread_id':'target'}}})
   output({'id':702,'method':'item/commandExecution/requestApproval','params':{'threadId':'fixture-thread','turnId':turn['id'],'itemId':'approval-item','command':'fixture command','cwd':str(root)}})
  else: output({'method':'turn/completed','params':{'threadId':'fixture-thread','turn':turn}})
"#;
    let source = source.replace(
        "__ROOT__",
        &serde_json::to_string(&root.to_string_lossy()).unwrap(),
    );
    let binary = root.join("codex-fixture.py");
    std::fs::write(&binary, source).unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
    binary
}

fn start_input(root: &std::path::Path, thread: &ThreadId) -> crate::agent_provider::StartSessionInput {
    serde_json::from_value(json!({
        "thread_id": thread.0, "cwd": root, "model": "fixture-model",
        "resume_cursor": null, "permission_mode": "danger-full-access",
        "additional_directories": [], "env": null, "workspace_id": null,
        "recorded_usage_baseline": null
    }))
    .unwrap()
}

/// Send one turn and wait until it settles, so the next send starts a turn.
async fn send(provider: &CodexAgentProvider, thread: &ThreadId, mode: Option<&str>) {
    let mut turn = super::tests::queued("turn").input;
    turn.thread_id = thread.clone();
    turn.text = "hi".into();
    turn.permission_mode_override = mode.map(str::to_owned);
    provider.send_turn(turn).await.unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        while provider.turn_active(thread).await {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("turn should settle");
}

/// Answer a question asked by a turn that has since ended, which starts a turn.
async fn answer(provider: &CodexAgentProvider, thread: &ThreadId, mode: Option<&str>) {
    let question = serde_json::from_value(json!({
        "id": "q1", "target": "fixture-thread", "source_item_id": "i1",
        "source_turn_id": "turn-1", "text": "Which?", "questions": [{"title": "Which?"}]
    }))
    .unwrap();
    provider
        .answer_question(crate::agent_provider::AnswerQuestionInput {
            thread_id: thread.clone(),
            question,
            answers: vec!["This one".into()],
            submission_id: "00000000-0000-4000-8000-000000000001".into(),
            checkpoint: None,
            permission_mode_override: mode.map(str::to_owned),
        })
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        while provider.turn_active(thread).await {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("turn should settle");
}

/// `(method, collaboration mode)` per request, in wire order.
fn wire(root: &std::path::Path) -> Vec<(String, Option<String>)> {
    std::fs::read_to_string(root.join("wire.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .filter(|m| matches!(m["method"].as_str(), Some("thread/start" | "turn/start")))
        .map(|m| {
            let mode = m["params"]["collaborationMode"]["mode"].as_str().map(str::to_owned);
            (m["method"].as_str().unwrap().to_owned(), mode)
        })
        .collect()
}

#[tokio::test]
async fn the_plan_pill_on_each_send_picks_the_turns_collaboration_mode() {
    let dir = tempfile::tempdir().unwrap();
    let provider = CodexAgentProvider::new(CodexProviderConfig {
        codex_binary: fixture_binary(dir.path()),
        ..Default::default()
    });
    let thread = ThreadId("plan".into());
    let mut events = provider.event_tx.subscribe();
    provider.start_session(start_input(dir.path(), &thread)).await.unwrap();

    send(&provider, &thread, Some("plan")).await;
    assert!(crate::delegation::authority::NativeAuthority::effective_ceiling(&thread.0).unwrap().unwrap().plan);
    send(&provider, &thread, Some("plan")).await;
    // A send that names no mode (e.g. a question answer) keeps Plan.
    send(&provider, &thread, None).await;
    // With the pill off the composer names the thread's own mode.
    send(&provider, &thread, Some("danger-full-access")).await;
    assert!(!crate::delegation::authority::NativeAuthority::effective_ceiling(&thread.0).unwrap().unwrap().plan);
    // A restart or rebuild cannot drop Plan: the next send names it again,
    // and so does an answer that starts a turn.
    provider.stop_session(thread.clone()).await.unwrap();
    provider.start_session(start_input(dir.path(), &thread)).await.unwrap();
    answer(&provider, &thread, Some("plan")).await;
    send(&provider, &thread, Some("plan")).await;

    let turn = |mode: &str| ("turn/start".to_owned(), Some(mode.to_owned()));
    let start = ("thread/start".to_owned(), None);
    assert_eq!(
        wire(dir.path()),
        [
            start.clone(),
            turn("plan"),
            turn("plan"),
            turn("plan"),
            turn("default"),
            start,
            turn("plan"),
            turn("plan"),
        ]
    );
    // Each accepted turn reports the mode it runs in, in turn order, so a
    // restart can resume it in that mode.
    let mut reported = Vec::new();
    while let Ok(event) = events.try_recv() {
        if let ProviderRuntimeEvent::TurnModeSelected { mode, .. } = event {
            reported.push(mode);
        }
    }
    assert_eq!(reported, ["plan", "plan", "plan", "default", "plan", "plan"]);
    // Plan is per turn, never a session permission change.
    assert!(provider
        .set_permission_mode(thread.clone(), "plan".into())
        .await
        .is_err());
    let _ = provider.stop_session(thread).await;
}

#[tokio::test]
async fn a_late_plugin_catalog_notice_preserves_plan_mode_and_is_sent_once() {
    let dir = tempfile::tempdir().unwrap();
    let registry = crate::mcp::registry::McpRegistry::new();
    let provider = CodexAgentProvider::new(CodexProviderConfig {
        codex_binary: fixture_binary(dir.path()),
        mcp_registry: Some(registry.clone()),
        ..Default::default()
    });
    let thread = ThreadId("plan-with-late-tools".into());
    provider
        .start_session(start_input(dir.path(), &thread))
        .await
        .unwrap();
    registry
        .register_plugin_server::<tauri::Wry>(
            None,
            "plan-fixture",
            "token".into(),
            serde_json::from_value(json!({
                "id":"fixture", "name":"Fixture", "toolPrefix":"fixture", "scope":"all",
                "transport":"stdio", "command":"python3",
                "args":[format!("{}/tests/fixtures/plugin_mcp.py",env!("CARGO_MANIFEST_DIR"))]
            }))
            .unwrap(),
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        while registry.plugin_catalog(None).await.len() != 4 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();

    send(&provider, &thread, Some("plan")).await;
    send(&provider, &thread, None).await;
    let turns: Vec<Value> = std::fs::read_to_string(dir.path().join("wire.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .filter(|message| message["method"] == "turn/start")
        .collect();
    assert_eq!(turns.len(), 2);
    for turn in &turns {
        assert_eq!(turn["params"]["collaborationMode"]["mode"], "plan");
        assert_eq!(
            turn["params"]["collaborationMode"]["settings"].get("developer_instructions"),
            Some(&Value::Null)
        );
    }
    let input = turns[0]["params"]["input"].as_array().unwrap();
    assert_eq!(input.len(), 2);
    let notice = input[0]["text"].as_str().unwrap();
    assert!(notice.starts_with("<codemux_plugin_tools_notice>"));
    assert!(notice.contains("fixture__image"));
    assert_eq!(input[1]["text"], "hi");
    assert_eq!(
        turns[1]["params"]["input"],
        json!([{"type":"text","text":"hi","text_elements":[]}])
    );
    provider.stop_session(thread).await.unwrap();
    registry.shutdown_all().await;
}

#[tokio::test]
async fn a_resumed_plan_thread_continues_in_plan_until_a_send_names_a_mode() {
    let dir = tempfile::tempdir().unwrap();
    let provider = CodexAgentProvider::new(CodexProviderConfig {
        codex_binary: fixture_binary(dir.path()),
        ..Default::default()
    });
    let thread = ThreadId("resumed".into());
    let mut input = start_input(dir.path(), &thread);
    // Restart auto-resume reopens the thread, which Codex reports in Plan, and
    // sends its continuation without a mode.
    input.resume_cursor = Some(json!({"threadId": "fixture-thread"}));
    provider.start_session(input).await.unwrap();
    send(&provider, &thread, None).await;
    send(&provider, &thread, Some("danger-full-access")).await;

    let modes: Vec<_> = wire(dir.path())
        .into_iter()
        .filter(|(method, _)| method == "turn/start")
        .map(|(_, mode)| mode.unwrap())
        .collect();
    assert_eq!(modes, ["plan", "default"]);
    let _ = provider.stop_session(thread).await;
}

#[tokio::test]
async fn a_server_that_reports_no_mode_resumes_in_the_recorded_one() {
    for (server, recorded, expected) in [
        // Older server: the mode Codemux recorded for the last turn.
        ("older-server", "plan", "plan"),
        // A server that reports the mode is the truth, over a stale record.
        ("reports-plan", "default", "plan"),
        ("reports-default", "plan", "default"),
    ] {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(server), "").unwrap();
        let provider = CodexAgentProvider::new(CodexProviderConfig {
            codex_binary: fixture_binary(dir.path()),
            ..Default::default()
        });
        let thread = ThreadId("recorded".into());
        let mut input = start_input(dir.path(), &thread);
        input.resume_cursor = Some(json!({"threadId": "fixture-thread"}));
        input.extra = json!({ "turnMode": recorded });
        provider.start_session(input).await.unwrap();
        send(&provider, &thread, None).await;

        let modes: Vec<_> = wire(dir.path())
            .into_iter()
            .filter(|(method, _)| method == "turn/start")
            .map(|(_, mode)| mode.unwrap())
            .collect();
        assert_eq!(modes, [expected], "{server}");
        let _ = provider.stop_session(thread).await;
    }
}

/// The permission mode a host tool called from turn `turn` sees.
async fn host_call_mode(provider: &CodexAgentProvider, thread: &ThreadId, turn: Option<&str>) -> Option<String> {
    let session = provider.sessions.read().await.get(thread).cloned().unwrap();
    session.host_caller_for_call(turn).permission_mode
}

#[tokio::test]
async fn host_tools_called_from_a_plan_turn_see_plan_mode() {
    let dir = tempfile::tempdir().unwrap();
    let provider = CodexAgentProvider::new(CodexProviderConfig {
        codex_binary: fixture_binary(dir.path()),
        ..Default::default()
    });
    let thread = ThreadId("plan-host-tools".into());
    provider.start_session(start_input(dir.path(), &thread)).await.unwrap();

    send(&provider, &thread, Some("plan")).await;
    assert_eq!(host_call_mode(&provider, &thread, Some("turn-1")).await.as_deref(), Some("plan"));
    // Naming the next turn's mode leaves the started turn's alone.
    let session = provider.sessions.read().await.get(&thread).cloned().unwrap();
    session.select_collaboration_mode(Some("danger-full-access"));
    assert_eq!(host_call_mode(&provider, &thread, Some("turn-1")).await.as_deref(), Some("plan"));

    send(&provider, &thread, Some("danger-full-access")).await;
    assert_eq!(host_call_mode(&provider, &thread, Some("turn-2")).await.as_deref(), Some("danger-full-access"));
    // A call from the Plan turn that arrives after it was replaced keeps Plan.
    assert_eq!(host_call_mode(&provider, &thread, Some("turn-1")).await.as_deref(), Some("plan"));
    // A turn this session did not start (or forgot) may be in Plan.
    assert_eq!(host_call_mode(&provider, &thread, Some("turn-9")).await.as_deref(), Some("plan"));
    let _ = provider.stop_session(thread).await;
}

#[tokio::test]
async fn a_resumed_thread_calls_host_tools_in_its_resumed_mode() {
    for (server, recorded, expected) in [
        ("older-server", "plan", "plan"),
        ("reports-plan", "default", "plan"),
        ("reports-default", "plan", "danger-full-access"),
    ] {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(server), "").unwrap();
        let provider = CodexAgentProvider::new(CodexProviderConfig {
            codex_binary: fixture_binary(dir.path()),
            ..Default::default()
        });
        let thread = ThreadId("resumed-host-tools".into());
        let mut input = start_input(dir.path(), &thread);
        input.resume_cursor = Some(json!({"threadId": "fixture-thread"}));
        input.extra = json!({ "turnMode": recorded });
        provider.start_session(input).await.unwrap();
        // Its continuation names no mode, so it runs in the resumed one.
        send(&provider, &thread, None).await;
        assert_eq!(host_call_mode(&provider, &thread, Some("turn-1")).await.as_deref(), Some(expected), "{server}");
        let _ = provider.stop_session(thread).await;
    }
}

#[test]
fn turn_modes_answer_plan_whenever_a_turns_mode_is_uncertain() {
    let mut modes = TurnModes::default();
    // A confirmed turn keeps its own mode after later turns start.
    let plan = modes.begin(Some(true));
    modes.confirm("t1", plan);
    let plan = modes.begin(Some(false));
    // An unknown id during a pending start may be a late call from a turn
    // whose start was never confirmed.
    assert!(modes.is_plan(Some("unknown")));
    modes.confirm("t2", plan);
    assert!(modes.is_plan(Some("t1")));
    assert!(!modes.is_plan(Some("t2")));
    // Idle, an unknown turn (a review, a failed start, a forgotten one) is Plan.
    assert!(modes.is_plan(Some("unknown")) && modes.is_plan(None));

    // A start that names no mode keeps the thread's.
    assert!(!modes.begin(None));
    assert!(modes.is_plan(Some("pending")), "unconfirmed, so not trusted yet");
    modes.confirm("t3", false);
    assert!(!modes.is_plan(Some("t3")));

    // A rejected Default start leaves an inherited Plan alone, so a later
    // start that names no mode is still Plan.
    let mut modes = TurnModes { inherited: true, ..Default::default() };
    modes.begin(Some(false));
    modes.fail();
    assert!(modes.begin(None));
    modes.confirm("t4", true);
    assert!(modes.is_plan(Some("t4")));

    // A Plan start whose answer was lost may have run: the thread counts as
    // Plan until a start is confirmed.
    let mut modes = TurnModes::default();
    modes.begin(Some(true));
    modes.fail();
    assert!(modes.begin(None));
    modes.fail();
    let plan = modes.begin(Some(false));
    assert!(modes.is_plan(Some("lost-plan-turn")));
    modes.confirm("t5", plan);
    assert!(!modes.is_plan(Some("t5")));
    assert!(modes.is_plan(Some("lost-plan-turn")));
    // ...and while yet another Default start is pending.
    modes.begin(Some(false));
    assert!(modes.is_plan(Some("lost-plan-turn")));

    // Old turns are forgotten, and then read as Plan.
    let mut modes = TurnModes::default();
    for turn in 0..=RECENT_TURN_MODES {
        let plan = modes.begin(Some(false));
        modes.confirm(&format!("t{turn}"), plan);
    }
    assert!(modes.is_plan(Some("t0")));
    assert!(!modes.is_plan(Some(&format!("t{RECENT_TURN_MODES}"))));
}

#[tokio::test]
async fn native_wait_tool_does_not_block_codex_approval_reader() {
    let dir=tempfile::tempdir().unwrap();std::fs::write(dir.path().join("waiting-tools"),"").unwrap();
    let registry=crate::mcp::registry::McpRegistry::new();
    let entered=Arc::new(tokio::sync::Notify::new());let release=Arc::new(tokio::sync::Semaphore::new(0));
    let start=entered.clone();let gate=release.clone();
    registry.set_native_delegation_handler(Arc::new(move |actor,name,_args|{let start=start.clone();let gate=gate.clone();Box::pin(async move {
        assert_eq!(name,"thread_wait");drop(actor.permit.admit()?);start.notify_one();gate.acquire().await.unwrap().forget();Ok(json!({"settled":false}))
    })}));
    let provider=CodexAgentProvider::new(CodexProviderConfig {codex_binary:fixture_binary(dir.path()),mcp_registry:Some(registry),..Default::default()});
    let thread=ThreadId("native-wait-reader".into());let mut events=provider.event_tx.subscribe();
    provider.start_session(start_input(dir.path(),&thread)).await.unwrap();
    let mut input=super::tests::queued("read").input;input.thread_id=thread.clone();
    provider.send_turn(input).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5),entered.notified()).await.unwrap();
    let approval=tokio::time::timeout(Duration::from_secs(5),async{loop{if let Ok(ProviderRuntimeEvent::RequestOpened{request_kind,..})=events.recv().await {break request_kind;}}}).await;
    release.add_permits(1);
    provider.stop_session(thread).await.unwrap();
    assert_eq!(approval.unwrap(),"command");
}
