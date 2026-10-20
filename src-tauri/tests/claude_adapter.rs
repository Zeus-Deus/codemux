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
    AgentProvider, ApprovalDecision, ProviderError, ProviderRuntimeEvent, RequestId, SendTurnInput,
    SessionStatus, StartSessionInput, SubagentStatus, ThreadId, TurnId, TurnStatus,
};

fn fake_sidecar() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_fake_claude_sidecar"))
}

fn try_real_sidecar_binary() -> Option<PathBuf> {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let triple = current_target_triple();
    let ext = if triple.contains("windows") {
        ".exe"
    } else {
        ""
    };
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
        fresh_session: false,
        permission_mode: None,
        effort: None,
        context_window: None,
        fast_mode: false,
        service_tier: None,
        additional_directories: vec![],
        recorded_usage_baseline: None,
        env: None,
        workspace_id: None,
        remote: None,
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
    let wrapper =
        wrapper_with_env(&[("FAKE_CLAUDE_SIDECAR_SCRIPT", &script.path.to_string_lossy())]);
    let provider = provider_with_custom_sidecar(wrapper.path.clone()).await;
    provider
        .start_session(start_input("t-stamp"))
        .await
        .unwrap();
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
            client_nonce: None,
            display_text: None,
            skill_invocations: vec![],
            turn_checkpoint: None,
            dispatch_guard: None,
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
            client_nonce: None,
            display_text: None,
            skill_invocations: vec![],
            turn_checkpoint: None,
            dispatch_guard: None,
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
    assert_ne!(
        delta1_turn, delta2_turn,
        "turn ids must differ across turns"
    );

    provider.stop_session(ThreadId("t-stamp".into())).await.ok();
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
            client_nonce: None,
            display_text: None,
            skill_invocations: vec![],
            turn_checkpoint: None,
            dispatch_guard: None,
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
    let wrapper =
        wrapper_with_env(&[("FAKE_CLAUDE_SIDECAR_SCRIPT", &script.path.to_string_lossy())]);
    let provider = provider_with_custom_sidecar(wrapper.path.clone()).await;
    let mut stream = provider.event_stream();
    provider
        .start_session(start_input("t-resume"))
        .await
        .unwrap();

    let mut saw_cursor = false;
    let _ = timeout(Duration::from_secs(3), async {
        while let Some(ev) = stream.next().await {
            if let ProviderRuntimeEvent::ResumeCursorUpdated { resume_cursor, .. } = &ev {
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
async fn resume_fallback_notification_clears_cursor_and_warns() {
    // The sidecar couldn't resume a stale session and rebuilt a fresh
    // query. The adapter must surface a null resume cursor (clear the
    // persisted id) followed by a `resume-fallback:`-prefixed warning that
    // the frontend promotes to an inline notice.
    let script = write_script(json!([
        {
            "after": "start-session",
            "delay_ms": 5,
            "emit": "notification",
            "method": "resume-fallback",
            "params": {
                "threadId": "t-fallback",
                "staleSessionId": "deadbeef-1234-5678-90ab-cdef12345678"
            }
        }
    ]));
    let wrapper =
        wrapper_with_env(&[("FAKE_CLAUDE_SIDECAR_SCRIPT", &script.path.to_string_lossy())]);
    let provider = provider_with_custom_sidecar(wrapper.path.clone()).await;
    let mut stream = provider.event_stream();
    provider
        .start_session(start_input("t-fallback"))
        .await
        .unwrap();

    let mut saw_null_cursor = false;
    let mut saw_warning = false;
    let _ = timeout(Duration::from_secs(3), async {
        while let Some(ev) = stream.next().await {
            match &ev {
                ProviderRuntimeEvent::ResumeCursorUpdated { resume_cursor, .. }
                    if resume_cursor.is_null() =>
                {
                    saw_null_cursor = true;
                }
                ProviderRuntimeEvent::RuntimeWarning { message, .. }
                    if message.starts_with("resume-fallback: ") =>
                {
                    saw_warning = true;
                }
                _ => {}
            }
            if saw_null_cursor && saw_warning {
                break;
            }
        }
    })
    .await;
    assert!(saw_null_cursor, "expected a null ResumeCursorUpdated");
    assert!(saw_warning, "expected a resume-fallback: RuntimeWarning");
    provider
        .stop_session(ThreadId("t-fallback".into()))
        .await
        .ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn interrupt_emits_session_state_changed_ready() {
    let wrapper = wrapper_with_env(&[]);
    let provider = provider_with_custom_sidecar(wrapper.path.clone()).await;
    provider.start_session(start_input("t-int")).await.unwrap();
    let returned_turn = provider
        .send_turn(SendTurnInput {
            thread_id: ThreadId("t-int".into()),
            text: "work forever".into(),
            images: vec![],
            model_override: None,
            effort_override: None,
            permission_mode_override: None,
            client_nonce: None,
            display_text: None,
            skill_invocations: vec![],
            turn_checkpoint: None,
            dispatch_guard: None,
        })
        .await
        .unwrap();
    let mut stream = provider.event_stream();
    provider
        .interrupt_turn(
            ThreadId("t-int".into()),
            Some(returned_turn.turn_id.clone()),
        )
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
    let wrapper =
        wrapper_with_env(&[("FAKE_CLAUDE_SIDECAR_SCRIPT", &script.path.to_string_lossy())]);
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
                client_nonce: None,
                display_text: None,
                skill_invocations: vec![],
                turn_checkpoint: None,
                dispatch_guard: None,
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
    let wrapper =
        wrapper_with_env(&[("FAKE_CLAUDE_SIDECAR_SCRIPT", &script.path.to_string_lossy())]);
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
            client_nonce: None,
            display_text: None,
            skill_invocations: vec![],
            turn_checkpoint: None,
            dispatch_guard: None,
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
    let wrapper =
        wrapper_with_env(&[("FAKE_CLAUDE_SIDECAR_SCRIPT", &script.path.to_string_lossy())]);
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
            client_nonce: None,
            display_text: None,
            skill_invocations: vec![],
            turn_checkpoint: None,
            dispatch_guard: None,
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
    let wrapper =
        wrapper_with_env(&[("FAKE_CLAUDE_SIDECAR_SCRIPT", &script.path.to_string_lossy())]);
    let provider = provider_with_custom_sidecar(wrapper.path.clone()).await;
    provider
        .start_session(start_input("t-sdk-unk"))
        .await
        .unwrap();
    let mut stream = provider.event_stream();
    provider
        .send_turn(SendTurnInput {
            thread_id: ThreadId("t-sdk-unk".into()),
            text: "x".into(),
            images: vec![],
            model_override: None,
            effort_override: None,
            permission_mode_override: None,
            client_nonce: None,
            display_text: None,
            skill_invocations: vec![],
            turn_checkpoint: None,
            dispatch_guard: None,
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
    provider
        .stop_session(ThreadId("t-sdk-unk".into()))
        .await
        .ok();
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
    let wrapper =
        wrapper_with_env(&[("FAKE_CLAUDE_SIDECAR_SCRIPT", &script.path.to_string_lossy())]);
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
            client_nonce: None,
            display_text: None,
            skill_invocations: vec![],
            turn_checkpoint: None,
            dispatch_guard: None,
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
            ApprovalDecision::Deny {
                message: "no".into(),
            },
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
            client_nonce: None,
            display_text: None,
            skill_invocations: vec![],
            turn_checkpoint: None,
            dispatch_guard: None,
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
            client_nonce: None,
            display_text: None,
            skill_invocations: vec![],
            turn_checkpoint: None,
            dispatch_guard: None,
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
    provider
        .start_session(start_input("t-bypass"))
        .await
        .unwrap();
    provider
        .set_permission_mode(ThreadId("t-bypass".into()), "bypassPermissions".into())
        .await
        .unwrap();
    provider
        .stop_session(ThreadId("t-bypass".into()))
        .await
        .ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stop_session_shuts_down_sidecar_and_removes_from_list() {
    let provider = provider_with_fake().await;
    provider
        .start_session(start_input("t-close"))
        .await
        .unwrap();
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
    provider
        .stop_session(ThreadId("t-idem".into()))
        .await
        .unwrap();
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
            client_nonce: None,
            display_text: None,
            skill_invocations: vec![],
            turn_checkpoint: None,
            dispatch_guard: None,
        })
        .await
        .unwrap_err();
    assert!(matches!(err, ProviderError::SessionNotFound { .. }));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn duplicate_start_session_returns_validation_error() {
    let provider = provider_with_fake().await;
    provider.start_session(start_input("t-dup")).await.unwrap();
    let err = provider
        .start_session(start_input("t-dup"))
        .await
        .unwrap_err();
    assert!(matches!(err, ProviderError::ValidationError { .. }));
    provider.stop_session(ThreadId("t-dup".into())).await.ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_send_turn_queues_instead_of_erroring() {
    // Follow-up queueing: a second send while the first turn is active is
    // now QUEUED, not rejected. The fake sidecar never scripts a
    // `result`, so the first turn stays active and the second parks.
    let provider = provider_with_fake().await;
    provider.start_session(start_input("t-busy")).await.unwrap();
    let mut stream = provider.event_stream();
    let first = provider
        .send_turn(SendTurnInput {
            thread_id: ThreadId("t-busy".into()),
            text: "first".into(),
            images: vec![],
            model_override: None,
            effort_override: None,
            permission_mode_override: None,
            client_nonce: None,
            display_text: None,
            skill_invocations: vec![],
            turn_checkpoint: None,
            dispatch_guard: None,
        })
        .await
        .unwrap();
    assert!(first.queued_id.is_none(), "first send starts immediately");

    let second = provider
        .send_turn(SendTurnInput {
            thread_id: ThreadId("t-busy".into()),
            text: "second".into(),
            images: vec![],
            model_override: None,
            effort_override: None,
            permission_mode_override: None,
            client_nonce: Some("nonce-2".into()),
            display_text: None,
            skill_invocations: vec![],
            turn_checkpoint: None,
            dispatch_guard: None,
        })
        .await
        .unwrap();
    let queued_id = second
        .queued_id
        .expect("second send should be queued behind the active turn");

    let ev = timeout(Duration::from_secs(3), async {
        while let Some(ev) = stream.next().await {
            if let ProviderRuntimeEvent::TurnQueued { .. } = ev {
                return Some(ev);
            }
        }
        None
    })
    .await
    .ok()
    .flatten()
    .expect("expected a TurnQueued event");
    match ev {
        ProviderRuntimeEvent::TurnQueued {
            queued_id: qid,
            client_nonce,
            text,
            ..
        } => {
            assert_eq!(qid, queued_id);
            assert_eq!(client_nonce.as_deref(), Some("nonce-2"));
            assert_eq!(text, "second");
        }
        other => panic!("unexpected event: {other:?}"),
    }
    provider.stop_session(ThreadId("t-busy".into())).await.ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn queued_turns_dispatch_fifo_on_completion() {
    // Two follow-ups queued behind an active turn dispatch in FIFO order
    // as each prior turn completes. The fake sidecar emits one scripted
    // `result` per send-turn RPC — the turn-boundary clear signal that
    // triggers the drain.
    let script = write_script(json!([
        { "after": "send-turn", "delay_ms": 5, "emit": "notification",
          "method": "sdk-message",
          "params": { "threadId": "t-fifo", "message": {
            "type": "result", "subtype": "success", "turn_id": "r1",
            "duration_ms": 1, "num_turns": 1 } } },
        { "after": "send-turn", "delay_ms": 5, "emit": "notification",
          "method": "sdk-message",
          "params": { "threadId": "t-fifo", "message": {
            "type": "result", "subtype": "success", "turn_id": "r2",
            "duration_ms": 1, "num_turns": 1 } } },
        { "after": "send-turn", "delay_ms": 5, "emit": "notification",
          "method": "sdk-message",
          "params": { "threadId": "t-fifo", "message": {
            "type": "result", "subtype": "success", "turn_id": "r3",
            "duration_ms": 1, "num_turns": 1 } } }
    ]));
    let wrapper =
        wrapper_with_env(&[("FAKE_CLAUDE_SIDECAR_SCRIPT", &script.path.to_string_lossy())]);
    let provider = provider_with_custom_sidecar(wrapper.path.clone()).await;
    provider.start_session(start_input("t-fifo")).await.unwrap();
    let mut stream = provider.event_stream();

    for (text, nonce) in [("A", "n-a"), ("B", "n-b"), ("C", "n-c")] {
        provider
            .send_turn(SendTurnInput {
                thread_id: ThreadId("t-fifo".into()),
                text: text.into(),
                images: vec![],
                model_override: None,
                effort_override: None,
                permission_mode_override: None,
                client_nonce: Some(nonce.into()),
                display_text: None,
                skill_invocations: vec![],
                turn_checkpoint: None,
                dispatch_guard: None,
            })
            .await
            .unwrap();
    }

    // The two follow-ups (B, C) dispatch in FIFO order.
    let dispatched = timeout(Duration::from_secs(5), async {
        let mut texts = Vec::new();
        while let Some(ev) = stream.next().await {
            if let ProviderRuntimeEvent::QueuedTurnDispatched { text, .. } = ev {
                texts.push(text);
                if texts.len() >= 2 {
                    break;
                }
            }
        }
        texts
    })
    .await
    .unwrap_or_default();
    assert_eq!(
        dispatched,
        vec!["B".to_string(), "C".to_string()],
        "queued follow-ups B, C dispatch in FIFO order"
    );
    provider.stop_session(ThreadId("t-fifo".into())).await.ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stop_holds_queued_turns_until_sent_now() {
    // Pressing Stop must not immediately send the next queued message: the
    // user may have stopped because the run was going wrong. The queue stays
    // put until the user sends it explicitly.
    let provider = provider_with_fake().await;
    let thread = ThreadId("t-stop-queue".into());
    provider.start_session(start_input(&thread.0)).await.unwrap();
    let mut stream = provider.event_stream();
    let input = |text: &str| SendTurnInput {
        thread_id: thread.clone(),
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
    };
    provider.send_turn(input("first")).await.unwrap();
    let queued = provider
        .send_turn(input("second"))
        .await
        .unwrap()
        .queued_id
        .expect("second send queued");

    provider.interrupt_turn(thread.clone(), None).await.unwrap();
    async fn next_dispatch(
        stream: &mut (impl futures_util::Stream<Item = ProviderRuntimeEvent> + Unpin),
    ) -> Option<String> {
        while let Some(ev) = stream.next().await {
            if let ProviderRuntimeEvent::QueuedTurnDispatched { queued_id, .. } = ev {
                return Some(queued_id);
            }
        }
        None
    }
    assert!(
        timeout(Duration::from_millis(500), next_dispatch(&mut stream))
            .await
            .is_err(),
        "Stop must leave the queued message queued"
    );

    provider
        .send_queued_turn_now(thread.clone(), queued.clone())
        .await
        .unwrap();
    let dispatched = timeout(Duration::from_secs(3), next_dispatch(&mut stream))
        .await
        .ok()
        .flatten();
    assert_eq!(dispatched.as_deref(), Some(queued.as_str()));
    provider.stop_session(thread).await.ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancel_queued_turn_removes_it() {
    let provider = provider_with_fake().await;
    provider
        .start_session(start_input("t-cancel"))
        .await
        .unwrap();
    let mut stream = provider.event_stream();
    provider
        .send_turn(SendTurnInput {
            thread_id: ThreadId("t-cancel".into()),
            text: "first".into(),
            images: vec![],
            model_override: None,
            effort_override: None,
            permission_mode_override: None,
            client_nonce: None,
            display_text: None,
            skill_invocations: vec![],
            turn_checkpoint: None,
            dispatch_guard: None,
        })
        .await
        .unwrap();
    let queued = provider
        .send_turn(SendTurnInput {
            thread_id: ThreadId("t-cancel".into()),
            text: "second".into(),
            images: vec![],
            model_override: None,
            effort_override: None,
            permission_mode_override: None,
            client_nonce: None,
            display_text: None,
            skill_invocations: vec![],
            turn_checkpoint: None,
            dispatch_guard: None,
        })
        .await
        .unwrap()
        .queued_id
        .expect("second send queued");

    let removed = provider
        .cancel_queued_turn(ThreadId("t-cancel".into()), queued.clone())
        .await
        .unwrap();
    assert!(removed, "the queued turn should be reported as removed");

    let cancelled = timeout(Duration::from_secs(3), async {
        while let Some(ev) = stream.next().await {
            if let ProviderRuntimeEvent::QueuedTurnCancelled { queued_id, .. } = ev {
                return Some(queued_id);
            }
        }
        None
    })
    .await
    .ok()
    .flatten();
    assert_eq!(cancelled.as_deref(), Some(queued.as_str()));

    // Cancelling again is a silent no-op (idempotent).
    let removed_again = provider
        .cancel_queued_turn(ThreadId("t-cancel".into()), queued)
        .await
        .unwrap();
    assert!(
        !removed_again,
        "an already-cancelled id should report false"
    );
    provider
        .stop_session(ThreadId("t-cancel".into()))
        .await
        .ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn closing_session_cancels_queued_turns() {
    let provider = provider_with_fake().await;
    provider
        .start_session(start_input("t-close"))
        .await
        .unwrap();
    let mut stream = provider.event_stream();
    provider
        .send_turn(SendTurnInput {
            thread_id: ThreadId("t-close".into()),
            text: "first".into(),
            images: vec![],
            model_override: None,
            effort_override: None,
            permission_mode_override: None,
            client_nonce: None,
            display_text: None,
            skill_invocations: vec![],
            turn_checkpoint: None,
            dispatch_guard: None,
        })
        .await
        .unwrap();
    let queued = provider
        .send_turn(SendTurnInput {
            thread_id: ThreadId("t-close".into()),
            text: "second".into(),
            images: vec![],
            model_override: None,
            effort_override: None,
            permission_mode_override: None,
            client_nonce: None,
            display_text: None,
            skill_invocations: vec![],
            turn_checkpoint: None,
            dispatch_guard: None,
        })
        .await
        .unwrap()
        .queued_id
        .expect("second send queued");

    provider.stop_session(ThreadId("t-close".into())).await.ok();

    let cancelled = timeout(Duration::from_secs(3), async {
        while let Some(ev) = stream.next().await {
            if let ProviderRuntimeEvent::QueuedTurnCancelled { queued_id, .. } = ev {
                return Some(queued_id);
            }
        }
        None
    })
    .await
    .ok()
    .flatten();
    assert_eq!(cancelled.as_deref(), Some(queued.as_str()));
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
    let wrapper = wrapper_with_env(&[("FAKE_CLAUDE_SIDECAR_EXIT_AFTER", "send-turn")]);
    let provider = provider_with_custom_sidecar(wrapper.path.clone()).await;
    provider
        .start_session(start_input("t-crash"))
        .await
        .unwrap();
    let mut stream = provider.event_stream();
    // send-turn is answered, then the fixture exits. The Ok is guaranteed
    // by JsonRpcChild's exit-drain: the fixture writes the response BEFORE
    // exiting, so it must be routed (not failed as "child process exited")
    // even when the watchdog observes the exit first — which is also what
    // deterministically arms `active_turn` for the synthetic completion
    // asserted below.
    provider
        .send_turn(SendTurnInput {
            thread_id: ThreadId("t-crash".into()),
            text: "x".into(),
            images: vec![],
            model_override: None,
            effort_override: None,
            permission_mode_override: None,
            client_nonce: None,
            display_text: None,
            skill_invocations: vec![],
            turn_checkpoint: None,
            dispatch_guard: None,
        })
        .await
        .expect("send_turn must succeed: the response is written before the exit");
    // Issue #154: a mid-turn child death must settle the in-flight turn with
    // a synthetic `TurnCompleted { child_exited }` BEFORE the terminal
    // `SessionStateChanged::Error`, so the turn settles through the whole
    // pipeline and persists a durable record for hydrate.
    let mut saw_child_exit_turn = false;
    let mut saw_error = false;
    let _ = timeout(Duration::from_secs(4), async {
        while let Some(ev) = stream.next().await {
            match &ev {
                ProviderRuntimeEvent::TurnCompleted { status, .. } => {
                    if let TurnStatus::Error { subtype, .. } = status {
                        if subtype == "child_exited" {
                            saw_child_exit_turn = true;
                        }
                    }
                }
                ProviderRuntimeEvent::SessionStateChanged { status, .. } => {
                    if matches!(status, SessionStatus::Error { .. }) {
                        saw_error = true;
                        break;
                    }
                }
                _ => {}
            }
        }
    })
    .await;
    assert!(saw_error, "expected a terminal SessionStateChanged::Error");
    assert!(
        saw_child_exit_turn,
        "expected a synthetic child_exited TurnCompleted before the Error"
    );
    let _ = provider.stop_session(ThreadId("t-crash".into())).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dead_sidecar_is_evicted_and_start_session_rebuilds() {
    // Issue #154 live-recovery path: after the child-exit watchdog marks a
    // session dead, `has_session` must report it absent AND a fresh
    // `start_session` for the SAME thread must evict the corpse and rebuild
    // instead of failing with "claude session already exists" — otherwise
    // every send after a mid-run death fails permanently.
    let wrapper = wrapper_with_env(&[("FAKE_CLAUDE_SIDECAR_EXIT_AFTER", "send-turn")]);
    let provider = provider_with_custom_sidecar(wrapper.path.clone()).await;
    let thread = ThreadId("t-evict".into());
    provider
        .start_session(start_input("t-evict"))
        .await
        .unwrap();
    assert!(
        provider.has_session(&thread).await,
        "session live after start"
    );
    // Deterministically Ok — the fixture answers send-turn before exiting
    // and JsonRpcChild's exit-drain routes that response (see
    // `sidecar_exit_mid_session_emits_error_state`).
    provider
        .send_turn(SendTurnInput {
            thread_id: thread.clone(),
            text: "x".into(),
            images: vec![],
            model_override: None,
            effort_override: None,
            permission_mode_override: None,
            client_nonce: None,
            display_text: None,
            skill_invocations: vec![],
            turn_checkpoint: None,
            dispatch_guard: None,
        })
        .await
        .expect("send_turn must succeed: the response is written before the exit");
    // Wait for the watchdog (100ms poll) to observe the dead child.
    let went_absent = timeout(Duration::from_secs(4), async {
        loop {
            if !provider.has_session(&thread).await {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    assert!(went_absent.is_ok(), "dead session must be reported absent");
    // The rebuild must succeed (evict-and-replace), not error on "already exists".
    provider
        .start_session(start_input("t-evict"))
        .await
        .expect("rebuild after death must succeed");
    assert!(provider.has_session(&thread).await, "rebuilt session live");
    let _ = provider.stop_session(thread).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn turn_active_false_for_unknown_thread() {
    // The frontend hydrate path treats an unbound thread as "not in flight".
    let provider = provider_with_fake().await;
    assert!(
        !provider.turn_active(&ThreadId("t-unknown".into())).await,
        "no session bound => turn_active must be false"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn turn_active_true_in_flight_then_false_after_settle() {
    // A `result` at the turn boundary clears `active_turn` while the
    // streaming-queue session stays alive, so turn_active flips true->false
    // without the session disappearing.
    let script = write_script(json!([
        {
            "after": "send-turn", "delay_ms": 300, "emit": "notification",
            "method": "sdk-message",
            "params": {
                "threadId": "t-ta",
                "message": {
                    "type": "result", "subtype": "success",
                    "turn_id": "sdk-turn-1", "duration_ms": 10, "num_turns": 1
                }
            }
        }
    ]));
    let wrapper =
        wrapper_with_env(&[("FAKE_CLAUDE_SIDECAR_SCRIPT", &script.path.to_string_lossy())]);
    let provider = provider_with_custom_sidecar(wrapper.path.clone()).await;
    provider.start_session(start_input("t-ta")).await.unwrap();
    assert!(
        !provider.turn_active(&ThreadId("t-ta".into())).await,
        "no turn yet => false"
    );
    provider
        .send_turn(SendTurnInput {
            thread_id: ThreadId("t-ta".into()),
            text: "hi".into(),
            images: vec![],
            model_override: None,
            effort_override: None,
            permission_mode_override: None,
            client_nonce: None,
            display_text: None,
            skill_invocations: vec![],
            turn_checkpoint: None,
            dispatch_guard: None,
        })
        .await
        .unwrap();
    // `active_turn` is armed synchronously before send_turn returns.
    assert!(
        provider.turn_active(&ThreadId("t-ta".into())).await,
        "turn in flight => true"
    );
    // The scripted result settles the turn ~300ms later.
    let settled = timeout(Duration::from_secs(3), async {
        loop {
            if !provider.turn_active(&ThreadId("t-ta".into())).await {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    assert!(
        settled.is_ok(),
        "turn_active must return to false after settle"
    );
    assert!(
        provider.has_session(&ThreadId("t-ta".into())).await,
        "session stays live across the turn boundary"
    );
    provider.stop_session(ThreadId("t-ta".into())).await.ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sdk_initiated_question_keeps_turn_active_and_queues_follow_ups() {
    // The SDK can start a turn on its own after a `result` (background
    // subagents finishing wake the main agent) and park it on an
    // AskUserQuestion callback. `active_turn` is `None` then, but the thread
    // is not idle: hydrate must keep the question answerable, and a new
    // message must queue behind it instead of being fed to an SDK that cannot
    // read input until the callback resolves (the send would hang forever).
    let script = write_script(json!([
        {
            "after": "send-turn", "delay_ms": 50, "emit": "notification",
            "method": "sdk-message",
            "params": {
                "threadId": "t-auto-q",
                "message": {
                    "type": "result", "subtype": "success",
                    "duration_ms": 10, "num_turns": 1
                }
            }
        },
        {
            "after": "send-turn", "delay_ms": 150, "emit": "notification",
            "method": "request-opened",
            "params": {
                "threadId": "t-auto-q",
                "requestId": "r-auto",
                "toolName": "AskUserQuestion",
                "toolInput": {"questions": []},
                "kind": "user-input"
            }
        },
        {
            "after": "respond-to-request", "delay_ms": 50, "emit": "notification",
            "method": "sdk-message",
            "params": {
                "threadId": "t-auto-q",
                "message": {
                    "type": "result", "subtype": "success",
                    "duration_ms": 10, "num_turns": 1
                }
            }
        }
    ]));
    let wrapper =
        wrapper_with_env(&[("FAKE_CLAUDE_SIDECAR_SCRIPT", &script.path.to_string_lossy())]);
    let provider = provider_with_custom_sidecar(wrapper.path.clone()).await;
    let thread = ThreadId("t-auto-q".into());
    provider
        .start_session(start_input("t-auto-q"))
        .await
        .unwrap();
    let mut stream = provider.event_stream();
    let send = |text: &str| SendTurnInput {
        thread_id: thread.clone(),
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
    };
    provider.send_turn(send("first")).await.unwrap();

    let opened = timeout(Duration::from_secs(3), async {
        while let Some(ev) = stream.next().await {
            if matches!(ev, ProviderRuntimeEvent::RequestOpened { .. }) {
                break;
            }
        }
    })
    .await;
    assert!(opened.is_ok(), "SDK-initiated question must open");
    assert!(
        provider.turn_active(&thread).await,
        "a pending question after `result` keeps the turn active"
    );

    let follow_up = provider.send_turn(send("repeat it")).await.unwrap();
    assert!(
        follow_up.queued_id.is_some(),
        "a message sent while the SDK waits on a question must queue"
    );

    provider
        .respond_to_request(
            thread.clone(),
            RequestId("r-auto".into()),
            ApprovalDecision::Allow {
                updated_input: None,
                updated_permissions: None,
            },
        )
        .await
        .unwrap();
    let dispatched = timeout(Duration::from_secs(3), async {
        while let Some(ev) = stream.next().await {
            if let ProviderRuntimeEvent::QueuedTurnDispatched { text, .. } = ev {
                return text;
            }
        }
        String::new()
    })
    .await;
    assert_eq!(
        dispatched.as_deref().ok(),
        Some("repeat it"),
        "the queued message dispatches once the question turn ends"
    );
    provider.stop_session(thread).await.ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn turn_active_false_when_session_dead() {
    // Mid-turn child death: the watchdog flips `dead`, and turn_active's
    // is_dead() short-circuit reports false for the corpse even though a turn
    // was in flight when the child exited — so hydrate treats a died-mid-turn
    // run as "not in flight" (it gets the interrupted marker via other means).
    let wrapper = wrapper_with_env(&[("FAKE_CLAUDE_SIDECAR_EXIT_AFTER", "send-turn")]);
    let provider = provider_with_custom_sidecar(wrapper.path.clone()).await;
    let thread = ThreadId("t-ta-dead".into());
    provider
        .start_session(start_input("t-ta-dead"))
        .await
        .unwrap();
    provider
        .send_turn(SendTurnInput {
            thread_id: thread.clone(),
            text: "x".into(),
            images: vec![],
            model_override: None,
            effort_override: None,
            permission_mode_override: None,
            client_nonce: None,
            display_text: None,
            skill_invocations: vec![],
            turn_checkpoint: None,
            dispatch_guard: None,
        })
        .await
        .expect("send_turn must succeed: the response is written before the exit");
    // Wait for the watchdog (100ms poll) to observe the dead child.
    let went_absent = timeout(Duration::from_secs(4), async {
        loop {
            if !provider.has_session(&thread).await {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    assert!(went_absent.is_ok(), "watchdog must mark the session dead");
    assert!(
        !provider.turn_active(&thread).await,
        "a dead session must report turn_active false"
    );
    provider.stop_session(thread).await.ok();
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
    let wrapper =
        wrapper_with_env(&[("FAKE_CLAUDE_SIDECAR_SCRIPT", &script.path.to_string_lossy())]);
    let provider = provider_with_custom_sidecar(wrapper.path.clone()).await;
    provider
        .start_session(start_input("t-end-ok"))
        .await
        .unwrap();
    let mut stream = provider.event_stream();
    provider
        .send_turn(SendTurnInput {
            thread_id: ThreadId("t-end-ok".into()),
            text: "x".into(),
            images: vec![],
            model_override: None,
            effort_override: None,
            permission_mode_override: None,
            client_nonce: None,
            display_text: None,
            skill_invocations: vec![],
            turn_checkpoint: None,
            dispatch_guard: None,
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
    provider
        .stop_session(ThreadId("t-end-ok".into()))
        .await
        .ok();
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
    let wrapper =
        wrapper_with_env(&[("FAKE_CLAUDE_SIDECAR_SCRIPT", &script.path.to_string_lossy())]);
    let provider = provider_with_custom_sidecar(wrapper.path.clone()).await;
    provider
        .start_session(start_input("t-end-int"))
        .await
        .unwrap();
    let mut stream = provider.event_stream();
    provider
        .send_turn(SendTurnInput {
            thread_id: ThreadId("t-end-int".into()),
            text: "x".into(),
            images: vec![],
            model_override: None,
            effort_override: None,
            permission_mode_override: None,
            client_nonce: None,
            display_text: None,
            skill_invocations: vec![],
            turn_checkpoint: None,
            dispatch_guard: None,
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
    provider
        .stop_session(ThreadId("t-end-int".into()))
        .await
        .ok();
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
    let wrapper =
        wrapper_with_env(&[("FAKE_CLAUDE_SIDECAR_SCRIPT", &script.path.to_string_lossy())]);
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
            client_nonce: None,
            display_text: None,
            skill_invocations: vec![],
            turn_checkpoint: None,
            dispatch_guard: None,
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
async fn respond_to_unknown_request_id_returns_request_not_pending() {
    // The fake sidecar rejects requestId=="unknown-request" with -32602
    // ("not found or already resolved"); the adapter classifies that as
    // RequestNotPending so the command layer can expire the control
    // instead of surfacing a generic validation failure.
    let provider = provider_with_fake().await;
    provider.start_session(start_input("t-unkr")).await.unwrap();
    let err = provider
        .respond_to_request(
            ThreadId("t-unkr".into()),
            RequestId("unknown-request".into()),
            ApprovalDecision::Deny {
                message: "n".into(),
            },
        )
        .await
        .unwrap_err();
    assert!(
        matches!(err, ProviderError::RequestNotPending { .. }),
        "expected RequestNotPending, got {err:?}"
    );
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
    let wrapper =
        wrapper_with_env(&[("FAKE_CLAUDE_SIDECAR_SCRIPT", &script.path.to_string_lossy())]);
    let provider = provider_with_custom_sidecar(wrapper.path.clone()).await;
    provider
        .start_session(start_input("t-burst"))
        .await
        .unwrap();
    let mut stream = provider.event_stream();
    provider
        .send_turn(SendTurnInput {
            thread_id: ThreadId("t-burst".into()),
            text: "x".into(),
            images: vec![],
            model_override: None,
            effort_override: None,
            permission_mode_override: None,
            client_nonce: None,
            display_text: None,
            skill_invocations: vec![],
            turn_checkpoint: None,
            dispatch_guard: None,
        })
        .await
        .unwrap();

    let mut texts = Vec::new();
    let _ = timeout(Duration::from_secs(5), async {
        while let Some(ev) = stream.next().await {
            if let ProviderRuntimeEvent::ContentDelta {
                delta: codemux_lib::agent_provider::ContentDelta::Text { text },
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
    let wrapper =
        wrapper_with_env(&[("FAKE_CLAUDE_SIDECAR_SCRIPT", &script.path.to_string_lossy())]);
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
            client_nonce: None,
            display_text: None,
            skill_invocations: vec![],
            turn_checkpoint: None,
            dispatch_guard: None,
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
                            assert_eq!(subagent.result_text.as_deref(), Some("Explored the tree"));
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
            client_nonce: None,
            display_text: None,
            skill_invocations: vec![],
            turn_checkpoint: None,
            dispatch_guard: None,
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

// ===========================================================================
// Plugin-requested compaction (`host.chat.requestCompaction`) and the
// status query behind `host.chat.getStatus`
// ===========================================================================

/// A session whose Claude settings come only from tempdirs, so the
/// developer's own `~/.claude` opt-outs never change the outcome.
struct CompactionHome {
    _home: TempDir,
    project: TempDir,
    env: std::collections::HashMap<String, String>,
}

fn compaction_home(extra_env: &[(&str, &str)]) -> CompactionHome {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let mut env = std::collections::HashMap::new();
    env.insert("HOME".to_string(), home.path().to_string_lossy().to_string());
    env.insert(
        "CLAUDE_CONFIG_DIR".to_string(),
        home.path().join(".claude").to_string_lossy().to_string(),
    );
    for (key, value) in extra_env {
        env.insert(key.to_string(), value.to_string());
    }
    CompactionHome {
        _home: home,
        project,
        env,
    }
}

fn compaction_start(thread: &str, home: &CompactionHome) -> StartSessionInput {
    let mut input = start_input(thread);
    input.cwd = home.project.path().to_path_buf();
    input.env = Some(home.env.clone());
    input
}

fn compaction_sdk(thread: &str, delay: u64, after: &str, message: serde_json::Value) -> serde_json::Value {
    json!({"after": after, "delay_ms": delay, "emit": "notification",
        "method": "sdk-message", "params": {"threadId": thread, "message": message}})
}

/// A user turn that completes, then a scripted `/compact` answering
/// `compact-idle` (held open by `compact_ms`) that also emits an assistant
/// message, which must stay out of the transcript.
fn compaction_script(thread: &str, compact_ms: u64) -> ScriptFile {
    write_script(json!([
        compaction_sdk(thread, 5, "send-turn",
            json!({"type": "result", "subtype": "success", "num_turns": 1})),
        compaction_sdk(thread, 5, "compact-idle",
            json!({"type": "system", "subtype": "status", "status": "compacting"})),
        compaction_sdk(thread, compact_ms, "compact-idle",
            json!({"type": "system", "subtype": "compact_boundary",
            "compact_metadata": {"trigger": "manual", "pre_tokens": 249003, "post_tokens": 6000}})),
        compaction_sdk(thread, 5, "compact-idle",
            json!({"type": "assistant", "parent_tool_use_id": null, "message": {"role": "assistant",
            "content": [{"type": "text", "text": "Conversation compacted"}]}})),
        compaction_sdk(thread, 5, "compact-idle",
            json!({"type": "system", "subtype": "status", "status": null, "compact_result": "success"})),
        compaction_sdk(thread, 5, "compact-idle",
            json!({"type": "result", "subtype": "success", "num_turns": 0})),
    ]))
}

async fn wait_turn_completed(stream: &mut codemux_lib::agent_provider::ProviderEventStream) {
    timeout(Duration::from_secs(10), async {
        while let Some(event) = stream.next().await {
            if matches!(event, ProviderRuntimeEvent::TurnCompleted { .. }) {
                return;
            }
        }
        panic!("stream ended");
    })
    .await
    .expect("user turn completed");
}

fn is_compaction_fact(event: &ProviderRuntimeEvent) -> bool {
    matches!(
        event,
        ProviderRuntimeEvent::ContextCompactionChanged { .. }
            | ProviderRuntimeEvent::ContextUsageUpdated { .. }
            | ProviderRuntimeEvent::ResumeCursorUpdated { .. }
            | ProviderRuntimeEvent::UsageRecorded { .. }
            | ProviderRuntimeEvent::PlanUsageUpdated { .. }
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn requested_compaction_hides_its_output_adds_a_notice_and_runs_queued_sends_after() {
    let thread = "t-compact";
    let home = compaction_home(&[]);
    let script = compaction_script(thread, 400);
    let capture_dir = tempfile::tempdir().unwrap();
    let capture = capture_dir.path().join("rpc.jsonl");
    let wrapper = wrapper_with_env(&[
        ("FAKE_CLAUDE_SIDECAR_SCRIPT", &script.path.to_string_lossy()),
        ("FAKE_CLAUDE_SIDECAR_CAPTURE", &capture.to_string_lossy()),
    ]);
    let provider = provider_with_custom_sidecar(wrapper.path.clone()).await;
    let id = ThreadId(thread.into());
    provider.start_session(compaction_start(thread, &home)).await.unwrap();
    let mut stream = provider.event_stream();
    provider.send_turn(turn_input(thread, "big task")).await.unwrap();
    wait_turn_completed(&mut stream).await;

    let idle = provider.runtime_status(&id).await.expect("live session");
    assert!(!idle.busy && !idle.compacting);
    assert_eq!(idle.queued, Some(0));
    assert_eq!(idle.status, "ready");

    provider.request_compaction(&id).await.expect("compaction starts");
    let running = provider.runtime_status(&id).await.unwrap();
    assert!(running.busy && running.compacting, "{running:?}");
    // A second request while one runs is refused.
    assert_eq!(provider.request_compaction(&id).await, Err("busy"));

    let sent = provider
        .send_turn(turn_input(thread, "follow-up"))
        .await
        .unwrap();
    assert!(sent.queued_id.is_some(), "a send during compaction queues: {sent:?}");
    assert_eq!(provider.runtime_status(&id).await.unwrap().queued, Some(1));

    let mut before_notice = Vec::new();
    let mut order = Vec::new();
    timeout(Duration::from_secs(10), async {
        while let Some(event) = stream.next().await {
            match &event {
                ProviderRuntimeEvent::RuntimeWarning { message, .. }
                    if message.starts_with("idle-compaction: ") =>
                {
                    assert_eq!(message, "idle-compaction: Compacted while idle");
                    order.push("notice");
                }
                ProviderRuntimeEvent::QueuedTurnDispatched { .. } => {
                    order.push("dispatched");
                    return;
                }
                _ if order.is_empty() => before_notice.push(event),
                _ => {}
            }
        }
        panic!("stream ended");
    })
    .await
    .expect("queued follow-up dispatched");
    assert_eq!(order, ["notice", "dispatched"]);
    // No Working state, no reply bubble, no turn end: only the queue
    // event of the user's send and the compaction's own facts.
    for event in &before_notice {
        assert!(
            is_compaction_fact(event) || matches!(event, ProviderRuntimeEvent::TurnQueued { .. }),
            "compaction output leaked: {event:?}"
        );
    }
    assert!(before_notice.iter().any(|e| matches!(
        e,
        ProviderRuntimeEvent::ContextCompactionChanged { active: true, .. }
    )));
    let rpcs = std::fs::read_to_string(&capture).unwrap();
    assert_eq!(rpcs.matches("\"compact-idle\"").count(), 1);
    provider.stop_session(id).await.ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn requested_compaction_is_refused_while_a_turn_runs() {
    let thread = "t-compact-busy";
    let home = compaction_home(&[]);
    // No `result`: the user's turn stays in flight.
    let script = write_script(json!([]));
    let capture_dir = tempfile::tempdir().unwrap();
    let capture = capture_dir.path().join("rpc.jsonl");
    let wrapper = wrapper_with_env(&[
        ("FAKE_CLAUDE_SIDECAR_SCRIPT", &script.path.to_string_lossy()),
        ("FAKE_CLAUDE_SIDECAR_CAPTURE", &capture.to_string_lossy()),
    ]);
    let provider = provider_with_custom_sidecar(wrapper.path.clone()).await;
    let id = ThreadId(thread.into());
    provider.start_session(compaction_start(thread, &home)).await.unwrap();
    provider.send_turn(turn_input(thread, "long task")).await.unwrap();
    let queued = provider.send_turn(turn_input(thread, "next")).await.unwrap();
    assert!(queued.queued_id.is_some());

    let status = provider.runtime_status(&id).await.unwrap();
    assert!(status.busy && !status.compacting);
    assert_eq!(status.status, "running");
    assert_eq!(status.queued, Some(1));
    assert_eq!(provider.request_compaction(&id).await, Err("busy"));
    let rpcs = std::fs::read_to_string(&capture).unwrap_or_default();
    assert!(!rpcs.contains("\"compact-idle\""));
    provider.stop_session(id).await.ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn requested_compaction_honours_claude_code_opt_outs() {
    // `DISABLE_COMPACT` in the session's environment.
    let thread = "t-compact-env";
    let home = compaction_home(&[("DISABLE_COMPACT", "1")]);
    let script = compaction_script(thread, 5);
    let wrapper =
        wrapper_with_env(&[("FAKE_CLAUDE_SIDECAR_SCRIPT", &script.path.to_string_lossy())]);
    let provider = provider_with_custom_sidecar(wrapper.path.clone()).await;
    provider.start_session(compaction_start(thread, &home)).await.unwrap();
    let id = ThreadId(thread.into());
    assert_eq!(provider.request_compaction(&id).await, Err("compaction_off"));
    assert!(!provider.runtime_status(&id).await.unwrap().compacting);
    provider.stop_session(id).await.ok();

    // `"idleCompaction": false` in the project's Claude settings.
    let thread = "t-compact-settings";
    let home = compaction_home(&[]);
    std::fs::create_dir_all(home.project.path().join(".claude")).unwrap();
    std::fs::write(
        home.project.path().join(".claude").join("settings.local.json"),
        r#"{"idleCompaction": false}"#,
    )
    .unwrap();
    let script = compaction_script(thread, 5);
    let wrapper =
        wrapper_with_env(&[("FAKE_CLAUDE_SIDECAR_SCRIPT", &script.path.to_string_lossy())]);
    let provider = provider_with_custom_sidecar(wrapper.path.clone()).await;
    provider.start_session(compaction_start(thread, &home)).await.unwrap();
    let id = ThreadId(thread.into());
    assert_eq!(provider.request_compaction(&id).await, Err("idle_compaction_off"));
    provider.stop_session(id).await.ok();

    // `"autoCompactEnabled": false` in the user's Claude settings.
    let thread = "t-compact-user";
    let home = compaction_home(&[]);
    let config = std::path::PathBuf::from(&home.env["CLAUDE_CONFIG_DIR"]);
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(config.join("settings.json"), r#"{"autoCompactEnabled": false}"#).unwrap();
    let script = compaction_script(thread, 5);
    let wrapper =
        wrapper_with_env(&[("FAKE_CLAUDE_SIDECAR_SCRIPT", &script.path.to_string_lossy())]);
    let provider = provider_with_custom_sidecar(wrapper.path.clone()).await;
    provider.start_session(compaction_start(thread, &home)).await.unwrap();
    let id = ThreadId(thread.into());
    assert_eq!(provider.request_compaction(&id).await, Err("compaction_off"));
    provider.stop_session(id).await.ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn status_and_compaction_for_a_thread_with_no_session() {
    let provider = provider_with_custom_sidecar(fake_sidecar()).await;
    let id = ThreadId("t-none".into());
    assert!(provider.runtime_status(&id).await.is_none());
    assert_eq!(provider.request_compaction(&id).await, Err("no_session"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_requested_compaction_stays_out_of_the_transcript() {
    let thread = "t-compact-fail";
    let home = compaction_home(&[]);
    // As observed live: a `/compact` whose request failed comes back as an
    // assistant message carrying the CLI's error text, then a result.
    let script = write_script(json!([
        compaction_sdk(thread, 5, "send-turn",
            json!({"type": "result", "subtype": "success", "num_turns": 1})),
        compaction_sdk(thread, 5, "compact-idle",
            json!({"type": "system", "subtype": "session_state_changed", "state": "running"})),
        compaction_sdk(thread, 5, "compact-idle",
            json!({"type": "assistant", "parent_tool_use_id": null, "message": {"role": "assistant",
            "content": [{"type": "text", "text": "Error during compaction: Failed to authenticate"}]}})),
        compaction_sdk(thread, 5, "compact-idle",
            json!({"type": "result", "subtype": "success", "num_turns": 0})),
    ]));
    let wrapper =
        wrapper_with_env(&[("FAKE_CLAUDE_SIDECAR_SCRIPT", &script.path.to_string_lossy())]);
    let provider = provider_with_custom_sidecar(wrapper.path.clone()).await;
    let id = ThreadId(thread.into());
    provider.start_session(compaction_start(thread, &home)).await.unwrap();
    let mut stream = provider.event_stream();
    provider.send_turn(turn_input(thread, "big task")).await.unwrap();
    wait_turn_completed(&mut stream).await;
    provider.request_compaction(&id).await.expect("compaction starts");

    let mut after = Vec::new();
    let _ = timeout(Duration::from_secs(2), async {
        while let Some(event) = stream.next().await {
            after.push(event);
        }
    })
    .await;
    for event in &after {
        assert!(is_compaction_fact(event), "a failed compaction leaked {event:?}");
    }
    assert!(after.iter().any(|e| matches!(
        e,
        ProviderRuntimeEvent::ContextCompactionChanged { active: false, .. }
    )));
    let status = provider.runtime_status(&id).await.unwrap();
    assert!(!status.busy && !status.compacting, "{status:?}");
    provider.stop_session(id).await.ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_timed_out_compaction_is_stopped_before_queued_sends_run() {
    let thread = "t-compact-timeout";
    let home = compaction_home(&[]);
    // The compaction never ends on its own. When the app interrupts it, a
    // late compaction `result` arrives before the confirmed interrupt; both
    // must stay hidden and must not end the follow-up turn.
    let script = write_script(json!([
        compaction_sdk(thread, 5, "send-turn",
            json!({"type": "result", "subtype": "success", "num_turns": 1})),
        compaction_sdk(thread, 5, "compact-idle",
            json!({"type": "system", "subtype": "status", "status": "compacting"})),
        compaction_sdk(thread, 5, "interrupt",
            json!({"type": "assistant", "parent_tool_use_id": null, "message": {"role": "assistant",
            "content": [{"type": "text", "text": "Late compaction output"}]}})),
        compaction_sdk(thread, 5, "interrupt",
            json!({"type": "result", "subtype": "error_during_execution", "num_turns": 0})),
        json!({"after": "interrupt", "delay_ms": 5, "emit": "notification",
            "method": "turn-interrupted", "params": {"threadId": thread}}),
    ]));
    let capture_dir = tempfile::tempdir().unwrap();
    let capture = capture_dir.path().join("rpc.jsonl");
    let wrapper = wrapper_with_env(&[
        ("FAKE_CLAUDE_SIDECAR_SCRIPT", &script.path.to_string_lossy()),
        ("FAKE_CLAUDE_SIDECAR_CAPTURE", &capture.to_string_lossy()),
    ]);
    let provider = provider_with_custom_sidecar(wrapper.path.clone()).await;
    let id = ThreadId(thread.into());
    provider.start_session(compaction_start(thread, &home)).await.unwrap();
    assert!(provider.set_compaction_timeout_for_tests(&id, Duration::from_millis(300)).await);
    let mut stream = provider.event_stream();
    provider.send_turn(turn_input(thread, "big task")).await.unwrap();
    wait_turn_completed(&mut stream).await;
    provider.request_compaction(&id).await.expect("compaction starts");
    let sent = provider.send_turn(turn_input(thread, "follow-up")).await.unwrap();
    assert!(sent.queued_id.is_some());

    let mut before = Vec::new();
    timeout(Duration::from_secs(10), async {
        while let Some(event) = stream.next().await {
            // The follow-up's own start (`Running`, then the dispatch
            // event) ends what the compaction could have produced.
            if matches!(
                event,
                ProviderRuntimeEvent::QueuedTurnDispatched { .. }
                    | ProviderRuntimeEvent::SessionStateChanged {
                        status: SessionStatus::Running { .. },
                        ..
                    }
            ) {
                return;
            }
            before.push(event);
        }
        panic!("stream ended");
    })
    .await
    .expect("queued follow-up dispatched after the stop");
    for event in &before {
        assert!(
            is_compaction_fact(event) || matches!(event, ProviderRuntimeEvent::TurnQueued { .. }),
            "a stopped compaction leaked {event:?}"
        );
    }
    assert!(matches!(
        before.iter().rev().find(|e| matches!(e, ProviderRuntimeEvent::ContextCompactionChanged { .. })),
        Some(ProviderRuntimeEvent::ContextCompactionChanged { active: false, .. })
    ));
    let rpcs = std::fs::read_to_string(&capture).unwrap();
    assert_eq!(rpcs.matches("\"interrupt\"").count(), 1);
    // The follow-up runs as a normal turn and ends with its own result.
    wait_turn_completed(&mut stream).await;
    let status = provider.runtime_status(&id).await.unwrap();
    assert!(!status.busy && !status.compacting, "{status:?}");
    provider.stop_session(id).await.ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_compaction_that_ends_before_its_ack_leaves_no_compacting_marker() {
    let thread = "t-compact-fast";
    let home = compaction_home(&[]);
    let script = compaction_script(thread, 5);
    let wrapper = wrapper_with_env(&[
        ("FAKE_CLAUDE_SIDECAR_SCRIPT", &script.path.to_string_lossy()),
        ("FAKE_CLAUDE_COMPACT_ACK_DELAY_MS", "300"),
    ]);
    let provider = provider_with_custom_sidecar(wrapper.path.clone()).await;
    let id = ThreadId(thread.into());
    provider.start_session(compaction_start(thread, &home)).await.unwrap();
    let mut stream = provider.event_stream();
    provider.send_turn(turn_input(thread, "big task")).await.unwrap();
    wait_turn_completed(&mut stream).await;
    provider.request_compaction(&id).await.expect("compaction starts");

    let mut markers = Vec::new();
    let mut notices = 0;
    let _ = timeout(Duration::from_millis(800), async {
        while let Some(event) = stream.next().await {
            match event {
                ProviderRuntimeEvent::ContextCompactionChanged { active, .. } => markers.push(active),
                ProviderRuntimeEvent::RuntimeWarning { message, .. }
                    if message.starts_with("idle-compaction: ") => notices += 1,
                _ => {}
            }
        }
    })
    .await;
    assert_eq!(markers.first(), Some(&true), "{markers:?}");
    assert_eq!(markers.last(), Some(&false), "{markers:?}");
    assert_eq!(notices, 1);
    assert!(!provider.runtime_status(&id).await.unwrap().compacting);
    provider.stop_session(id).await.ok();
}

/// A compaction claimed while the user's `result` is being processed must
/// not take that result as its own. Races a request against every result.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_compaction_claimed_during_a_user_result_never_swallows_it() {
    const ROUNDS: usize = 20;
    let thread = "t-compact-race";
    let home = compaction_home(&[]);
    let script = compaction_script(thread, 1);
    let wrapper =
        wrapper_with_env(&[("FAKE_CLAUDE_SIDECAR_SCRIPT", &script.path.to_string_lossy())]);
    let provider = Arc::new(provider_with_custom_sidecar(wrapper.path.clone()).await);
    let id = ThreadId(thread.into());
    provider.start_session(compaction_start(thread, &home)).await.unwrap();
    let mut stream = provider.event_stream();
    for round in 0..ROUNDS {
        provider.send_turn(turn_input(thread, "task")).await.unwrap();
        // Ask as fast as possible, so the claim lands right as the
        // session turns idle.
        let racer = {
            let provider = Arc::clone(&provider);
            let id = id.clone();
            tokio::spawn(async move {
                loop {
                    match provider.request_compaction(&id).await {
                        Ok(()) => return,
                        Err("busy") => tokio::task::yield_now().await,
                        Err(other) => panic!("unexpected refusal {other}"),
                    }
                }
            })
        };
        let mut completed = 0;
        timeout(Duration::from_secs(10), async {
            while let Some(event) = stream.next().await {
                let text = format!("{event:?}");
                assert!(
                    !text.contains("Conversation compacted"),
                    "round {round}: compaction output leaked: {text}"
                );
                match event {
                    ProviderRuntimeEvent::TurnCompleted { .. } => completed += 1,
                    ProviderRuntimeEvent::RuntimeWarning { message, .. }
                        if message.starts_with("idle-compaction: ") =>
                    {
                        return
                    }
                    _ => {}
                }
            }
            panic!("stream ended");
        })
        .await
        .unwrap_or_else(|_| panic!("round {round}: compaction notice"));
        racer.await.unwrap();
        assert_eq!(completed, 1, "round {round}: the user's turn must end exactly once");
        // Let the notification task release the claim before the next send.
        timeout(Duration::from_secs(5), async {
            while provider.runtime_status(&id).await.unwrap().compacting {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .unwrap();
    }
    provider.stop_session(id).await.ok();
}

/// Drives a compaction into its timeout with `script_after_interrupt`
/// answering the app's interrupt, sends a follow-up meanwhile, and returns
/// every event seen until the session is torn down.
async fn unconfirmed_stop_events(
    thread: &str,
    extra_env: &[(&str, &str)],
    script_after_interrupt: Vec<serde_json::Value>,
) -> (Vec<ProviderRuntimeEvent>, ClaudeAgentProvider, Option<String>) {
    let home = compaction_home(&[]);
    let mut script = vec![
        compaction_sdk(thread, 5, "send-turn",
            json!({"type": "result", "subtype": "success", "num_turns": 1})),
        compaction_sdk(thread, 5, "compact-idle",
            json!({"type": "system", "subtype": "status", "status": "compacting"})),
    ];
    script.extend(script_after_interrupt);
    let script = write_script(json!(script));
    let mut env = vec![("FAKE_CLAUDE_SIDECAR_SCRIPT".to_string(), script.path.to_string_lossy().to_string())];
    env.extend(extra_env.iter().map(|(k, v)| (k.to_string(), v.to_string())));
    let env_refs: Vec<(&str, &str)> = env.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let wrapper = wrapper_with_env(&env_refs);
    let provider = provider_with_custom_sidecar(wrapper.path.clone()).await;
    let id = ThreadId(thread.into());
    provider.start_session(compaction_start(thread, &home)).await.unwrap();
    assert!(provider.set_compaction_timeout_for_tests(&id, Duration::from_millis(200)).await);
    let mut stream = provider.event_stream();
    provider.send_turn(turn_input(thread, "big task")).await.unwrap();
    wait_turn_completed(&mut stream).await;
    provider.request_compaction(&id).await.expect("compaction starts");
    let sent = provider.send_turn(turn_input(thread, "follow-up")).await.unwrap();
    let mut events = Vec::new();
    let _ = timeout(Duration::from_secs(8), async {
        while let Some(event) = stream.next().await {
            let gone = matches!(
                event,
                ProviderRuntimeEvent::SessionStateChanged { status: SessionStatus::Error { .. }, .. }
            );
            events.push(event);
            if gone {
                // Let anything still draining arrive.
                tokio::time::sleep(Duration::from_millis(300)).await;
                while let Some(Some(event)) =
                    tokio::time::timeout(Duration::from_millis(50), stream.next()).await.ok()
                {
                    events.push(event);
                }
                return;
            }
        }
    })
    .await;
    // Keep the script alive until the run is over.
    drop(script);
    (events, provider, sent.queued_id)
}

fn assert_no_turn_ran_after_compaction(events: &[ProviderRuntimeEvent]) {
    for event in events {
        let text = format!("{event:?}");
        assert!(!text.contains("Late compaction output"), "compaction output leaked: {text}");
        assert!(
            !matches!(
                event,
                ProviderRuntimeEvent::QueuedTurnDispatched { .. }
                    | ProviderRuntimeEvent::SessionStateChanged { status: SessionStatus::Running { .. }, .. }
            ),
            "a follow-up ran on a query that never confirmed its stop: {event:?}"
        );
    }
    assert!(events.iter().any(|e| matches!(e, ProviderRuntimeEvent::QueuedTurnCancelled { .. })));
    assert!(events.iter().any(|e| matches!(
        e,
        ProviderRuntimeEvent::SessionStateChanged { status: SessionStatus::Error { .. }, .. }
    )));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_compaction_result_does_not_release_an_interrupt_with_an_unknown_outcome() {
    let thread = "t-compact-unknown-stop";
    // The interrupt RPC fails, the compaction's own result arrives, and the
    // sidecar's late `turn-interrupted` follows.
    let (events, provider, queued) = unconfirmed_stop_events(
        thread,
        &[("FAKE_CLAUDE_INTERRUPT_ERROR", "1")],
        vec![
            compaction_sdk(thread, 5, "interrupt",
                json!({"type": "result", "subtype": "error_during_execution", "num_turns": 0})),
            json!({"after": "interrupt", "delay_ms": 400, "emit": "notification",
                "method": "turn-interrupted", "params": {"threadId": thread}}),
        ],
    )
    .await;
    assert!(queued.is_some());
    assert_no_turn_ran_after_compaction(&events);
    // The session is gone; the app's next send rebuilds it.
    assert!(!provider.has_session(&ThreadId(thread.into())).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_compaction_torn_down_keeps_its_output_hidden_and_its_queue_closed() {
    let thread = "t-compact-teardown";
    let marker_dir = tempfile::tempdir().unwrap();
    let marker = marker_dir.path().join("emitted");
    let marker_env = marker.to_string_lossy().to_string();
    // The interrupt is acknowledged but never confirmed, and the run keeps
    // talking after the app has started tearing its sidecar down (the
    // fixture stays alive past stdin EOF to write it).
    let (events, provider, queued) = unconfirmed_stop_events(
        thread,
        &[
            ("FAKE_CLAUDE_LINGER_AFTER_EOF_MS", "400"),
            ("FAKE_CLAUDE_EMITTED_MARKER", &marker_env),
        ],
        vec![
            compaction_sdk(thread, 2150, "interrupt",
                json!({"type": "assistant", "parent_tool_use_id": null, "message": {"role": "assistant",
                "content": [{"type": "text", "text": "Late compaction output"}]}})),
        ],
    )
    .await;
    let emitted = std::fs::read_to_string(&marker).unwrap_or_default();
    assert!(
        emitted.contains("Late compaction output"),
        "the fixture must really write the late output during teardown"
    );
    assert!(queued.is_some());
    assert_no_turn_ran_after_compaction(&events);
    let id = ThreadId(thread.into());
    assert!(!provider.has_session(&id).await);
    assert_eq!(provider.request_compaction(&id).await, Err("no_session"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_torn_down_compaction_reports_compacting_until_its_child_is_reaped() {
    // A fork reads `compacting` to wait out a compaction. Teardown marks the
    // session dead before its child exits; the run may still write history
    // until then, so the status must not vanish early.
    let thread = "t-compact-draining";
    let home = compaction_home(&[]);
    let script = write_script(json!([
        compaction_sdk(thread, 5, "send-turn",
            json!({"type": "result", "subtype": "success", "num_turns": 1})),
        compaction_sdk(thread, 5, "compact-idle",
            json!({"type": "system", "subtype": "status", "status": "compacting"})),
    ]));
    let script_env = script.path.to_string_lossy().to_string();
    let wrapper = wrapper_with_env(&[
        ("FAKE_CLAUDE_SIDECAR_SCRIPT", &script_env),
        ("FAKE_CLAUDE_LINGER_AFTER_EOF_MS", "1000"),
    ]);
    let provider = provider_with_custom_sidecar(wrapper.path.clone()).await;
    let id = ThreadId(thread.into());
    provider.start_session(compaction_start(thread, &home)).await.unwrap();
    assert!(provider.set_compaction_timeout_for_tests(&id, Duration::from_millis(200)).await);
    let mut stream = provider.event_stream();
    provider.send_turn(turn_input(thread, "big task")).await.unwrap();
    wait_turn_completed(&mut stream).await;
    provider.request_compaction(&id).await.expect("compaction starts");

    let mut draining_seen = false;
    let reaped = timeout(Duration::from_secs(10), async {
        loop {
            let dead = !provider.has_session(&id).await;
            match provider.runtime_status(&id).await {
                Some(status) if dead => {
                    assert!(status.compacting, "{status:?}");
                    draining_seen = true;
                }
                None if dead => return,
                _ => {}
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    assert!(reaped.is_ok(), "the status never cleared after teardown");
    assert!(draining_seen, "the status vanished before the child was reaped");
    drop(script);
}

// ===========================================================================
// Steering
// ===========================================================================

fn turn_input(thread: &str, text: &str) -> SendTurnInput {
    SendTurnInput {
        thread_id: ThreadId(thread.into()),
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

/// A scripted top-level `result` echoing `uuids` as the consumed prompts.
fn result_after(after: &str, delay_ms: u64, thread: &str, uuids: serde_json::Value) -> serde_json::Value {
    json!({ "after": after, "delay_ms": delay_ms, "emit": "notification",
        "method": "sdk-message",
        "params": { "threadId": thread, "message": {
            "type": "result", "subtype": "success", "duration_ms": 1, "num_turns": 1,
            "user_message_uuids": uuids } } })
}

/// The first turn's assistant message, echoing its prompt the way the CLI
/// does. Steering waits until it has seen such an echo.
fn echo_after_send(thread: &str) -> serde_json::Value {
    json!({ "after": "send-turn", "delay_ms": 0, "emit": "notification",
        "method": "sdk-message",
        "params": { "threadId": thread, "message": {
            "type": "assistant", "user_message_uuids": ["first-prompt"],
            "message": { "role": "assistant", "content": [{ "type": "text", "text": "working" }] } } } })
}

struct SteerFixture {
    provider: ClaudeAgentProvider,
    thread: ThreadId,
    first: codemux_lib::agent_provider::TurnStartResult,
    _script: ScriptFile,
    _wrapper: ScriptFile,
}

/// A session with a first turn running whose prompt echo has arrived.
async fn steer_fixture(
    thread: &str,
    entries: Vec<serde_json::Value>,
    env: &[(&str, &str)],
) -> SteerFixture {
    let mut script_entries = vec![echo_after_send(thread)];
    script_entries.extend(entries);
    let script = write_script(json!(script_entries));
    let script_path = script.path.to_string_lossy().into_owned();
    let mut vars = vec![("FAKE_CLAUDE_SIDECAR_SCRIPT", script_path.as_str())];
    vars.extend_from_slice(env);
    let wrapper = wrapper_with_env(&vars);
    let provider = provider_with_custom_sidecar(wrapper.path.clone()).await;
    provider.start_session(start_input(thread)).await.unwrap();
    let mut stream = provider.event_stream();
    let first = provider.send_turn(turn_input(thread, "first")).await.unwrap();
    timeout(Duration::from_secs(3), async {
        while let Some(ev) = stream.next().await {
            if !matches!(ev, ProviderRuntimeEvent::SessionStateChanged { .. }) {
                break;
            }
        }
    })
    .await
    .expect("echo observed");
    SteerFixture {
        provider,
        thread: ThreadId(thread.into()),
        first,
        _script: script,
        _wrapper: wrapper,
    }
}

/// Lifecycle events in arrival order until `done` matches or `ms` elapse.
async fn lifecycle_until(
    stream: &mut (impl futures_util::Stream<Item = ProviderRuntimeEvent> + Unpin),
    ms: u64,
    done: impl Fn(&ProviderRuntimeEvent) -> bool,
) -> Vec<ProviderRuntimeEvent> {
    let mut seen = Vec::new();
    let _ = timeout(Duration::from_millis(ms), async {
        while let Some(ev) = stream.next().await {
            let keep = matches!(
                ev,
                ProviderRuntimeEvent::TurnCompleted { .. }
                    | ProviderRuntimeEvent::SessionStateChanged { .. }
                    | ProviderRuntimeEvent::QueuedTurnDispatched { .. }
            );
            let finished = done(&ev);
            if keep {
                seen.push(ev);
            }
            if finished {
                break;
            }
        }
    })
    .await;
    seen
}

fn running_turns(events: &[ProviderRuntimeEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|ev| match ev {
            ProviderRuntimeEvent::SessionStateChanged {
                status: SessionStatus::Running { active_turn },
                ..
            } => Some(active_turn.0.clone()),
            _ => None,
        })
        .collect()
}

fn is_dispatch(ev: &ProviderRuntimeEvent) -> bool {
    matches!(ev, ProviderRuntimeEvent::QueuedTurnDispatched { .. })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn steer_lands_as_guidance_in_the_running_turn() {
    let thread = "t-steer-guidance";
    let capture = tempfile::NamedTempFile::new().unwrap();
    let capture_path = capture.path().to_string_lossy().into_owned();
    let f = steer_fixture(
        thread,
        vec![result_after("steer-turn", 20, thread, json!("$steered"))],
        &[("FAKE_CLAUDE_SIDECAR_CAPTURE", &capture_path)],
    )
    .await;
    let mut stream = f.provider.event_stream();

    let steered = f.provider.steer_turn(turn_input(thread, "also this")).await.unwrap();
    assert!(steered.steered);
    assert_eq!(steered.turn_id, f.first.turn_id);
    assert!(steered.queued_id.is_none());

    // The one result consumed both prompts: the turn completes and nothing
    // else starts.
    let events = lifecycle_until(&mut stream, 600, |_| false).await;
    assert!(events.iter().any(|ev| matches!(ev,
        ProviderRuntimeEvent::TurnCompleted { turn_id, .. } if *turn_id == f.first.turn_id)));
    assert!(running_turns(&events).is_empty(), "{events:?}");
    assert!(!f.provider.turn_active(&f.thread).await);

    let captured = std::fs::read_to_string(capture.path()).unwrap();
    let steer = captured
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .find(|record| record["method"] == "steer-turn")
        .expect("steer-turn RPC");
    assert_eq!(steer["params"]["text"], "also this");
    assert!(!steer["params"]["uuid"].as_str().unwrap().is_empty());
    f.provider.stop_session(f.thread).await.ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn guidance_the_turn_missed_runs_as_a_continuation_before_the_queue() {
    // The turn ends before the CLI reads the guidance, so the CLI runs it as
    // a turn of its own. That turn must be tracked, and queued messages wait
    // for it.
    let thread = "t-steer-missed";
    let f = steer_fixture(
        thread,
        vec![
            result_after("steer-turn", 150, thread, json!(["first-prompt"])),
            result_after("steer-turn", 300, thread, json!("$steered")),
        ],
        &[],
    )
    .await;
    let mut stream = f.provider.event_stream();
    assert!(f.provider.steer_turn(turn_input(thread, "guidance")).await.unwrap().steered);
    let queued = f.provider.send_turn(turn_input(thread, "follow-up")).await.unwrap();
    assert!(queued.queued_id.is_some());

    let events = lifecycle_until(&mut stream, 3000, is_dispatch).await;
    let order: Vec<String> = events
        .iter()
        .filter_map(|ev| match ev {
            ProviderRuntimeEvent::TurnCompleted { turn_id, .. } => Some(format!("done:{}", turn_id.0)),
            ProviderRuntimeEvent::SessionStateChanged {
                status: SessionStatus::Running { active_turn }, ..
            } => Some(format!("run:{}", active_turn.0)),
            ProviderRuntimeEvent::QueuedTurnDispatched { .. } => Some("dispatch".into()),
            _ => None,
        })
        .collect();
    let continuation = running_turns(&events)
        .into_iter()
        .next()
        .expect("continuation turn announced");
    assert_ne!(continuation, f.first.turn_id.0);
    // The queued follow-up starts (its own `run:` then `dispatch`) only
    // after the continuation completes.
    assert_eq!(
        order[..3],
        [
            format!("done:{}", f.first.turn_id.0),
            format!("run:{continuation}"),
            format!("done:{continuation}"),
        ],
        "{order:?}"
    );
    assert_eq!(order.last().map(String::as_str), Some("dispatch"), "{order:?}");
    f.provider.stop_session(f.thread).await.ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stop_wins_over_pending_guidance() {
    // An aborting turn can still report a result. Guidance it never consumed
    // died with the Stop and must not start a phantom continuation.
    let thread = "t-steer-stop";
    let f = steer_fixture(
        thread,
        vec![result_after("interrupt", 20, thread, json!(["first-prompt"]))],
        &[],
    )
    .await;
    assert!(f.provider.steer_turn(turn_input(thread, "guidance")).await.unwrap().steered);
    let mut stream = f.provider.event_stream();
    f.provider.interrupt_turn(f.thread.clone(), None).await.unwrap();

    let events = lifecycle_until(&mut stream, 500, |_| false).await;
    assert!(running_turns(&events).is_empty(), "{events:?}");
    assert!(!f.provider.turn_active(&f.thread).await);
    f.provider.stop_session(f.thread).await.ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_stop_keeps_tracking_delivered_guidance() {
    // The interrupt fails, so the turn runs on and ends without reading the
    // guidance. The CLI runs it next, which must be a tracked turn.
    let thread = "t-steer-stop-failed";
    let f = steer_fixture(
        thread,
        vec![result_after("interrupt", 50, thread, json!(["first-prompt"]))],
        &[("FAKE_CLAUDE_INTERRUPT_ERROR", "interrupt failed")],
    )
    .await;
    assert!(f.provider.steer_turn(turn_input(thread, "guidance")).await.unwrap().steered);
    let mut stream = f.provider.event_stream();
    assert!(f.provider.interrupt_turn(f.thread.clone(), None).await.is_err());

    let events = lifecycle_until(&mut stream, 500, |ev| {
        matches!(ev, ProviderRuntimeEvent::SessionStateChanged { status: SessionStatus::Running { .. }, .. })
    })
    .await;
    assert_eq!(running_turns(&events).len(), 1, "{events:?}");
    assert!(f.provider.turn_active(&f.thread).await);
    f.provider.stop_session(f.thread).await.ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stop_that_fails_after_the_turn_ended_still_tracks_the_guidance() {
    // The turn ends while the interrupt is in flight and the interrupt then
    // fails: the guidance it never read runs next and must be tracked.
    let thread = "t-steer-stop-late-fail";
    let f = steer_fixture(
        thread,
        vec![result_after("interrupt", 20, thread, json!(["first-prompt"]))],
        &[
            ("FAKE_CLAUDE_INTERRUPT_ERROR", "interrupt failed"),
            ("FAKE_CLAUDE_INTERRUPT_DELAY_MS", "200"),
        ],
    )
    .await;
    assert!(f.provider.steer_turn(turn_input(thread, "guidance")).await.unwrap().steered);
    let mut stream = f.provider.event_stream();
    assert!(f.provider.interrupt_turn(f.thread.clone(), None).await.is_err());
    let events = lifecycle_until(&mut stream, 300, |_| false).await;
    assert_eq!(running_turns(&events).len(), 1, "{events:?}");
    assert!(f.provider.turn_active(&f.thread).await);
    f.provider.stop_session(f.thread).await.ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_steer_strands_nothing() {
    let thread = "t-steer-fail";
    let f = steer_fixture(thread, vec![], &[("FAKE_CLAUDE_STEER_ERROR", "-32603:boom")]).await;
    let queued = f
        .provider
        .send_turn(turn_input(thread, "second"))
        .await
        .unwrap()
        .queued_id
        .unwrap();

    assert!(f.provider.steer_turn(turn_input(thread, "guidance")).await.is_err());
    assert!(f
        .provider
        .steer_queued_turn(f.thread.clone(), queued.clone())
        .await
        .is_err());
    // The turn keeps running and the queued message is still queued.
    assert!(f.provider.turn_active(&f.thread).await);
    assert!(f.provider.cancel_queued_turn(f.thread.clone(), queued).await.unwrap());
    f.provider.stop_session(f.thread).await.ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_steer_rejected_after_the_turn_ended_still_sends_the_queued_message() {
    // The turn's result lands while the queued message's steer is in flight;
    // the steer is then rejected. The message goes back in the queue before
    // it drains, so it still dispatches.
    let thread = "t-steer-late-reject";
    let f = steer_fixture(
        thread,
        vec![result_after("steer-turn", 20, thread, json!(["first-prompt"]))],
        &[
            ("FAKE_CLAUDE_STEER_ERROR", "-32603:boom"),
            ("FAKE_CLAUDE_STEER_DELAY_MS", "300"),
        ],
    )
    .await;
    let queued = f
        .provider
        .send_turn(turn_input(thread, "second"))
        .await
        .unwrap()
        .queued_id
        .unwrap();
    let mut stream = f.provider.event_stream();
    assert!(f
        .provider
        .steer_queued_turn(f.thread.clone(), queued.clone())
        .await
        .is_err());
    let events = lifecycle_until(&mut stream, 2000, is_dispatch).await;
    assert!(events.iter().any(|ev| matches!(ev,
        ProviderRuntimeEvent::QueuedTurnDispatched { queued_id, steered: false, .. }
            if *queued_id == queued)), "{events:?}");
    f.provider.stop_session(f.thread).await.ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn steering_a_queued_message_delivers_it_into_the_running_turn() {
    let thread = "t-steer-queued";
    let f = steer_fixture(thread, vec![], &[]).await;
    let queued = f
        .provider
        .send_turn(turn_input(thread, "second"))
        .await
        .unwrap()
        .queued_id
        .unwrap();
    let mut stream = f.provider.event_stream();
    f.provider
        .steer_queued_turn(f.thread.clone(), queued.clone())
        .await
        .unwrap();
    let events = lifecycle_until(&mut stream, 1000, is_dispatch).await;
    assert!(events.iter().any(|ev| matches!(ev,
        ProviderRuntimeEvent::QueuedTurnDispatched { queued_id, turn_id, steered: true, .. }
            if *queued_id == queued && *turn_id == f.first.turn_id)), "{events:?}");
    assert!(!f.provider.cancel_queued_turn(f.thread.clone(), queued).await.unwrap());
    f.provider.stop_session(f.thread).await.ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn steer_without_a_running_turn_sends_normally() {
    let thread = "t-steer-idle";
    let provider = provider_with_fake().await;
    provider.start_session(start_input(thread)).await.unwrap();
    let started = provider.steer_turn(turn_input(thread, "hello")).await.unwrap();
    assert!(!started.steered);
    assert!(started.queued_id.is_none());
    assert!(!started.turn_id.0.is_empty());
    assert!(provider.turn_active(&ThreadId(thread.into())).await);
    provider.stop_session(ThreadId(thread.into())).await.ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn steer_waits_for_a_cli_that_reports_consumed_prompts() {
    // Without the echo a missed steer cannot be told from an absorbed one, so
    // the message queues instead.
    let thread = "t-steer-no-echo";
    let provider = provider_with_fake().await;
    provider.start_session(start_input(thread)).await.unwrap();
    provider.send_turn(turn_input(thread, "first")).await.unwrap();
    let sent = provider.steer_turn(turn_input(thread, "guidance")).await.unwrap();
    assert!(!sent.steered);
    assert!(sent.queued_id.is_some());
    provider.stop_session(ThreadId(thread.into())).await.ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn steer_refuses_slash_commands_and_old_sidecars() {
    let thread = "t-steer-refuse";
    let f = steer_fixture(
        thread,
        vec![],
        &[("FAKE_CLAUDE_STEER_ERROR", "-32601:unknown method: steer-turn")],
    )
    .await;
    let slash = f.provider.steer_turn(turn_input(thread, "/compact")).await;
    assert!(matches!(slash, Err(ProviderError::ValidationError { .. })), "{slash:?}");
    let old = f.provider.steer_turn(turn_input(thread, "guidance")).await;
    assert!(
        matches!(&old, Err(ProviderError::ValidationError { message }) if message.contains("can't steer")),
        "{old:?}"
    );
    f.provider.stop_session(f.thread).await.ok();
}

#[derive(Debug)]
struct MachineQueueCheckpoint;
#[async_trait::async_trait]
impl codemux_lib::agent_provider::TurnDispatchCheckpoint for MachineQueueCheckpoint {
    fn releases_queue_hold(&self) -> bool { false }
    async fn prepare(&self) {}
    async fn commit(&self) {}
    async fn abort(&self) {}
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn orchestration_message_claude_machine_send_preserves_stop_queue_hold() {
    let provider=provider_with_fake().await;
    let thread=ThreadId("machine-held-queue".into());
    provider.start_session(start_input(&thread.0)).await.unwrap();
    let mut events=provider.event_stream();
    provider.send_turn(turn_input(&thread.0,"first")).await.unwrap();
    let held=provider.send_turn(turn_input(&thread.0,"held-B")).await.unwrap().queued_id.unwrap();
    provider.interrupt_turn(thread.clone(),None).await.unwrap();
    let mut machine=turn_input(&thread.0,"machine-C");
    machine.turn_checkpoint=Some(Arc::new(MachineQueueCheckpoint));
    let started=provider.send_turn(machine).await.unwrap();
    assert!(started.queued_id.is_none());
    // A selected-turn interrupt is not another Stop. Its completion would
    // drain B if the machine send had taken the existing provider hold.
    provider.interrupt_turn(thread.clone(),Some(started.turn_id)).await.unwrap();
    let observed=timeout(Duration::from_millis(500),async {
        while let Some(event)=events.next().await {
            if matches!(event,ProviderRuntimeEvent::QueuedTurnDispatched {ref queued_id,..}|ProviderRuntimeEvent::QueuedTurnCancelled {ref queued_id,..} if queued_id==&held) { return event; }
        }
        panic!("event stream closed");
    }).await;
    assert!(observed.is_err(),"held B must stay queued: {observed:?}");
    provider.send_queued_turn_now(thread.clone(),held).await.unwrap();
    provider.stop_session(thread).await.unwrap();
}

#[derive(Debug)]
struct PromotionAuthority(std::sync::atomic::AtomicBool);
#[async_trait::async_trait]
impl codemux_lib::agent_provider::TurnDispatchCheckpoint for PromotionAuthority {
    fn queued_delivery_state(&self) -> codemux_lib::agent_provider::types::QueuedDeliveryState {
        if self.0.load(std::sync::atomic::Ordering::SeqCst) { codemux_lib::agent_provider::types::QueuedDeliveryState::Blocked }
        else { codemux_lib::agent_provider::types::QueuedDeliveryState::Ready }
    }
    fn authorize_dispatch(&self) -> Result<(),ProviderError> {
        if self.0.load(std::sync::atomic::Ordering::SeqCst) { Err(ProviderError::ValidationError {message:"grant_revoked".into()}) } else { Ok(()) }
    }
    async fn prepare(&self) {}
    async fn commit(&self) {}
    async fn abort(&self) {}
}

#[tokio::test(flavor="multi_thread",worker_threads=2)]
async fn orchestration_message_claude_blocked_promotion_cannot_interrupt_for_another_message() {
    let provider=provider_with_fake().await;
    let thread=ThreadId("refused-send-now".into());
    provider.start_session(start_input(&thread.0)).await.unwrap();
    let running=provider.send_turn(turn_input(&thread.0,"running C")).await.unwrap();
    let authority=Arc::new(PromotionAuthority(std::sync::atomic::AtomicBool::new(true)));
    let mut b=turn_input(&thread.0,"refused B");b.turn_checkpoint=Some(authority.clone());
    let b=provider.send_turn(b).await.unwrap().queued_id.unwrap();
    let d=provider.send_turn(turn_input(&thread.0,"queued D")).await.unwrap().queued_id.unwrap();
    let mut events=provider.event_stream();
    let denied=provider.send_queued_turn_now(thread.clone(),b.clone()).await;
    assert!(matches!(&denied,Err(ProviderError::ValidationError {message}) if message.contains("held")),"{denied:?}");
    assert!(provider.turn_active(&thread).await,"running C must not be interrupted for D");
    assert!(timeout(Duration::from_millis(250),async { while let Some(e)=events.next().await {
        if matches!(e,ProviderRuntimeEvent::TurnCompleted {ref turn_id,..} if turn_id==&running.turn_id) || matches!(e,ProviderRuntimeEvent::QueuedTurnDispatched {ref queued_id,..} if queued_id==&d) { return; }
    }}).await.is_err());
    authority.0.store(false,std::sync::atomic::Ordering::SeqCst);
    provider.send_queued_turn_now(thread.clone(),b.clone()).await.unwrap();
    let mut dispatched=vec![];
    timeout(Duration::from_secs(3),async { while let Some(e)=events.next().await {
        if let ProviderRuntimeEvent::QueuedTurnDispatched {queued_id,..}=e { dispatched.push(queued_id);break; }
    }}).await.unwrap();
    assert_eq!(dispatched,vec![b.clone()]);
    provider.send_queued_turn_now(thread.clone(),b).await.unwrap();
    assert!(timeout(Duration::from_millis(250),async { while let Some(e)=events.next().await {
        if matches!(e,ProviderRuntimeEvent::QueuedTurnDispatched {..}) { return; }
    }}).await.is_err(),"idempotent promotion must not send B twice or interrupt it for D");
    provider.stop_session(thread).await.unwrap();
}
