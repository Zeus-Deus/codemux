//! Credential-free process qualification; parent owns execution of this lane.
#[path = "helpers/custom_acp_python.rs"]
mod test_python;

use codemux_lib::agent_provider::{
    custom_acp::{AcpAgent, AcpBinding, AcpLaunchConfig, AcpStore, GenericAcpProvider},
    AgentProvider, ApprovalDecision, ProviderRuntimeEvent, SendTurnInput, StartSessionInput,
    ThreadId,
};
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, HashMap},
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};

#[derive(Default)]
struct MemoryStore {
    agents: Mutex<HashMap<String, AcpLaunchConfig>>,
    bindings: Mutex<HashMap<String, AcpBinding>>,
}
impl AcpStore for MemoryStore {
    fn launch_config(&self, id: &str) -> Result<AcpLaunchConfig, String> {
        self.agents
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or_else(|| "unavailable: definition was removed".into())
    }
    fn binding(&self, id: &str) -> Result<Option<AcpBinding>, String> {
        Ok(self.bindings.lock().unwrap().get(id).cloned())
    }
    fn save_binding(&self, value: &AcpBinding) -> Result<(), String> {
        self.bindings
            .lock()
            .unwrap()
            .insert(value.thread_id.clone(), value.clone());
        Ok(())
    }
}
struct Fixture {
    dir: tempfile::TempDir,
    store: Arc<MemoryStore>,
    provider: GenericAcpProvider,
}
impl Fixture {
    fn new(mode: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(MemoryStore::default());
        let fixture =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/helpers/fake_custom_acp.py");
        for id in ["a", "b"] {
            store.agents.lock().unwrap().insert(
                id.into(),
                AcpLaunchConfig {
                    agent: AcpAgent {
                        id: id.into(),
                        name: format!("Agent {id}"),
                        executable: test_python::executable(),
                        args: vec![
                            "-I".into(),
                            fixture.to_string_lossy().into_owned(),
                            " spaced ".into(),
                            "$(touch MUST_NOT_EXIST); $VALUE".into(),
                            "".into(),
                        ],
                        environment: BTreeMap::new(),
                        enabled: true,
                        auth_method: None,
                        revision: "rev".into(),
                    },
                    environment: HashMap::from([
                        ("FAKE_ACP_MODE".into(), mode.into()),
                        (
                            "FAKE_ACP_LOG".into(),
                            dir.path()
                                .join(format!("{id}.jsonl"))
                                .to_string_lossy()
                                .into_owned(),
                        ),
                        ("FAKE_LITERAL".into(), " literal $(value) ".into()),
                    ]),
                },
            );
        }
        let provider = GenericAcpProvider::new(store.clone());
        Self {
            dir,
            store,
            provider,
        }
    }
    fn input(&self, thread: &str, agent: Option<&str>) -> StartSessionInput {
        serde_json::from_value(json!({"thread_id":thread,"cwd":self.dir.path(),"model":null,"resume_cursor":null,"permission_mode":null,"additional_directories":[],"env":null,"extra":agent.map(|id|json!({"acp_agent_id":id})).unwrap_or(Value::Null)})).unwrap()
    }
    fn logs(&self, agent: &str) -> Vec<Value> {
        std::fs::read_to_string(self.dir.path().join(format!("{agent}.jsonl")))
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
}
#[tokio::test]
async fn correction_dynamic_replacement_retired_ids_restart() {
    replacement_restart("dynamic-ids", "effort A", "effort B").await;
}
#[tokio::test]
async fn correction_dynamic_replacement_retired_values_restart() {
    replacement_restart("dynamic-values", "same effort ID", "same effort ID").await;
}
async fn replacement_restart(mode: &str, old_id: &str, new_id: &str) {
    let f = Fixture::new(mode);
    let thread = ThreadId("replacement".into());
    let first = f.provider.start_session(f.input(&thread.0, Some("a"))).await.unwrap();
    f.provider.set_config(thread.clone(), old_id.into(), json!(" A effort ")).await.unwrap();
    f.provider.set_model(thread.clone(), "vendor:model [1m]".into()).await.unwrap();
    let binding = f.store.binding(&thread.0).unwrap().unwrap();
    f.provider.stop_session(thread.clone()).await.unwrap();
    // Reach actual resume before inspecting intent: retired values must not wedge startup.
    let resumed = f.provider.start_session(f.input(&thread.0, None)).await;
    if resumed.is_ok() { f.provider.stop_session(thread.clone()).await.unwrap(); }
    assert!(resumed.is_ok(), "replacement catalog must remain resumable: {resumed:?}");
    assert_eq!(resumed.unwrap().session_id, first.session_id);
    if old_id != new_id { assert!(!binding.config_values.contains_key(old_id)); }
    assert_eq!(binding.config_values.get(new_id), Some(&json!(" B effort ")));
    let logs = f.logs("a");
    let restart = logs.iter().rposition(|v| v.get("launch_argv").is_some()).unwrap();
    assert!(logs[restart..].iter().any(|v| v["method"] == "session/resume"));
    assert!(!logs[restart..].iter().any(|v| v["params"]["configId"] == old_id && v["params"]["value"] == " A effort "));
}

fn turn(thread: &str) -> SendTurnInput {
    serde_json::from_value(json!({"thread_id":thread,"text":"test","model_override":null})).unwrap()
}

#[tokio::test]
async fn generic_acp_idle_catalog_updates_are_authoritative_and_do_not_bypass_host_approval() {
    let f = Fixture::new("resume");
    f.provider
        .start_session(f.input("one", Some("a")))
        .await
        .unwrap();
    let mut updates = f.provider.catalog_updates();
    assert!(!f.provider.turn_active(&ThreadId("one".into())).await);
    f.provider
        .set_config(
            ThreadId("one".into()),
            "boolean selector".into(),
            json!(true),
        )
        .await
        .unwrap();
    let changed = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let update = updates.recv().await.unwrap();
            if update
                .catalog
                .config_options
                .iter()
                .any(|o| o.id == "boolean selector" && o.current_value == json!(true))
            {
                break update;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(changed.thread_id, "one");
    assert_eq!(
        changed
            .catalog
            .capabilities
            .default_permission_mode
            .as_deref(),
        Some("supervised")
    );
    assert_eq!(
        f.store.binding("one").unwrap().unwrap().config_values["boolean selector"],
        json!(true)
    );
    assert!(!f.provider.turn_active(&ThreadId("one".into())).await);
    f.provider
        .stop_session(ThreadId("one".into()))
        .await
        .unwrap();
}
#[tokio::test]
async fn generic_acp_explicit_advertised_auth_only() {
    let f = Fixture::new("auth");
    f.store
        .agents
        .lock()
        .unwrap()
        .get_mut("a")
        .unwrap()
        .agent
        .auth_method = Some("exact-auth ID".into());
    f.provider
        .start_session(f.input("one", Some("a")))
        .await
        .unwrap();
    f.provider
        .stop_session(ThreadId("one".into()))
        .await
        .unwrap();
    assert_eq!(
        f.logs("a")
            .iter()
            .filter(|v| v["method"] == "authenticate")
            .count(),
        1
    );
    f.store
        .agents
        .lock()
        .unwrap()
        .get_mut("b")
        .unwrap()
        .agent
        .auth_method = Some("not advertised".into());
    assert!(f
        .provider
        .start_session(f.input("two", Some("b")))
        .await
        .is_err());
    assert!(!f.logs("b").iter().any(|v| v["method"] == "authenticate"));
}
#[tokio::test]
async fn generic_acp_probe_never_binds_or_prompts_and_closes_its_owned_process() {
    let f = Fixture::new("resume");
    let catalog = f
        .provider
        .probe("a", f.dir.path().to_owned())
        .await
        .unwrap();
    assert!(catalog.supports_resume);
    assert!(f.store.bindings.lock().unwrap().is_empty());
    assert!(f.provider.list_sessions().await.unwrap().is_empty());
    let logs = f.logs("a");
    assert!(logs.iter().any(|v| v["method"] == "session/close"));
    assert!(!logs.iter().any(|v| v["method"] == "session/prompt"));
}
#[tokio::test]
async fn generic_acp_queue_is_bounded_and_cancellation_releases_capacity() {
    let f = Fixture::new("hold");
    f.provider
        .start_session(f.input("one", Some("a")))
        .await
        .unwrap();
    f.provider.send_turn(turn("one")).await.unwrap();
    let mut first = None;
    for _ in 0..32 {
        let queued = f.provider.send_turn(turn("one")).await.unwrap();
        assert!(queued.queued_id.is_some());
        if first.is_none() {
            first = queued.queued_id;
        }
    }
    assert!(f.provider.send_turn(turn("one")).await.is_err());
    assert!(f
        .provider
        .cancel_queued_turn(ThreadId("one".into()), first.unwrap())
        .await
        .unwrap());
    assert!(f
        .provider
        .send_turn(turn("one"))
        .await
        .unwrap()
        .queued_id
        .is_some());
    f.provider
        .stop_session(ThreadId("one".into()))
        .await
        .unwrap();
}
#[tokio::test]
async fn generic_acp_malformed_protocol_and_process_exit_settle_and_teardown() {
    // bad-json requires the parent's JsonRpcChild internal health notification;
    // raw malformed bytes cannot be observed through today's lossy transport API.
    for mode in ["bad-update", "exit", "bad-json"] {
        let f = Fixture::new(mode);
        f.provider
            .start_session(f.input("one", Some("a")))
            .await
            .unwrap();
        let mut events = f.provider.event_stream();
        let t = f.provider.send_turn(turn("one")).await.unwrap().turn_id;
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if let Some(ProviderRuntimeEvent::TurnCompleted { turn_id, .. }) =
                    events.next().await
                {
                    if turn_id == t {
                        break;
                    }
                }
            }
        })
        .await
        .expect(
            "protocol/exit failure must settle the active turn without a 24-hour prompt timeout",
        );
        assert!(!f.provider.has_session(&ThreadId("one".into())).await);
        assert!(f.provider.send_turn(turn("one")).await.is_err());
        f.provider
            .stop_session(ThreadId("one".into()))
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn generic_acp_literal_launch_no_auth_resume_only_and_collision() {
    let f = Fixture::new("resume");
    let a = f
        .provider
        .start_session(f.input("one", Some("a")))
        .await
        .unwrap();
    let b = f
        .provider
        .start_session(f.input("two", Some("b")))
        .await
        .unwrap();
    assert_ne!(a.session_id, b.session_id);
    assert_eq!(
        f.provider
            .catalog(&ThreadId("one".into()))
            .await
            .unwrap()
            .capabilities
            .models[1]
            .id,
        "vendor:model [1m]"
    );
    f.provider
        .stop_session(ThreadId("one".into()))
        .await
        .unwrap();
    let mut resume_input = f.input("one", None);
    resume_input.resume_cursor = Some(json!({"resume":a.session_id.0}));
    let resumed = f.provider.start_session(resume_input).await.unwrap();
    assert_eq!(a.session_id, resumed.session_id);
    f.provider.disconnect_agent("a").await.unwrap();
    f.provider.disconnect_agent("b").await.unwrap();
    let logs = f.logs("a");
    assert_eq!(
        logs[0]["launch_argv"],
        json!([" spaced ", "$(touch MUST_NOT_EXIST); $VALUE", ""])
    );
    assert_eq!(logs[0]["launch_env"], " literal $(value) ");
    assert!(!f.dir.path().join("MUST_NOT_EXIST").exists());
    assert!(logs.iter().any(|v| v["method"] == "session/resume"));
    assert!(!logs
        .iter()
        .any(|v| v["method"] == "authenticate" || v["method"] == "session/load"));
    let caps =
        &logs.iter().find(|v| v["method"] == "initialize").unwrap()["params"]["clientCapabilities"];
    assert!(!caps["terminal"].as_bool().unwrap_or(false));
    assert!(!caps["fs"]["readTextFile"].as_bool().unwrap_or(false));
    assert!(!caps["fs"]["writeTextFile"].as_bool().unwrap_or(false));
}
#[tokio::test]
async fn generic_acp_no_catalog_model_still_accepts_default_and_rejects_unadvertised_model() {
    let f = Fixture::new("no-model");
    f.provider
        .start_session(f.input("one", Some("a")))
        .await
        .unwrap();
    assert!(f
        .provider
        .catalog(&ThreadId("one".into()))
        .await
        .unwrap()
        .capabilities
        .models
        .is_empty());
    assert!(f
        .provider
        .set_model(ThreadId("one".into()), "default".into())
        .await
        .is_err());
    f.provider.send_turn(turn("one")).await.unwrap();
    f.provider
        .stop_session(ThreadId("one".into()))
        .await
        .unwrap();
}
#[tokio::test]
async fn generic_acp_exact_config_and_permissions_cancel_pending_wait_before_prompt_settlement() {
    let f = Fixture::new("permission");
    f.provider
        .start_session(f.input("one", Some("a")))
        .await
        .unwrap();
    assert!(f
        .provider
        .set_config(
            ThreadId("one".into()),
            "model selector".into(),
            json!("Default")
        )
        .await
        .is_err());
    f.provider
        .set_model(ThreadId("one".into()), "default".into())
        .await
        .unwrap();
    f.provider
        .set_config(
            ThreadId("one".into()),
            "boolean selector".into(),
            json!(true),
        )
        .await
        .unwrap();
    let mut events = f.provider.event_stream();
    let result = f.provider.send_turn(turn("one")).await.unwrap();
    let request = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Some(ProviderRuntimeEvent::RequestOpened {
                request_id,
                tool_use_id,
                ..
            }) = events.next().await
            {
                assert_eq!(tool_use_id.as_deref(), Some("real-tool"));
                break request_id;
            }
        }
    })
    .await
    .unwrap();
    assert!(f
        .provider
        .respond_to_request(
            ThreadId("one".into()),
            request,
            ApprovalDecision::ProviderOption {
                option_id: "INVALID".into()
            }
        )
        .await
        .is_err());
    f.provider
        .interrupt_turn(ThreadId("one".into()), Some(result.turn_id.clone()))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Some(ProviderRuntimeEvent::TurnCompleted { turn_id, .. }) = events.next().await {
                if turn_id == result.turn_id {
                    break;
                }
            }
        }
    })
    .await
    .expect("cancel must resolve callback before waiting for prompt result");
    f.provider
        .stop_session(ThreadId("one".into()))
        .await
        .unwrap();
    let logs = f.logs("a");
    for id in ["wrong session", "permission callback"] {
        let response = logs
            .iter()
            .find(|v| v["id"] == id && v.get("result").is_some())
            .unwrap();
        assert_eq!(
            response["result"],
            json!({"outcome":{"outcome":"cancelled"}})
        );
    }
}
#[tokio::test]
async fn generic_acp_unavailable_definition_and_revision_never_reroute() {
    let f = Fixture::new("resume");
    f.provider
        .start_session(f.input("one", Some("a")))
        .await
        .unwrap();
    f.provider
        .stop_session(ThreadId("one".into()))
        .await
        .unwrap();
    assert!(f
        .provider
        .start_session(f.input("one", Some("b")))
        .await
        .is_err());
    f.store
        .agents
        .lock()
        .unwrap()
        .get_mut("a")
        .unwrap()
        .agent
        .enabled = false;
    assert!(f
        .provider
        .start_session(f.input("one", None))
        .await
        .is_err());
    f.store
        .agents
        .lock()
        .unwrap()
        .get_mut("a")
        .unwrap()
        .agent
        .enabled = true;
    f.store
        .agents
        .lock()
        .unwrap()
        .get_mut("a")
        .unwrap()
        .agent
        .revision = "next revision".into();
    assert!(f
        .provider
        .start_session(f.input("one", None))
        .await
        .is_err());
    f.store.agents.lock().unwrap().remove("a");
    assert!(f
        .provider
        .start_session(f.input("one", None))
        .await
        .is_err());
}
#[tokio::test]
async fn generic_acp_protocol_rejection_and_resume_failure_have_no_fresh_fallback() {
    for mode in ["version", "bad-initialize"] {
        let f = Fixture::new(mode);
        assert!(f
            .provider
            .start_session(f.input("one", Some("a")))
            .await
            .is_err());
        assert!(f.store.binding("one").unwrap().is_none());
    }
    let f = Fixture::new("resume-fails");
    f.provider
        .start_session(f.input("one", Some("a")))
        .await
        .unwrap();
    f.provider
        .stop_session(ThreadId("one".into()))
        .await
        .unwrap();
    assert!(f
        .provider
        .start_session(f.input("one", None))
        .await
        .is_err());
    assert_eq!(
        f.logs("a")
            .iter()
            .filter(|v| v["method"] == "session/new")
            .count(),
        1
    );
}
#[tokio::test]
async fn generic_acp_load_replay_is_suppressed_and_tail_chunk_precedes_completion() {
    let f = Fixture::new("load");
    f.provider
        .start_session(f.input("one", Some("a")))
        .await
        .unwrap();
    f.provider
        .stop_session(ThreadId("one".into()))
        .await
        .unwrap();
    let mut events = f.provider.event_stream();
    f.provider
        .start_session(f.input("one", None))
        .await
        .unwrap();
    let t = f.provider.send_turn(turn("one")).await.unwrap().turn_id;
    let received = tokio::time::timeout(Duration::from_secs(3), async {
        let mut collected = Vec::new();
        loop {
            let e = events.next().await.unwrap();
            let done = matches!(&e,ProviderRuntimeEvent::TurnCompleted {turn_id,..} if *turn_id==t);
            collected.push(e);
            if done {
                break;
            }
        }
        collected
    })
    .await
    .unwrap();
    let serialized = serde_json::to_string(&received).unwrap();
    assert!(serialized.contains("tail chunk"));
    assert!(!serialized.contains("historical replay"));
    f.provider
        .stop_session(ThreadId("one".into()))
        .await
        .unwrap();
}
