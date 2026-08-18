//! Cursor ACP adapter integration coverage.
//!
//! The live smoke test is ignored in CI because it requires an installed,
//! authenticated Cursor Agent CLI. Run it manually when changing session
//! lifecycle or model configuration behavior.

use codemux_lib::agent_provider::cursor::{CursorAgentProvider, CursorProviderConfig};
use codemux_lib::agent_provider::{
    AgentProvider, ProviderRuntimeEvent, SendTurnInput, StartSessionInput, ThreadId,
};
use futures_util::StreamExt;
use tokio::time::{timeout, Duration};

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
        permission_mode: Some("agent".into()),
        effort: Some("high".into()),
        context_window: None,
        fast_mode,
        additional_directories: vec![],
        env: None,
        extra: serde_json::Value::Null,
        recorded_usage_baseline: None,
    }
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
