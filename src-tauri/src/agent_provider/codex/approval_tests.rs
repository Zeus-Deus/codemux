use super::*;
use std::os::unix::fs::PermissionsExt;

/// Fake `codex app-server`: every turn raises one command approval. Turn 1
/// completes once the approval is answered, turn 2 waits for an interrupt,
/// turn 3 ends on its own with the approval unanswered, as Codex does when
/// it aborts a turn's requests, and turn 4 ends before a sub-agent raises an
/// approval, after which the app-server crashes.
fn fixture_binary(root: &std::path::Path) -> PathBuf {
    let source = r#"#!/usr/bin/python3
import sys,json,time
from pathlib import Path
root=Path(__ROOT__)
if '--version' in sys.argv:
 print('codex-cli 0.162.0');sys.exit(0)
def output(v): print(json.dumps(v),flush=True)
def record(v):
 with (root/'wire.jsonl').open('a') as f: f.write(json.dumps(v)+'\n')
def complete(turn,status): output({'method':'turn/completed','params':{'threadId':'fixture-thread','turn':{'id':turn,'status':status}}})
count=0
for line in sys.stdin:
 m=json.loads(line);method=m.get('method');record(m)
 if 'id' not in m: continue
 if method is None:
  if m['id']==101: complete('turn-1','completed')
  continue
 if method=='model/list': result={'data':[{'id':'fixture-model','isDefault':True,'displayName':'Fixture','inputModalities':['text'],'defaultReasoningEffort':'medium','supportedReasoningEfforts':[{'reasoningEffort':'medium','description':'Fixture'}]}]}
 elif method=='account/read': result={'account':None,'requiresOpenaiAuth':False}
 elif method in ['thread/start','thread/resume']: result={'thread':{'id':'fixture-thread','environments':[]},'approvalPolicy':'onRequest','sandbox':{'type':'readOnly'}}
 elif method=='turn/start':
  count+=1;result={'turn':{'id':'turn-'+str(count),'status':'inProgress'}}
 else: result={}
 output({'id':m['id'],'result':result})
 if method=='turn/start':
  turn='turn-'+str(count)
  output({'method':'turn/started','params':{'threadId':'fixture-thread','turn':{'id':turn,'status':'inProgress'}}})
  if count==4:
   complete(turn,'completed')
   output({'id':104,'method':'item/commandExecution/requestApproval','params':{'threadId':'child-thread','turnId':'child-turn','itemId':'item-4','command':'ls'}})
   time.sleep(.5);sys.exit(0)
  output({'id':100+count,'method':'item/commandExecution/requestApproval','params':{'threadId':'fixture-thread','turnId':turn,'itemId':'item-'+str(count),'command':'ls'}})
  if count==3: complete(turn,'interrupted')
 if method=='turn/interrupt': complete(m['params']['turnId'],'interrupted')
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

async fn next_matching<T>(
    rx: &mut broadcast::Receiver<ProviderRuntimeEvent>,
    mut pick: impl FnMut(ProviderRuntimeEvent) -> Option<T>,
) -> T {
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let Some(found) = pick(rx.recv().await.unwrap()) {
                return found;
            }
        }
    })
    .await
    .expect("expected provider event")
}

async fn opened(rx: &mut broadcast::Receiver<ProviderRuntimeEvent>) -> RequestId {
    next_matching(rx, |event| match event {
        ProviderRuntimeEvent::RequestOpened { request_id, .. } => Some(request_id),
        _ => None,
    })
    .await
}

async fn resolved(
    rx: &mut broadcast::Receiver<ProviderRuntimeEvent>,
) -> (RequestId, Value) {
    next_matching(rx, |event| match event {
        ProviderRuntimeEvent::RequestResolved {
            request_id,
            decision,
            ..
        } => Some((request_id, serde_json::to_value(decision).unwrap())),
        _ => None,
    })
    .await
}

async fn turn_completed(rx: &mut broadcast::Receiver<ProviderRuntimeEvent>) {
    next_matching(rx, |event| {
        matches!(event, ProviderRuntimeEvent::TurnCompleted { .. }).then_some(())
    })
    .await
}

fn turn(text: &str) -> SendTurnInput {
    let mut input = super::tests::queued("approval").input;
    input.text = text.into();
    input
}

// Multi-threaded so request admission and turn settlement really race.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn approvals_resolve_on_answer_and_settle_when_their_turn_ends() {
    let dir = tempfile::tempdir().unwrap();
    let (tx, mut rx) = broadcast::channel(256);
    let session = CodexSession::spawn_and_initialize(
        ThreadId("t".into()),
        dir.path().into(),
        Some("fixture-model".into()),
        None,
        None,
        "default".into(),
        None,
        None,
        None,
        CodexSpawnConfig {
            codex_binary: fixture_binary(dir.path()),
            chatgpt_owner: None,
            codex_home: None,
            remote: None,
            client_info: ClientInfo {
                name: "codemux".into(),
                title: "Codemux".into(),
                version: "fixture".into(),
            },
            mcp_registry: None,
        },
        tx,
        None,
    )
    .await
    .unwrap();

    // An answered approval reports its decision; Codex itself never does.
    session.enqueue_or_send(turn("first")).await.unwrap();
    let request = opened(&mut rx).await;
    let allow = ApprovalDecision::Allow {
        updated_input: None,
        updated_permissions: None,
    };
    session
        .respond_to_request(request.clone(), allow.clone())
        .await
        .unwrap();
    // The fixture completes the turn as soon as it reads the reply, so the
    // completion may be published before the resolution.
    let (mut decision, mut completed) = (None, false);
    while decision.is_none() || !completed {
        match next_matching(&mut rx, |event| match event {
            ProviderRuntimeEvent::RequestResolved {
                request_id,
                decision,
                ..
            } if request_id == request => Some(Some(decision)),
            ProviderRuntimeEvent::TurnCompleted { .. } => Some(None),
            _ => None,
        })
        .await
        {
            Some(found) => decision = Some(serde_json::to_value(found).unwrap()),
            None => completed = true,
        }
    }
    assert_eq!(decision, Some(serde_json::to_value(allow).unwrap()));

    // Stop during an approval settles it and unblocks the next send.
    session.enqueue_or_send(turn("second")).await.unwrap();
    let request = opened(&mut rx).await;
    session.interrupt_turn(None).await.unwrap();
    assert_eq!(resolved(&mut rx).await, (request, json!({"decision": "cancel"})));
    assert!(session.state.lock().await.pending_approvals.is_empty());
    turn_completed(&mut rx).await;

    // A turn that ends with its approval unanswered settles it too. The
    // request and the completion race on separate tasks, so either may be
    // handled first; both orders must settle it.
    let third = session.enqueue_or_send(turn("third")).await.unwrap();
    assert!(matches!(third, SendOutcome::Started(_)), "{third:?}");
    let request = opened(&mut rx).await;
    assert_eq!(resolved(&mut rx).await, (request, json!({"decision": "cancel"})));
    assert!(session.state.lock().await.pending_approvals.is_empty());

    // A sub-agent approval outlives the parent turn, then the app-server
    // crashes with no parent turn in flight: the request still expires.
    session.enqueue_or_send(turn("fourth")).await.unwrap();
    let request = opened(&mut rx).await;
    let expired = next_matching(&mut rx, |event| match event {
        ProviderRuntimeEvent::RequestResolved { request_id, .. } if request_id == request => {
            Some(false)
        }
        ProviderRuntimeEvent::RequestResponseFailed { request_id, .. }
            if request_id == request =>
        {
            Some(true)
        }
        _ => None,
    })
    .await;
    assert!(expired, "a sub-agent approval must not settle with the parent turn");
    let _ = session.shutdown().await;
}
