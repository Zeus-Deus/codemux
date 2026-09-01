//! Grok ACP adapter integration coverage at the real child-process boundary.

use std::path::PathBuf;
use std::sync::Arc;

use codemux_lib::agent_provider::grok::slash_commands::GrokSlashCommandCache;
use codemux_lib::agent_provider::grok::{GrokAgentProvider, GrokProviderConfig};
use codemux_lib::agent_provider::{
    AgentProvider, CompletedItem, CostSource, ProviderError, ProviderKind, ProviderRuntimeEvent,
    SendTurnInput, StartSessionInput, ThreadId, TurnStatus,
};
use futures_util::StreamExt;
use serde_json::json;
use tokio::time::{timeout, Duration};

fn fixture_provider() -> (GrokAgentProvider, Arc<GrokSlashCommandCache>) {
    let commands = Arc::new(GrokSlashCommandCache::new());
    let provider = GrokAgentProvider::new_with_slash_command_cache(
        GrokProviderConfig {
            binary: PathBuf::from(env!("CARGO_BIN_EXE_fake_grok_acp")),
            event_channel_capacity: 1024,
        },
        Arc::clone(&commands),
    );
    (provider, commands)
}

fn fresh_start_input(thread_id: &str) -> StartSessionInput {
    StartSessionInput {
        thread_id: ThreadId(thread_id.into()),
        cwd: std::env::current_dir().expect("test cwd"),
        model: Some("grok-4.6".into()),
        // Deliberately contradictory: fresh_session is authoritative and the
        // stale native id must never cross the provider boundary as a load.
        resume_cursor: Some(json!({
            "schemaVersion": 1,
            "sessionId": "persisted-session-that-must-not-load"
        })),
        fresh_session: true,
        permission_mode: Some("agent".into()),
        effort: Some("high".into()),
        context_window: None,
        fast_mode: false,
        additional_directories: vec![],
        env: None,
        workspace_id: None,
        extra: serde_json::Value::Null,
        recorded_usage_baseline: None,
    }
}

fn fixture_turn(thread_id: &str) -> SendTurnInput {
    SendTurnInput {
        thread_id: ThreadId(thread_id.into()),
        text: "exercise the Grok process boundary".into(),
        images: vec![],
        model_override: None,
        effort_override: None,
        permission_mode_override: None,
        client_nonce: None,
        display_text: None,
        skill_invocations: vec![],
        turn_checkpoint: None,
    }
}

#[derive(Debug)]
struct ObservedTurn {
    assistant_text: Option<String>,
    usage_events: Vec<ProviderRuntimeEvent>,
    status: TurnStatus,
    turn_usage: Option<codemux_lib::agent_provider::TurnUsage>,
}

/// One test intentionally covers the complete boundary so every assertion is
/// made against one child lifecycle: exact CLI args, initialize/auth/new,
/// fresh-session behavior, structured model errors, xAI lifecycle ordering,
/// model acknowledgement, and one authoritative usage emission.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn grok_process_boundary_preserves_lifecycle_and_usage_contracts() {
    let (provider, command_cache) = fixture_provider();
    let mut events = provider.event_stream();
    let thread_id = "grok-process-boundary";
    let cwd = std::env::current_dir().expect("test cwd");

    let session = provider
        .start_session(fresh_start_input(thread_id))
        .await
        .expect("fake Grok initialize/auth/new succeeds without session/load");
    assert_eq!(session.provider, ProviderKind::Grok);
    assert_eq!(session.session_id.0, "fake-grok-session");

    let error = provider
        .set_model(ThreadId(thread_id.into()), "grok-incompatible".into())
        .await
        .expect_err("incompatible Grok family must request a fresh session");
    assert!(matches!(
        error,
        ProviderError::ValidationError { ref message }
            if message.starts_with("grok_model_restart_required:")
    ));

    provider
        .send_turn(fixture_turn(thread_id))
        .await
        .expect("send fixture turn");

    let observed = timeout(Duration::from_secs(10), async {
        let mut assistant_text = None;
        let mut usage_events = Vec::new();
        while let Some(event) = events.next().await {
            match event {
                ProviderRuntimeEvent::ItemCompleted {
                    item: CompletedItem::AssistantText { text },
                    ..
                } => assistant_text = Some(text),
                event @ ProviderRuntimeEvent::UsageRecorded { .. } => {
                    usage_events.push(event);
                }
                ProviderRuntimeEvent::TurnCompleted { status, usage, .. } => {
                    return ObservedTurn {
                        assistant_text,
                        usage_events,
                        status,
                        turn_usage: usage,
                    };
                }
                _ => {}
            }
        }
        panic!("Grok event stream closed before turn completion");
    })
    .await
    .expect("Grok fixture turn timed out");

    // The chunk is intentionally emitted after prompt_complete. Keeping it
    // proves that the early announcement did not settle the turn.
    assert_eq!(observed.assistant_text.as_deref(), Some("after-early"));
    // Durable completion advertises an error, while the standard response
    // shortly after it succeeds. Standard ACP owns the result during grace.
    assert!(matches!(observed.status, TurnStatus::Success));

    assert_eq!(
        observed.usage_events.len(),
        1,
        "early, durable, and standard usage frames must produce one ledger row"
    );
    match &observed.usage_events[0] {
        ProviderRuntimeEvent::UsageRecorded {
            provider,
            model,
            input_tokens,
            output_tokens,
            cache_read_tokens,
            cache_write_tokens,
            reasoning_tokens,
            cost_usd,
            cost_source,
            ..
        } => {
            assert_eq!(*provider, ProviderKind::Grok);
            // Comes from the live model_changed lifecycle notification, not
            // the model requested when the process was created.
            assert_eq!(model.as_deref(), Some("grok-provider-effective"));
            assert_eq!(*input_tokens, 75);
            assert_eq!(*output_tokens, 10);
            assert_eq!(*cache_read_tokens, 20);
            assert_eq!(*cache_write_tokens, 5);
            assert_eq!(*reasoning_tokens, 3);
            assert_eq!(*cost_usd, Some(0.25));
            assert_eq!(*cost_source, Some(CostSource::Provider));
        }
        other => panic!("expected UsageRecorded, got {other:?}"),
    }
    let usage = observed.turn_usage.expect("standard response turn usage");
    assert_eq!(usage.total_cost_usd, Some(0.25));
    assert_eq!(usage.duration_ms, 123);
    assert_eq!(usage.num_turns, 2);

    let commands = command_cache
        .get_or_harvest(&PathBuf::from(env!("CARGO_BIN_EXE_fake_grok_acp")), &cwd)
        .await
        .expect("running session populated the shared command cache");
    assert_eq!(
        commands
            .iter()
            .map(|command| command.name.as_str())
            .collect::<Vec<_>>(),
        vec!["live-command"],
        "the latest xAI full snapshot must replace initialize and standard snapshots"
    );
    assert_eq!(commands[0].argument_hint, "<query>");

    provider
        .stop_session(ThreadId(thread_id.into()))
        .await
        .expect("stop fixture session");
}
