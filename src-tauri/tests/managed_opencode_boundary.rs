//! Opt-in readiness of the actual installed native server; never sends a prompt.
#![cfg(target_os = "linux")]

use async_trait::async_trait;
use codemux_lib::agent_provider::{
    managed::{ManagedSession, ManagedTool, ManagedToolHandler},
    managed_bridge::ManagedBridgeSession,
    ProviderKind, StartSessionInput, ThreadId,
};
use codemux_lib::json_rpc_child::SpawnConfig;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

#[derive(Default)]
struct ReadinessOnly {
    evidence: Mutex<Option<Value>>,
}

#[async_trait]
impl ManagedToolHandler for ReadinessOnly {
    fn tools(&self) -> Vec<ManagedTool> {
        vec![ManagedTool {
            name: "workflow_submit_result".into(),
            description: "Submit result".into(),
            input_schema: json!({"type":"object"}),
        }]
    }
    async fn call(&self, _: &str, _: Value) -> Result<Value, String> {
        panic!("Native readiness must not infer or invoke host tools")
    }
    fn record_runtime(&self, evidence: Value) -> Result<(), String> {
        *self.evidence.lock().unwrap() = Some(evidence);
        Ok(())
    }
    async fn quiesce(&self) -> Result<(), String> {
        Ok(())
    }
}

#[tokio::test]
#[ignore = "requires explicitly prepared OpenCode 1.18.35 and compiled managed sidecar; token-free"]
async fn managed_opencode_native_readiness_and_verified_stop_without_tokens() {
    let binary =
        std::env::var_os("CODEMUX_TEST_OPENCODE_BINARY").expect("prepared OpenCode binary");
    let sidecar =
        std::env::var_os("CODEMUX_TEST_MANAGED_SIDECAR").expect("compiled managed sidecar");
    let checkout = tempfile::tempdir().unwrap();
    let captured = Arc::new(ReadinessOnly::default());
    let (events, _) = tokio::sync::broadcast::channel(32);
    let input = StartSessionInput {
        thread_id: ThreadId(format!("opencode-ready-{}", uuid::Uuid::new_v4())),
        cwd: checkout.path().into(),
        model: Some("openai/gpt-4.1".into()),
        effort: None,
        resume_cursor: None,
        fresh_session: true,
        permission_mode: None,
        context_window: None,
        fast_mode: false,
        additional_directories: vec![],
        env: None,
        workspace_id: None,
        extra: json!({}),
        recorded_usage_baseline: None,
    };
    let session = ManagedBridgeSession::spawn(
        input,
        ProviderKind::OpenCode,
        "opencode-managed-v1",
        Arc::new(ManagedSession {
            handler: captured.clone(),
            read_only: true,
        }),
        SpawnConfig {
            program: sidecar.into(),
            args: vec![],
            env: HashMap::from([("OPENAI_API_KEY".into(), "sk-dummy-no-inference".into())]),
            cwd: Some(checkout.path().into()),
            default_timeout: Duration::from_secs(60),
        },
        json!({"nativeBinary":binary.to_string_lossy()}),
        events,
    )
    .await
    .unwrap();
    assert!(!session.turn_active().await);
    assert!(!session.is_dead());
    session.shutdown_managed().await.unwrap();
    let pid = captured.evidence.lock().unwrap().as_ref().unwrap()["pid"]
        .as_u64()
        .unwrap() as i32;
    assert_ne!(unsafe { libc::kill(-pid, 0) }, 0);
    assert_eq!(
        std::io::Error::last_os_error().raw_os_error(),
        Some(libc::ESRCH)
    );
}
