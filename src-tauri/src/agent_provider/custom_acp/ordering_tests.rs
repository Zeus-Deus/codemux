//! Scheduling-only barriers around actual approval/interrupt/close operations.
use super::*;
use futures_util::StreamExt;
#[derive(Default)]
struct Store {
    launch: std::sync::Mutex<Option<AcpLaunchConfig>>,
    bindings: std::sync::Mutex<HashMap<String, AcpBinding>>,
}
impl AcpStore for Store {
    fn launch_config(&self, _: &str) -> Result<AcpLaunchConfig, String> {
        Ok(self.launch.lock().unwrap().clone().unwrap())
    }
    fn binding(&self, id: &str) -> Result<Option<AcpBinding>, String> {
        Ok(self.bindings.lock().unwrap().get(id).cloned())
    }
    fn save_binding(&self, b: &AcpBinding) -> Result<(), String> {
        self.bindings
            .lock()
            .unwrap()
            .insert(b.thread_id.clone(), b.clone());
        Ok(())
    }
}
async fn queued_approval_after_cancellation(close: bool) {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("wire.jsonl");
    let store = Arc::new(Store::default());
    *store.launch.lock().unwrap() = Some(AcpLaunchConfig {
        agent: super::super::AcpAgent {
            id: "ordering".into(),
            name: "Synthetic ordering agent".into(),
            executable: super::super::test_python::executable(),
            args: vec!["-I".into(), format!(
                "{}/tests/helpers/fake_custom_acp.py",
                env!("CARGO_MANIFEST_DIR")
            )],
            environment: Default::default(),
            enabled: true,
            auth_method: None,
            revision: "ordering-revision".into(),
        },
        environment: HashMap::from([
            ("FAKE_ACP_MODE".into(), "permission".into()),
            ("FAKE_ACP_LOG".into(), log.to_string_lossy().into_owned()),
        ]),
    });
    let provider = GenericAcpProvider::new(store);
    let thread = ThreadId(format!("ordering-{close}"));
    let input=serde_json::from_value(json!({"thread_id":thread.0,"cwd":dir.path(),"additional_directories":[],"extra":{"acp_agent_id":"ordering"}})).unwrap();
    provider.start_session(input).await.unwrap();
    let mut events = provider.event_stream();
    let turn = provider
        .send_turn(
            serde_json::from_value(json!({"thread_id":thread.0,"text":"synthetic"})).unwrap(),
        )
        .await
        .unwrap()
        .turn_id;
    let request = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Some(ProviderRuntimeEvent::RequestOpened { request_id, .. }) =
                events.next().await
            {
                break request_id;
            }
        }
    })
    .await
    .unwrap();
    let session = provider.session(&thread).await.unwrap();
    let gate = session.callbacks.lock().await;
    let approval = session.respond(
        request,
        ApprovalDecision::Allow {
            updated_input: None,
            updated_permissions: None,
        },
    );
    tokio::pin!(approval);
    assert!(
        futures_util::poll!(&mut approval).is_pending(),
        "approval must actually queue on the held mutex"
    );
    if close {
        let cancel = session.close();
        tokio::pin!(cancel);
        assert!(futures_util::poll!(&mut cancel).is_pending());
        assert!(session.ended.load(Ordering::SeqCst));
        drop(gate);
        let (_, cancelled) = tokio::time::timeout(Duration::from_secs(4), async {
            tokio::join!(&mut approval, &mut cancel)
        })
        .await
        .unwrap();
        cancelled.unwrap();
    } else {
        let cancel = session.interrupt(Some(turn));
        tokio::pin!(cancel);
        assert!(futures_util::poll!(&mut cancel).is_pending());
        assert!(session.state.lock().await.interrupted);
        drop(gate);
        let (_, cancelled) = tokio::time::timeout(Duration::from_secs(4), async {
            tokio::join!(&mut approval, &mut cancel)
        })
        .await
        .unwrap();
        cancelled.unwrap();
        provider.stop_session(thread).await.unwrap();
    }
    let wire = std::fs::read_to_string(log).unwrap();
    let replies: Vec<Value> = wire
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .filter(|v| v["id"] == "permission callback" && v.get("result").is_some())
        .collect();
    assert_eq!(
        replies.len(),
        1,
        "one callback must have exactly one wire response"
    );
    assert_eq!(
        replies[0]["result"],
        json!({"outcome":{"outcome":"cancelled"}}),
        "a queued approval must not grant permission after cancellation has taken effect"
    );
}
#[tokio::test]
async fn correction_queued_approval_after_interrupt_is_cancelled() {
    queued_approval_after_cancellation(false).await;
}
#[tokio::test]
async fn correction_queued_approval_after_close_is_cancelled() {
    queued_approval_after_cancellation(true).await;
}
