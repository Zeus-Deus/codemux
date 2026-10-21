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

#[tokio::test]
async fn all_provider_custom_acp_new_load_resume_inject_and_discovery_stays_unprivileged() {
    for mode in ["load", "resume"] {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("wire.jsonl");
        let store = Arc::new(Store::default());
        *store.launch.lock().unwrap() = Some(AcpLaunchConfig {
            agent: super::super::AcpAgent {
                id: "injected".into(),
                name: "Injected fixture".into(),
                executable: super::super::test_python::executable(),
                args: vec![
                    "-I".into(),
                    format!(
                        "{}/tests/helpers/fake_custom_acp.py",
                        env!("CARGO_MANIFEST_DIR")
                    ),
                ],
                environment: Default::default(),
                enabled: true,
                auth_method: None,
                revision: "injection-revision".into(),
            },
            environment: HashMap::from([
                ("FAKE_ACP_MODE".into(), mode.into()),
                ("FAKE_ACP_LOG".into(), log.to_string_lossy().into_owned()),
                ("HOME".into(), dir.path().to_string_lossy().into_owned()),
                ("TMPDIR".into(), dir.path().to_string_lossy().into_owned()),
            ]),
        });
        let provider = GenericAcpProvider::new_with_mcp_registry(
            store,
            Some(crate::mcp::registry::McpRegistry::new()),
        );
        provider.probe("injected", dir.path().into()).await.unwrap();
        let thread = ThreadId(format!("injected-{mode}"));
        let start = || {
            serde_json::from_value(json!({"thread_id":thread.0,"cwd":dir.path(),"additional_directories":[],"extra":{"acp_agent_id":"injected"}})).unwrap()
        };
        provider.start_session(start()).await.unwrap();
        provider.stop_session(thread.clone()).await.unwrap();
        provider.start_session(start()).await.unwrap();
        // Stop twice through the authority path the Stop command uses.
        crate::delegation::authority::NativeAuthority::user_activity_thread(&thread.0, true);
        crate::delegation::authority::NativeAuthority::user_activity_thread(&thread.0, true);
        let mut events = provider.event_stream();
        let sent = provider
            .send_turn(
                serde_json::from_value(
                    json!({"thread_id":thread.0,"text":"A fresh turn after Stop"}),
                )
                .unwrap(),
            )
            .await
            .unwrap();
        let completed = tokio::time::timeout(Duration::from_secs(5), async {
            let mut text = String::new();
            loop {
                match events.next().await.unwrap() {
                    ProviderRuntimeEvent::ItemCompleted {
                        item: CompletedItem::AssistantText { text: chunk },
                        ..
                    } => text.push_str(&chunk),
                    ProviderRuntimeEvent::TurnCompleted { turn_id, .. }
                        if turn_id == sent.turn_id =>
                    {
                        break text
                    }
                    _ => {}
                }
            }
        })
        .await
        .unwrap();
        assert!(
            !completed.contains("historical replay MUST NOT duplicate"),
            "a turn after Stop must not import native load history: {completed}"
        );
        provider.stop_session(thread).await.unwrap();
        let frames: Vec<Value> = std::fs::read_to_string(log)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        let sessions: Vec<_> = frames
            .iter()
            .filter(|f| {
                matches!(
                    f["method"].as_str(),
                    Some("session/new" | "session/load" | "session/resume")
                )
            })
            .collect();
        // Probe, first chat, reopened chat: Stop reloads nothing.
        assert_eq!(sessions.len(), 3, "{frames:?}");
        assert_eq!(
            sessions[0]["params"]["mcpServers"],
            json!([]),
            "catalog probe must never receive chat authority"
        );
        assert_eq!(sessions[1]["method"], "session/new");
        assert_eq!(sessions[2]["method"], format!("session/{mode}"));
        for session in &sessions[1..] {
            let servers = &session["params"]["mcpServers"];
            assert_eq!(servers.as_array().unwrap().len(), 1);
            assert_eq!(servers[0]["args"], json!(["mcp-bridge"]));
        }
        assert_ne!(
            sessions[1]["params"]["mcpServers"][0]["env"],
            sessions[2]["params"]["mcpServers"][0]["env"],
            "replaced native sessions must receive new credentials"
        );
        // Each chat process carries its own session's credential for the
        // shell fallback; the probe carries none.
        let launches: Vec<_> = frames.iter().filter(|f| f.get("launch_argv").is_some()).collect();
        assert_eq!(launches.len(), 3, "{frames:?}");
        assert_eq!(launches[0]["gateway_token"], Value::Null);
        for (launch, session) in launches[1..].iter().zip(&sessions[1..]) {
            assert_eq!(launch["gateway_token"], session["params"]["mcpServers"][0]["env"][1]["value"]);
        }
    }
}

/// An unsolicited mid-turn catalog notification changes the opaque policy
/// fingerprint. The gateway ceiling must follow it, or orchestration calls
/// fail validation until the next turn.
#[tokio::test]
async fn all_provider_custom_acp_unsolicited_catalog_update_refreshes_gateway_policy() {
    use crate::agent_control::permissions::Ceiling;
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(Store::default());
    *store.launch.lock().unwrap() = Some(AcpLaunchConfig {
        agent: super::super::AcpAgent {
            id: "notified".into(), name: "Notified fixture".into(), executable: super::super::test_python::executable(),
            args: vec!["-I".into(),format!("{}/tests/helpers/fake_custom_acp.py",env!("CARGO_MANIFEST_DIR"))],
            environment: Default::default(), enabled: true, auth_method: None, revision: "notified-revision".into(),
        },
        environment: HashMap::from([
            ("FAKE_ACP_MODE".into(),"notify-catalog-hold".into()),
            ("FAKE_ACP_LOG".into(),dir.path().join("wire.jsonl").to_string_lossy().into_owned()),
            ("HOME".into(),dir.path().to_string_lossy().into_owned()),
            ("TMPDIR".into(),dir.path().to_string_lossy().into_owned()),
        ]),
    });
    let provider = GenericAcpProvider::new_with_mcp_registry(store, Some(crate::mcp::registry::McpRegistry::new()));
    let thread = ThreadId("notified-policy".into());
    provider.start_session(serde_json::from_value(json!({"thread_id":thread.0,"cwd":dir.path(),"additional_directories":[],"extra":{"acp_agent_id":"notified"}})).unwrap()).await.unwrap();
    let session = provider.session(&thread).await.unwrap();
    let authority = session.mcp.as_ref().expect("chat MCP").session.authority.clone();
    let before = Ceiling::opaque_policy(&*session.binding.lock().await);
    assert_eq!(crate::delegation::authority::NativeAuthority::effective_ceiling(&thread.0).unwrap(), Some(before));
    provider.send_turn(serde_json::from_value(json!({"thread_id":thread.0,"text":"held"})).unwrap()).await.unwrap();
    // The fixture flips a boolean option mid-turn, unprompted, and holds the turn.
    let canonical = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let binding = session.binding.lock().await.clone();
            if binding.catalog.config_options.iter().any(|o| o.id == "boolean selector" && o.current_value == json!(true)) {
                break Ceiling::opaque_policy(&binding);
            }
            drop(binding);
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }).await.expect("the notification was applied");
    assert_ne!(canonical, before, "the notification changes the policy fingerprint");
    let effective = crate::delegation::authority::NativeAuthority::effective_ceiling(&thread.0).unwrap().unwrap();
    assert_eq!(effective, canonical, "the gateway ceiling kept the old fingerprint");
    // The checks orchestration runs: admission takes effective ∩ canonical,
    // validation requires the canonical record to permit it.
    assert!(canonical.permits(effective.intersection(canonical)));
    // Callbacks captured now run under the refreshed policy.
    tokio::time::timeout(Duration::from_secs(2), authority.capture().wait_ready()).await.unwrap().unwrap();
    let _ = provider.stop_session(thread).await;
}
