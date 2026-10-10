use super::*;
use std::os::unix::fs::PermissionsExt;

pub(crate) fn fixture_binary(root: &std::path::Path) -> PathBuf {
    let source = r#"#!/usr/bin/python3
import sys,json,os,threading,time
from pathlib import Path
root=Path(__ROOT__)
if '--version' in sys.argv:
 print('codex-cli 0.162.0');sys.exit(0)
if 'login' in sys.argv:
 (root/'auth-called').write_text('native-login-probe');print('Not logged in');sys.exit(1)
lock=threading.Lock()
def output(v):
 with lock: print(json.dumps(v),flush=True)
def record(v):
 with (root/'wire.jsonl').open('a') as f: f.write(json.dumps(v)+'\n')
record({'spawn':os.getpid(),'args':sys.argv[1:],'token':os.environ.get('ACCESS_TOKEN'),'home':os.environ.get('CODEX_HOME'),'base':os.environ.get('OPENAI_BASE_URL'),'key':os.environ.get('OPENAI_API_KEY'),'proxy':os.environ.get('HTTPS_PROXY')})
count=0
def finish():
 while not (root/'finish').exists(): time.sleep(.01)
 output({'method':'turn/completed','params':{'threadId':'fixture-thread','turn':{'id':'turn-1','status':'completed'}}})
for line in sys.stdin:
 m=json.loads(line);method=m.get('method');record({'rpc':method,'params':m.get('params'),'pid':os.getpid()})
 if 'id' not in m: continue
 if method=='initialize':
  if os.environ.get('ACCESS_TOKEN')=='synthetic-runtime-b' and (root/'renewal-block').exists():
   (root/'renewal-entered').write_text(str(os.getpid()))
   while not (root/'renewal-release').exists(): time.sleep(.01)
  result={'userAgent':'fixture'}
 elif method=='model/list':
  if (root/'catalog-block').exists():
   while not (root/'catalog-release').exists(): time.sleep(.01)
  result={'data':[{'id':'fixture-model','isDefault':True,'displayName':'Fixture model','inputModalities':['text'],'defaultReasoningEffort':'medium','supportedReasoningEfforts':[{'reasoningEffort':'medium','description':'Fixture effort'}]}]}
  if (root/'catalog-pages').exists():
   if m.get('params',{}).get('cursor')=='page-2': result={'data':[{'id':'fixture-next-model','displayName':'Next model','supportedReasoningEfforts':[{'reasoningEffort':'ultra','description':'Provider-authored effort'}]}]}
   else: result['nextCursor']='page-2'
 elif method=='account/read': result={'account':None,'requiresOpenaiAuth':False}
 elif method=='account/rateLimits/read': result={'rateLimits':{}}
 elif method in ['thread/start','thread/resume']: result={'thread':{'id':'fixture-thread'}}
 elif method=='turn/start':
  count+=1;result={'turn':{'id':'turn-'+str(count),'status':'inProgress'}}
 else: result={}
 output({'id':m['id'],'result':result})
 if method=='turn/start' and (root/'close-stdout-live').exists():
  os.close(1);time.sleep(30)
 if method=='turn/start' and os.environ.get('ACCESS_TOKEN')=='synthetic-runtime-a': threading.Thread(target=finish,daemon=True).start()
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
pub(crate) fn wire(root: &std::path::Path) -> Vec<Value> {
    std::fs::read_to_string(root.join("wire.jsonl"))
        .unwrap_or_default()
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect()
}

#[tokio::test]
async fn chatgpt_runtime_renews_idle_child_and_resumes_queued_turn() {
    let dir = tempfile::tempdir().unwrap();
    let owner = super::super::chatgpt::Owner::testing(dir.path().join("chatgpt"));
    let binary = fixture_binary(dir.path());
    let (tx, _) = broadcast::channel(128);
    let session = CodexSession::spawn_and_initialize(
        ThreadId("t".into()),
        dir.path().into(),
        Some("fixture-model".into()),
        Some("read-only".into()),
        Some("medium".into()),
        false,
        None,
        Some(HashMap::from([
            ("OPENAI_BASE_URL".into(), "https://evil.example.test".into()),
            ("ACCESS_TOKEN".into(), "foreign".into()),
        ])),
        None,
        CodexSpawnConfig {
            codex_binary: binary,
            chatgpt_owner: Some(owner.clone()),
            codex_home: None,
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
    let first = wire(dir.path())
        .into_iter()
        .find(|v| v.get("spawn").is_some())
        .unwrap();
    if first["token"] != "synthetic-runtime-a" {
        session.shutdown().await;
        assert_eq!(
            first["token"], "synthetic-runtime-a",
            "managed grant must reach actual Codex child rather than CLI login"
        );
    }
    assert!(owner.begin(None).await.is_err());
    assert!(owner.disconnect().await.is_err());
    let mut input = super::tests::queued("one").input;
    input.text = "first".into();
    session.enqueue_or_send(input.clone()).await.unwrap();
    owner.testing_rotate().await;
    input.text = "queued".into();
    let queued = session.enqueue_or_send(input).await.unwrap();
    assert!(matches!(queued, SendOutcome::Queued(_)));
    assert_eq!(
        wire(dir.path())
            .iter()
            .filter(|v| v.get("spawn").is_some())
            .count(),
        1,
        "renewal must preserve an in-flight turn"
    );
    std::fs::write(dir.path().join("finish"), b"complete").unwrap();
    for _ in 0..300 {
        if wire(dir.path())
            .iter()
            .filter(|v| v["rpc"] == "turn/start")
            .count()
            >= 2
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let captured = wire(dir.path());
    let id = session.provider_session_id.0.clone();
    session.shutdown().await;
    let launches: Vec<_> = captured
        .iter()
        .filter(|v| v.get("spawn").is_some())
        .collect();
    assert_eq!(
        launches.len(),
        2,
        "queued turn must restart idle child with replacement token"
    );
    assert_eq!(launches[1]["token"], "synthetic-runtime-b");
    for v in launches {
        assert_eq!(v["base"], Value::Null);
        assert_eq!(v["key"], Value::Null);
        assert_eq!(v["proxy"], Value::Null);
        assert!(!v["args"].to_string().contains("synthetic-runtime"));
        assert!(v["args"].to_string().contains("https://api.openai.com/v1"));
    }
    assert_eq!(id, "fixture-thread");
    assert_eq!(
        captured
            .iter()
            .filter(|v| v["rpc"] == "thread/start")
            .count(),
        1
    );
    assert_eq!(
        captured
            .iter()
            .filter(|v| v["rpc"] == "thread/resume" && v["params"]["threadId"] == "fixture-thread")
            .count(),
        1
    );
    assert_eq!(
        captured.iter().filter(|v| v["rpc"] == "turn/start").count(),
        2
    );
    assert!(
        owner.begin(None).await.is_ok(),
        "Stop must release real runtime owner admission"
    );
}

#[tokio::test]
async fn chatgpt_stop_reaps_owned_child_even_when_stdout_closes_early() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("close-stdout-live"), b"close").unwrap();
    let owner = super::super::chatgpt::Owner::testing(dir.path().join("chatgpt"));
    let binary = fixture_binary(dir.path());
    let (tx, _) = broadcast::channel(32);
    let session = CodexSession::spawn_and_initialize(
        ThreadId("t".into()),
        dir.path().into(),
        Some("fixture-model".into()),
        None,
        None,
        false,
        None,
        None,
        None,
        CodexSpawnConfig {
            codex_binary: binary,
            chatgpt_owner: Some(owner.clone()),
            codex_home: None,
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
    let pid = wire(dir.path())
        .iter()
        .find_map(|v| v["spawn"].as_u64())
        .unwrap();
    let mut input = super::tests::queued("one").input;
    input.text = "close".into();
    session.enqueue_or_send(input).await.unwrap();
    for _ in 0..100 {
        if !session.child().await.is_alive() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    session.shutdown().await;
    let still_alive = std::path::Path::new(&format!("/proc/{pid}")).exists();
    // Failure-safe cleanup is restricted to this fixture's recorded owned PID.
    if still_alive {
        unsafe {
            libc::kill(pid as i32, libc::SIGKILL);
        }
        for _ in 0..100 {
            if !std::path::Path::new(&format!("/proc/{pid}")).exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
    assert!(!still_alive,"stdout EOF is not owned process reap; account changes cannot be admitted while an old child retains the token");
    assert!(owner.acquire_runtime().is_ok());
}

#[tokio::test]
async fn chatgpt_retry_after_retirement_reacquires_runtime_admission() {
    for rotate in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("chatgpt");
        let owner = super::super::chatgpt::Owner::testing(root.clone());
        let binary = fixture_binary(dir.path());
        let (tx, _) = broadcast::channel(32);
        let session = CodexSession::spawn_and_initialize(
            ThreadId("retry".into()),
            dir.path().into(),
            Some("fixture-model".into()),
            None,
            None,
            false,
            None,
            None,
            None,
            CodexSpawnConfig {
                codex_binary: binary,
                chatgpt_owner: Some(owner.clone()),
                codex_home: None,
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
        let state = root.join("state.json");
        let saved = std::fs::read(&state).unwrap(); // Synthetic fixture credentials only.
        std::fs::write(&state, b"temporary invalid fixture state").unwrap();
        let mut input = super::tests::queued("retry").input;
        input.text = "retry".into();
        assert!(session.enqueue_or_send(input.clone()).await.is_err());
        std::fs::write(&state, saved).unwrap();
        if rotate {
            owner.testing_rotate().await;
        }
        let sent = session.enqueue_or_send(input).await;
        let child_alive = session.child().await.is_alive();
        let connection = owner.begin(None).await;
        let blocked = connection.is_err();
        if let Ok(launch) = connection {
            owner
                .cancel(launch.status.attempt_id.as_deref().unwrap())
                .await
                .unwrap();
        }
        session.shutdown().await;
        assert!(
            sent.is_ok(),
            "a recovered grant must resume the original managed session"
        );
        assert!(
            child_alive,
            "the recovered managed child must actually be running"
        );
        assert!(
            blocked,
            "a retry may not launch a bearer-owning child without runtime admission"
        );
    }
}


/// The executable gates only replacement initialize. All session-map checks,
/// eviction, independent notification-driven queue drain and shutdown are real.
#[tokio::test]
async fn chatgpt_renewal_cannot_be_evicted_by_auto_resume_or_start() {
    use crate::agent_provider::{AgentProvider, StartSessionInput};
    use super::super::{CodexAgentProvider, CodexProviderConfig};

    let mut observations = Vec::new();
    for direct_start in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let owner = super::super::chatgpt::Owner::testing(dir.path().join("chatgpt"));
        let mut provider = CodexAgentProvider::new(CodexProviderConfig {
            codex_binary: fixture_binary(dir.path()),
            ..Default::default()
        });
        provider.chatgpt_owner = Some(owner.clone());
        let thread = ThreadId("renewal-race".into());
        let input: StartSessionInput = serde_json::from_value(json!({
            "thread_id": thread.0, "cwd": dir.path(), "model": "fixture-model",
            "resume_cursor": null, "permission_mode": "read-only",
            "additional_directories": [], "env": null, "workspace_id": null,
            "recorded_usage_baseline": null
        })).unwrap();
        provider.start_session(input.clone()).await.unwrap();
        let session = provider.sessions.read().await.get(&thread).unwrap().clone();
        let mut events = provider.event_tx.subscribe();
        let mut turn = super::tests::queued("race").input;
        turn.thread_id = thread.clone();
        turn.text = "first".into();
        session.enqueue_or_send(turn.clone()).await.unwrap();
        owner.testing_rotate().await;
        turn.text = "queued".into();
        let SendOutcome::Queued(queued_id) = session.enqueue_or_send(turn).await.unwrap() else {
            panic!("fixture must queue behind the active turn");
        };
        std::fs::write(dir.path().join("renewal-block"), b"gate").unwrap();
        std::fs::write(dir.path().join("finish"), b"complete").unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            while !dir.path().join("renewal-entered").exists() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        }).await.expect("replacement must enter the real initialize gate");

        // Poll the actual provider path while renewal owns outbound. Auto-resume
        // repeats its has_session check exactly as ensure_live_session does.
        let mut resume = Box::pin(async {
            if direct_start || (!provider.has_session(&thread).await
                && !provider.has_session(&thread).await) {
                provider.start_session(input.clone()).await.map(|_| ())
            } else {
                Ok(())
            }
        });
        let first_poll = futures_util::poll!(resume.as_mut());
        let retained_during_init = provider.sessions.try_read().ok().is_some_and(|map| {
            map.get(&thread).is_some_and(|current| Arc::ptr_eq(current, &session))
        });
        std::fs::write(dir.path().join("renewal-release"), b"release").unwrap();
        let resume_result = match first_poll {
            std::task::Poll::Ready(result) => result,
            std::task::Poll::Pending => tokio::time::timeout(Duration::from_secs(5), resume)
                .await.expect("auto-resume must settle after initialization"),
        };
        let dispatched = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let ProviderRuntimeEvent::QueuedTurnDispatched { queued_id: id, .. } = events.recv().await.unwrap() {
                    if id == queued_id { break; }
                }
            }
        }).await.is_ok();
        let retained_after = provider.sessions.read().await.get(&thread)
            .is_some_and(|current| Arc::ptr_eq(current, &session));
        let renewed_alive = session.child().await.is_alive();
        let launches = wire(dir.path()).iter().filter(|v| v.get("spawn").is_some()).count();
        let expected_resume = if direct_start {
            matches!(resume_result, Err(ProviderError::ValidationError { .. }))
        } else {
            resume_result.is_ok()
        };
        // Always clean up before assertions, including the baseline's eviction.
        provider.stop_session(thread.clone()).await.unwrap();
        session.shutdown().await;
        let stopped = !session.child().await.is_alive();
        let admission_released = owner.acquire_runtime().is_ok();
        observations.push((direct_start, retained_during_init, retained_after,
            renewed_alive, launches, dispatched, expected_resume, stopped, admission_released));
    }
    for (direct, during, after, alive, launches, dispatched, result, stopped, released) in observations {
        assert!(during, "renewal must retain a readable exact map binding during initialize (direct_start={direct})");
        assert!(after, "renewal must retain the exact provider session owner (direct_start={direct})");
        assert!(alive, "auto-resume must not kill the child that just dispatched the queue");
        assert_eq!(launches, 2, "renewal must not spawn a third auto-resume child");
        assert!(dispatched, "the queued turn must dispatch exactly through renewal");
        assert!(result, "auto-resume succeeds; explicit duplicate start remains rejected");
        assert!(stopped && released, "Stop must reap the replacement and release runtime admission");
    }
}


#[tokio::test]
async fn chatgpt_dead_child_recovery_preserves_generic_and_managed_provider_paths() {
    use crate::agent_provider::{AgentProvider, StartSessionInput};
    use super::super::{CodexAgentProvider, CodexProviderConfig};

    for managed in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let owner = super::super::chatgpt::Owner::testing(dir.path().join("chatgpt"));
        let mut provider = CodexAgentProvider::new(CodexProviderConfig {
            codex_binary: fixture_binary(dir.path()),
            codex_home: Some(dir.path().join("generic-home")),
            ..Default::default()
        });
        if managed { provider.chatgpt_owner = Some(owner.clone()); }
        let thread = ThreadId("dead-child-control".into());
        let input: StartSessionInput = serde_json::from_value(json!({
            "thread_id": thread.0, "cwd": dir.path(), "model": "fixture-model",
            "resume_cursor": null, "permission_mode": "read-only",
            "additional_directories": [], "workspace_id": null,
            "recorded_usage_baseline": null,
            "env": { "ACCESS_TOKEN": "synthetic-generic", "OPENAI_API_KEY": "",
                "OPENAI_BASE_URL": "", "HTTPS_PROXY": "" }
        })).unwrap();
        provider.start_session(input.clone()).await.unwrap();
        let old = provider.sessions.read().await.get(&thread).unwrap().clone();
        old.child().await.shutdown_owned().await.unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            while provider.has_session(&thread).await {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        }).await.expect("watchdog must expose an actually dead child as absent");
        let mut resume = input;
        resume.resume_cursor = Some(json!({"threadId": "fixture-thread"}));
        provider.start_session(resume).await.unwrap();
        let current = provider.sessions.read().await.get(&thread).unwrap().clone();
        let live = provider.has_session(&thread).await && current.child().await.is_alive();
        let replaced = !Arc::ptr_eq(&old, &current);
        let captured = wire(dir.path());
        provider.stop_session(thread).await.unwrap();
        assert!(live && replaced, "actual dead-child auto-resume must remain available (managed={managed})");
        assert_eq!(captured.iter().filter(|v| v.get("spawn").is_some()).count(), 2);
        assert_eq!(captured.iter().filter(|v| v["rpc"] == "thread/resume").count(), 1);
        assert!(!current.child().await.is_alive(), "Stop must reap the resumed child");
        assert!(owner.acquire_runtime().is_ok(), "Stop must release managed ownership");
    }
}


/// Scheduling-only equivalent of GitTurnDispatchCheckpoint's timeline gate:
/// session::enqueue_or_send owns outbound before prepare; prepare retains the
/// timeline until commit/abort (commands/agent_chat.rs:1435-1467). No AppHandle,
/// global DB, native UI, or checkpoint implementation is replaced in production.
#[derive(Debug)]
struct DispatchTimelineFixture {
    timeline: Arc<Mutex<()>>,
    prepared: Mutex<Option<tokio::sync::OwnedMutexGuard<()>>>,
    entered: std::sync::atomic::AtomicBool,
}

#[async_trait::async_trait]
impl crate::agent_provider::TurnDispatchCheckpoint for DispatchTimelineFixture {
    async fn prepare(&self) {
        self.entered.store(true, Ordering::SeqCst);
        let guard = self.timeline.clone().lock_owned().await;
        *self.prepared.lock().await = Some(guard);
    }
    async fn commit(&self) { self.prepared.lock().await.take(); }
    async fn abort(&self) { self.prepared.lock().await.take(); }
}

fn timeline_start_input(root: &std::path::Path, thread: &ThreadId) -> crate::agent_provider::StartSessionInput {
    serde_json::from_value(json!({
        "thread_id": thread.0, "cwd": root, "model": "fixture-model",
        "resume_cursor": null, "permission_mode": "read-only",
        "additional_directories": [], "env": null, "workspace_id": null,
        "recorded_usage_baseline": null
    })).unwrap()
}

#[tokio::test]
async fn chatgpt_checkpoint_dispatch_revert_and_duplicate_start_do_not_deadlock() {
    use crate::agent_provider::AgentProvider;
    use super::super::{CodexAgentProvider, CodexProviderConfig};
    use std::task::Poll;

    let dir = tempfile::tempdir().unwrap();
    let owner = super::super::chatgpt::Owner::testing(dir.path().join("chatgpt"));
    let mut provider = CodexAgentProvider::new(CodexProviderConfig {
        codex_binary: fixture_binary(dir.path()), ..Default::default()
    });
    provider.chatgpt_owner = Some(owner.clone());
    let thread = ThreadId("timeline-deadlock".into());
    let input = timeline_start_input(dir.path(), &thread);
    provider.start_session(input.clone()).await.unwrap();
    let session = provider.sessions.read().await.get(&thread).unwrap().clone();
    let timeline = Arc::new(Mutex::new(()));
    // Revert owns timeline before reading provider.turn_active, exactly as
    // agent_chat_revert_turn_checkpoint does at commands/agent_chat.rs:1749-1750.
    let timeline_guard = timeline.clone().lock_owned().await;
    let checkpoint = Arc::new(DispatchTimelineFixture {
        timeline, prepared: Mutex::new(None),
        entered: std::sync::atomic::AtomicBool::new(false),
    });
    let mut turn = super::tests::queued("timeline").input;
    turn.thread_id = thread.clone();
    turn.turn_checkpoint = Some(checkpoint.clone());
    let mut send = Box::pin(provider.send_turn(turn));
    assert!(matches!(futures_util::poll!(send.as_mut()), Poll::Pending));
    assert!(checkpoint.entered.load(Ordering::SeqCst), "actual dispatch must reach prepare while owning outbound");
    let mut duplicate = Box::pin(provider.start_session(input));
    assert!(matches!(futures_util::poll!(duplicate.as_mut()), Poll::Pending));
    let mut revert_read = Box::pin(provider.turn_active(&thread));
    let revert_poll = futures_util::poll!(revert_read.as_mut());
    let readable = matches!(revert_poll, Poll::Ready(false));
    drop(revert_read);
    // On RED, dropping duplicate breaks map -> outbound so cleanup itself cannot
    // deadlock. On GREEN it must remain pending through the real rollback RPC.
    let mut duplicate = Some(duplicate);
    if !readable { duplicate.take(); }
    let reverted = if readable {
        tokio::time::timeout(Duration::from_secs(5), provider.rollback_conversation(thread.clone(), 1))
            .await.expect("timeline-held revert must finish").is_ok()
    } else { false };
    drop(timeline_guard);
    let sent = tokio::time::timeout(Duration::from_secs(5), send)
        .await.expect("checkpointed dispatch must finish after timeline release").is_ok();
    let rejected = if let Some(duplicate) = duplicate {
        matches!(tokio::time::timeout(Duration::from_secs(5), duplicate)
            .await.expect("duplicate start must settle"), Err(ProviderError::ValidationError { .. }))
    } else { false };
    let retained = provider.sessions.read().await.get(&thread)
        .is_some_and(|current| Arc::ptr_eq(current, &session));
    let captured = wire(dir.path());
    provider.stop_session(thread).await.unwrap();
    let stopped = !session.child().await.is_alive();
    let released = owner.acquire_runtime().is_ok();
    assert!(readable, "session-map/outbound/timeline deadlock: duplicate start blocked timeline-held revert's actual turn_active read");
    assert!(reverted && sent && rejected && retained, "revert and real dispatch must complete without replacing the duplicate's exact session binding");
    let rpcs: Vec<_> = captured.iter().filter_map(|v| v["rpc"].as_str()).collect();
    assert!(rpcs.iter().position(|r| *r == "thread/rollback").unwrap()
        < rpcs.iter().position(|r| *r == "turn/start").unwrap());
    assert_eq!(captured.iter().filter(|v| v.get("spawn").is_some()).count(), 1);
    assert!(stopped && released, "Stop must reap the exact owned child and release managed admission");
}


/// Await the production watchdog event, not a sleep chosen to guess its timing.
async fn timeline_wait_for_dead_session(
    events: &mut broadcast::Receiver<ProviderRuntimeEvent>, thread: &ThreadId,
) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let ProviderRuntimeEvent::SessionStateChanged {
                thread_id, status: SessionStatus::Error { .. },
            } = events.recv().await.unwrap() {
                if &thread_id == thread { break; }
            }
        }
    }).await.expect("actual dead child must be classified by the watchdog");
}

#[tokio::test]
async fn chatgpt_duplicate_start_rechecks_exact_arc_after_outbound_wait() {
    use crate::agent_provider::AgentProvider;
    use super::super::{CodexAgentProvider, CodexProviderConfig};
    use std::task::Poll;

    let dir = tempfile::tempdir().unwrap();
    let owner = super::super::chatgpt::Owner::testing(dir.path().join("chatgpt"));
    let config = CodexProviderConfig {
        codex_binary: fixture_binary(dir.path()), ..Default::default()
    };
    let mut provider = CodexAgentProvider::new(config.clone());
    provider.chatgpt_owner = Some(owner.clone());
    let thread = ThreadId("binding-recheck".into());
    let input = timeline_start_input(dir.path(), &thread);
    provider.start_session(input.clone()).await.unwrap();
    let old = provider.sessions.read().await.get(&thread).unwrap().clone();
    let mut events = provider.event_tx.subscribe();
    old.child().await.shutdown_owned().await.unwrap();
    timeline_wait_for_dead_session(&mut events, &thread).await;
    assert!(old.is_dead());

    // Prebuild a real replacement at the same logical thread in an independent
    // map. Publishing its Arc is the only scheduling fixture seam: it models a
    // competing rebuild while the candidate waits for the OLD outbound owner.
    let mut donor = CodexAgentProvider::new(config);
    donor.chatgpt_owner = Some(owner.clone());
    donor.start_session(input.clone()).await.unwrap();
    let replacement = donor.sessions.write().await.remove(&thread).unwrap();
    let outbound = old.lifecycle_guard().await;
    let mut start = Box::pin(provider.start_session(input));
    assert!(matches!(futures_util::poll!(start.as_mut()), Poll::Pending));
    let changed = if let Ok(mut map) = provider.sessions.try_write() {
        map.insert(thread.clone(), replacement.clone()); true
    } else { false };
    // Drop the future on RED to release the original map owner before cleanup.
    let mut start = Some(start);
    if !changed { start.take(); }
    drop(outbound);
    let rejected = if let Some(start) = start {
        matches!(tokio::time::timeout(Duration::from_secs(5), start)
            .await.expect("changed binding must be rechecked"), Err(ProviderError::ValidationError { .. }))
    } else { false };
    let retained = provider.sessions.read().await.get(&thread)
        .is_some_and(|current| Arc::ptr_eq(current, &replacement));
    let alive = replacement.child().await.is_alive();
    let captured = wire(dir.path());
    provider.stop_session(thread).await.unwrap();
    old.shutdown().await;
    replacement.shutdown().await;
    assert!(changed, "start may not hold the session map while awaiting old outbound");
    assert!(rejected && retained && alive, "stale dead classification must not evict the live replacement Arc");
    assert_eq!(captured.iter().filter(|v| v.get("spawn").is_some()).count(), 2,
        "binding recheck must not spawn a third child");
    assert!(owner.acquire_runtime().is_ok());
}

#[tokio::test]
async fn chatgpt_duplicate_start_retries_removed_binding_while_stop_waits() {
    use crate::agent_provider::AgentProvider;
    use super::super::{CodexAgentProvider, CodexProviderConfig};
    use std::task::Poll;

    let dir = tempfile::tempdir().unwrap();
    let owner = super::super::chatgpt::Owner::testing(dir.path().join("chatgpt"));
    let mut provider = CodexAgentProvider::new(CodexProviderConfig {
        codex_binary: fixture_binary(dir.path()), ..Default::default()
    });
    provider.chatgpt_owner = Some(owner.clone());
    let thread = ThreadId("stop-binding-recheck".into());
    let mut input = timeline_start_input(dir.path(), &thread);
    provider.start_session(input.clone()).await.unwrap();
    let old = provider.sessions.read().await.get(&thread).unwrap().clone();
    input.resume_cursor = Some(json!({"threadId": "fixture-thread"}));
    let outbound = old.lifecycle_guard().await;
    let mut start = Box::pin(provider.start_session(input));
    assert!(matches!(futures_util::poll!(start.as_mut()), Poll::Pending));
    let mut stop = Box::pin(provider.stop_session(thread.clone()));
    assert!(matches!(futures_util::poll!(stop.as_mut()), Poll::Pending));
    // Stop must unbind before waiting for shutdown's outbound gate. A duplicate
    // carrying a now-obsolete live Arc must retry the empty map, not reject it.
    let unbound = provider.sessions.try_read().ok()
        .is_some_and(|map| !map.contains_key(&thread));
    let mut start = Some(start);
    if !unbound { start.take(); }
    drop(outbound);
    let resumed = if let Some(start) = start {
        let (started, stopped) = tokio::time::timeout(Duration::from_secs(5), async {
            tokio::join!(start, stop)
        }).await.expect("duplicate and Stop must both settle after outbound release");
        stopped.unwrap(); started.is_ok()
    } else {
        tokio::time::timeout(Duration::from_secs(5), stop).await.unwrap().unwrap(); false
    };
    let current = provider.sessions.read().await.get(&thread).cloned();
    let fresh = if let Some(current) = current.as_ref() {
        !Arc::ptr_eq(current, &old) && current.child().await.is_alive()
    } else { false };
    let captured = wire(dir.path());
    if current.is_some() { provider.stop_session(thread).await.unwrap(); }
    assert!(unbound, "pending duplicate start must not prevent Stop from unbinding the map");
    assert!(resumed && fresh, "changed-to-absent binding must retry instead of rejecting the stale live Arc");
    assert!(!old.child().await.is_alive());
    assert!(matches!(old.state.lock().await.status, SessionStatus::Closed));
    assert!(!current.unwrap().child().await.is_alive());
    assert_eq!(captured.iter().filter(|v| v.get("spawn").is_some()).count(), 2);
    assert_eq!(captured.iter().filter(|v| v["rpc"] == "thread/resume").count(), 1);
    assert!(owner.acquire_runtime().is_ok(), "final Stop must release all managed runtime ownership");
}
