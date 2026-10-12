//! Real OpenCode 1.x/2.x acceptance with an isolated, scripted loopback model.
//! Never uses a real account. The Node runner owns CLI installation, config,
//! the model peer and the disposable checkout; this exercises production Rust.
use std::{path::PathBuf, sync::Arc, time::Duration};

use codemux_lib::agent_provider::{
    events::{CompletedItem, ContentDelta, ProviderRuntimeEvent, TurnStatus},
    opencode::{
        api::OpenCodeApi, OpenCodeAgentProvider, OpenCodeClient, OpenCodeClientConfig,
        OpenCodeProviderConfig, OpenCodeServer, OpenCodeServerManager,
    },
    provider::{AgentProvider, ProviderEventStream},
    ApprovalDecision, SendTurnInput, StartSessionInput, ThreadId, TurnId,
};
use futures_util::StreamExt;
use serde_json::json;

fn cwd() -> PathBuf {
    let path = PathBuf::from(
        std::env::var_os("CODEMUX_TEST_OPENCODE_CWD").expect("Use the isolated Node runner"),
    );
    assert!(path.is_absolute() && path.is_dir());
    path
}

fn start_input(thread: &str) -> StartSessionInput {
    serde_json::from_value(json!({
        "thread_id": thread, "cwd": cwd(), "model": "compatibility/fixture",
        "resume_cursor": null, "permission_mode": null, "effort": "low",
        "additional_directories": [], "env": null, "extra": {}
    }))
    .unwrap()
}

fn turn_input(thread: &str, text: &str) -> SendTurnInput {
    serde_json::from_value(json!({
        "thread_id": thread, "text": text, "images": [], "model_override": null,
        "effort_override": "high", "permission_mode_override": null
    }))
    .unwrap()
}

async fn until_complete(
    provider: &OpenCodeAgentProvider,
    stream: &mut ProviderEventStream,
    thread: &ThreadId,
    turn: &TurnId,
) -> Vec<ProviderRuntimeEvent> {
    tokio::time::timeout(Duration::from_secs(45), async {
        let mut observed = Vec::new();
        loop {
            let event = stream.next().await.expect("Provider event stream closed");
            if let ProviderRuntimeEvent::RequestOpened { thread_id, request_id, .. } = &event {
                assert_eq!(thread_id, thread);
                provider.respond_to_request(thread.clone(), request_id.clone(), ApprovalDecision::Allow {
                    updated_input: None, updated_permissions: None,
                }).await.expect("Real permission reply must be delivered");
            }
            let completed = matches!(&event, ProviderRuntimeEvent::TurnCompleted { turn_id, .. } if turn_id == turn);
            observed.push(event);
            if completed { return observed; }
        }
    }).await.expect("Real OpenCode turn did not settle")
}

async fn assert_single_completion(stream: &mut ProviderEventStream, turn: &TurnId) {
    let deadline = tokio::time::Instant::now() + Duration::from_millis(300);
    while let Ok(Some(event)) = tokio::time::timeout_at(deadline, stream.next()).await {
        assert!(
            !matches!(event, ProviderRuntimeEvent::TurnCompleted { turn_id, .. } if &turn_id == turn),
            "Duplicate terminal event for the same real turn"
        );
    }
}

#[tokio::test]
#[ignore = "requires isolated real npm CLI and loopback model; scripts/e2e/opencode-native-smoke.mjs"]
async fn real_opencode_models_approvals_tools_interrupt_and_resume() {
    let manager = Arc::new(OpenCodeServerManager::new());
    let handle = manager.ensure_running().await.unwrap();
    let expected = if std::env::var("CODEMUX_TEST_OPENCODE_API").unwrap() == "2" {
        OpenCodeApi::V2
    } else {
        OpenCodeApi::Legacy
    };
    assert_eq!(handle.api, expected);
    let mut config = OpenCodeClientConfig::new(handle.base_url.clone());
    config.api = handle.api;
    config.server_password = Some(handle.server_password.clone());
    let client = OpenCodeClient::new(config).unwrap();
    let providers = client.list_models().await.unwrap();
    let models = &providers
        .iter()
        .find(|provider| provider.id == "compatibility")
        .expect("Actual model catalog must contain the configured fixture")
        .models;
    assert!(models["fixture"]
        .variants
        .iter()
        .any(|variant| variant == "high"));
    assert_eq!(models["fixture"].context_window, Some(32768));
    let unauthorized = reqwest::Client::new()
        .get(format!(
            "{}{}",
            handle.base_url,
            expected.route("/provider", "/api/model")
        ))
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .unwrap();
    assert_eq!(
        unauthorized.status().as_u16(),
        401,
        "Owned server must require its minted password"
    );

    let provider = OpenCodeAgentProvider::new(manager.clone(), OpenCodeProviderConfig::default());
    let thread = ThreadId("native-compatibility".into());
    let mut stream = provider.event_stream();
    let session = provider
        .start_session(start_input(&thread.0))
        .await
        .unwrap();
    let turn = provider
        .send_turn(turn_input(
            &thread.0,
            "OC_FIXTURE_EDIT: create opencode-proof.txt in this checkout",
        ))
        .await
        .unwrap();
    let observed = until_complete(&provider, &mut stream, &thread, &turn.turn_id).await;
    assert!(
        observed
            .iter()
            .any(|event| matches!(event, ProviderRuntimeEvent::RequestOpened { .. })),
        "Real file write must reach Codemux's approval path"
    );
    assert!(observed.iter().any(|event| matches!(event, ProviderRuntimeEvent::ItemCompleted { item: CompletedItem::ToolUse { tool_name, .. }, .. } if tool_name == "write")));
    assert!(observed.iter().any(|event| matches!(
        event,
        ProviderRuntimeEvent::ItemCompleted {
            item: CompletedItem::ToolResult {
                is_error: false,
                ..
            },
            ..
        }
    )));
    assert!(observed.iter().any(|event| matches!(event, ProviderRuntimeEvent::ItemCompleted { item: CompletedItem::AssistantText { text }, .. } if text.contains("fixture response complete"))));
    assert!(observed.iter().any(|event| matches!(
        event,
        ProviderRuntimeEvent::TurnCompleted {
            status: TurnStatus::Success,
            ..
        }
    )));
    assert_eq!(
        std::fs::read_to_string(cwd().join("opencode-proof.txt")).unwrap(),
        "OpenCode compatibility verified\n"
    );
    assert_single_completion(&mut stream, &turn.turn_id).await;
    let history = client
        .get_session_messages(&session.session_id.0, None)
        .await
        .expect("Native typed history must decode through the production client");
    assert!(history
        .iter()
        .any(|message| message.info.role == "assistant"));
    assert!(history
        .iter()
        .all(|message| message.info.session_id == session.session_id.0));
    assert!(client
        .list_children(&session.session_id.0)
        .await
        .expect("Native child listing must use this server's dialect")
        .is_empty());

    let held = provider
        .send_turn(turn_input(
            &thread.0,
            "OC_FIXTURE_HOLD: keep this turn open",
        ))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            if let Some(ProviderRuntimeEvent::ContentDelta {
                turn_id,
                delta: ContentDelta::Text { .. },
                ..
            }) = stream.next().await
            {
                if turn_id == held.turn_id {
                    break;
                }
            }
        }
    })
    .await
    .expect("Held turn must actually stream before interruption");
    provider
        .interrupt_turn(thread.clone(), Some(held.turn_id.clone()))
        .await
        .unwrap();
    let stopped = until_complete(&provider, &mut stream, &thread, &held.turn_id).await;
    assert!(
        stopped.iter().any(|event| matches!(
            event,
            ProviderRuntimeEvent::TurnCompleted {
                status: TurnStatus::Error { .. },
                ..
            }
        )),
        "Interrupted turn must not be reported as a normal success"
    );
    assert_single_completion(&mut stream, &held.turn_id).await;

    // Native detach acknowledges pending admission and preserves history, so
    // finish its server-side fence before reaping the owned process. Legacy
    // detach deletes live history; reap first to keep its resume fixture.
    if expected == OpenCodeApi::V2 {
        provider.stop_session(thread.clone()).await.unwrap();
        manager.stop().await;
    } else {
        manager.stop().await;
        provider.stop_session(thread.clone()).await.unwrap();
    }
    let mut resume = start_input(&thread.0);
    resume.resume_cursor = session.resume_cursor.clone();
    let resumed = provider.start_session(resume).await.unwrap();
    assert_eq!(
        resumed.session_id, session.session_id,
        "Restart must readopt the persisted session, not create an empty one"
    );
    let continued = provider
        .send_turn(turn_input(
            &thread.0,
            "OC_FIXTURE_RESUME: continue the saved conversation",
        ))
        .await
        .unwrap();
    let resumed_events = until_complete(&provider, &mut stream, &thread, &continued.turn_id).await;
    assert!(resumed_events.iter().any(|event| matches!(
        event,
        ProviderRuntimeEvent::TurnCompleted {
            status: TurnStatus::Success,
            ..
        }
    )));
    assert_single_completion(&mut stream, &continued.turn_id).await;
    provider.stop_session(thread).await.unwrap();
    manager.shutdown().await.unwrap();
    assert_eq!(
        manager.ensure_running().await.unwrap_err(),
        "opencode_manager_shutdown"
    );
}

#[tokio::test]
#[ignore = "requires the runner's prepared executable; never uses account credentials"]
async fn real_opencode_owned_server_is_reaped_on_shutdown() {
    let binary = PathBuf::from(
        std::env::var_os("CODEMUX_TEST_OPENCODE_BINARY").expect("prepared native executable"),
    );
    let mut server = OpenCodeServer::spawn(&binary).await.unwrap();
    let pid = sysinfo::Pid::from_u32(server.pid().unwrap());
    server.shutdown().await.unwrap();
    assert_eq!(
        server.pid(),
        None,
        "Shutdown must await the native direct child"
    );
    let mut system = sysinfo::System::new();
    system.refresh_processes_specifics(
        sysinfo::ProcessesToUpdate::Some(&[pid]),
        true,
        sysinfo::ProcessRefreshKind::nothing(),
    );
    assert!(
        system.process(pid).is_none(),
        "Owned native child must be gone when shutdown returns"
    );
    server.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore = "requires the runner's prepared executable; never uses account credentials"]
async fn real_opencode_owned_server_is_reaped_on_drop() {
    let binary = PathBuf::from(
        std::env::var_os("CODEMUX_TEST_OPENCODE_BINARY").expect("prepared native executable"),
    );
    let server = OpenCodeServer::spawn(&binary).await.unwrap();
    let pid = sysinfo::Pid::from_u32(server.pid().unwrap());
    drop(server);
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut system = sysinfo::System::new();
        loop {
            system.refresh_processes_specifics(
                sysinfo::ProcessesToUpdate::Some(&[pid]),
                true,
                sysinfo::ProcessRefreshKind::nothing(),
            );
            if system.process(pid).is_none() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("Actual native child must be reaped on both platforms");
}
