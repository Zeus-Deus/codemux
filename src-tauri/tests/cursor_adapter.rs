//! Cursor ACP adapter integration coverage.
//!
//! The live smoke test is ignored in CI because it requires an installed,
//! authenticated Cursor Agent CLI. Run it manually when changing session
//! lifecycle or model configuration behavior.

use std::path::PathBuf;
use std::sync::Arc;

use codemux_lib::agent_provider::acp::session::AcpDialect;
use codemux_lib::agent_provider::acp::slash_commands::AcpSlashCommandCache;
use codemux_lib::agent_provider::cursor::capabilities::{
    harvest_cursor_capabilities, HarvestError,
};
use codemux_lib::agent_provider::cursor::{CursorAgentProvider, CursorProviderConfig};
use codemux_lib::agent_provider::{
    AgentProvider, ApprovalDecision, CompletedItem, ProviderError, ProviderRuntimeEvent, RequestId,
    SendTurnInput, StartSessionInput, ThreadId, TurnStatus,
};
use futures_util::StreamExt;
use tokio::time::{timeout, Duration};

#[tokio::test]
async fn cursor_discovery_uses_existing_credentials_without_requesting_login() {
    let capabilities =
        harvest_cursor_capabilities(std::path::Path::new(env!("CARGO_BIN_EXE_fake_cursor_acp")))
            .await
            .expect("discovery must read models without requesting interactive authentication");

    assert_eq!(capabilities.models.len(), 1);
    assert_eq!(capabilities.models[0].id, "fake-model");
}

#[tokio::test]
async fn cursor_discovery_reports_signed_out_without_requesting_login() {
    let fixture_dir = tempfile::tempdir().expect("fixture directory");
    let binary = fixture_dir
        .path()
        .join(format!("signed-out{}", std::env::consts::EXE_SUFFIX));
    std::fs::copy(env!("CARGO_BIN_EXE_fake_cursor_acp"), &binary).expect("copy signed-out fixture");

    let error = harvest_cursor_capabilities(&binary)
        .await
        .expect_err("signed-out discovery must fail without requesting interactive authentication");

    assert!(matches!(error, HarvestError::NotAuthenticated { hint }
        if hint.contains("cursor-agent login")));
}

fn start_input(
    thread_id: &str,
    resume_cursor: Option<serde_json::Value>,
    fast_mode: bool,
) -> StartSessionInput {
    StartSessionInput {
        thread_id: ThreadId(thread_id.into()),
        cwd: std::env::current_dir().expect("test cwd"),
        model: Some("grok-4.6".into()),
        resume_cursor,
        fresh_session: false,
        permission_mode: Some("agent".into()),
        effort: Some("high".into()),
        context_window: None,
        fast_mode,
        service_tier: None,
        additional_directories: vec![],
        env: None,
        workspace_id: None,
        remote: None,
        extra: serde_json::Value::Null,
        recorded_usage_baseline: None,
    }
}

/// Provider wired to the `fake_cursor_acp` fixture instead of a real CLI.
fn fixture_provider() -> CursorAgentProvider {
    fixture_provider_with_commands().0
}

/// Same fixture provider, plus the command cache it shares with the IPC
/// layer so a test can read what the live session published.
fn fixture_provider_with_commands() -> (CursorAgentProvider, Arc<AcpSlashCommandCache>) {
    let commands = Arc::new(AcpSlashCommandCache::new());
    let provider = CursorAgentProvider::new_with_slash_command_cache(
        CursorProviderConfig {
            binary: PathBuf::from(env!("CARGO_BIN_EXE_fake_cursor_acp")),
            event_channel_capacity: 1024,
            mcp_registry: None,
        },
        Arc::clone(&commands),
    );
    (provider, commands)
}

fn fixture_start_input(thread_id: &str) -> StartSessionInput {
    StartSessionInput {
        thread_id: ThreadId(thread_id.into()),
        cwd: std::env::current_dir().expect("test cwd"),
        model: None,
        resume_cursor: None,
        fresh_session: false,
        permission_mode: Some("ask".into()),
        effort: None,
        context_window: None,
        fast_mode: false,
        service_tier: None,
        additional_directories: vec![],
        env: None,
        workspace_id: None,
        remote: None,
        extra: serde_json::Value::Null,
        recorded_usage_baseline: None,
    }
}

fn fixture_turn(thread_id: &str, text: &str) -> SendTurnInput {
    SendTurnInput {
        thread_id: ThreadId(thread_id.into()),
        text: text.into(),
        images: vec![],
        model_override: None,
        effort_override: None,
        permission_mode_override: None,
        client_nonce: None,
        display_text: None,
        skill_invocations: vec![],
        turn_checkpoint: None,
        dispatch_guard: None,
    }
}

/// Regression: `session/prompt`'s response is resolved through the
/// transport's oneshot while the trailing `agent_message_chunk`
/// notifications ahead of it on stdout are still queued in the session's
/// broadcast channel. Completing the turn on the response alone dropped
/// that tail — the assistant message the transcript persisted was short by
/// however many chunks lost the race. The adapter now waits for its
/// notification pipeline to drain before finishing the turn, so the
/// completed item must carry every chunk the fixture wrote.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cursor_turn_keeps_trailing_message_chunks() {
    const CHUNKS: usize = 40;
    let provider = fixture_provider();
    let mut events = provider.event_stream();
    let thread_id = "cursor-chunk-order";

    provider
        .start_session(fixture_start_input(thread_id))
        .await
        .expect("start fixture session");
    provider
        .send_turn(fixture_turn(thread_id, &format!("chunks:{CHUNKS}")))
        .await
        .expect("send turn");

    let text = timeout(Duration::from_secs(20), async {
        let mut assistant_text = None;
        while let Some(event) = events.next().await {
            match event {
                ProviderRuntimeEvent::ItemCompleted {
                    item: CompletedItem::AssistantText { text },
                    ..
                } => assistant_text = Some(text),
                ProviderRuntimeEvent::TurnCompleted { .. } => return assistant_text,
                _ => {}
            }
        }
        panic!("event stream closed before turn completion");
    })
    .await
    .expect("turn timed out")
    .expect("assistant text item");

    let expected = (0..CHUNKS)
        .map(|index| format!("[{index}]"))
        .collect::<String>();
    assert_eq!(text, expected);
    provider
        .stop_session(ThreadId(thread_id.into()))
        .await
        .expect("stop fixture session");
}

/// The catalogue arrives as a plain `session/update` after `session/new`,
/// before any turn is active, and its entries carry no argument hint. The
/// session must record it anyway, under its own provider key.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn live_session_update_populates_the_shared_command_cache() {
    let (provider, commands) = fixture_provider_with_commands();
    let thread_id = "cursor-available-commands";
    let cwd = std::env::current_dir().expect("test cwd");

    provider
        .start_session(fixture_start_input(thread_id))
        .await
        .expect("start fixture session");

    // The catalogue is pushed independently of the `session/new` response,
    // so it can legitimately land a moment after the session is ready.
    let published = timeout(Duration::from_secs(10), async {
        loop {
            let entries = commands.cached(AcpDialect::Cursor, &cwd).await;
            if !entries.is_empty() {
                return entries;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("session never published its command catalogue");

    assert_eq!(
        provider
            .session_slash_commands(ThreadId(thread_id.into()), &cwd)
            .await
            .unwrap(),
        published
    );
    assert!(provider
        .session_slash_commands(
            ThreadId(thread_id.into()),
            std::path::Path::new("/wrong-directory")
        )
        .await
        .is_err());
    // A catalogue owned by another conversation must not be returned just
    // because it has the same cwd.
    assert!(provider
        .session_slash_commands(ThreadId("missing-conversation".into()), &cwd)
        .await
        .unwrap()
        .is_empty());

    assert_eq!(
        published
            .iter()
            .map(|command| command.name.as_str())
            .collect::<Vec<_>>(),
        vec!["copy-request-id", "plan"]
    );
    assert!(published
        .iter()
        .all(|command| command.argument_hint.is_empty()));
    assert_eq!(published[1].description, "Draft a plan (project)");
    assert!(
        commands.cached(AcpDialect::Grok, &cwd).await.is_empty(),
        "one provider's catalogue must never be served for another"
    );

    provider
        .stop_session(ThreadId(thread_id.into()))
        .await
        .expect("stop fixture session");
}

/// Regression: a turn that ends with a `session/request_permission` still
/// outstanding used to clear the pending map silently. The child never got
/// the `cancelled` outcome ACP requires, and the UI's approval surface was
/// never resolved — clicking Allow afterwards failed with
/// `RequestNotPending` on a card that could no longer be dismissed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cursor_turn_cancels_an_open_permission_request() {
    let provider = fixture_provider();
    let mut events = provider.event_stream();
    let thread_id = "cursor-pending-cancel";

    provider
        .start_session(fixture_start_input(thread_id))
        .await
        .expect("start fixture session");
    provider
        .send_turn(fixture_turn(thread_id, "please ask for permission"))
        .await
        .expect("send turn");

    let (opened, resolved) = timeout(Duration::from_secs(20), async {
        let mut opened: Option<RequestId> = None;
        let mut resolved: Option<(RequestId, ApprovalDecision)> = None;
        while let Some(event) = events.next().await {
            match event {
                ProviderRuntimeEvent::RequestOpened { request_id, .. } => {
                    opened = Some(request_id);
                }
                ProviderRuntimeEvent::RequestResolved {
                    request_id,
                    decision,
                    ..
                } => resolved = Some((request_id, decision)),
                ProviderRuntimeEvent::TurnCompleted { .. } => return (opened, resolved),
                _ => {}
            }
        }
        panic!("event stream closed before turn completion");
    })
    .await
    .expect("turn timed out");

    let opened = opened.expect("permission request was opened");
    let (resolved_id, decision) = resolved.expect("pending request resolved on turn end");
    assert_eq!(resolved_id, opened);
    assert!(matches!(decision, ApprovalDecision::Cancel));

    // The approval is genuinely gone, not just visually resolved.
    let err = provider
        .respond_to_request(
            ThreadId(thread_id.into()),
            opened,
            ApprovalDecision::Allow {
                updated_input: None,
                updated_permissions: None,
            },
        )
        .await
        .expect_err("a cancelled request is no longer pending");
    assert!(matches!(err, ProviderError::RequestNotPending { .. }));
    provider
        .stop_session(ThreadId(thread_id.into()))
        .await
        .expect("stop fixture session");
}

/// Regression: Stop must reach the child no matter where in the turn's
/// lifecycle it lands. The fixture holds this prompt open until it
/// receives a `session/cancel`, so the turn can only complete if the
/// adapter actually delivered the interrupt — and delivered it after the
/// prompt, since the fixture had to receive the prompt to hold it. A
/// Stop that arrives before the prompt goes out is equally honored: the
/// worker abandons the turn instead of sending it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cursor_stop_is_never_lost_whenever_it_lands() {
    let provider = fixture_provider();
    let mut events = provider.event_stream();
    let thread_id = "cursor-stop";

    provider
        .start_session(fixture_start_input(thread_id))
        .await
        .expect("start fixture session");
    let started = provider
        .send_turn(fixture_turn(thread_id, "await-cancel please"))
        .await
        .expect("send turn");
    provider
        .interrupt_turn(ThreadId(thread_id.into()), Some(started.turn_id))
        .await
        .expect("interrupt");

    let status = timeout(Duration::from_secs(20), async {
        while let Some(event) = events.next().await {
            if let ProviderRuntimeEvent::TurnCompleted { status, .. } = event {
                return status;
            }
        }
        panic!("event stream closed before turn completion");
    })
    .await
    .expect("the interrupted turn never completed — Stop was dropped");

    assert!(
        matches!(status, TurnStatus::Error { ref subtype, .. } if subtype == "interrupted"),
        "expected an interrupted turn, got {status:?}"
    );
    provider
        .stop_session(ThreadId(thread_id.into()))
        .await
        .expect("stop fixture session");
}

/// Run manually with:
/// `cargo test -j 2 --manifest-path src-tauri/Cargo.toml --test cursor_adapter \
///   cursor_real_session_resumes_across_fast_tier_changes -- --ignored --nocapture`
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires real cursor-agent CLI + Cursor auth; run manually"]
async fn cursor_real_session_resumes_across_fast_tier_changes() {
    let provider = CursorAgentProvider::new(CursorProviderConfig::default());
    let mut events = provider.event_stream();
    let thread_id = format!(
        "cursor-live-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock")
            .as_nanos()
    );

    let standard = provider
        .start_session(start_input(&thread_id, None, false))
        .await
        .expect("start Standard Cursor session");
    let resume_cursor = standard.resume_cursor.clone();
    let provider_session_id = standard.session_id.clone();
    provider
        .send_turn(SendTurnInput {
            thread_id: ThreadId(thread_id.clone()),
            text: "Reply with OK only.".into(),
            images: vec![],
            model_override: None,
            effort_override: Some("high".into()),
            permission_mode_override: Some("agent".into()),
            client_nonce: None,
            display_text: None,
            skill_invocations: vec![],
            turn_checkpoint: None,
            dispatch_guard: None,
        })
        .await
        .expect("send first Cursor turn");
    timeout(Duration::from_secs(90), async {
        while let Some(event) = events.next().await {
            if matches!(event, ProviderRuntimeEvent::TurnCompleted { .. }) {
                return;
            }
        }
        panic!("Cursor event stream closed before turn completion");
    })
    .await
    .expect("Cursor first turn timed out");
    provider
        .stop_session(ThreadId(thread_id.clone()))
        .await
        .expect("stop Standard Cursor session");

    let fast = provider
        .start_session(start_input(&thread_id, resume_cursor.clone(), true))
        .await
        .expect("resume Cursor session in Fast mode");
    assert_eq!(fast.session_id, provider_session_id);
    provider
        .stop_session(ThreadId(thread_id.clone()))
        .await
        .expect("stop Fast Cursor session");

    let standard_again = provider
        .start_session(start_input(&thread_id, resume_cursor, false))
        .await
        .expect("resume Cursor session back in Standard mode");
    assert_eq!(standard_again.session_id, provider_session_id);
    provider
        .stop_session(ThreadId(thread_id))
        .await
        .expect("stop final Cursor session");
}

/// A local chat receives Codemux's MCP gateway as one HTTP server whose
/// token, unique to the chat, the gateway accepts.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cursor_chat_receives_the_codemux_mcp_gateway() {
    let provider = CursorAgentProvider::new(CursorProviderConfig {
        binary: PathBuf::from(env!("CARGO_BIN_EXE_fake_cursor_acp")),
        event_channel_capacity: 1024,
        mcp_registry: Some(codemux_lib::mcp::registry::McpRegistry::new()),
    });
    let mut events = provider.event_stream();
    let thread_id = "cursor-mcp-gateway";
    let mut input = fixture_start_input(thread_id);
    input.workspace_id = Some("ws-1".into());
    provider.start_session(input).await.expect("start fixture session");
    provider
        .send_turn(fixture_turn(thread_id, "mcp-servers"))
        .await
        .expect("send turn");
    let text = timeout(Duration::from_secs(20), async {
        let mut assistant_text = None;
        while let Some(event) = events.next().await {
            match event {
                ProviderRuntimeEvent::ItemCompleted {
                    item: CompletedItem::AssistantText { text },
                    ..
                } => assistant_text = Some(text),
                ProviderRuntimeEvent::TurnCompleted { .. } => return assistant_text,
                _ => {}
            }
        }
        panic!("event stream closed before turn completion");
    })
    .await
    .expect("turn timed out")
    .expect("assistant text item");

    let servers: serde_json::Value = serde_json::from_str(&text).expect("mcpServers JSON");
    let servers = servers.as_array().expect("mcpServers array");
    assert_eq!(servers.len(), 1, "{text}");
    assert_eq!(servers[0]["type"], "http");
    assert!(servers[0]["name"].as_str().unwrap().starts_with("codemux_"));
    assert_eq!(servers[0]["headers"][0]["name"], "Authorization");
    let url = servers[0]["url"].as_str().unwrap();
    let authorization = servers[0]["headers"][0]["value"].as_str().unwrap();
    assert!(url.starts_with("http://127.0.0.1:"), "{url}");

    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let list = serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"});
    let response = client
        .post(url)
        .header("authorization", authorization)
        .json(&list)
        .send()
        .await
        .expect("gateway reachable");
    assert_eq!(response.status(), 200);
    let body: serde_json::Value = response.json().await.unwrap();
    assert!(body["result"]["tools"].is_array(), "{body}");
    let anonymous = client.post(url).json(&list).send().await.unwrap();
    assert_eq!(anonymous.status(), 401);

    provider
        .stop_session(ThreadId(thread_id.into()))
        .await
        .expect("stop fixture session");
}

/// The `mcpServers` the fake agent saw on its latest `session/new` or
/// `session/load`, read back through a `mcp-servers` turn.
async fn mcp_servers_seen(
    provider: &CursorAgentProvider,
    events: &mut codemux_lib::agent_provider::ProviderEventStream,
    thread_id: &str,
) -> serde_json::Value {
    provider
        .send_turn(fixture_turn(thread_id, "mcp-servers"))
        .await
        .expect("send turn");
    let text = timeout(Duration::from_secs(20), async {
        let mut assistant_text = None;
        while let Some(event) = events.next().await {
            match event {
                ProviderRuntimeEvent::ItemCompleted {
                    item: CompletedItem::AssistantText { text },
                    ..
                } => assistant_text = Some(text),
                ProviderRuntimeEvent::TurnCompleted { .. } => return assistant_text,
                _ => {}
            }
        }
        panic!("event stream closed before turn completion");
    })
    .await
    .expect("turn timed out")
    .expect("assistant text item");
    serde_json::from_str(&text).expect("mcpServers JSON")
}

async fn listed_tool_names(servers: &serde_json::Value) -> Vec<String> {
    let response = reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap()
        .post(servers[0]["url"].as_str().unwrap())
        .header("authorization", servers[0]["headers"][0]["value"].as_str().unwrap())
        .json(&serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = response.json().await.unwrap();
    body["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap().to_string())
        .collect()
}

/// A reopened chat rotates its credential and acts for its current workspace: a
/// plugin server scoped to the old workspace disappears from its tools.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reopened_cursor_chat_takes_its_new_workspace() {
    let registry = codemux_lib::mcp::registry::McpRegistry::new();
    let server: codemux_lib::mcp::registry::PluginMcpServer = serde_json::from_value(serde_json::json!({
        "id": "fixture", "name": "Fixture", "toolPrefix": "fixture",
        "scope": {"workspaceIds": ["ws-a"]},
        "transport": "stdio", "command": "python3",
        "args": [format!("{}/tests/fixtures/plugin_mcp.py", env!("CARGO_MANIFEST_DIR"))],
    }))
    .unwrap();
    registry
        .register_plugin_server_for_page::<tauri::Wry>(
            None,
            registry.plugin_page_epoch(),
            "demo",
            "token-1".into(),
            server,
        )
        .await
        .unwrap();
    timeout(Duration::from_secs(10), async {
        while registry.list_tools_for_workspace(&[], Some("ws-a")).await.is_empty() {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("fixture tools load");

    let provider = CursorAgentProvider::new(CursorProviderConfig {
        binary: PathBuf::from(env!("CARGO_BIN_EXE_fake_cursor_acp")),
        event_channel_capacity: 1024,
        mcp_registry: Some(registry),
    });
    let mut events = provider.event_stream();
    let thread_id = "cursor-rebind";

    let mut input = fixture_start_input(thread_id);
    input.workspace_id = Some("ws-a".into());
    provider.start_session(input).await.expect("start in ws-a");
    let in_a = mcp_servers_seen(&provider, &mut events, thread_id).await;
    assert!(listed_tool_names(&in_a).await.iter().any(|name| name == "fixture__echo"));
    provider.stop_session(ThreadId(thread_id.into())).await.expect("stop");

    let mut input = fixture_start_input(thread_id);
    input.workspace_id = Some("ws-b".into());
    provider.start_session(input).await.expect("reopen in ws-b");
    let in_b = mcp_servers_seen(&provider, &mut events, thread_id).await;
    assert_ne!(in_a[0]["headers"], in_b[0]["headers"],
        "session replacement must rotate the chat token"
    );
    let names = listed_tool_names(&in_b).await;
    assert!(!names.iter().any(|name| name.starts_with("fixture__")), "{names:?}");
    provider.stop_session(ThreadId(thread_id.into())).await.expect("stop");
}
