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
 elif method=='thread/resume': result={'thread':{'id':'fixture-thread'},'model':'fixture-model','collaborationMode':{'mode':'plan','settings':{'model':'fixture-model'}}}
 elif method=='thread/read': result={'thread':{'id':'fixture-thread','turns':[]}}
 elif method=='turn/start':
  count+=1;result={'turn':{'id':'turn-'+str(count),'status':'inProgress'}}
 else: result={}
 output({'id':m['id'],'result':result})
 if method=='turn/start':
  turn={'id':'turn-'+str(count),'status':'completed'}
  output({'method':'turn/started','params':{'threadId':'fixture-thread','turn':turn}})
  output({'method':'turn/completed','params':{'threadId':'fixture-thread','turn':turn}})
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
    provider.start_session(start_input(dir.path(), &thread)).await.unwrap();

    send(&provider, &thread, Some("plan")).await;
    send(&provider, &thread, Some("plan")).await;
    // A send that names no mode (e.g. a question answer) keeps Plan.
    send(&provider, &thread, None).await;
    // With the pill off the composer names the thread's own mode.
    send(&provider, &thread, Some("danger-full-access")).await;
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
    // Plan is per turn, never a session permission change.
    assert!(provider
        .set_permission_mode(thread.clone(), "plan".into())
        .await
        .is_err());
    let _ = provider.stop_session(thread).await;
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
