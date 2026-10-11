//! Scheduling-only gates on actual child routing and actual prompt settlement.
use super::*;
use futures_util::StreamExt;
#[derive(Default)]
struct Store {
    launch: std::sync::Mutex<Option<AcpLaunchConfig>>,
    bindings: std::sync::Mutex<HashMap<String, AcpBinding>>,
}
impl AcpStore for Store {
    fn launch_config(&self, _: &str) -> Result<AcpLaunchConfig, String> { Ok(self.launch.lock().unwrap().clone().unwrap()) }
    fn binding(&self, id: &str) -> Result<Option<AcpBinding>, String> { Ok(self.bindings.lock().unwrap().get(id).cloned()) }
    fn save_binding(&self, b: &AcpBinding) -> Result<(), String> { self.bindings.lock().unwrap().insert(b.thread_id.clone(), b.clone()); Ok(()) }
}
async fn exiting_prompt(reader_first: bool) {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::default());
    *store.launch.lock().unwrap() = Some(AcpLaunchConfig {
        agent: super::super::AcpAgent {
            id: "transport".into(), name: "Synthetic transport agent".into(), executable: super::super::test_python::executable(),
            args: vec!["-I".into(),format!("{}/tests/helpers/fake_custom_acp.py",env!("CARGO_MANIFEST_DIR"))],
            environment: Default::default(), enabled: true, auth_method: None, revision: "transport-revision".into(),
        },
        environment: HashMap::from([
            ("FAKE_ACP_MODE".into(),"final-exit".into()),
            ("FAKE_ACP_LOG".into(),dir.path().join("wire.jsonl").to_string_lossy().into_owned()),
            ("HOME".into(),dir.path().to_string_lossy().into_owned()),
            ("TMPDIR".into(),dir.path().to_string_lossy().into_owned()),
        ]),
    });
    let provider = GenericAcpProvider::new(store);
    let thread = ThreadId(format!("transport-{reader_first}"));
    provider.start_session(serde_json::from_value(json!({"thread_id":thread.0,"cwd":dir.path(),"additional_directories":[],"extra":{"acp_agent_id":"transport"}})).unwrap()).await.unwrap();
    let session = provider.session(&thread).await.unwrap();
    // Only scheduling changes: neither hook changes protocol bytes or outcomes.
    let reader = if reader_first { Some(session.child.reader_gate.lock().await) } else { None };
    let worker = if !reader_first { Some(session.prompt_settled_gate.lock().await) } else { None };
    let mut events = provider.event_stream();
    let turn = provider.send_turn(serde_json::from_value(json!({"thread_id":thread.0,"text":"synthetic"})).unwrap()).await.unwrap().turn_id;
    let exited = tokio::time::timeout(Duration::from_secs(2), async {
        while session.child.is_alive() { tokio::time::sleep(Duration::from_millis(5)).await; }
    }).await;
    // Give the actual pump at least two health ticks while routing/worker is held.
    tokio::time::sleep(Duration::from_millis(150)).await;
    let prematurely_ended = session.ended.load(Ordering::SeqCst);
    // The retained health sender closes beside the incoming keepalive after
    // pending requests settle. This proves real stream closure in the second
    // schedule while its actual prompt worker still cannot send the barrier.
    let streams_closed = session.child.transport_failures().has_changed().is_err();
    drop(reader);
    drop(worker);
    let result = tokio::time::timeout(Duration::from_secs(4), async {
        let mut text = String::new();
        loop {
            match events.next().await.unwrap() {
                ProviderRuntimeEvent::ItemCompleted { item: CompletedItem::AssistantText { text: value }, .. } => text.push_str(&value),
                ProviderRuntimeEvent::TurnCompleted { turn_id, status, .. } if turn_id == turn => break (text,status),
                _ => {}
            }
        }
    }).await;
    let _ = provider.stop_session(thread).await;
    assert!(exited.is_ok(), "the real peer must exit while the scheduling gate is held");
    if !reader_first { assert!(streams_closed, "incoming/health keepalives must actually close before releasing the worker"); }
    assert!(!prematurely_ended, "process/stream closure must not end an active prompt before buffered messages and its worker barrier settle");
    let (text,status) = result.expect("turn must settle promptly");
    assert!(matches!(status,TurnStatus::Success), "a delivered success must remain success: {status:?}");
    assert_eq!(text,"tail chunk", "buffered final update must precede completion");
}
#[tokio::test]
async fn transport_final_success_update_before_peer_exit_survives_delayed_reader() { exiting_prompt(true).await; }
#[tokio::test]
async fn transport_closed_incoming_stream_before_prompt_worker_barrier_preserves_success() { exiting_prompt(false).await; }
