//! Cursor ACP adapter integration coverage.
//!
//! The live smoke test is ignored in CI because it requires an installed,
//! authenticated Cursor Agent CLI. Run it manually when changing session
//! lifecycle or model configuration behavior.

use std::path::PathBuf;

use codemux_lib::agent_provider::cursor::{CursorAgentProvider, CursorProviderConfig};
use codemux_lib::agent_provider::{
    AgentProvider, ApprovalDecision, CompletedItem, ProviderError, ProviderRuntimeEvent,
    RequestId, SendTurnInput, StartSessionInput, ThreadId, TurnStatus,
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
        fresh_session: false,
        permission_mode: Some("agent".into()),
        effort: Some("high".into()),
        context_window: None,
        fast_mode,
        additional_directories: vec![],
        env: None,
        workspace_id: None,
        extra: serde_json::Value::Null,
        recorded_usage_baseline: None,
    }
}

/// Provider wired to the `fake_cursor_acp` fixture instead of a real CLI.
fn fixture_provider() -> CursorAgentProvider {
    CursorAgentProvider::new(CursorProviderConfig {
        binary: PathBuf::from(env!("CARGO_BIN_EXE_fake_cursor_acp")),
        event_channel_capacity: 1024,
    })
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
        additional_directories: vec![],
        env: None,
        workspace_id: None,
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
