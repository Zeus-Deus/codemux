use super::*;
use std::path::Path;
impl BindingStore for crate::database::DatabaseStore {
    fn load(&self, thread: &str) -> Result<Option<Binding>, String> {
        self.hermes_binding(thread)
    }
    fn save(&self, binding: &Binding) -> Result<(), String> {
        self.save_hermes_binding(binding)
    }
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn review_l3_native_stop_wakes_cold_hermes_profile_operation_wait() {
    use tauri::Manager;
    use crate::commands::agent_chat::{self, ProviderRegistry};
    let binary = std::env::var_os("CODEMUX_NATIVE_CURSOR_FIXTURE").map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::current_exe().unwrap().parent().unwrap().parent().unwrap().join(format!("fake_cursor_acp{}", std::env::consts::EXE_SUFFIX)));
    assert!(binary.is_file(), "synthetic ACP fixture required, never a real Hermes CLI");
    let placeholder = Arc::new(crate::agent_provider::cursor::CursorAgentProvider::new(
        crate::agent_provider::cursor::CursorProviderConfig {binary:binary.clone(), event_channel_capacity:128},
    ));
    let (app, root, workspace, pane) = crate::agent_control::guard_tests::fixture(placeholder, "cold-hermes-b").await;
    let provider = Arc::new(HermesProvider::new(Arc::new(binding::AppBindingStore(app.handle().clone()))));
    app.state::<ProviderRegistry>().set_hermes(provider.clone()).await;
    app.state::<crate::state::AppStateStore>().claim_agent_chat_pane_checked(&pane,
        ProviderKind::Hermes,"cold-hermes-b",Some("cold-hermes-b"), |_| Ok(())).unwrap();
    let db = app.state::<crate::database::DatabaseStore>();
    db.upsert_agent_chat_session("cold-hermes-b",&workspace,Some(root.path().to_str().unwrap()),"hermes").unwrap();
    db.update_agent_chat_session_config("cold-hermes-b", &crate::database::AgentChatSessionConfig {
        permission_mode:Some(None),..Default::default()
    }).unwrap();
    std::fs::write(root.path().join("config.yaml"),"model: {provider: custom}\n").unwrap();
    let profile = profile::resolve(&binary, root.path(), "default").unwrap();
    let mut b = test_binding(root.path()); b.profile=profile.clone(); b.thread_id="cold-hermes-b".into();
    b.workspace_id=Some(workspace); b.current_native_id=b.acp_session_id.clone(); b.cleanup_pending=false;
    db.save_hermes_binding(&b).unwrap();
    let child = JsonRpcChild::spawn(SpawnConfig {program:binary,args:vec![],env:HashMap::new(),cwd:Some(root.path().into()),default_timeout:Duration::from_secs(2)}).await.unwrap();
    let (barrier, _rx) = mpsc::unbounded_channel();
    let runtime = Arc::new(Runtime {
        fixture_start_wait:Notify::new(),child,ownership:std::sync::Mutex::new(None),installation_stamp:installation_stamp(&profile).unwrap(),profile:profile.clone(),
        operation:Mutex::new(()),routes:Mutex::new(HashMap::new()),commands:Mutex::new(HashMap::new()),queue:Mutex::new(Queue::default()),barrier,
    });
    provider.inner.runtimes.lock().await.insert(runtime_key(&profile),runtime.clone());
    // Same actual runtime.operation mutex that A's foreground approval owns.
    let held_by_a = runtime.operation.lock().await;
    let handle = app.handle().clone();
    let send = tokio::spawn(async move {
        agent_chat::agent_chat_send_turn(handle,ProviderKind::Hermes,
            serde_json::from_value(json!({"thread_id":"cold-hermes-b","text":"Never dispatch","model_override":null})).unwrap()).await
    });
    tokio::time::timeout(Duration::from_secs(2),runtime.fixture_start_wait.notified()).await.unwrap();
    let stop = agent_chat::agent_chat_stop_session(app.handle().clone(),ProviderKind::Hermes,ThreadId("cold-hermes-b".into()));
    tokio::pin!(stop);
    let stop_before_release = tokio::time::timeout(Duration::from_millis(150),stop.as_mut()).await;
    let woke = stop_before_release.is_ok();
    if let Ok(result) = stop_before_release { result.unwrap(); }
    drop(held_by_a);
    if !woke { stop.await.unwrap(); }
    assert!(send.await.unwrap().is_err());
    runtime.child.shutdown().await.unwrap();
    assert!(woke, "Stop must release B's activity/native locks while A still owns the Hermes profile gate");
    assert!(!db.hermes_binding("cold-hermes-b").unwrap().unwrap().cleanup_pending,
        "cancelled pre-dispatch startup must not access new/load or commit a worktree execution hold");
    assert!(!provider.has_session(&ThreadId("cold-hermes-b".into())).await);
}

#[derive(Debug, Default)]
struct ReviewDispatchAuthority {
    revoked: std::sync::atomic::AtomicBool,
    prepared: std::sync::atomic::AtomicUsize,
    committed: std::sync::atomic::AtomicUsize,
    aborted: std::sync::atomic::AtomicUsize,
}
#[async_trait]
impl crate::agent_provider::types::TurnDispatchCheckpoint for ReviewDispatchAuthority {
    fn authorize_dispatch(&self) -> Result<(), ProviderError> {
        if self.revoked.load(std::sync::atomic::Ordering::SeqCst) {
            Err(invalid("grant_revoked: synthetic Q1 authority revoked"))
        } else { Ok(()) }
    }
    async fn prepare(&self) { self.prepared.fetch_add(1, std::sync::atomic::Ordering::SeqCst); }
    async fn commit(&self) { self.committed.fetch_add(1, std::sync::atomic::Ordering::SeqCst); }
    async fn abort(&self) { self.aborted.fetch_add(1, std::sync::atomic::Ordering::SeqCst); }
}

async fn review_pump_fixture() -> (tempfile::TempDir, HermesProvider, Arc<Runtime>, Arc<Chat>, tokio::task::JoinHandle<()>) {
    let root = tempfile::tempdir().unwrap();
    // Launch only the compiled synthetic ACP peer, never the official Hermes CLI.
    let binary = std::env::var_os("CODEMUX_NATIVE_CURSOR_FIXTURE").map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::current_exe().unwrap().parent().unwrap().parent().unwrap().join(format!("fake_cursor_acp{}", std::env::consts::EXE_SUFFIX)));
    assert!(binary.is_file(), "build the fake_cursor_acp test fixture first");
    std::fs::write(root.path().join("config.yaml"), "model: {provider: custom}\n").unwrap();
    let profile = profile::resolve(&binary, root.path(), "default").unwrap();
    let provider = HermesProvider::new(Arc::new(crate::database::init_test_database()));
    let env = HashMap::from([(
        "CODEMUX_AGENT_CONTROL_FIXTURE_ROOT".into(), root.path().to_string_lossy().into_owned(),
    )]);
    let child = JsonRpcChild::spawn(SpawnConfig {program:binary, args:vec![], env, cwd:Some(root.path().into()), default_timeout:Duration::from_secs(3)}).await.unwrap();
    let mut notifications = child.notifications();
    let mut requests = child.incoming_requests().unwrap();
    let (barrier, mut barriers) = mpsc::unbounded_channel::<oneshot::Sender<()>>();
    let runtime = Arc::new(Runtime {fixture_start_wait:Notify::new(), child, ownership:std::sync::Mutex::new(None),
        installation_stamp:installation_stamp(&profile).unwrap(), profile:profile.clone(), operation:Mutex::new(()),
        routes:Mutex::new(HashMap::new()), commands:Mutex::new(HashMap::new()), queue:Mutex::new(Queue::default()), barrier});
    let mut binding = test_binding(root.path()); binding.profile=profile; binding.acp_session_id=Some("review-session".into());
    binding.current_native_id=Some("existing-native".into());
    let chat = Arc::new(Chat {binding:Mutex::new(binding), runtime:runtime.clone(), state:Mutex::new(ChatState::default()), finished:Notify::new()});
    provider.inner.chats.lock().await.insert(ThreadId("thread".into()), chat.clone());
    runtime.routes.lock().await.insert("review-session".into(), Arc::downgrade(&chat));
    let inner = provider.inner.clone(); let r = runtime.clone();
    let task = tokio::spawn(async move {
        loop {
            tokio::select! { biased;
                Some(request) = requests.recv() => inner.permission(&r, request).await,
                Ok(n) = notifications.recv() => inner.notification(&r, n).await,
                Some(done) = barriers.recv() => { let _ = done.send(()); },
                else => break,
            }
        }
    });
    (root, provider, runtime, chat, task)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn review_r2_denied_hermes_job_preserves_healthy_chat_and_native_followup() {
    let (_root, provider, runtime, chat, task) = review_pump_fixture().await;
    let mut rx = provider.inner.events.subscribe();
    // Deterministic busy-profile barrier: queue both inputs before pump ownership.
    let held = runtime.operation.lock().await;
    runtime.queue.lock().await.running = true;
    let authority = Arc::new(ReviewDispatchAuthority::default());
    let mut outside = turn("thread", "never-dispatch-q1"); outside.turn_checkpoint=Some(authority.clone());
    let q1 = provider.send_turn(outside).await.unwrap().queued_id.unwrap();
    let q2 = provider.send_turn(turn("thread", "native-q2")).await.unwrap().queued_id.unwrap();
    authority.revoked.store(true, std::sync::atomic::Ordering::SeqCst);
    drop(held);
    tokio::time::timeout(Duration::from_secs(3), provider.inner.clone().pump(runtime.clone())).await.unwrap();
    let mut events = vec![]; while let Ok(e) = rx.try_recv() { events.push(e); }
    let healthy = provider.has_session(&ThreadId("thread".into())).await;
    let error = chat.state.lock().await.error.clone();
    let active = chat.state.lock().await.active.clone();
    runtime.child.shutdown().await.unwrap(); task.abort(); let _ = task.await;
    assert!(healthy, "input authority denial must not make has_session false: {events:?}");
    assert!(error.is_none(), "input denial poisoned healthy chat: {error:?}");
    assert!(active.is_none(), "denied claim must be released");
    assert!(!events.iter().any(|e| matches!(e, ProviderRuntimeEvent::SessionStateChanged {status:SessionStatus::Error {..}, ..})), "input rejection is not session-fatal: {events:?}");
    assert!(events.iter().any(|e| matches!(e, ProviderRuntimeEvent::QueuedTurnCancelled {queued_id, ..} if queued_id == &q1)));
    assert!(!events.iter().any(|e| matches!(e, ProviderRuntimeEvent::QueuedTurnDispatched {queued_id, ..} if queued_id == &q1)));
    assert!(events.iter().any(|e| matches!(e, ProviderRuntimeEvent::QueuedTurnDispatched {queued_id, ..} if queued_id == &q2)), "Q2 was lost: {events:?}");
    assert!(events.iter().any(|e| matches!(e, ProviderRuntimeEvent::ItemCompleted {item:CompletedItem::AssistantText {text}, ..} if text == "native-q2")), "Q2 never reached transport: {events:?}");
    assert_eq!(authority.prepared.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(authority.aborted.load(std::sync::atomic::Ordering::SeqCst), 1);
    assert_eq!(authority.committed.load(std::sync::atomic::Ordering::SeqCst), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn review_r2_transport_failure_remains_chat_fatal_and_cancels_followup() {
    let (_root, provider, runtime, chat, task) = review_pump_fixture().await;
    let mut rx=provider.inner.events.subscribe();
    runtime.queue.lock().await.running=true;
    let q1=provider.send_turn(turn("thread", "review-runtime-fatal")).await.unwrap().queued_id.unwrap();
    let q2=provider.send_turn(turn("thread", "native-q2")).await.unwrap().queued_id.unwrap();
    tokio::time::timeout(Duration::from_secs(3), provider.inner.clone().pump(runtime.clone())).await.unwrap();
    let mut events=vec![]; while let Ok(event)=rx.try_recv() { events.push(event); }
    assert!(runtime.child.is_alive(), "RPC failure control must not be a child-exit fixture");
    assert!(!provider.has_session(&ThreadId("thread".into())).await);
    assert!(chat.state.lock().await.error.as_deref().unwrap().contains("synthetic runtime transport failure"));
    assert!(events.iter().any(|e| matches!(e, ProviderRuntimeEvent::QueuedTurnDispatched {queued_id, ..} if queued_id == &q1)));
    assert!(events.iter().any(|e| matches!(e, ProviderRuntimeEvent::QueuedTurnCancelled {queued_id, ..} if queued_id == &q2)));
    assert!(!events.iter().any(|e| matches!(e, ProviderRuntimeEvent::QueuedTurnDispatched {queued_id, ..} if queued_id == &q2)));
    assert!(events.iter().any(|e| matches!(e, ProviderRuntimeEvent::SessionStateChanged {status:SessionStatus::Error {..}, ..})));
    runtime.child.shutdown().await.unwrap(); task.abort(); let _=task.await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn review_r2_interrupt_cancels_real_prompt_and_queue_but_allows_fresh_native_work() {
    let (_root, provider, runtime, _chat, task) = review_pump_fixture().await;
    let mut rx=provider.inner.events.subscribe();
    runtime.queue.lock().await.running=true;
    provider.send_turn(turn("thread", "review-hold")).await.unwrap();
    let inner=provider.inner.clone(); let r=runtime.clone();
    let pump=tokio::spawn(async move { inner.pump(r).await });
    tokio::time::timeout(Duration::from_secs(3), async {
        loop { if matches!(rx.recv().await.unwrap(), ProviderRuntimeEvent::RequestOpened {..}) { break; } }
    }).await.unwrap();
    let q2=provider.send_turn(turn("thread", "never-dispatch-q2")).await.unwrap().queued_id.unwrap();
    provider.interrupt_turn(ThreadId("thread".into()), None).await.unwrap();
    tokio::time::timeout(Duration::from_secs(3), pump).await.unwrap().unwrap();
    let mut cancelled=vec![]; while let Ok(e)=rx.try_recv() { cancelled.push(e); }
    assert!(cancelled.iter().any(|e| matches!(e, ProviderRuntimeEvent::QueuedTurnCancelled {queued_id, ..} if queued_id == &q2)));
    assert!(!cancelled.iter().any(|e| matches!(e, ProviderRuntimeEvent::QueuedTurnDispatched {queued_id, ..} if queued_id == &q2)));
    assert!(cancelled.iter().any(|e| matches!(e, ProviderRuntimeEvent::TurnCompleted {status:TurnStatus::Error {subtype, ..}, ..} if subtype == "cancelled")));
    assert!(provider.has_session(&ThreadId("thread".into())).await);
    runtime.queue.lock().await.running=true;
    provider.send_turn(turn("thread", "fresh-native-after-interrupt")).await.unwrap();
    tokio::time::timeout(Duration::from_secs(3), provider.inner.clone().pump(runtime.clone())).await.unwrap();
    let mut fresh=vec![]; while let Ok(e)=rx.try_recv() { fresh.push(e); }
    assert!(fresh.iter().any(|e| matches!(e, ProviderRuntimeEvent::ItemCompleted {item:CompletedItem::AssistantText {text}, ..} if text == "fresh-native-after-interrupt")));
    provider.stop_session(ThreadId("thread".into())).await.unwrap();
    assert!(!provider.has_session(&ThreadId("thread".into())).await);
    runtime.child.shutdown().await.unwrap(); task.abort(); let _=task.await;
}

fn start(thread: &str, cwd: &Path, profile: &Profile) -> StartSessionInput {
    serde_json::from_value(json!({"thread_id":thread,"cwd":cwd,"model":null,"resume_cursor":null,"permission_mode":null,"additional_directories":[],"env":null,"extra":{"hermes_profile":profile}})).unwrap()
}
fn turn(thread: &str, prompt: &str) -> SendTurnInput {
    serde_json::from_value(json!({"thread_id":thread,"text":prompt,"model_override":null})).unwrap()
}
fn test_binding(root: &Path) -> Binding {
    Binding {
        schema_version: 1,
        profile: profile::resolve(&std::env::current_exe().unwrap(), root, "default").unwrap(),
        thread_id: "thread".into(),
        workspace_id: Some("workspace".into()),
        // Match start_session's durable-path contract (Windows adds a verbatim prefix).
        cwd: root.canonicalize().unwrap(),
        acp_session_id: Some("stable".into()),
        current_native_id: Some("native".into()),
        root_native_id: Some("native".into()),
        model_override: None,
        permission_mode: None,
        resolved_model: Some("custom:fixture".into()),
        compatibility: "ready".into(),
        cleanup_pending: true,
    }
}
#[test]
fn hermes_durable_binding_provenance_and_cleanup_hold() {
    let root = tempfile::tempdir().unwrap();
    let db = crate::database::init_test_database();
    let mut b = test_binding(root.path());
    b.validate_cwd().unwrap();
    db.save_hermes_binding(&b).unwrap();
    b.provenance(&json!({"_meta":{"hermes":{"sessionProvenance":{"acpSessionId":"stable","currentHermesSessionId":"head-2","rootHermesSessionId":"native","future":true}}}})).unwrap();
    db.save_hermes_binding(&b).unwrap();
    assert_eq!(
        db.hermes_binding("thread")
            .unwrap()
            .unwrap()
            .current_native_id
            .as_deref(),
        Some("head-2")
    );
    assert!(db.hermes_cleanup_pending("workspace").unwrap());
    db.delete_agent_chat_session("thread").unwrap();
    assert!(db.hermes_cleanup_pending("workspace").unwrap());
    assert!(b
        .provenance(&json!({"_meta":{"hermes":{"sessionProvenance":{"acpSessionId":"other"}}}}))
        .is_err());
    let moved = root.path().join("missing");
    b.cwd = moved;
    assert!(b.validate_cwd().is_err());
}
#[test]
fn hermes_known_custom_resume_route_is_blocked() {
    assert!(durable_model("custom:fixture:gpt-4.1").is_err());
    assert!(durable_model("custom:model:opaque").is_err());
    assert!(durable_model("custom:gpt-4.1").is_ok());
    assert!(durable_model("openrouter:org/model").is_ok());
    assert!(durable_model("").is_err());
}

#[test]
fn hermes_archive_restore_reattaches_history_without_changing_native_binding() {
    let root = tempfile::tempdir().unwrap();
    let db = crate::database::init_test_database();
    let binding = test_binding(root.path());
    db.save_hermes_binding(&binding).unwrap();
    for (id, cwd, provider) in [
        ("thread", root.path().to_str().unwrap(), "hermes"),
        ("other-cwd", "/other", "hermes"),
        ("other-provider", root.path().to_str().unwrap(), "claude"),
    ] {
        db.upsert_agent_chat_session(id, "workspace", Some(cwd), provider)
            .unwrap();
    }
    db.restore_hermes_chat_history("workspace", "restored", root.path().to_str().unwrap())
        .unwrap();
    assert_eq!(
        db.get_agent_chat_session("thread").unwrap().workspace_id,
        "restored"
    );
    assert_eq!(
        db.get_agent_chat_session("other-cwd").unwrap().workspace_id,
        "workspace"
    );
    assert_eq!(
        db.get_agent_chat_session("other-provider")
            .unwrap()
            .workspace_id,
        "workspace"
    );
    let retained = db.hermes_binding("thread").unwrap().unwrap();
    assert_eq!(retained.acp_session_id, binding.acp_session_id);
    assert!(db
        .hermes_cleanup_pending_path(root.path().to_str().unwrap())
        .unwrap());
}
async fn finished(
    rx: &mut broadcast::Receiver<ProviderRuntimeEvent>,
    thread: &str,
) -> Vec<ProviderRuntimeEvent> {
    tokio::time::timeout(Duration::from_secs(120),async {
        let mut events=vec![];
        loop {let e=rx.recv().await.unwrap();let done=matches!(&e,ProviderRuntimeEvent::TurnCompleted {thread_id,..} if thread_id.0==thread);if let Ok(root) = std::env::var("CODEMUX_HERMES_TEST_ROOT") { use std::io::Write; let mut file = std::fs::OpenOptions::new().create(true).append(true).open(Path::new(&root).join("adapter-events.txt")).unwrap(); writeln!(file,"{e:?}").unwrap(); } events.push(e);if done{return events;}}
    }).await.expect("Hermes turn timed out")
}
fn success(events: &[ProviderRuntimeEvent]) {
    assert!(
        events.iter().any(|e| matches!(
            e,
            ProviderRuntimeEvent::TurnCompleted {
                status: TurnStatus::Success,
                ..
            }
        )),
        "{events:?}"
    );
}

/// Run using scripts/test-hermes-integration.py. Executes ONLY the installed official Hermes CLI.
#[tokio::test]
#[ignore = "requires an official Hermes installation, ACP dependencies and isolated loopback fixture"]
async fn official_runtime_contract() {
    let binary = std::env::var("CODEMUX_HERMES_TEST_BINARY")
        .expect("explicit official test binary required");
    let root = std::path::PathBuf::from(std::env::var("CODEMUX_HERMES_TEST_ROOT").unwrap());
    let coder = profile::resolve(Path::new(&binary), &root, "coder").unwrap();
    let research = profile::resolve(Path::new(&binary), &root, "research").unwrap();
    let a = root.join("work-a");
    let b = root.join("work-b");
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    let mut db = Arc::new(crate::database::init_database().unwrap());
    let mut provider = HermesProvider::new(db.clone());
    let mut rx = provider.inner.events.subscribe();
    // No inference on this keyless profile: exercise the official catalog/session
    // and prove the adapter rejects a protocol-changing switch before dispatch.
    let free = profile::resolve(Path::new(&binary), &root, "free").unwrap();
    provider.start_session(start("free-route", &a, &free)).await.unwrap();
    let free_before = db.hermes_binding("free-route").unwrap().unwrap();
    assert!(provider.set_model(ThreadId("free-route".into()), "opencode-free:muse-spark-1.3-contributor-free".into()).await.unwrap_err().to_string().contains("API protocol"));
    let free_after = db.hermes_binding("free-route").unwrap().unwrap();
    assert_eq!(free_before.resolved_model, free_after.resolved_model);
    assert_eq!(free_before.acp_session_id, free_after.acp_session_id);
    let mut incompatible_start = start("free-incompatible-start", &a, &free);
    incompatible_start.model = Some("opencode-free:muse-spark-1.3-contributor-free".into());
    assert!(provider.start_session(incompatible_start).await.unwrap_err().to_string().contains("API protocol"));
    provider.disconnect(free).await.unwrap();
    let named = profile::resolve(Path::new(&binary), &root, "named").unwrap();
    let named_runtime = provider.inner.runtime(named.clone()).await.unwrap();
    let raw_catalog = named_runtime
        .child
        .request("session/new", json!({"cwd":a,"mcpServers":[]}))
        .await
        .unwrap();
    assert_eq!(
        raw_catalog
            .pointer("/models/currentModelId")
            .and_then(Value::as_str),
        Some("custom:gpt-5"),
        "original named-provider reproduction"
    );
    assert!(durable_catalog(&raw_catalog).is_err());
    assert!(provider
        .catalog(named.clone())
        .await
        .unwrap_err()
        .to_string()
        .contains("named provider"));
    for selected in [
        None,
        Some("custom:gpt-5"),
        Some("custom:fixture:gpt-4.1"),
        Some("openai:gpt-5"),
    ] {
        let mut input = start("named-blocked", &a, &named);
        input.model = selected.map(String::from);
        assert!(provider
            .start_session(input)
            .await
            .unwrap_err()
            .to_string()
            .contains("named provider"));
        assert!(db.hermes_binding("named-blocked").unwrap().is_none());
    }
    provider.disconnect(named).await.unwrap();

    provider
        .start_session(start("a", &a, &coder))
        .await
        .unwrap();
    provider
        .set_model(ThreadId("a".into()), "profile_default".into())
        .await
        .unwrap();
    provider
        .start_session(start("b", &b, &coder))
        .await
        .unwrap();
    provider
        .start_session(start("research", &b, &research))
        .await
        .unwrap();
    assert_eq!(
        provider.inner.runtimes.lock().await.len(),
        2,
        "one runtime per profile"
    );
    provider.send_turn(turn("a", "CWD")).await.unwrap();
    success(&finished(&mut rx, "a").await);
    assert_eq!(
        std::fs::read_to_string(a.join("cwd.txt")).unwrap().trim(),
        a.to_str().unwrap()
    );
    provider.send_turn(turn("b", "CWD")).await.unwrap();
    success(&finished(&mut rx, "b").await);
    assert_eq!(
        std::fs::read_to_string(b.join("cwd.txt")).unwrap().trim(),
        b.to_str().unwrap()
    );
    provider
        .set_permission_mode(ThreadId("b".into()), "accept_edits".into())
        .await
        .unwrap();
    provider
        .send_turn(turn("b", "EDIT mode-approved.txt"))
        .await
        .unwrap();
    success(&finished(&mut rx, "b").await);
    assert!(b.join("mode-approved.txt").is_file());
    assert_eq!(
        db.hermes_binding("b")
            .unwrap()
            .unwrap()
            .permission_mode
            .as_deref(),
        Some("accept_edits")
    );
    provider.send_turn(turn("a", "SLOW_STREAM")).await.unwrap();
    tokio::time::timeout(Duration::from_secs(60), async {
        loop { if matches!(rx.recv().await.unwrap(), ProviderRuntimeEvent::ContentDelta {thread_id,..} if thread_id.0 == "a") {break;} }
    }).await.unwrap();
    provider
        .interrupt_turn(ThreadId("a".into()), None)
        .await
        .unwrap();
    assert!(finished(&mut rx, "a").await.iter().any(|e| matches!(e, ProviderRuntimeEvent::TurnCompleted { status: TurnStatus::Error {subtype,..}, ..} if subtype == "cancelled")));
    provider.send_turn(turn("a", "MEMORY")).await.unwrap();
    success(&finished(&mut rx, "a").await);
    provider.send_turn(turn("a", "SKILL_CREATE")).await.unwrap();
    success(&finished(&mut rx, "a").await);
    provider.send_turn(turn("a", "SKILL_UPDATE")).await.unwrap();
    success(&finished(&mut rx, "a").await);
    assert!(coder.home.join("memories/MEMORY.md").is_file());
    assert!(
        std::fs::read_to_string(coder.home.join("skills/synthetic-check/SKILL.md"))
            .unwrap()
            .contains("Violet procedure")
    );
    assert!(!research.home.join("memories/MEMORY.md").is_file());
    provider
        .send_turn(turn("a", "MEMORY_UPDATE"))
        .await
        .unwrap();
    success(&finished(&mut rx, "a").await);
    assert!(
        std::fs::read_to_string(coder.home.join("memories/MEMORY.md"))
            .unwrap()
            .contains("violet tests only")
    );

    // Approval deny must not mutate the worktree; pending option IDs are Hermes-owned.
    provider
        .send_turn(turn("a", "EDIT denied.txt"))
        .await
        .unwrap();
    let request = tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            if let ProviderRuntimeEvent::RequestOpened { request_id, .. } = rx.recv().await.unwrap()
            {
                break request_id;
            }
        }
    })
    .await
    .unwrap();
    // A *new* same-profile chat waits behind approval; a new profile must not
    // wait for that startup. Poll explicitly to establish the blocked ordering.
    let mut waiting = Box::pin(provider.start_session(start("waiting-a", &a, &coder)));
    assert!(futures_util::poll!(&mut waiting).is_pending());
    let independent = profile::resolve(Path::new(&binary), &root, "independent").unwrap();
    let independent_start = async {
        let (one, two) = tokio::join!(
            provider.start_session(start("new-profile", &b, &independent)),
            provider.start_session(start("new-profile", &b, &independent)),
        );
        assert_eq!(
            usize::from(one.is_ok()) + usize::from(two.is_ok()),
            1,
            "duplicate startup must create exactly one session"
        );
    };
    tokio::time::timeout(Duration::from_secs(45), independent_start)
        .await
        .expect("profile B startup blocked by A");
    provider
        .send_turn(turn("new-profile", "INDEPENDENT_START"))
        .await
        .unwrap();
    success(&finished(&mut rx, "new-profile").await);
    // A blocked approval holds this profile only. A sibling queues and can be
    // cancelled without dispatch; a different profile still completes.
    let queued = provider
        .send_turn(turn("b", "EDIT never-dispatched.txt"))
        .await
        .unwrap()
        .queued_id
        .unwrap();
    provider
        .send_turn(turn("research", "ISOLATION"))
        .await
        .unwrap();
    success(&finished(&mut rx, "research").await);
    assert!(!b.join("never-dispatched.txt").exists());
    assert!(provider
        .cancel_queued_turn(ThreadId("b".into()), queued)
        .await
        .unwrap());
    let stopped = provider
        .send_turn(turn("b", "EDIT stopped-queued.txt"))
        .await
        .unwrap()
        .queued_id
        .unwrap();
    provider
        .interrupt_turn(ThreadId("b".into()), None)
        .await
        .unwrap();
    assert!(!provider
        .cancel_queued_turn(ThreadId("b".into()), stopped)
        .await
        .unwrap());
    provider
        .respond_to_request(
            ThreadId("a".into()),
            request.clone(),
            ApprovalDecision::Deny {
                message: "test deny".into(),
            },
        )
        .await
        .unwrap();
    finished(&mut rx, "a").await;
    assert!(!a.join("denied.txt").exists());
    assert!(!b.join("stopped-queued.txt").exists());
    waiting.as_mut().await.unwrap();
    drop(waiting);
    provider
        .stop_session(ThreadId("waiting-a".into()))
        .await
        .unwrap();
    provider
        .stop_session(ThreadId("new-profile".into()))
        .await
        .unwrap();
    provider.disconnect(independent).await.unwrap();

    assert!(provider
        .respond_to_request(
            ThreadId("a".into()),
            request,
            ApprovalDecision::AllowForSession
        )
        .await
        .is_err());
    provider
        .send_turn(turn("a", "EDIT allowed.txt"))
        .await
        .unwrap();
    let request = tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            if let ProviderRuntimeEvent::RequestOpened { request_id, .. } = rx.recv().await.unwrap()
            {
                break request_id;
            }
        }
    })
    .await
    .unwrap();
    provider
        .respond_to_request(
            ThreadId("a".into()),
            request,
            ApprovalDecision::Allow {
                updated_input: None,
                updated_permissions: None,
            },
        )
        .await
        .unwrap();
    success(&finished(&mut rx, "a").await);
    assert!(a.join("allowed.txt").exists());
    provider
        .send_turn(turn("a", "EDIT cancelled.txt"))
        .await
        .unwrap();
    let stale = tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            if let ProviderRuntimeEvent::RequestOpened { request_id, .. } = rx.recv().await.unwrap()
            {
                break request_id;
            }
        }
    })
    .await
    .unwrap();
    provider
        .interrupt_turn(ThreadId("a".into()), None)
        .await
        .unwrap();
    let cancelled = finished(&mut rx, "a").await;
    assert!(cancelled.iter().any(|e| matches!(e, ProviderRuntimeEvent::TurnCompleted { status: TurnStatus::Error {subtype,..}, ..} if subtype == "cancelled")));
    assert!(!a.join("cancelled.txt").exists());
    assert!(provider
        .respond_to_request(
            ThreadId("a".into()),
            stale,
            ApprovalDecision::AllowForSession
        )
        .await
        .is_err());
    // Select an advertised model explicitly and verify effective native state.
    let selected = db
        .hermes_binding("b")
        .unwrap()
        .unwrap()
        .resolved_model
        .unwrap();
    provider
        .set_model(ThreadId("b".into()), selected.clone())
        .await
        .unwrap();
    assert_eq!(
        db.hermes_binding("b").unwrap().unwrap().model_override,
        Some(selected)
    );
    // Closing one chat cannot kill the shared profile child or a sibling's native session.
    let original = db
        .hermes_binding("b")
        .unwrap()
        .unwrap()
        .acp_session_id
        .unwrap();
    provider.stop_session(ThreadId("a".into())).await.unwrap();
    provider.send_turn(turn("b", "PING")).await.unwrap();
    success(&finished(&mut rx, "b").await);
    provider.stop_session(ThreadId("b".into())).await.unwrap();
    provider.disconnect(coder.clone()).await.unwrap();
    provider.disconnect(research.clone()).await.unwrap();
    drop(provider);
    drop(db);
    db = Arc::new(crate::database::init_database().unwrap());
    provider = HermesProvider::new(db.clone());
    rx = provider.inner.events.subscribe();
    let mut resumed = start("b", &b, &coder);
    resumed.extra = Value::Null;
    // Offline UI intent overrides the old durable accept_edits binding.
    resumed.permission_mode = Some("default".into());
    resumed.model = Some("custom:fixture-coder".into());

    provider.start_session(resumed).await.unwrap();
    assert_eq!(
        db.hermes_binding("b")
            .unwrap()
            .unwrap()
            .acp_session_id
            .as_deref(),
        Some(original.as_str())
    );
    provider
        .send_turn(turn("b", "AFTER_RESTART"))
        .await
        .unwrap();
    success(&finished(&mut rx, "b").await);
    assert_eq!(
        db.hermes_binding("b")
            .unwrap()
            .unwrap()
            .permission_mode
            .as_deref(),
        Some("default")
    );
    assert_eq!(
        db.hermes_binding("b")
            .unwrap()
            .unwrap()
            .model_override
            .as_deref(),
        Some("custom:fixture-coder")
    );
    provider
        .send_turn(turn("b", "EDIT offline-denied.txt"))
        .await
        .unwrap();
    let offline_request = tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            if let ProviderRuntimeEvent::RequestOpened { request_id, .. } = rx.recv().await.unwrap()
            {
                break request_id;
            }
        }
    })
    .await
    .expect("offline approval-required intent was ignored");
    provider
        .respond_to_request(
            ThreadId("b".into()),
            offline_request,
            ApprovalDecision::Deny {
                message: "offline policy regression".into(),
            },
        )
        .await
        .unwrap();
    finished(&mut rx, "b").await;
    assert!(!b.join("offline-denied.txt").exists());
    // Unexpected child loss is recoverable without silently creating a new chat.
    let runtime = provider
        .chat(&ThreadId("b".into()))
        .await
        .unwrap()
        .runtime
        .clone();
    runtime.child.shutdown().await.unwrap();
    assert!(!provider.has_session(&ThreadId("b".into())).await);
    provider
        .start_session(start("b", &b, &coder))
        .await
        .unwrap();
    assert_eq!(
        db.hermes_binding("b")
            .unwrap()
            .unwrap()
            .acp_session_id
            .as_deref(),
        Some(original.as_str())
    );
    provider
        .send_turn(turn("b", "AFTER_CHILD_LOSS"))
        .await
        .unwrap();
    success(&finished(&mut rx, "b").await);
    provider
        .start_session(start("fresh", &a, &coder))
        .await
        .unwrap();
    provider
        .send_turn(turn("fresh", "FRESH_MEMORY_SKILLS"))
        .await
        .unwrap();
    success(&finished(&mut rx, "fresh").await);
    provider
        .stop_session(ThreadId("fresh".into()))
        .await
        .unwrap();
    provider.stop_session(ThreadId("b".into())).await.unwrap();
    std::fs::rename(&b, root.join("moved-worktree")).unwrap();
    assert!(provider
        .start_session(start("b", &b, &coder))
        .await
        .unwrap_err()
        .to_string()
        .contains("cwd"));
    provider.disconnect(coder).await.unwrap();
    provider.disconnect(research).await.unwrap();
}

#[tokio::test]
async fn hermes_workspace_delete_is_blocked_even_with_force() {
    let root = tempfile::tempdir().unwrap();
    let sentinel = root.path().join("retained.txt");
    std::fs::write(&sentinel, "retained").unwrap();
    let db = crate::database::init_test_database();
    let binding = test_binding(root.path());
    db.save_hermes_binding(&binding).unwrap();
    let state = crate::state::AppStateStore::default();
    let restored = state.create_workspace_with_layout(
        root.path().to_path_buf(),
        crate::state::WorkspacePresetLayout::Single,
    );
    let app = tauri::test::mock_app();
    for workspace in ["workspace".to_string(), restored.0.clone()] {
        let error = crate::commands::workspace::close_workspace_with_worktree_impl(
            app.handle().clone(),
            &state,
            &db,
            workspace,
            true,
            Some(true),
            Some(true),
        )
        .await
        .unwrap_err();
        assert!(error.contains("Hermes worktree cleanup pending"), "{error}");
        assert!(sentinel.is_file());
        assert!(state
            .snapshot()
            .workspaces
            .iter()
            .any(|w| w.workspace_id == restored));
    }
    use tauri::Manager;
    app.manage(db);
    let error = crate::commands::git::remove_worktree(
        app.state(),
        root.path().display().to_string(),
        None,
        Some(true),
    )
    .await
    .unwrap_err();
    assert!(error.contains("Hermes worktree cleanup pending"));
    assert!(sentinel.is_file());
}

/// The close path must not hold the lifecycle lock across teardown, yet a
/// hold committed after teardown and before the delete must still win.
#[tokio::test]
async fn hermes_deletion_rechecks_cleanup_hold_under_lifecycle_lock() {
    use tauri::Manager;
    let root = tempfile::tempdir().unwrap();
    let repo = root.path().join("repo");
    let work = root.path().join("work");
    std::fs::create_dir(&repo).unwrap();
    for args in [
        vec!["init", "-b", "main"],
        vec![
            "-c",
            "user.name=Synthetic",
            "-c",
            "user.email=synthetic@example.invalid",
            "commit",
            "--allow-empty",
            "-m",
            "fixture",
        ],
        vec!["worktree", "add", "-b", "fixture", work.to_str().unwrap()],
    ] {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let app = tauri::test::mock_app();
    app.manage(crate::state::AppStateStore::default());
    app.manage(crate::database::init_test_database());
    app.manage(crate::terminal::PtyState::default());
    let state = app.state::<crate::state::AppStateStore>();
    let db = app.state::<crate::database::DatabaseStore>();
    let workspace = state.create_workspace_at_path(work.clone());
    state.set_workspace_worktree(&workspace.0, work.display().to_string(), "fixture".into());
    let starting = WORKTREE_LIFECYCLE.lock().await;
    let deleting = crate::commands::workspace::close_workspace_with_worktree_impl(
        app.handle().clone(),
        &state,
        &db,
        workspace.0.clone(),
        true,
        None,
        Some(true),
    );
    tokio::pin!(deleting);
    // Everything before the delete, including the state close, proceeds
    // while a Hermes start holds the lock.
    for _ in 0..1000 {
        assert!(futures_util::poll!(&mut deleting).is_pending());
        if !state.snapshot().workspaces.iter().any(|w| w.workspace_id == workspace) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    assert!(!state.snapshot().workspaces.iter().any(|w| w.workspace_id == workspace));
    let mut binding = test_binding(root.path());
    binding.workspace_id = None;
    binding.cwd = work.canonicalize().unwrap();
    db.save_hermes_binding(&binding).unwrap();
    drop(starting);
    assert!(deleting
        .await
        .unwrap_err()
        .contains("Hermes worktree cleanup pending"));
    assert!(work.is_dir());
}

/// An executable that exits non-zero without reading stdin. Windows has no
/// `/bin/false`, and std runs a `.cmd` through cmd.exe.
fn failing_executable(dir: &Path) -> std::path::PathBuf {
    if cfg!(windows) {
        let script = dir.join("fail.cmd");
        std::fs::write(&script, "@exit /b 1\r\n").unwrap();
        script
    } else {
        "/bin/false".into()
    }
}

/// The failing executable is a transport-failure fixture, never a substitute Hermes.
#[tokio::test]
async fn hermes_failed_initialization_does_not_hold_worktree() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("config.yaml"),
        "model:\n  provider: custom\n",
    )
    .unwrap();
    let profile = profile::resolve(&failing_executable(root.path()), root.path(), "default").unwrap();
    let repo = root.path().join("repo");
    let work = root.path().join("work");
    std::fs::create_dir(&repo).unwrap();
    for args in [
        vec!["init", "-b", "main"],
        vec![
            "-c",
            "user.name=Synthetic",
            "-c",
            "user.email=synthetic@example.invalid",
            "commit",
            "--allow-empty",
            "-m",
            "fixture",
        ],
        vec!["worktree", "add", "-b", "fixture", work.to_str().unwrap()],
    ] {
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(&repo)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let db = Arc::new(crate::database::init_test_database());
    let provider = HermesProvider::new(db.clone());
    for _ in 0..2 {
        let mut input = start("failed", &work, &profile);
        input.workspace_id = Some("failed-workspace".into());
        assert!(provider
            .start_session(input)
            .await
            .unwrap_err()
            .to_string()
            .contains("initialize"));
        let binding = db.hermes_binding("failed").unwrap().unwrap();
        assert!(!binding.cleanup_pending);
        assert!(binding.acp_session_id.is_none());
        assert!(!db.hermes_cleanup_pending("failed-workspace").unwrap());
        assert!(!db
            .hermes_cleanup_pending_path(root.path().to_str().unwrap())
            .unwrap());
    }
    // Exercise the real destructive command on a disposable Git worktree.
    use tauri::Manager;
    drop(provider);
    let db = Arc::try_unwrap(db).ok().unwrap();
    let app = tauri::test::mock_app();
    app.manage(db);
    crate::commands::git::remove_worktree(
        app.state(),
        work.display().to_string(),
        None,
        Some(true),
    )
    .await
    .unwrap();
    assert!(!work.exists());
    let db = app.state::<crate::database::DatabaseStore>();
    assert!(!db.hermes_cleanup_pending("failed-workspace").unwrap());
}

#[test]
fn hermes_named_route_config_and_aliases_fail_closed() {
    let root = tempfile::tempdir().unwrap();
    let profile =
        profile::resolve(&std::env::current_exe().unwrap(), root.path(), "default").unwrap();
    for config in [
        "model: {provider: 'custom:fixture'}",
        "model: {provider: custom}\nproviders: {fixture: {models: [gpt-5]}}",
        "model: {provider: custom}\ncustom_providers: [{name: fixture}]",
    ] {
        std::fs::write(root.path().join("config.yaml"), config).unwrap();
        assert!(durable_profile(&profile).is_err());
    }
    std::fs::write(root.path().join("config.yaml"), "model: {provider: custom}").unwrap();
    assert!(durable_profile(&profile).is_ok());
    assert!(durable_catalog(&json!({"models":{"currentModelId":"custom:gpt-5", "availableModels":[{"modelId":"custom:fixture:gpt-5"},{"modelId":"openai:gpt-5"}]}})).is_err());
}

#[test]
fn hermes_offline_intent_is_atomic_with_binding() {
    let root = tempfile::tempdir().unwrap();
    let db = crate::database::init_test_database();
    let mut binding = test_binding(root.path());
    binding.permission_mode = Some("accept_edits".into());
    db.save_hermes_binding(&binding).unwrap();
    db.upsert_agent_chat_session(
        "thread",
        "workspace",
        Some(root.path().to_str().unwrap()),
        "hermes",
    )
    .unwrap();
    db.update_hermes_intent("thread", Some("custom:other-model"), Some("default"))
        .unwrap();
    let generic = db.get_agent_chat_session("thread").unwrap();
    assert_eq!(generic.model.as_deref(), Some("custom:other-model"));
    assert_eq!(generic.permission_mode.as_deref(), Some("default"));
    let updated = db.hermes_binding("thread").unwrap().unwrap();
    assert_eq!(
        updated.model_override.as_deref(),
        Some("custom:other-model")
    );
    assert_eq!(updated.permission_mode.as_deref(), Some("default"));
    assert_eq!(updated.resolved_model, binding.resolved_model);
    assert_eq!(updated.acp_session_id, binding.acp_session_id);
    assert!(updated.cleanup_pending);
}

#[test]
fn hermes_free_model_switch_rejects_protocol_changes() {
    let chat = "opencode-free:deepseek-v4-flash-free";
    let responses = "opencode-free:muse-spark-1.3-contributor-free";
    let messages = "opencode-free:qwen-free";
    for (from, to) in [(chat, responses), (responses, chat), (chat, messages), (messages, responses)] {
        assert!(durable_model_change(Some(from), to).is_err());
    }
    assert!(durable_model_change(Some(chat), "opencode-free:mimo-v2.5-free").is_ok());
    assert!(durable_model_change(Some(responses), "opencode-free:muse-spark-1.2-contributor-free").is_ok());
    assert!(durable_model_change(Some(chat), chat).is_ok());
    assert!(durable_model_change(Some(chat), "profile_default").is_ok());
    // Different providers cause official ACP to resolve a new endpoint/protocol.
    assert!(durable_model_change(Some("custom:fixture"), responses).is_ok());
}

#[test]
fn hermes_offline_free_model_change_fails_without_mutating_binding() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("config.yaml"), "model:\n  provider: opencode-free\n  default: deepseek-v4-flash-free\n").unwrap();
    let db = Arc::new(crate::database::init_test_database());
    let mut binding = test_binding(root.path());
    binding.resolved_model = Some("opencode-free:deepseek-v4-flash-free".into());
    db.save_hermes_binding(&binding).unwrap();
    let provider = HermesProvider::new(db.clone());
    assert!(provider.validate_intent("thread", Some("opencode-free:muse-spark-1.3-contributor-free"), None).is_err());
    let retained = db.hermes_binding("thread").unwrap().unwrap();
    assert_eq!(retained.model_override, binding.model_override);
    assert_eq!(retained.resolved_model, binding.resolved_model);
    assert_eq!(retained.acp_session_id, binding.acp_session_id);
    assert!(provider.validate_intent("thread", Some("opencode-free:mimo-v2.5-free"), None).is_ok());
}
