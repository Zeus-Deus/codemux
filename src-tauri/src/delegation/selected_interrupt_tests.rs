use std::os::unix::fs::PermissionsExt;
use std::{path::Path, sync::Arc, time::Duration};

use futures_util::StreamExt;
use serde_json::{json, Value};
use tauri::Manager;

use super::{
    app::{native_call, NativeActor},
    authority::{DispatchPermit, NativeAuthority},
    journal::Journal,
};
use crate::agent_provider::{
    AgentProvider, ProviderKind, ProviderRuntimeEvent, SendTurnInput, ThreadId,
};

#[tokio::test]
async fn stale_selected_codex_interrupt_preserves_native_and_result_authority() {
    stale_selected_interrupt(ProviderKind::Codex).await;
}

#[tokio::test]
async fn stale_selected_claude_interrupt_preserves_native_and_result_authority() {
    stale_selected_interrupt(ProviderKind::Claude).await;
}

async fn stale_selected_interrupt(kind: ProviderKind) {
    let dir = tempfile::TempDir::new().unwrap();
    let root = dir.path();
    let thread = format!("selected-interrupt-{kind:?}-{}", uuid::Uuid::new_v4());
    let mode = if kind == ProviderKind::Codex {
        "danger-full-access"
    } else {
        "bypassPermissions"
    };
    let method = if kind == ProviderKind::Codex {
        "turn/interrupt"
    } else {
        "interrupt"
    };
    let mut script = Vec::new();
    if kind == ProviderKind::Codex {
        // The marker follows both completions, so no old notification can
        // settle turn B after the first interrupt has finished.
        for turn in ["t-1", "t-2"] {
            script.push(
                json!({"after":method,"emit":"notification","method":"turn/completed",
                "params":{"threadId":"c-1","turnId":turn,"status":"succeeded"}}),
            );
        }
    } else {
        script.push(
            json!({"after":method,"emit":"notification","method":"turn-interrupted",
            "params":{"threadId":thread}}),
        );
    }
    script.push(json!({"after":method,"emit":"notification","method":"fixture/selected-interrupt-marker","params":{}}));
    let script_path = root.join("script.json");
    let trace_path = root.join("trace.jsonl");
    std::fs::write(&script_path, json!(script).to_string()).unwrap();
    let helper = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target/debug")
        .join(if kind == ProviderKind::Codex {
            "fake_codex_app_server"
        } else {
            "fake_claude_sidecar"
        });
    assert!(
        helper.is_file(),
        "build the owned fake native providers first"
    );
    let binary = root.join("fixture");
    let (script_key, trace_key) = if kind == ProviderKind::Codex {
        ("FAKE_CODEX_SCRIPT", "FAKE_CODEX_TRACE")
    } else {
        ("FAKE_CLAUDE_SIDECAR_SCRIPT", "FAKE_CLAUDE_SIDECAR_CAPTURE")
    };
    std::fs::write(
        &binary,
        format!(
            "#!/bin/sh\nexport {script_key}='{}'\nexport {trace_key}='{}'\nexec '{}' \"$@\"\n",
            script_path.display(),
            trace_path.display(),
            helper.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
    let provider: Arc<dyn AgentProvider> = if kind == ProviderKind::Codex {
        Arc::new(crate::agent_provider::codex::CodexAgentProvider::new(
            crate::agent_provider::codex::CodexProviderConfig {
                codex_binary: binary,
                codex_home: Some(root.join("codex")),
                ..Default::default()
            },
        ))
    } else {
        Arc::new(
            crate::agent_provider::claude::ClaudeAgentProvider::new(
                crate::agent_provider::claude::ClaudeProviderConfig {
                    sidecar_binary: Some(binary),
                    claude_binary: Some(helper),
                    ..Default::default()
                },
            )
            .await
            .unwrap(),
        )
    };
    let desktop = tauri::test::mock_app();
    let state = crate::state::AppStateStore::default();
    let workspace = state.create_workspace_at_path(root.to_path_buf()).0;
    state
        .create_agent_chat_pane(&workspace, Some(kind), None, None, Some(thread.clone()))
        .unwrap();
    let db = crate::database::DatabaseStore::new_in_memory();
    db.upsert_agent_chat_session(
        &thread,
        &workspace,
        Some(root.to_str().unwrap()),
        if kind == ProviderKind::Codex {
            "codex"
        } else {
            "claude"
        },
    )
    .unwrap();
    db.update_agent_chat_session_config(
        &thread,
        &crate::database::AgentChatSessionConfig {
            permission_mode: crate::database::AgentChatSessionConfig::set(mode),
            ..Default::default()
        },
    )
    .unwrap();
    let host = db.insert_host("Fixture", "fixture-host").unwrap();
    let journal = Arc::new(Journal::open(&root.join("journal")).unwrap());
    let (mut parent, mut grant, input) = super::tests::fixture();
    parent.thread_id = thread.clone();
    parent.workspace_id = workspace.clone();
    parent.provider = kind;
    parent.permission_mode = mode.into();
    grant.host_id = host.id;
    grant.ssh_target = host.ssh_target;
    journal.put_grant(&workspace, &grant).unwrap();
    let task = journal.admit(&parent, &grant, "checkout", &input).unwrap();
    desktop.manage(state);
    desktop.manage(db);
    desktop.manage(journal);
    let registry = crate::commands::agent_chat::ProviderRegistry::new();
    if kind == ProviderKind::Codex {
        registry.set_codex(provider.clone()).await;
    } else {
        registry.set_claude(provider.clone()).await;
    }
    desktop.manage(registry);
    let session = provider
        .start_session(
            serde_json::from_value(json!({"thread_id":thread,"cwd":root,
        "additional_directories":[],"permission_mode":mode,"workspace_id":workspace}))
            .unwrap(),
        )
        .await
        .unwrap();
    let mut events = provider.event_stream();
    let send = |text: &str| -> SendTurnInput {
        serde_json::from_value(json!({"thread_id":thread,"text":text,"images":[]})).unwrap()
    };
    let first = provider.send_turn(send("turn A")).await.unwrap();
    provider
        .interrupt_turn(ThreadId(thread.clone()), Some(first.turn_id.clone()))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(event) = events.next().await {
            if matches!(event, ProviderRuntimeEvent::RuntimeWarning { message, .. } if message.contains("selected-interrupt-marker")) { return; }
        }
        panic!("native interrupt marker disappeared");
    }).await.unwrap();
    let second = provider.send_turn(send("turn B")).await.unwrap();
    assert_ne!(first.turn_id, second.turn_id);
    let native = NativeAuthority::current_permit(&thread);
    let result = DispatchPermit::capture(&thread).unwrap();
    let actor = NativeActor {
        thread_id: thread.clone(),
        provider: kind,
        workspace_id: Some(workspace),
        session_id: session.session_id.0,
        permission_mode: Some(mode.into()),
        permit: native.clone(),
    };
    let targets_before = native_call(
        desktop.handle().clone(),
        actor.clone(),
        "delegation_targets".into(),
        json!({}),
    )
    .await
    .unwrap();
    let status_before = native_call(
        desktop.handle().clone(),
        actor.clone(),
        "task_status".into(),
        json!({"task_id":task.id}),
    )
    .await
    .unwrap();
    let stale = provider
        .interrupt_turn(ThreadId(thread.clone()), Some(first.turn_id))
        .await;
    let native_survived = native.admit().is_ok();
    let result_survived = result.admit().is_ok();
    let targets_after = native_call(
        desktop.handle().clone(),
        actor.clone(),
        "delegation_targets".into(),
        json!({}),
    )
    .await;
    let status_after = native_call(
        desktop.handle().clone(),
        actor,
        "task_status".into(),
        json!({"task_id":task.id}),
    )
    .await;
    let trace = std::fs::read_to_string(&trace_path).unwrap();
    let interrupts = trace
        .lines()
        .filter(|line| serde_json::from_str::<Value>(line).unwrap()["method"] == method)
        .count();
    let blanket = provider
        .interrupt_turn(ThreadId(thread.clone()), None)
        .await;
    let blanket_revoked = native.admit().is_err() && result.admit().is_err();
    provider.stop_session(ThreadId(thread)).await.unwrap();
    assert!(stale.unwrap_err().to_string().contains("turn id mismatch"));
    assert!(
        native_survived && result_survived,
        "{kind:?}: stale selection revoked B: native={native_survived}, result={result_survived}"
    );
    assert_eq!(targets_after.unwrap(), targets_before);
    assert_eq!(status_after.unwrap(), status_before);
    assert_eq!(
        interrupts, 1,
        "stale selection crossed the native interrupt wire"
    );
    assert!(
        blanket.is_ok() && blanket_revoked,
        "blanket Stop must revoke both authorities"
    );
}
