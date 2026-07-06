//! End-to-end integration tests for
//! [`codemux_lib::agent_provider::claude::ClaudeAgentProvider`].
//!
//! Each test wires the provider up against the `fake_claude_sidecar`
//! binary — never the real SDK — so we never need a logged-in
//! Anthropic account to run CI. The last two tests use the REAL
//! compiled sidecar (if present) to prove the auth-probe path works
//! end-to-end; they skip with a clear message when the real binary
//! is missing.
//!
//! Unix-only: every test here drives the provider through a bash
//! wrapper script (`wrapper_with_env` + the auth_probe tests' inline
//! `#!/usr/bin/env bash` mocks), and the chmod 755 on those scripts
//! uses `std::os::unix::fs::PermissionsExt`. None of that has a
//! Windows analogue, so we gate the whole file rather than carry
//! parallel cmd.exe versions of every fixture.

#![cfg(unix)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use serde_json::json;
use tempfile::TempDir;
use tokio::time::timeout;

use codemux_lib::agent_provider::claude::{
    auth::{probe_authenticated, probe_installed, AuthStatus},
    ClaudeAgentProvider, ClaudeProviderConfig,
};
use codemux_lib::agent_provider::{
    AgentProvider, ApprovalDecision, ProviderError, ProviderRuntimeEvent, RequestId,
    SendTurnInput, SessionStatus, StartSessionInput, SubagentStatus, ThreadId, TurnId, TurnStatus,
};

fn fake_sidecar() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_fake_claude_sidecar"))
}

fn try_real_sidecar_binary() -> Option<PathBuf> {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let triple = current_target_triple();
    let ext = if triple.contains("windows") { ".exe" } else { "" };
    let candidate = manifest
        .join("binaries")
        .join(format!("codemux-claude-sidecar-{triple}{ext}"));
    if candidate.exists() && is_executable(&candidate) {
        Some(candidate)
    } else {
        None
    }
}

fn current_target_triple() -> String {
    if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        "x86_64-unknown-linux-gnu".into()
    } else if cfg!(all(target_os = "linux", target_arch = "aarch64")) {
        "aarch64-unknown-linux-gnu".into()
    } else if cfg!(all(target_os = "macos", target_arch = "x86_64")) {
        "x86_64-apple-darwin".into()
    } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        "aarch64-apple-darwin".into()
    } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        "x86_64-pc-windows-msvc".into()
    } else {
        "unknown".into()
    }
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|m| m.is_file() && (m.permissions().mode() & 0o111 != 0))
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

async fn provider_with_fake() -> ClaudeAgentProvider {
    ClaudeAgentProvider::new(ClaudeProviderConfig {
        sidecar_binary: Some(fake_sidecar()),
        claude_binary: Some(PathBuf::from("/usr/bin/claude")),
        event_channel_capacity: 1024,
        mcp_registry: None,
    })
    .await
    .expect("provider")
}

async fn provider_with_custom_sidecar(bin: PathBuf) -> ClaudeAgentProvider {
    ClaudeAgentProvider::new(ClaudeProviderConfig {
        sidecar_binary: Some(bin),
        claude_binary: Some(PathBuf::from("/usr/bin/claude")),
        event_channel_capacity: 1024,
        mcp_registry: None,
    })
    .await
    .expect("provider")
}

fn start_input(thread_id: &str) -> StartSessionInput {
    StartSessionInput {
        thread_id: ThreadId(thread_id.into()),
        cwd: std::env::temp_dir(),
        model: None,
        resume_cursor: None,
        permission_mode: None,
        effort: None,
        context_window: None,
        additional_directories: vec![],
        env: None,
        extra: serde_json::Value::Null,
    }
}

/// A (dir, path) pair to keep a scripted JSON file alive for the
/// test's duration. Using a dedicated tempdir per test dodges the
/// ETXTBSY race the Codex adapter tests hit on heavy parallel load.
struct ScriptFile {
    _dir: TempDir,
    path: PathBuf,
}

fn write_script(value: serde_json::Value) -> ScriptFile {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("script.json");
    std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    ScriptFile { _dir: dir, path }
}

/// Small bash wrapper that `exec`s the fake sidecar with extra env
/// vars set. Returns a tempdir-backed path that stays alive for the
/// caller's lifetime.
fn wrapper_with_env(env: &[(&str, &str)]) -> ScriptFile {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("wrap.sh");
    let mut body = String::from("#!/usr/bin/env bash\nset -e\n");
    for (k, v) in env {
        body.push_str(&format!("export {k}={}\n", shell_escape::escape(v)));
    }
    body.push_str(&format!(
        "exec {} \"$@\"\n",
        shell_escape::escape(fake_sidecar().to_string_lossy()),
    ));
    std::fs::write(&path, body.as_bytes()).unwrap();
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    ScriptFile { _dir: dir, path }
}

mod shell_escape {
    pub fn escape<S: AsRef<str>>(s: S) -> String {
        let s = s.as_ref();
        if s.is_empty() {
            return "''".into();
        }
        if s.chars()
            .all(|c| c.is_ascii_alphanumeric() || "._-/=".contains(c))
        {
            return s.to_string();
        }
        let mut out = String::from("'");
        for c in s.chars() {
            if c == '\'' {
                out.push_str("'\\''");
            } else {
                out.push(c);
            }
        }
        out.push('\'');
        out
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn starts_session_and_reports_ready() {
    let provider = provider_with_fake().await;
    let mut stream = provider.event_stream();
    let session = provider.start_session(start_input("t-1")).await.unwrap();
    assert!(matches!(session.status, SessionStatus::Ready));
    // SessionConfigured should fire on the stream.
    let mut saw_configured = false;
    let _ = timeout(Duration::from_secs(2), async {
        while let Some(ev) = stream.next().await {
            if matches!(ev, ProviderRuntimeEvent::SessionConfigured { .. }) {
                saw_configured = true;
                break;
            }
        }
    })
    .await;
    assert!(saw_configured);
    provider.stop_session(ThreadId("t-1".into())).await.ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn consecutive_turns_stamp_distinct_turn_ids_on_content_deltas() {
    // Regression: Claude's SDK messages carry session_id but no
    // per-turn id. The translator defaults to turn_id="" which
    // confuses frontend consumers that merge assistant deltas by
    // turn_id — the 2nd turn's deltas end up coalesced into the 1st
    // turn's assistant message. The adapter must stamp its own
    // turn_id (from `send_turn`) on outbound events so consecutive
    // turns stay distinguishable.
    let script = write_script(json!([
        {
            "after": "send-turn", "delay_ms": 5, "emit": "notification",
            "method": "sdk-message",
            "params": {
                "threadId": "t-stamp",
                "message": {
                    "type": "stream_event",
                    "event": {
                        "type": "content_block_delta",
                        "delta": {"type": "text_delta", "text": "reply one"}
                    }
                }
            }
        },
        {
            "after": "send-turn", "delay_ms": 5, "emit": "notification",
            "method": "sdk-message",
            "params": {
                "threadId": "t-stamp",
                "message": {
                    "type": "result",
                    "subtype": "success",
                    "duration_ms": 10,
                    "num_turns": 1
                }
            }
        },
        {
            "after": "send-turn", "delay_ms": 5, "emit": "notification",
            "method": "sdk-message",
            "params": {
                "threadId": "t-stamp",
                "message": {
                    "type": "stream_event",
                    "event": {
                        "type": "content_block_delta",
                        "delta": {"type": "text_delta", "text": "reply two"}
                    }
                }
            }
        },
        {
            "after": "send-turn", "delay_ms": 5, "emit": "notification",
            "method": "sdk-message",
            "params": {
                "threadId": "t-stamp",
                "message": {
                    "type": "result",
                    "subtype": "success",
                    "duration_ms": 10,
                    "num_turns": 1
                }
            }
        }
    ]));
    let wrapper = wrapper_with_env(&[(
        "FAKE_CLAUDE_SIDECAR_SCRIPT",
        &script.path.to_string_lossy(),
    )]);
    let provider = provider_with_custom_sidecar(wrapper.path.clone()).await;
    provider.start_session(start_input("t-stamp")).await.unwrap();
    let mut stream = provider.event_stream();

    // Turn 1
    let turn1 = provider
        .send_turn(SendTurnInput {
            thread_id: ThreadId("t-stamp".into()),
            text: "first".into(),
            images: vec![],
            model_override: None,
            effort_override: None,
            permission_mode_override: None,
        })
        .await
        .unwrap()
        .turn_id;
    let mut delta1_turn: Option<TurnId> = None;
    let _ = timeout(Duration::from_secs(3), async {
        while let Some(ev) = stream.next().await {
            if let ProviderRuntimeEvent::ContentDelta { turn_id, .. } = &ev {
                delta1_turn = Some(turn_id.clone());
                break;
            }
        }
    })
    .await;
    // Drain the TurnCompleted so turn 2 can start cleanly.
    let _ = timeout(Duration::from_secs(3), async {
        while let Some(ev) = stream.next().await {
            if matches!(ev, ProviderRuntimeEvent::TurnCompleted { .. }) {
                break;
            }
        }
    })
    .await;

    // Turn 2
    let turn2 = provider
        .send_turn(SendTurnInput {
            thread_id: ThreadId("t-stamp".into()),
            text: "second".into(),
            images: vec![],
            model_override: None,
            effort_override: None,
            permission_mode_override: None,
        })
        .await
        .unwrap()
        .turn_id;
    let mut delta2_turn: Option<TurnId> = None;
    let _ = timeout(Duration::from_secs(3), async {
        while let Some(ev) = stream.next().await {
            if let ProviderRuntimeEvent::ContentDelta { turn_id, .. } = &ev {
                delta2_turn = Some(turn_id.clone());
                break;
            }
        }
    })
    .await;

    assert_eq!(
        delta1_turn.as_ref(),
        Some(&turn1),
        "first turn's delta should carry the send_turn turn_id"
    );
    assert_eq!(
        delta2_turn.as_ref(),
        Some(&turn2),
        "second turn's delta should carry a fresh send_turn turn_id"
    );
    assert_ne!(delta1_turn, delta2_turn, "turn ids must differ across turns");

    provider
        .stop_session(ThreadId("t-stamp".into()))
        .await
        .ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_turn_emits_session_state_changed_running_with_matching_turn_id() {
    // Claude's SDK never emits a text delta for a pure-tool turn, so
    // the frontend's streaming flag relies on SessionStateChanged
    // firing right after send_turn succeeds. This test locks down
    // that behaviour at the adapter boundary.
    let wrapper = wrapper_with_env(&[]);
    let provider = provider_with_custom_sidecar(wrapper.path.clone()).await;
    provider
        .start_session(start_input("t-running"))
        .await
        .unwrap();
    let mut stream = provider.event_stream();
    let returned_turn = provider
        .send_turn(SendTurnInput {
            thread_id: ThreadId("t-running".into()),
            text: "hello".into(),
            images: vec![],
            model_override: None,
            effort_override: None,
            permission_mode_override: None,
        })
        .await
        .unwrap();

    let mut saw_running_with_matching_turn = false;
    let _ = timeout(Duration::from_secs(3), async {
        while let Some(ev) = stream.next().await {
            if let ProviderRuntimeEvent::SessionStateChanged {
                status: SessionStatus::Running { active_turn },
                ..
            } = &ev
            {
                assert_eq!(active_turn, &returned_turn.turn_id);
                saw_running_with_matching_turn = true;
                break;
            }
        }
    })
    .await;
    assert!(saw_running_with_matching_turn);
    provider
        .stop_session(ThreadId("t-running".into()))
        .await
        .ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sdk_session_id_notification_is_translated_into_resume_cursor_updated() {
    let script = write_script(json!([
        {
            "after": "start-session",
            "delay_ms": 5,
            "emit": "notification",
            "method": "sdk-session-id",
            "params": {
                "threadId": "t-resume",
                "sessionId": "deadbeef-1234-5678-90ab-cdef12345678"
            }
        }
    ]));
    let wrapper = wrapper_with_env(&[(
        "FAKE_CLAUDE_SIDECAR_SCRIPT",
        &script.path.to_string_lossy(),
    )]);
    let provider = provider_with_custom_sidecar(wrapper.path.clone()).await;
    let mut stream = provider.event_stream();
    provider
        .start_session(start_input("t-resume"))
        .await
        .unwrap();

    let mut saw_cursor = false;
    let _ = timeout(Duration::from_secs(3), async {
        while let Some(ev) = stream.next().await {
            if let ProviderRuntimeEvent::ResumeCursorUpdated {
                resume_cursor, ..
            } = &ev
            {
                let resume = resume_cursor.get("resume").and_then(|v| v.as_str());
                assert_eq!(resume, Some("deadbeef-1234-5678-90ab-cdef12345678"));
                saw_cursor = true;
                break;
            }
        }
    })
    .await;
    assert!(saw_cursor);
    provider
        .stop_session(ThreadId("t-resume".into()))
        .await
        .ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn interrupt_emits_session_state_changed_ready() {
    let wrapper = wrapper_with_env(&[]);
    let provider = provider_with_custom_sidecar(wrapper.path.clone()).await;
    provider
        .start_session(start_input("t-int"))
        .await
        .unwrap();
    let returned_turn = provider
        .send_turn(SendTurnInput {
            thread_id: ThreadId("t-int".into()),
            text: "work forever".into(),
            images: vec![],
            model_override: None,
            effort_override: None,
            permission_mode_override: None,
        })
        .await
        .unwrap();
    let mut stream = provider.event_stream();
    provider
        .interrupt_turn(ThreadId("t-int".into()), Some(returned_turn.turn_id.clone()))
        .await
        .unwrap();
    let mut saw_ready = false;
    let _ = timeout(Duration::from_secs(3), async {
        while let Some(ev) = stream.next().await {
            if let ProviderRuntimeEvent::SessionStateChanged {
                status: SessionStatus::Ready,
                ..
            } = &ev
            {
                saw_ready = true;
                break;
            }
        }
    })
    .await;
    assert!(saw_ready);
    provider.stop_session(ThreadId("t-int".into())).await.ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_turn_three_times_in_a_row_succeeds_without_validation_error() {
    // Regression: Claude's SDK streaming-queue mode keeps the session
    // alive across turns, so `session-ended` never fires between
    // turns. Without per-turn clearing of `state.active_turn`, the
    // second `send_turn` rejects with "session has an active turn".
    // Each scripted "result" is what the SDK emits at a turn
    // boundary; the adapter must treat it as the clear signal.
    let script = write_script(json!([
        {
            "after": "send-turn", "delay_ms": 5, "emit": "notification",
            "method": "sdk-message",
            "params": {
                "threadId": "t-3x",
                "message": {
                    "type": "result",
                    "subtype": "success",
                    "turn_id": "sdk-turn-1",
                    "duration_ms": 10,
                    "num_turns": 1
                }
            }
        },
        {
            "after": "send-turn", "delay_ms": 5, "emit": "notification",
            "method": "sdk-message",
            "params": {
                "threadId": "t-3x",
                "message": {
                    "type": "result",
                    "subtype": "success",
                    "turn_id": "sdk-turn-2",
                    "duration_ms": 10,
                    "num_turns": 1
                }
            }
        },
        {
            "after": "send-turn", "delay_ms": 5, "emit": "notification",
            "method": "sdk-message",
            "params": {
                "threadId": "t-3x",
                "message": {
                    "type": "result",
                    "subtype": "success",
                    "turn_id": "sdk-turn-3",
                    "duration_ms": 10,
                    "num_turns": 1
                }
            }
        }
    ]));
    let wrapper = wrapper_with_env(&[(
        "FAKE_CLAUDE_SIDECAR_SCRIPT",
        &script.path.to_string_lossy(),
    )]);
    let provider = provider_with_custom_sidecar(wrapper.path.clone()).await;
    provider.start_session(start_input("t-3x")).await.unwrap();
    let mut stream = provider.event_stream();

    for i in 1..=3 {
        provider
            .send_turn(SendTurnInput {
                thread_id: ThreadId("t-3x".into()),
                text: format!("turn {i}"),
                images: vec![],
                model_override: None,
                effort_override: None,
                permission_mode_override: None,
            })
            .await
            .unwrap_or_else(|e| {
                panic!("send_turn {i} failed: {e:?}");
            });
        // Wait for the scripted result to flow back and clear
        // active_turn before the next send_turn.
        let _ = timeout(Duration::from_secs(3), async {
            while let Some(ev) = stream.next().await {
                if let ProviderRuntimeEvent::TurnCompleted { .. } = ev {
                    break;
                }
            }
        })
        .await;
    }
    provider.stop_session(ThreadId("t-3x".into())).await.ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_turn_emits_content_deltas_then_item_completed_then_turn_completed() {
    let script = write_script(json!([
        {
            "after": "send-turn", "delay_ms": 5, "emit": "notification",
            "method": "sdk-message",
            "params": {
                "threadId": "t-seq",
                "message": {
                    "type": "stream_event",
                    "turn_id": "sdk-turn-1",
                    "event": {
                        "type": "content_block_delta",
                        "delta": {"type": "text_delta", "text": "Hi"}
                    }
                }
            }
        },
        {
            "after": "send-turn", "delay_ms": 15, "emit": "notification",
            "method": "sdk-message",
            "params": {
                "threadId": "t-seq",
                "message": {
                    "type": "assistant",
                    "turn_id": "sdk-turn-1",
                    "message": {"content": [{"type": "text", "text": "Hi there"}]}
                }
            }
        },
        {
            "after": "send-turn", "delay_ms": 25, "emit": "notification",
            "method": "sdk-message",
            "params": {
                "threadId": "t-seq",
                "message": {
                    "type": "result",
                    "subtype": "success",
                    "turn_id": "sdk-turn-1",
                    "duration_ms": 100,
                    "num_turns": 1
                }
            }
        }
    ]));
    let wrapper = wrapper_with_env(&[(
        "FAKE_CLAUDE_SIDECAR_SCRIPT",
        &script.path.to_string_lossy(),
    )]);
    let provider = provider_with_custom_sidecar(wrapper.path.clone()).await;
    provider.start_session(start_input("t-seq")).await.unwrap();
    let mut stream = provider.event_stream();
    provider
        .send_turn(SendTurnInput {
            thread_id: ThreadId("t-seq".into()),
            text: "hello".into(),
            images: vec![],
            model_override: None,
            effort_override: None,
            permission_mode_override: None,
        })
        .await
        .unwrap();

    let mut saw_delta = false;
    let mut saw_item = false;
    let mut saw_turn_completed = false;
    let _ = timeout(Duration::from_secs(4), async {
        while let Some(ev) = stream.next().await {
            match &ev {
                ProviderRuntimeEvent::ContentDelta { .. } => saw_delta = true,
                ProviderRuntimeEvent::ItemCompleted { .. } => saw_item = true,
                ProviderRuntimeEvent::TurnCompleted {
                    status: TurnStatus::Success,
                    ..
                } => saw_turn_completed = true,
                _ => {}
            }
            if saw_delta && saw_item && saw_turn_completed {
                break;
            }
        }
    })
    .await;
    assert!(saw_delta && saw_item && saw_turn_completed);
    provider.stop_session(ThreadId("t-seq".into())).await.ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_notification_surfaces_as_runtime_warning() {
    let script = write_script(json!([
        {
            "after": "send-turn", "delay_ms": 5, "emit": "notification",
            "method": "mystery/method",
            "params": {"interesting": "data"}
        }
    ]));
    let wrapper = wrapper_with_env(&[(
        "FAKE_CLAUDE_SIDECAR_SCRIPT",
        &script.path.to_string_lossy(),
    )]);
    let provider = provider_with_custom_sidecar(wrapper.path.clone()).await;
    provider.start_session(start_input("t-unk")).await.unwrap();
    let mut stream = provider.event_stream();
    provider
        .send_turn(SendTurnInput {
            thread_id: ThreadId("t-unk".into()),
            text: "x".into(),
            images: vec![],
            model_override: None,
            effort_override: None,
            permission_mode_override: None,
        })
        .await
        .unwrap();
    let mut saw = false;
    let _ = timeout(Duration::from_secs(3), async {
        while let Some(ev) = stream.next().await {
            if let ProviderRuntimeEvent::RuntimeWarning { message, .. } = &ev {
                if message.contains("mystery/method") {
                    saw = true;
                    break;
                }
            }
        }
    })
    .await;
    assert!(saw);
    provider.stop_session(ThreadId("t-unk".into())).await.ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_sdk_message_variant_surfaces_as_runtime_warning() {
    let script = write_script(json!([
        {
            "after": "send-turn", "delay_ms": 5, "emit": "notification",
            "method": "sdk-message",
            "params": {
                "threadId": "t-sdk-unk",
                "message": {"type": "brand_new_variant", "foo": "bar"}
            }
        }
    ]));
    let wrapper = wrapper_with_env(&[(
        "FAKE_CLAUDE_SIDECAR_SCRIPT",
        &script.path.to_string_lossy(),
    )]);
    let provider = provider_with_custom_sidecar(wrapper.path.clone()).await;
    provider.start_session(start_input("t-sdk-unk")).await.unwrap();
    let mut stream = provider.event_stream();
    provider
        .send_turn(SendTurnInput {
            thread_id: ThreadId("t-sdk-unk".into()),
            text: "x".into(),
            images: vec![],
            model_override: None,
            effort_override: None,
            permission_mode_override: None,
        })
        .await
        .unwrap();
    let mut saw = false;
    let _ = timeout(Duration::from_secs(3), async {
        while let Some(ev) = stream.next().await {
            if let ProviderRuntimeEvent::RuntimeWarning { message, .. } = &ev {
                if message.contains("brand_new_variant") {
                    saw = true;
                    break;
                }
            }
        }
    })
    .await;
    assert!(saw);
    provider.stop_session(ThreadId("t-sdk-unk".into())).await.ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn request_opened_for_command_tool_routes_to_request_opened_event() {
    let script = write_script(json!([
        {
            "after": "send-turn", "delay_ms": 5, "emit": "notification",
            "method": "request-opened",
            "params": {
                "threadId": "t-req",
                "requestId": "r-1",
                "toolName": "Bash",
                "toolInput": {"command": "ls"},
                "kind": "command"
            }
        }
    ]));
    let wrapper = wrapper_with_env(&[(
        "FAKE_CLAUDE_SIDECAR_SCRIPT",
        &script.path.to_string_lossy(),
    )]);
    let provider = provider_with_custom_sidecar(wrapper.path.clone()).await;
    provider.start_session(start_input("t-req")).await.unwrap();
    let mut stream = provider.event_stream();
    provider
        .send_turn(SendTurnInput {
            thread_id: ThreadId("t-req".into()),
            text: "please ls".into(),
            images: vec![],
            model_override: None,
            effort_override: None,
            permission_mode_override: None,
        })
        .await
        .unwrap();
    let mut seen_kind = None;
    let _ = timeout(Duration::from_secs(3), async {
        while let Some(ev) = stream.next().await {
            if let ProviderRuntimeEvent::RequestOpened { request_kind, .. } = &ev {
                seen_kind = Some(request_kind.clone());
                break;
            }
        }
    })
    .await;
    assert_eq!(seen_kind.as_deref(), Some("command"));
    provider.stop_session(ThreadId("t-req".into())).await.ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn respond_to_request_allow_forwards_to_sidecar() {
    // The fake sidecar replies OK to any requestId that isn't
    // "unknown-request". Just prove the RPC roundtrips.
    let provider = provider_with_fake().await;
    provider.start_session(start_input("t-ok")).await.unwrap();
    provider
        .respond_to_request(
            ThreadId("t-ok".into()),
            RequestId("r-123".into()),
            ApprovalDecision::Allow {
                updated_input: None,
                updated_permissions: None,
            },
        )
        .await
        .unwrap();
    provider.stop_session(ThreadId("t-ok".into())).await.ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn respond_to_request_deny_forwards_to_sidecar() {
    let provider = provider_with_fake().await;
    provider.start_session(start_input("t-deny")).await.unwrap();
    provider
        .respond_to_request(
            ThreadId("t-deny".into()),
            RequestId("r-deny".into()),
            ApprovalDecision::Deny { message: "no".into() },
        )
        .await
        .unwrap();
    provider.stop_session(ThreadId("t-deny".into())).await.ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn interrupt_turn_sends_interrupt_rpc() {
    let provider = provider_with_fake().await;
    provider.start_session(start_input("t-int")).await.unwrap();
    // Start a turn so there is something to interrupt.
    provider
        .send_turn(SendTurnInput {
            thread_id: ThreadId("t-int".into()),
            text: "go".into(),
            images: vec![],
            model_override: None,
            effort_override: None,
            permission_mode_override: None,
        })
        .await
        .unwrap();
    provider
        .interrupt_turn(ThreadId("t-int".into()), None)
        .await
        .unwrap();
    provider.stop_session(ThreadId("t-int".into())).await.ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn interrupt_turn_with_wrong_turn_id_fails_validation() {
    let provider = provider_with_fake().await;
    provider.start_session(start_input("t-mis")).await.unwrap();
    provider
        .send_turn(SendTurnInput {
            thread_id: ThreadId("t-mis".into()),
            text: "go".into(),
            images: vec![],
            model_override: None,
            effort_override: None,
            permission_mode_override: None,
        })
        .await
        .unwrap();
    let err = provider
        .interrupt_turn(ThreadId("t-mis".into()), Some(TurnId("wrong".into())))
        .await
        .unwrap_err();
    assert!(matches!(err, ProviderError::ValidationError { .. }));
    provider.stop_session(ThreadId("t-mis".into())).await.ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn set_model_forwards_to_sidecar_and_updates_session_state() {
    let provider = provider_with_fake().await;
    provider.start_session(start_input("t-mod")).await.unwrap();
    provider
        .set_model(ThreadId("t-mod".into()), "claude-opus-4-7".into())
        .await
        .unwrap();
    provider.stop_session(ThreadId("t-mod".into())).await.ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn set_permission_mode_forwards_to_sidecar() {
    let provider = provider_with_fake().await;
    provider.start_session(start_input("t-pm")).await.unwrap();
    provider
        .set_permission_mode(ThreadId("t-pm".into()), "acceptEdits".into())
        .await
        .unwrap();
    provider.stop_session(ThreadId("t-pm".into())).await.ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn set_permission_mode_plan_mode_is_accepted() {
    let provider = provider_with_fake().await;
    provider.start_session(start_input("t-plan")).await.unwrap();
    provider
        .set_permission_mode(ThreadId("t-plan".into()), "plan".into())
        .await
        .unwrap();
    provider.stop_session(ThreadId("t-plan".into())).await.ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn set_permission_mode_bypass_is_accepted() {
    // The Options wiring (`allowDangerouslySkipPermissions`) is
    // sorted at start-session time via the permission_mode field on
    // StartSessionInput. Mid-session setting via set-permission-mode
    // just forwards the mode string; the sidecar and SDK decide
    // whether the bypass requires a companion flag.
    let provider = provider_with_fake().await;
    provider.start_session(start_input("t-bypass")).await.unwrap();
    provider
        .set_permission_mode(ThreadId("t-bypass".into()), "bypassPermissions".into())
        .await
        .unwrap();
    provider.stop_session(ThreadId("t-bypass".into())).await.ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stop_session_shuts_down_sidecar_and_removes_from_list() {
    let provider = provider_with_fake().await;
    provider.start_session(start_input("t-close")).await.unwrap();
    assert_eq!(provider.list_sessions().await.unwrap().len(), 1);
    provider
        .stop_session(ThreadId("t-close".into()))
        .await
        .unwrap();
    assert_eq!(provider.list_sessions().await.unwrap().len(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stop_session_is_idempotent() {
    let provider = provider_with_fake().await;
    provider.start_session(start_input("t-idem")).await.unwrap();
    provider.stop_session(ThreadId("t-idem".into())).await.unwrap();
    let err = provider
        .stop_session(ThreadId("t-idem".into()))
        .await
        .unwrap_err();
    assert!(matches!(err, ProviderError::SessionNotFound { .. }));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_turn_on_nonexistent_thread_returns_session_not_found() {
    let provider = provider_with_fake().await;
    let err = provider
        .send_turn(SendTurnInput {
            thread_id: ThreadId("nope".into()),
            text: "hi".into(),
            images: vec![],
            model_override: None,
            effort_override: None,
            permission_mode_override: None,
        })
        .await
        .unwrap_err();
    assert!(matches!(err, ProviderError::SessionNotFound { .. }));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn duplicate_start_session_returns_validation_error() {
    let provider = provider_with_fake().await;
    provider.start_session(start_input("t-dup")).await.unwrap();
    let err = provider.start_session(start_input("t-dup")).await.unwrap_err();
    assert!(matches!(err, ProviderError::ValidationError { .. }));
    provider.stop_session(ThreadId("t-dup".into())).await.ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_send_turn_returns_validation_error() {
    let provider = provider_with_fake().await;
    provider.start_session(start_input("t-busy")).await.unwrap();
    provider
        .send_turn(SendTurnInput {
            thread_id: ThreadId("t-busy".into()),
            text: "first".into(),
            images: vec![],
            model_override: None,
            effort_override: None,
            permission_mode_override: None,
        })
        .await
        .unwrap();
    let err = provider
        .send_turn(SendTurnInput {
            thread_id: ThreadId("t-busy".into()),
            text: "second".into(),
            images: vec![],
            model_override: None,
            effort_override: None,
            permission_mode_override: None,
        })
        .await
        .unwrap_err();
    assert!(matches!(err, ProviderError::ValidationError { .. }));
    provider.stop_session(ThreadId("t-busy".into())).await.ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn multiple_concurrent_sessions_are_isolated() {
    let provider = Arc::new(provider_with_fake().await);
    let mut handles = Vec::new();
    for i in 0..3 {
        let p = Arc::clone(&provider);
        handles.push(tokio::spawn(async move {
            p.start_session(start_input(&format!("t-iso-{i}"))).await
        }));
    }
    for h in handles {
        h.await.unwrap().unwrap();
    }
    let sessions = provider.list_sessions().await.unwrap();
    assert_eq!(sessions.len(), 3);
    for i in 0..3 {
        provider
            .stop_session(ThreadId(format!("t-iso-{i}")))
            .await
            .ok();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sidecar_exit_mid_session_emits_error_state() {
    let wrapper = wrapper_with_env(&[(
        "FAKE_CLAUDE_SIDECAR_EXIT_AFTER",
        "send-turn",
    )]);
    let provider = provider_with_custom_sidecar(wrapper.path.clone()).await;
    provider.start_session(start_input("t-crash")).await.unwrap();
    let mut stream = provider.event_stream();
    let _ = provider
        .send_turn(SendTurnInput {
            thread_id: ThreadId("t-crash".into()),
            text: "x".into(),
            images: vec![],
            model_override: None,
            effort_override: None,
            permission_mode_override: None,
        })
        .await;
    let mut saw_error = false;
    let _ = timeout(Duration::from_secs(4), async {
        while let Some(ev) = stream.next().await {
            if let ProviderRuntimeEvent::SessionStateChanged { status, .. } = &ev {
                if matches!(status, SessionStatus::Error { .. }) {
                    saw_error = true;
                    break;
                }
            }
        }
    })
    .await;
    assert!(saw_error);
    let _ = provider.stop_session(ThreadId("t-crash".into())).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_ended_with_iteration_complete_emits_turn_completed_success() {
    let script = write_script(json!([
        {
            "after": "send-turn", "delay_ms": 5, "emit": "notification",
            "method": "session-ended",
            "params": {"threadId": "t-end-ok", "reason": "iteration-complete"}
        }
    ]));
    let wrapper = wrapper_with_env(&[(
        "FAKE_CLAUDE_SIDECAR_SCRIPT",
        &script.path.to_string_lossy(),
    )]);
    let provider = provider_with_custom_sidecar(wrapper.path.clone()).await;
    provider.start_session(start_input("t-end-ok")).await.unwrap();
    let mut stream = provider.event_stream();
    provider
        .send_turn(SendTurnInput {
            thread_id: ThreadId("t-end-ok".into()),
            text: "x".into(),
            images: vec![],
            model_override: None,
            effort_override: None,
            permission_mode_override: None,
        })
        .await
        .unwrap();
    let mut saw = false;
    let _ = timeout(Duration::from_secs(3), async {
        while let Some(ev) = stream.next().await {
            if matches!(
                ev,
                ProviderRuntimeEvent::TurnCompleted {
                    status: TurnStatus::Success,
                    ..
                }
            ) {
                saw = true;
                break;
            }
        }
    })
    .await;
    assert!(saw);
    provider.stop_session(ThreadId("t-end-ok".into())).await.ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_ended_with_interrupted_emits_turn_error_interrupted() {
    let script = write_script(json!([
        {
            "after": "send-turn", "delay_ms": 5, "emit": "notification",
            "method": "session-ended",
            "params": {"threadId": "t-end-int", "reason": "interrupted"}
        }
    ]));
    let wrapper = wrapper_with_env(&[(
        "FAKE_CLAUDE_SIDECAR_SCRIPT",
        &script.path.to_string_lossy(),
    )]);
    let provider = provider_with_custom_sidecar(wrapper.path.clone()).await;
    provider.start_session(start_input("t-end-int")).await.unwrap();
    let mut stream = provider.event_stream();
    provider
        .send_turn(SendTurnInput {
            thread_id: ThreadId("t-end-int".into()),
            text: "x".into(),
            images: vec![],
            model_override: None,
            effort_override: None,
            permission_mode_override: None,
        })
        .await
        .unwrap();
    let mut saw = false;
    let _ = timeout(Duration::from_secs(3), async {
        while let Some(ev) = stream.next().await {
            if let ProviderRuntimeEvent::TurnCompleted { status, .. } = &ev {
                if matches!(status, TurnStatus::Error { subtype, .. } if subtype == "interrupted") {
                    saw = true;
                    break;
                }
            }
        }
    })
    .await;
    assert!(saw);
    provider.stop_session(ThreadId("t-end-int".into())).await.ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_error_emits_session_state_changed_plus_warning() {
    let script = write_script(json!([
        {
            "after": "send-turn", "delay_ms": 5, "emit": "notification",
            "method": "session-error",
            "params": {
                "threadId": "t-err",
                "error": {"message": "boom"}
            }
        }
    ]));
    let wrapper = wrapper_with_env(&[(
        "FAKE_CLAUDE_SIDECAR_SCRIPT",
        &script.path.to_string_lossy(),
    )]);
    let provider = provider_with_custom_sidecar(wrapper.path.clone()).await;
    provider.start_session(start_input("t-err")).await.unwrap();
    let mut stream = provider.event_stream();
    provider
        .send_turn(SendTurnInput {
            thread_id: ThreadId("t-err".into()),
            text: "x".into(),
            images: vec![],
            model_override: None,
            effort_override: None,
            permission_mode_override: None,
        })
        .await
        .unwrap();
    let mut saw_state = false;
    let mut saw_warn = false;
    let _ = timeout(Duration::from_secs(3), async {
        while let Some(ev) = stream.next().await {
            match &ev {
                ProviderRuntimeEvent::SessionStateChanged {
                    status: SessionStatus::Error { .. },
                    ..
                } => saw_state = true,
                ProviderRuntimeEvent::RuntimeWarning { message, .. } => {
                    if message.contains("boom") {
                        saw_warn = true;
                    }
                }
                _ => {}
            }
            if saw_state && saw_warn {
                break;
            }
        }
    })
    .await;
    assert!(saw_state);
    assert!(saw_warn);
    provider.stop_session(ThreadId("t-err".into())).await.ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn respond_to_unknown_request_id_returns_validation_error() {
    // The fake sidecar rejects requestId=="unknown-request" with
    // -32602; the adapter maps that to ValidationError.
    let provider = provider_with_fake().await;
    provider.start_session(start_input("t-unkr")).await.unwrap();
    let err = provider
        .respond_to_request(
            ThreadId("t-unkr".into()),
            RequestId("unknown-request".into()),
            ApprovalDecision::Deny { message: "n".into() },
        )
        .await
        .unwrap_err();
    assert!(matches!(err, ProviderError::ValidationError { .. }));
    provider.stop_session(ThreadId("t-unkr".into())).await.ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dropping_provider_shuts_down_all_sessions() {
    let provider = provider_with_fake().await;
    provider.start_session(start_input("t-drop")).await.unwrap();
    assert_eq!(provider.list_sessions().await.unwrap().len(), 1);
    drop(provider);
    // Kill_on_drop reaps the subprocess even if the runtime is
    // still alive and the spawned cleanup task hasn't finished.
    // Give both paths a beat to run without asserting — the test
    // passes as long as drop does not panic.
    tokio::time::sleep(Duration::from_millis(200)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn event_ordering_across_rapid_bursts() {
    // 20 sdk-message notifications fired in quick succession; assert
    // they land in order and all arrive.
    let mut entries = Vec::new();
    for i in 0..20u64 {
        entries.push(json!({
            "after": "send-turn",
            "delay_ms": i,
            "emit": "notification",
            "method": "sdk-message",
            "params": {
                "threadId": "t-burst",
                "message": {
                    "type": "stream_event",
                    "turn_id": "sdk-burst",
                    "event": {
                        "type": "content_block_delta",
                        "delta": {"type": "text_delta", "text": format!("{i}")}
                    }
                }
            }
        }));
    }
    let script = write_script(serde_json::Value::Array(entries));
    let wrapper = wrapper_with_env(&[(
        "FAKE_CLAUDE_SIDECAR_SCRIPT",
        &script.path.to_string_lossy(),
    )]);
    let provider = provider_with_custom_sidecar(wrapper.path.clone()).await;
    provider.start_session(start_input("t-burst")).await.unwrap();
    let mut stream = provider.event_stream();
    provider
        .send_turn(SendTurnInput {
            thread_id: ThreadId("t-burst".into()),
            text: "x".into(),
            images: vec![],
            model_override: None,
            effort_override: None,
            permission_mode_override: None,
        })
        .await
        .unwrap();

    let mut texts = Vec::new();
    let _ = timeout(Duration::from_secs(5), async {
        while let Some(ev) = stream.next().await {
            if let ProviderRuntimeEvent::ContentDelta {
                delta:
                    codemux_lib::agent_provider::ContentDelta::Text { text },
                ..
            } = &ev
            {
                texts.push(text.clone());
                if texts.len() >= 20 {
                    break;
                }
            }
        }
    })
    .await;
    assert_eq!(texts.len(), 20);
    for (i, t) in texts.iter().enumerate() {
        assert_eq!(t, &format!("{i}"));
    }
    provider.stop_session(ThreadId("t-burst".into())).await.ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn subagent_lifecycle_launch_progress_completion_flows_through_adapter() {
    // Full Claude subagent lifecycle driven through the real notification
    // task (so the per-session SubagentDemux persists across messages):
    // launch (Agent tool_use) → task_progress → foreground completion via
    // tool_result + structured tool_use_result AgentOutput.
    let script = write_script(json!([
        {
            "after": "send-turn", "delay_ms": 5, "emit": "notification",
            "method": "sdk-message",
            "params": {
                "threadId": "t-sub",
                "message": {
                    "type": "assistant",
                    "parent_tool_use_id": null,
                    "turn_id": "sdk-turn-1",
                    "message": { "content": [{
                        "type": "tool_use",
                        "id": "toolu_sub",
                        "name": "Agent",
                        "input": {
                            "subagent_type": "Explore",
                            "model": "claude-sonnet-4",
                            "description": "explore",
                            "prompt": "look"
                        }
                    }]}
                }
            }
        },
        {
            "after": "send-turn", "delay_ms": 10, "emit": "notification",
            "method": "sdk-message",
            "params": {
                "threadId": "t-sub",
                "message": {
                    "type": "system",
                    "subtype": "task_progress",
                    "task_id": "task_1",
                    "tool_use_id": "toolu_sub",
                    "usage": {"total_tokens": 800, "tool_uses": 2, "duration_ms": 1500},
                    "summary": "Reading files"
                }
            }
        },
        {
            "after": "send-turn", "delay_ms": 15, "emit": "notification",
            "method": "sdk-message",
            "params": {
                "threadId": "t-sub",
                "message": {
                    "type": "user",
                    "parent_tool_use_id": null,
                    "turn_id": "sdk-turn-1",
                    "message": { "content": [{
                        "type": "tool_result",
                        "tool_use_id": "toolu_sub",
                        "content": "done",
                        "is_error": false
                    }]},
                    "tool_use_result": {
                        "status": "completed",
                        "agentId": "agent_final",
                        "agentType": "Explore",
                        "totalToolUseCount": 5,
                        "totalDurationMs": 9000,
                        "totalTokens": 4200,
                        "content": [{"type": "text", "text": "Explored the tree"}],
                        "prompt": "look"
                    }
                }
            }
        },
        {
            "after": "send-turn", "delay_ms": 20, "emit": "notification",
            "method": "sdk-message",
            "params": {
                "threadId": "t-sub",
                "message": {
                    "type": "result",
                    "subtype": "success",
                    "turn_id": "sdk-turn-1",
                    "duration_ms": 30,
                    "num_turns": 1
                }
            }
        }
    ]));
    let wrapper = wrapper_with_env(&[(
        "FAKE_CLAUDE_SIDECAR_SCRIPT",
        &script.path.to_string_lossy(),
    )]);
    let provider = provider_with_custom_sidecar(wrapper.path.clone()).await;
    provider.start_session(start_input("t-sub")).await.unwrap();
    let mut stream = provider.event_stream();
    provider
        .send_turn(SendTurnInput {
            thread_id: ThreadId("t-sub".into()),
            text: "spawn a subagent".into(),
            images: vec![],
            model_override: None,
            effort_override: None,
            permission_mode_override: None,
        })
        .await
        .unwrap();

    let mut saw_running = false;
    let mut saw_progress_tokens = false;
    let mut saw_completed = false;
    let mut leaked_generic_item = false;
    let _ = timeout(Duration::from_secs(5), async {
        while let Some(ev) = stream.next().await {
            match &ev {
                ProviderRuntimeEvent::SubagentUpdated { subagent, .. } => {
                    assert_eq!(subagent.subagent_id, "toolu_sub");
                    match subagent.status {
                        SubagentStatus::Running => {
                            saw_running = true;
                            if subagent.total_tokens == Some(800) {
                                saw_progress_tokens = true;
                            }
                        }
                        SubagentStatus::Completed => {
                            saw_completed = true;
                            assert_eq!(subagent.provider_ref.as_deref(), Some("agent_final"));
                            assert_eq!(subagent.total_tokens, Some(4200));
                            assert_eq!(subagent.tool_use_count, Some(5));
                            assert_eq!(
                                subagent.result_text.as_deref(),
                                Some("Explored the tree")
                            );
                        }
                        other => panic!("unexpected subagent status {other:?}"),
                    }
                }
                // The Agent launch tool_use and its tool_result must be
                // suppressed — never surfaced as generic items.
                ProviderRuntimeEvent::ItemCompleted {
                    subagent_id: None, ..
                } => {
                    leaked_generic_item = true;
                }
                _ => {}
            }
            if saw_completed {
                break;
            }
        }
    })
    .await;

    assert!(saw_running, "expected a Running SubagentUpdated on launch");
    assert!(saw_progress_tokens, "expected task_progress usage to flow");
    assert!(saw_completed, "expected a Completed SubagentUpdated");
    assert!(
        !leaked_generic_item,
        "Agent tool_use / tool_result must be suppressed, not surfaced as generic items"
    );
    provider.stop_session(ThreadId("t-sub".into())).await.ok();
}

// --------------------------------------------------------------------------
// Auth probes against the REAL compiled sidecar. Skip cleanly if
// the binary is missing.
// --------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auth_probe_installed_via_real_sidecar() {
    let Some(real) = try_real_sidecar_binary() else {
        eprintln!("[claude_adapter] real sidecar binary not found; skipping");
        return;
    };
    // Mock claude binary that prints a version string.
    let dir = tempfile::tempdir().unwrap();
    let claude = dir.path().join("claude");
    std::fs::write(
        &claude,
        b"#!/usr/bin/env bash\nif [ \"$1\" = \"--version\" ]; then echo 'claude 9.9.9'; exit 0; fi\nexit 1\n",
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    let mut p = std::fs::metadata(&claude).unwrap().permissions();
    p.set_mode(0o755);
    std::fs::set_permissions(&claude, p).unwrap();

    let result = probe_installed(&real, Some(&claude)).await.unwrap();
    assert!(result.installed, "expected installed=true");
    assert_eq!(result.version.as_deref(), Some("9.9.9"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auth_probe_unauthenticated_matches_common_patterns() {
    let Some(real) = try_real_sidecar_binary() else {
        eprintln!("[claude_adapter] real sidecar binary not found; skipping");
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let claude = dir.path().join("claude");
    std::fs::write(
        &claude,
        b"#!/usr/bin/env bash\nif [ \"$1 $2\" = \"auth status\" ]; then echo 'You are not logged in.'; exit 0; fi\nexit 0\n",
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    let mut p = std::fs::metadata(&claude).unwrap().permissions();
    p.set_mode(0o755);
    std::fs::set_permissions(&claude, p).unwrap();

    let status = probe_authenticated(&real, Some(&claude)).await.unwrap();
    assert!(matches!(status, AuthStatus::Unauthenticated { .. }));
}

// --------------------------------------------------------------------------
// Manual dogfood smoke test — requires real `claude` binary + logged-in
// Anthropic account. NEVER run in CI.
// --------------------------------------------------------------------------

/// Run manually with:
/// `cargo test --manifest-path src-tauri/Cargo.toml --test claude_adapter \
///   claude_real_session -- --ignored --nocapture`
///
/// The test only runs if the developer has
/// `CODEMUX_CLAUDE_SIDECAR_PATH` set (or a bundled binary) AND a
/// working `claude` CLI logged into an Anthropic account. It starts
/// a session, sends "Say hi." and asserts a content delta arrives.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires real claude CLI + Anthropic auth; run manually"]
async fn claude_real_session() {
    let Some(real) = try_real_sidecar_binary() else {
        panic!("real sidecar not built; run bash scripts/build-claude-sidecar.sh");
    };
    let provider = ClaudeAgentProvider::new(ClaudeProviderConfig {
        sidecar_binary: Some(real),
        claude_binary: None,
        event_channel_capacity: 1024,
        mcp_registry: None,
    })
    .await
    .unwrap();
    provider
        .start_session(start_input("t-dogfood"))
        .await
        .unwrap();
    let mut stream = provider.event_stream();
    provider
        .send_turn(SendTurnInput {
            thread_id: ThreadId("t-dogfood".into()),
            text: "Say hi.".into(),
            images: vec![],
            model_override: None,
            effort_override: None,
            permission_mode_override: None,
        })
        .await
        .unwrap();

    let mut saw_delta = false;
    let _ = timeout(Duration::from_secs(60), async {
        while let Some(ev) = stream.next().await {
            if matches!(
                ev,
                ProviderRuntimeEvent::ContentDelta { .. }
                    | ProviderRuntimeEvent::ItemCompleted { .. }
            ) {
                saw_delta = true;
                break;
            }
        }
    })
    .await;
    assert!(saw_delta, "no content delta within 60s");
    provider
        .stop_session(ThreadId("t-dogfood".into()))
        .await
        .ok();
}
