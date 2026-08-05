//! Step 12 Stage 8 live smoke test — drives the OpenCode AgentProvider
//! end-to-end against a real `opencode serve` child.
//!
//! Walks through the full chat path: discover binary → spawn server →
//! start session → subscribe to events → send a turn → drain events
//! until `TurnCompleted`. Designed as the deliverable evidence that
//! Stage 8 actually works against OpenCode 1.14.x with a connected
//! upstream provider.
//!
//! Invocation:
//!
//! ```bash
//! cargo run --manifest-path src-tauri/Cargo.toml \
//!     --example opencode_chat_smoke -- openrouter/x-ai/grok-code-fast-1
//! ```
//!
//! The argument is the federated model id Codemux passes around
//! (`<sub-provider>/<model-id>`). The model must come from a
//! provider OpenCode reports as `connected` in its `/provider`
//! response — anything else fails with `ProviderAuthError` mid-turn.
//!
//! Skips politely when `opencode` isn't on PATH so the example is
//! safe to run on CI / fresh worktrees.

use std::sync::Arc;
use std::time::{Duration, Instant};

use codemux_lib::agent_provider::events::{
    CompletedItem, ContentDelta, ProviderRuntimeEvent, TurnStatus,
};
use codemux_lib::agent_provider::opencode::{
    OpenCodeAgentProvider, OpenCodeProviderConfig, OpenCodeServerManager,
};
use codemux_lib::agent_provider::provider::AgentProvider;
use codemux_lib::agent_provider::types::{SendTurnInput, StartSessionInput, ThreadId};
use futures_util::StreamExt;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    if which::which("opencode").is_err() {
        eprintln!("[chat-smoke] opencode not on PATH; skipping");
        return Ok(());
    }

    let model = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "openrouter/x-ai/grok-code-fast-1".to_string());
    eprintln!("[chat-smoke] model: {model}");

    let manager = Arc::new(OpenCodeServerManager::new());
    let provider = OpenCodeAgentProvider::new(manager, OpenCodeProviderConfig::default());

    // Subscribe to the global event stream BEFORE start_session so we
    // catch the SessionConfigured event.
    let mut events = provider.event_stream();

    let thread_id = ThreadId(format!("smoke-{}", uuid::Uuid::new_v4()));
    let cwd = std::env::current_dir()?;
    let session = provider
        .start_session(StartSessionInput {
            thread_id: thread_id.clone(),
            cwd,
            model: Some(model.clone()),
            resume_cursor: None,
            permission_mode: None,
            effort: None,
            context_window: None,
            fast_mode: false,
            additional_directories: vec![],
            env: None,
            extra: serde_json::Value::Null,
        })
        .await?;
    eprintln!(
        "[chat-smoke] session started: {} (provider_session_id: {})",
        session.thread_id.0, session.session_id.0
    );

    let turn = provider
        .send_turn(SendTurnInput {
            thread_id: thread_id.clone(),
            text: "Say only: hello".into(),
            images: vec![],
            model_override: None,
            effort_override: None,
            permission_mode_override: None,
            client_nonce: None,
            display_text: None,
            skill_invocations: vec![],
        })
        .await?;
    eprintln!("[chat-smoke] turn started: {}", turn.turn_id.0);

    let deadline = Instant::now() + Duration::from_secs(120);
    let mut text_buf = String::new();
    let mut completed = false;
    while Instant::now() < deadline && !completed {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let next = match tokio::time::timeout(remaining, events.next()).await {
            Ok(Some(event)) => event,
            Ok(None) => break,
            Err(_) => {
                eprintln!("[chat-smoke] timed out waiting for TurnCompleted");
                break;
            }
        };
        match next {
            ProviderRuntimeEvent::ContentDelta { delta, .. } => match delta {
                ContentDelta::Text { text } => {
                    text_buf.push_str(&text);
                    eprint!("{text}");
                }
                ContentDelta::Thinking { text } => {
                    eprint!("\x1b[90m{text}\x1b[0m");
                }
                ContentDelta::ToolInput { tool_name, .. } => {
                    eprint!("\n[tool-input: {tool_name}] ");
                }
            },
            ProviderRuntimeEvent::ItemCompleted { item, .. } => match item {
                CompletedItem::AssistantText { text } => {
                    eprintln!("\n[assistant text final]: {text}");
                }
                CompletedItem::ToolUse {
                    tool_name,
                    tool_use_id,
                    ..
                } => {
                    eprintln!("\n[tool use: {tool_name} ({tool_use_id})]");
                }
                CompletedItem::ToolResult {
                    tool_use_id,
                    is_error,
                    ..
                } => {
                    eprintln!("\n[tool result: {tool_use_id} (error={is_error})]");
                }
                CompletedItem::AssistantThinking { .. } => {}
            },
            ProviderRuntimeEvent::TurnCompleted { status, usage, .. } => {
                eprintln!("\n[chat-smoke] turn completed: status={status:?} usage={usage:?}");
                if !matches!(status, TurnStatus::Success) {
                    eprintln!("[chat-smoke] non-success status — exiting non-zero");
                    provider.stop_session(thread_id).await?;
                    std::process::exit(1);
                }
                completed = true;
            }
            ProviderRuntimeEvent::SessionStateChanged { status, .. } => {
                eprintln!("[chat-smoke] state change: {status:?}");
            }
            ProviderRuntimeEvent::SessionConfigured {
                provider_session_id,
                ..
            } => {
                eprintln!("[chat-smoke] session_configured: {}", provider_session_id.0);
            }
            ProviderRuntimeEvent::RequestOpened {
                request_kind,
                request_id,
                ..
            } => {
                eprintln!(
                    "[chat-smoke] request opened: kind={request_kind} id={}",
                    request_id.0
                );
            }
            ProviderRuntimeEvent::RuntimeWarning { message, .. } => {
                eprintln!("[chat-smoke] warning: {message}");
            }
            other => {
                eprintln!("[chat-smoke] event: {other:?}");
            }
        }
    }

    eprintln!("\n[chat-smoke] tearing down session");
    provider.stop_session(thread_id).await?;

    if !completed {
        eprintln!("[chat-smoke] FAIL: never observed TurnCompleted");
        std::process::exit(1);
    }
    eprintln!(
        "[chat-smoke] OK — assistant produced {} chars across content deltas",
        text_buf.len()
    );
    Ok(())
}
