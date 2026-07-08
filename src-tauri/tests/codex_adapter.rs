//! End-to-end integration tests for
//! [`codemux_lib::agent_provider::codex::CodexAgentProvider`].
//!
//! Each test spawns the `fake_codex_app_server` helper binary as the
//! provider's `codex` backend and drives the adapter's public trait
//! surface. The helper supports a JSON "script" loaded from the path in
//! `FAKE_CODEX_SCRIPT`, which lets each test choreograph the exact
//! sequence of notifications and server-initiated requests the fixture
//! should emit.
//!
//! Unix-only: `wrapper_with_env` and `write_bash_script` both write
//! `#!/usr/bin/env bash` files and chmod them with
//! `std::os::unix::fs::PermissionsExt`. Windows has no analogue, and
//! every test in this file depends on those wrappers, so we gate the
//! whole file rather than carry parallel cmd.exe fixtures.

#![cfg(unix)]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use serde_json::json;
use tempfile::TempDir;
use tokio::time::timeout;

use codemux_lib::agent_provider::codex::auth::{
    classify_auth_output, probe_authenticated, probe_installed, AuthStatus,
};
use codemux_lib::agent_provider::codex::protocol::ClientInfo;
use codemux_lib::agent_provider::codex::{CodexAgentProvider, CodexProviderConfig};
use codemux_lib::agent_provider::{
    AgentProvider, ApprovalDecision, CompletedItem, ContentDelta, ProviderError,
    ProviderRuntimeEvent, RequestId, SendTurnInput, SessionStatus, StartSessionInput,
    SubagentStatus, ThreadId, TurnId,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_fake_codex_app_server"))
}

fn provider_with_fixture() -> CodexAgentProvider {
    provider_with_fixture_and_binary(fixture_path())
}

fn provider_with_fixture_and_binary(bin: PathBuf) -> CodexAgentProvider {
    CodexAgentProvider::new(CodexProviderConfig {
        codex_binary: bin,
        codex_home: None,
        event_channel_capacity: 1024,
        client_info: ClientInfo {
            name: "codemux-test".into(),
            title: "Codemux Test".into(),
            version: "0.0.0-test".into(),
        },
    })
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

/// Collect events from the provider's event stream until `stop`
/// returns true or the deadline elapses.
#[allow(dead_code)]
async fn collect_events_until<F>(
    provider: &CodexAgentProvider,
    mut stop: F,
    deadline: Duration,
) -> Vec<ProviderRuntimeEvent>
where
    F: FnMut(&[ProviderRuntimeEvent]) -> bool,
{
    let mut stream = provider.event_stream();
    let out = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let out_inner = std::sync::Arc::clone(&out);
    let _ = timeout(deadline, async move {
        while let Some(ev) = stream.next().await {
            let mut guard = out_inner.lock().unwrap();
            guard.push(ev);
            if stop(&guard) {
                break;
            }
        }
    })
    .await;
    let final_out = out.lock().unwrap().clone();
    final_out
}

/// Guarded (dir, path) pair for a script that lives inside its own
/// tempdir. Using a dedicated tempdir per script avoids inheritance /
/// ETXTBSY races with concurrent tests that would briefly hold a
/// writable fd on a shared tempfile.
struct ScriptFile {
    _dir: TempDir,
    path: PathBuf,
}

impl ScriptFile {
    fn path(&self) -> &std::path::Path {
        &self.path
    }
    fn to_string_lossy(&self) -> std::borrow::Cow<'_, str> {
        self.path.to_string_lossy()
    }
    fn to_path_buf(&self) -> PathBuf {
        self.path.clone()
    }
}

/// Write `value` as JSON to a fresh tempdir/script.json and return the
/// guarded path.
fn write_script(value: serde_json::Value) -> ScriptFile {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("script.json");
    // Write+close in one call; std::fs::write drops the fd before
    // returning.
    std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    ScriptFile { _dir: dir, path }
}

/// Write a bash wrapper script to a fresh tempdir and return it with +x
/// permissions. The wrapper exports the requested env vars then execs
/// the fake fixture.
fn wrapper_with_env(env: &[(&str, &str)]) -> ScriptFile {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("wrap.sh");
    let mut body = String::from("#!/usr/bin/env bash\nset -e\n");
    for (k, v) in env {
        body.push_str(&format!("export {k}={}\n", shell_escape::escape(v)));
    }
    body.push_str(&format!(
        "exec {} \"$@\"\n",
        shell_escape::escape(fixture_path().to_string_lossy()),
    ));
    std::fs::write(&path, body.as_bytes()).unwrap();
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    ScriptFile { _dir: dir, path }
}

mod shell_escape {
    /// Trivial POSIX shell escaper sufficient for our test wrapper usage.
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn starts_session_and_reports_ready() {
    let provider = provider_with_fixture();
    // Subscribe BEFORE starting so we catch the SessionConfigured event.
    let mut stream = provider.event_stream();
    let handle = tokio::spawn(async move {
        let mut evs = Vec::new();
        for _ in 0..2 {
            if let Some(ev) = tokio::time::timeout(Duration::from_secs(2), stream.next())
                .await
                .ok()
                .flatten()
            {
                evs.push(ev);
            }
        }
        evs
    });

    let session = start_session_resilient(&provider, start_input("t-ready")).await.unwrap();
    assert_eq!(session.thread_id.0, "t-ready");
    assert!(matches!(session.status, SessionStatus::Ready));

    let events = handle.await.unwrap();
    assert!(events.iter().any(|e| matches!(
        e,
        ProviderRuntimeEvent::SessionConfigured { .. }
    )));
    provider.stop_session(ThreadId("t-ready".into())).await.ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_turn_emits_turn_started_then_delta_then_completed() {
    let script = write_script(json!([
        {"after":"turn/start","delay_ms":5,"emit":"notification","method":"turn/started",
         "params":{"threadId":"c-1","turnId":"t-scripted"}},
        {"after":"turn/start","delay_ms":10,"emit":"notification",
         "method":"item/agentMessage/delta",
         "params":{"threadId":"c-1","turnId":"t-scripted","itemId":"i-1","delta":"Hi"}},
        {"after":"turn/start","delay_ms":20,"emit":"notification",
         "method":"turn/completed",
         "params":{"threadId":"c-1","turnId":"t-scripted","status":"succeeded"}}
    ]));
    let wrapper = wrapper_with_env(&[
        ("FAKE_CODEX_SCRIPT", &script.to_string_lossy()),
    ]);
    let provider = provider_with_fixture_and_binary(wrapper.to_path_buf());
    start_session_resilient(&provider, start_input("t-seq")).await.unwrap();
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
        })
        .await
        .unwrap();

    let mut saw_running = false;
    let mut saw_delta = false;
    let mut saw_completed = false;
    let collect = timeout(Duration::from_secs(5), async {
        while let Some(ev) = stream.next().await {
            match &ev {
                ProviderRuntimeEvent::SessionStateChanged { status, .. } => {
                    if matches!(status, SessionStatus::Running { .. }) {
                        saw_running = true;
                    }
                }
                ProviderRuntimeEvent::ContentDelta { delta, .. } => {
                    if let ContentDelta::Text { text } = delta {
                        if text == "Hi" {
                            saw_delta = true;
                        }
                    }
                }
                ProviderRuntimeEvent::TurnCompleted { .. } => {
                    saw_completed = true;
                }
                _ => {}
            }
            if saw_running && saw_delta && saw_completed {
                break;
            }
        }
    })
    .await;
    assert!(collect.is_ok(), "timed out waiting for events");
    assert!(saw_running, "missing SessionStateChanged Running");
    assert!(saw_delta, "missing ContentDelta Text");
    assert!(saw_completed, "missing TurnCompleted");
    provider.stop_session(ThreadId("t-seq".into())).await.ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn thread_resume_with_recoverable_error_falls_back_to_start() {
    let wrapper = wrapper_with_env(&[("FAKE_CODEX_FAIL_RESUME", "1")]);
    let provider = provider_with_fixture_and_binary(wrapper.to_path_buf());
    let mut stream = provider.event_stream();

    let mut input = start_input("t-resume");
    input.resume_cursor = Some(json!({"threadId":"old-thread"}));
    let session = start_session_resilient(&provider, input).await.unwrap();
    assert!(matches!(session.status, SessionStatus::Ready));

    // Look for the RuntimeWarning on the stream.
    let mut saw_warning = false;
    let collect = timeout(Duration::from_secs(3), async {
        while let Some(ev) = stream.next().await {
            if let ProviderRuntimeEvent::RuntimeWarning { message, .. } = &ev {
                if message.contains("thread/resume") && message.contains("fall") {
                    saw_warning = true;
                    break;
                }
            }
        }
    })
    .await;
    assert!(collect.is_ok(), "timed out waiting for warning");
    assert!(saw_warning, "expected fallback warning");
    provider.stop_session(ThreadId("t-resume".into())).await.ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_notification_surfaces_as_runtime_warning() {
    let script = write_script(json!([
        {"after":"turn/start","delay_ms":5,"emit":"notification",
         "method":"foo/bar","params":{"interesting":"data"}},
        {"after":"turn/start","delay_ms":15,"emit":"notification",
         "method":"turn/completed",
         "params":{"threadId":"c-1","turnId":"t-u","status":"succeeded"}}
    ]));
    let wrapper = wrapper_with_env(&[
        ("FAKE_CODEX_SCRIPT", &script.to_string_lossy()),
    ]);
    let provider = provider_with_fixture_and_binary(wrapper.to_path_buf());
    start_session_resilient(&provider, start_input("t-unk")).await.unwrap();
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
        })
        .await
        .unwrap();

    let mut saw = false;
    let _ = timeout(Duration::from_secs(3), async {
        while let Some(ev) = stream.next().await {
            if let ProviderRuntimeEvent::RuntimeWarning {
                message,
                original_payload,
                ..
            } = &ev
            {
                if message.contains("foo/bar") && original_payload.is_some() {
                    saw = true;
                    break;
                }
            }
        }
    })
    .await;
    assert!(saw, "expected RuntimeWarning for unknown method");
    provider.stop_session(ThreadId("t-unk".into())).await.ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn command_approval_request_roundtrip() {
    let script = write_script(json!([
        {"after":"turn/start","delay_ms":5,"emit":"server_request",
         "method":"item/commandExecution/requestApproval",
         "params":{"turnId":"t-ap","cmd":"ls"}}
    ]));
    let wrapper = wrapper_with_env(&[
        ("FAKE_CODEX_SCRIPT", &script.to_string_lossy()),
    ]);
    let provider = provider_with_fixture_and_binary(wrapper.to_path_buf());
    start_session_resilient(&provider, start_input("t-ap")).await.unwrap();
    let mut stream = provider.event_stream();
    provider
        .send_turn(SendTurnInput {
            thread_id: ThreadId("t-ap".into()),
            text: "run ls".into(),
            images: vec![],
            model_override: None,
            effort_override: None,
            permission_mode_override: None,
            client_nonce: None,
        })
        .await
        .unwrap();

    let mut request_id: Option<RequestId> = None;
    let _ = timeout(Duration::from_secs(3), async {
        while let Some(ev) = stream.next().await {
            if let ProviderRuntimeEvent::RequestOpened {
                request_kind,
                request_id: rid,
                ..
            } = ev
            {
                assert_eq!(request_kind, "command");
                request_id = Some(rid);
                break;
            }
        }
    })
    .await;
    let request_id = request_id.expect("RequestOpened event");
    provider
        .respond_to_request(
            ThreadId("t-ap".into()),
            request_id,
            ApprovalDecision::Allow {
                updated_input: None,
                updated_permissions: None,
            },
        )
        .await
        .unwrap();
    provider.stop_session(ThreadId("t-ap".into())).await.ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn command_approval_deny_roundtrip() {
    let script = write_script(json!([
        {"after":"turn/start","delay_ms":5,"emit":"server_request",
         "method":"item/commandExecution/requestApproval",
         "params":{"turnId":"t-deny","cmd":"rm -rf /"}}
    ]));
    let wrapper = wrapper_with_env(&[
        ("FAKE_CODEX_SCRIPT", &script.to_string_lossy()),
    ]);
    let provider = provider_with_fixture_and_binary(wrapper.to_path_buf());
    start_session_resilient(&provider, start_input("t-deny")).await.unwrap();
    let mut stream = provider.event_stream();
    provider
        .send_turn(SendTurnInput {
            thread_id: ThreadId("t-deny".into()),
            text: "do it".into(),
            images: vec![],
            model_override: None,
            effort_override: None,
            permission_mode_override: None,
            client_nonce: None,
        })
        .await
        .unwrap();

    let mut request_id: Option<RequestId> = None;
    let _ = timeout(Duration::from_secs(3), async {
        while let Some(ev) = stream.next().await {
            if let ProviderRuntimeEvent::RequestOpened { request_id: rid, .. } = ev {
                request_id = Some(rid);
                break;
            }
        }
    })
    .await;
    let request_id = request_id.expect("RequestOpened");
    provider
        .respond_to_request(
            ThreadId("t-deny".into()),
            request_id,
            ApprovalDecision::Deny {
                message: "nope".into(),
            },
        )
        .await
        .unwrap();
    provider.stop_session(ThreadId("t-deny".into())).await.ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn interrupt_turn_sends_turn_interrupt() {
    let script = write_script(json!([
        {"after":"turn/start","delay_ms":5,"emit":"notification","method":"turn/started",
         "params":{"threadId":"c-1","turnId":"t-int-1"}}
    ]));
    let wrapper = wrapper_with_env(&[
        ("FAKE_CODEX_SCRIPT", &script.to_string_lossy()),
    ]);
    let provider = provider_with_fixture_and_binary(wrapper.to_path_buf());
    start_session_resilient(&provider, start_input("t-int")).await.unwrap();
    let res = provider
        .send_turn(SendTurnInput {
            thread_id: ThreadId("t-int".into()),
            text: "x".into(),
            images: vec![],
            model_override: None,
            effort_override: None,
            permission_mode_override: None,
            client_nonce: None,
        })
        .await
        .unwrap();
    // Give the scripted notification time to land and update active_turn.
    tokio::time::sleep(Duration::from_millis(50)).await;
    let _ = res;

    provider
        .interrupt_turn(ThreadId("t-int".into()), None)
        .await
        .unwrap();

    provider.stop_session(ThreadId("t-int".into())).await.ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn interrupt_turn_with_wrong_turn_id_fails_validation() {
    let script = write_script(json!([
        {"after":"turn/start","delay_ms":5,"emit":"notification","method":"turn/started",
         "params":{"threadId":"c-1","turnId":"t-active"}}
    ]));
    let wrapper = wrapper_with_env(&[
        ("FAKE_CODEX_SCRIPT", &script.to_string_lossy()),
    ]);
    let provider = provider_with_fixture_and_binary(wrapper.to_path_buf());
    start_session_resilient(&provider, start_input("t-mm")).await.unwrap();
    provider
        .send_turn(SendTurnInput {
            thread_id: ThreadId("t-mm".into()),
            text: "x".into(),
            images: vec![],
            model_override: None,
            effort_override: None,
            permission_mode_override: None,
            client_nonce: None,
        })
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;

    let err = provider
        .interrupt_turn(ThreadId("t-mm".into()), Some(TurnId("t-wrong".into())))
        .await
        .unwrap_err();
    assert!(matches!(err, ProviderError::ValidationError { .. }));
    provider.stop_session(ThreadId("t-mm".into())).await.ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn set_permission_mode_returns_validation_error() {
    let provider = provider_with_fixture();
    start_session_resilient(&provider, start_input("t-perm")).await.unwrap();
    let err = provider
        .set_permission_mode(ThreadId("t-perm".into()), "acceptEdits".into())
        .await
        .unwrap_err();
    assert!(matches!(err, ProviderError::ValidationError { .. }));
    provider.stop_session(ThreadId("t-perm".into())).await.ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn set_model_updates_session_state() {
    let provider = provider_with_fixture();
    start_session_resilient(&provider, start_input("t-model")).await.unwrap();
    provider
        .set_model(ThreadId("t-model".into()), "opus-4-7".into())
        .await
        .unwrap();
    // Start a turn; we can't snoop on the outgoing params easily, but
    // the call should succeed and not panic.
    provider
        .send_turn(SendTurnInput {
            thread_id: ThreadId("t-model".into()),
            text: "go".into(),
            images: vec![],
            model_override: None,
            effort_override: None,
            permission_mode_override: None,
            client_nonce: None,
        })
        .await
        .unwrap();
    provider.stop_session(ThreadId("t-model".into())).await.ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stop_session_removes_from_list_and_closes_child() {
    let provider = provider_with_fixture();
    start_session_resilient(&provider, start_input("t-close")).await.unwrap();
    assert_eq!(provider.list_sessions().await.unwrap().len(), 1);
    provider
        .stop_session(ThreadId("t-close".into()))
        .await
        .unwrap();
    assert_eq!(provider.list_sessions().await.unwrap().len(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stop_session_is_idempotent() {
    let provider = provider_with_fixture();
    start_session_resilient(&provider, start_input("t-idem")).await.unwrap();
    provider.stop_session(ThreadId("t-idem".into())).await.unwrap();
    let err = provider
        .stop_session(ThreadId("t-idem".into()))
        .await
        .unwrap_err();
    assert!(matches!(err, ProviderError::SessionNotFound { .. }));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn send_turn_on_nonexistent_thread_returns_session_not_found() {
    let provider = provider_with_fixture();
    let err = provider
        .send_turn(SendTurnInput {
            thread_id: ThreadId("nope".into()),
            text: "hi".into(),
            images: vec![],
            model_override: None,
            effort_override: None,
            permission_mode_override: None,
            client_nonce: None,
        })
        .await
        .unwrap_err();
    assert!(matches!(err, ProviderError::SessionNotFound { .. }));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn duplicate_start_session_returns_validation_error() {
    let provider = provider_with_fixture();
    start_session_resilient(&provider, start_input("t-dup")).await.unwrap();
    let err = start_session_resilient(&provider, start_input("t-dup")).await.unwrap_err();
    assert!(matches!(err, ProviderError::ValidationError { .. }));
    provider.stop_session(ThreadId("t-dup".into())).await.ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn child_process_crash_emits_error_state() {
    // Exit-on turn/start triggers the watchdog after the turn has been
    // acknowledged.
    let wrapper = wrapper_with_env(&[("FAKE_CODEX_EXIT_AFTER", "turn/start")]);
    let provider = provider_with_fixture_and_binary(wrapper.to_path_buf());
    start_session_resilient(&provider, start_input("t-crash")).await.unwrap();
    let mut stream = provider.event_stream();
    // turn/start will succeed then the fixture exits.
    let _ = provider
        .send_turn(SendTurnInput {
            thread_id: ThreadId("t-crash".into()),
            text: "x".into(),
            images: vec![],
            model_override: None,
            effort_override: None,
            permission_mode_override: None,
            client_nonce: None,
        })
        .await;

    let mut saw_error = false;
    let _ = timeout(Duration::from_secs(3), async {
        while let Some(ev) = stream.next().await {
            if let ProviderRuntimeEvent::SessionStateChanged { status, .. } = &ev {
                if let SessionStatus::Error { message } = status {
                    if message.contains("exited") {
                        saw_error = true;
                        break;
                    }
                }
            }
        }
    })
    .await;
    assert!(saw_error, "expected SessionStateChanged Error after crash");
    let _ = provider.stop_session(ThreadId("t-crash".into())).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_send_turn_queues_instead_of_erroring() {
    // Follow-up queueing: a second send while the first turn is active is
    // now QUEUED (not rejected). The fixture emits only turn/started, so
    // the first turn stays active and the second parks in the queue.
    let script = write_script(json!([
        {"after":"turn/start","delay_ms":5,"emit":"notification","method":"turn/started",
         "params":{"threadId":"c-1","turnId":"t-busy"}}
    ]));
    let wrapper = wrapper_with_env(&[
        ("FAKE_CODEX_SCRIPT", &script.to_string_lossy()),
    ]);
    let provider = provider_with_fixture_and_binary(wrapper.to_path_buf());
    start_session_resilient(&provider, start_input("t-busy")).await.unwrap();
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
        })
        .await
        .unwrap();
    assert!(first.queued_id.is_none(), "first send starts immediately");
    tokio::time::sleep(Duration::from_millis(50)).await;
    let second = provider
        .send_turn(SendTurnInput {
            thread_id: ThreadId("t-busy".into()),
            text: "second".into(),
            images: vec![],
            model_override: None,
            effort_override: None,
            permission_mode_override: None,
            client_nonce: Some("nonce-2".into()),
        })
        .await
        .unwrap();
    let queued_id = second
        .queued_id
        .expect("second send should be queued behind the active turn");

    let saw = timeout(Duration::from_secs(3), async {
        while let Some(ev) = stream.next().await {
            if let ProviderRuntimeEvent::TurnQueued {
                queued_id: qid,
                client_nonce,
                text,
                ..
            } = ev
            {
                assert_eq!(qid, queued_id);
                assert_eq!(client_nonce.as_deref(), Some("nonce-2"));
                assert_eq!(text, "second");
                return true;
            }
        }
        false
    })
    .await
    .unwrap_or(false);
    assert!(saw, "expected a TurnQueued event");
    provider.stop_session(ThreadId("t-busy".into())).await.ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancel_queued_turn_removes_it_codex() {
    let script = write_script(json!([
        {"after":"turn/start","delay_ms":5,"emit":"notification","method":"turn/started",
         "params":{"threadId":"c-1","turnId":"t-cq"}}
    ]));
    let wrapper = wrapper_with_env(&[
        ("FAKE_CODEX_SCRIPT", &script.to_string_lossy()),
    ]);
    let provider = provider_with_fixture_and_binary(wrapper.to_path_buf());
    start_session_resilient(&provider, start_input("t-cq"))
        .await
        .unwrap();
    let mut stream = provider.event_stream();
    provider
        .send_turn(SendTurnInput {
            thread_id: ThreadId("t-cq".into()),
            text: "first".into(),
            images: vec![],
            model_override: None,
            effort_override: None,
            permission_mode_override: None,
            client_nonce: None,
        })
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    let queued = provider
        .send_turn(SendTurnInput {
            thread_id: ThreadId("t-cq".into()),
            text: "second".into(),
            images: vec![],
            model_override: None,
            effort_override: None,
            permission_mode_override: None,
            client_nonce: None,
        })
        .await
        .unwrap()
        .queued_id
        .expect("second send queued");

    provider
        .cancel_queued_turn(ThreadId("t-cq".into()), queued.clone())
        .await
        .unwrap();

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
    provider.stop_session(ThreadId("t-cq".into())).await.ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn multiple_concurrent_sessions_are_isolated() {
    let provider = Arc::new(provider_with_fixture());
    let mut handles = Vec::new();
    for i in 0..3 {
        let p = Arc::clone(&provider);
        handles.push(tokio::spawn(async move {
            start_session_resilient(&p, start_input(&format!("t-iso-{i}"))).await
        }));
    }
    for h in handles {
        h.await.unwrap().unwrap();
    }
    let sessions = provider.list_sessions().await.unwrap();
    assert_eq!(sessions.len(), 3);
    let ids: Vec<_> = sessions.iter().map(|s| s.thread_id.0.clone()).collect();
    for i in 0..3 {
        assert!(ids.contains(&format!("t-iso-{i}")));
    }
    for i in 0..3 {
        provider.stop_session(ThreadId(format!("t-iso-{i}"))).await.ok();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn event_stream_subscribers_each_receive_events() {
    let provider = provider_with_fixture();
    let mut a = provider.event_stream();
    let mut b = provider.event_stream();
    start_session_resilient(&provider, start_input("t-sub")).await.unwrap();
    let got_a = timeout(Duration::from_secs(2), a.next()).await.unwrap();
    let got_b = timeout(Duration::from_secs(2), b.next()).await.unwrap();
    assert!(got_a.is_some());
    assert!(got_b.is_some());
    provider.stop_session(ThreadId("t-sub".into())).await.ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn late_subscriber_does_not_get_old_events() {
    let provider = provider_with_fixture();
    start_session_resilient(&provider, start_input("t-late")).await.unwrap();
    // Wait for the first burst of events to pass.
    tokio::time::sleep(Duration::from_millis(100)).await;

    let mut late = provider.event_stream();
    // No new events should arrive; poll briefly and confirm.
    let res = timeout(Duration::from_millis(200), late.next()).await;
    assert!(res.is_err(), "late subscriber should see nothing yet");
    provider.stop_session(ThreadId("t-late".into())).await.ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn translate_turn_completed_error_emits_both_events_end_to_end() {
    let script = write_script(json!([
        {"after":"turn/start","delay_ms":5,"emit":"notification","method":"turn/completed",
         "params":{"threadId":"c-1","turnId":"t-fail","status":"failed","error":"oops"}}
    ]));
    let wrapper = wrapper_with_env(&[
        ("FAKE_CODEX_SCRIPT", &script.to_string_lossy()),
    ]);
    let provider = provider_with_fixture_and_binary(wrapper.to_path_buf());
    start_session_resilient(&provider, start_input("t-fail")).await.unwrap();
    let mut stream = provider.event_stream();
    provider
        .send_turn(SendTurnInput {
            thread_id: ThreadId("t-fail".into()),
            text: "x".into(),
            images: vec![],
            model_override: None,
            effort_override: None,
            permission_mode_override: None,
            client_nonce: None,
        })
        .await
        .unwrap();

    let mut saw_turn_completed = false;
    let mut saw_session_error = false;
    let _ = timeout(Duration::from_secs(3), async {
        while let Some(ev) = stream.next().await {
            match &ev {
                ProviderRuntimeEvent::TurnCompleted { .. } => saw_turn_completed = true,
                ProviderRuntimeEvent::SessionStateChanged { status, .. } => {
                    if matches!(status, SessionStatus::Error { .. }) {
                        saw_session_error = true;
                    }
                }
                _ => {}
            }
            if saw_turn_completed && saw_session_error {
                break;
            }
        }
    })
    .await;
    assert!(saw_turn_completed && saw_session_error);
    provider.stop_session(ThreadId("t-fail".into())).await.ok();
}

/// Write a bash script body to a fresh dedicated tempdir with +x
/// permissions and return the guarded path. Dedicated tempdir prevents
/// ETXTBSY races with other concurrent tests writing to the same
/// directory.
fn write_bash_script(_prefix: &str, body: &str) -> ScriptFile {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("script.sh");
    std::fs::write(&path, body.as_bytes()).unwrap();
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(&path).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&path, perms).unwrap();
    ScriptFile { _dir: dir, path }
}

/// Start a session, retrying transient ETXTBSY ("Text file busy")
/// failures from the kernel.
///
/// Why this exists: cargo runs tests in parallel inside one binary,
/// and tokio's `Command::spawn` falls back to fork+exec on Linux when
/// it can't use `posix_spawn`. If another test's `File::create` for
/// its own wrapper script is in-flight at the moment we fork, the
/// child inherits that write fd briefly; then our `execve()` of OUR
/// wrapper sees a non-zero `i_writecount` on the inode (the kernel
/// doesn't distinguish "this exec target" from "any open writer
/// across all fds the child inherited") and rejects with ETXTBSY.
/// The race window is microseconds, but it's real and shows up under
/// CI load. A small retry loop sidesteps it without changing
/// production code.
async fn start_session_resilient(
    provider: &CodexAgentProvider,
    input: StartSessionInput,
) -> Result<codemux_lib::agent_provider::ProviderSession, ProviderError> {
    for _ in 0..10 {
        match provider.start_session(input.clone()).await {
            Ok(s) => return Ok(s),
            Err(e) if format!("{e:?}").contains("Text file busy") => {
                tokio::time::sleep(Duration::from_millis(50)).await;
                continue;
            }
            Err(e) => return Err(e),
        }
    }
    provider.start_session(input).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auth_probe_installed_returns_version_when_codex_works() {
    let wrapper = write_bash_script(
        "codex-ver-",
        "#!/usr/bin/env bash\nif [ \"$1\" = \"--version\" ]; then echo 'codex 1.0.0'; exit 0; fi\nexit 1\n",
    );
    let result = probe_installed(wrapper.path()).await.unwrap();
    assert_eq!(result.as_deref(), Some("1.0.0"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auth_probe_unauthenticated_matches_common_patterns() {
    let wrapper = write_bash_script(
        "codex-auth-",
        "#!/usr/bin/env bash\nif [ \"$2\" = \"status\" ]; then echo 'Not logged in. Run codex login.'; exit 0; fi\nexit 0\n",
    );
    let status = probe_authenticated(wrapper.path()).await.unwrap();
    assert!(matches!(status, AuthStatus::Unauthenticated { .. }));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bogus_response_to_unknown_jsonrpc_id_does_not_crash_adapter() {
    // Send a bogus response message the adapter did not request. The
    // JsonRpcChild layer logs + drops it. This test proves the adapter
    // is still alive afterward.
    let body = format!(
        "#!/usr/bin/env bash\nset -e\necho '{{\"jsonrpc\":\"2.0\",\"id\":99999,\"result\":{{\"orphan\":true}}}}'\nexec {} \"$@\"\n",
        shell_escape::escape(fixture_path().to_string_lossy())
    );
    let helper = write_bash_script("codex-bogus-", &body);
    let provider = provider_with_fixture_and_binary(helper.to_path_buf());
    start_session_resilient(&provider, start_input("t-bogus"))
        .await
        .unwrap();
    // Adapter should still work after the spurious id.
    provider
        .send_turn(SendTurnInput {
            thread_id: ThreadId("t-bogus".into()),
            text: "ping".into(),
            images: vec![],
            model_override: None,
            effort_override: None,
            permission_mode_override: None,
            client_nonce: None,
        })
        .await
        .unwrap();
    provider.stop_session(ThreadId("t-bogus".into())).await.ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dropping_provider_shuts_down_sessions() {
    let provider = provider_with_fixture();
    start_session_resilient(&provider, start_input("t-drop")).await.unwrap();
    let sessions = provider.list_sessions().await.unwrap();
    assert_eq!(sessions.len(), 1);
    drop(provider);
    // Give the Drop-spawned task time to shutdown. The test passes as
    // long as the runtime does not leak children — manual verification
    // only; at minimum we confirm the drop does not panic.
    tokio::time::sleep(Duration::from_millis(300)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auth_probe_installed_returns_none_when_binary_missing() {
    let bogus = PathBuf::from("/nonexistent/codex-zzz-404");
    let res = probe_installed(&bogus).await.unwrap();
    assert!(res.is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn classify_auth_output_pure_function() {
    assert!(matches!(
        classify_auth_output("not logged in"),
        AuthStatus::Unauthenticated { .. }
    ));
    assert!(matches!(
        classify_auth_output(""),
        AuthStatus::Unknown { .. }
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn subagent_lifecycle_spawn_child_thread_items_wait_flows_through_adapter() {
    // A full Codex multi-agent lifecycle, exercised end-to-end through the
    // real adapter + fake app-server:
    //   1. parent turn/started
    //   2. collabAgentToolCall spawnAgent (inProgress) → child registered,
    //      SubagentUpdated (Pending/Running)
    //   3. collabAgentToolCall spawnAgent (completed) with agentsStates
    //      running + activity message → SubagentUpdated Running
    //   4. child thread/started (v2 nested) with nickname/role → identity
    //   5. child turn/started (v2 nested) → SubagentUpdated Running
    //   6. child agentMessage item/completed → ItemCompleted tagged with
    //      subagent_id (the drill-in transcript)
    //   7. subAgentActivity (interacted) → status tick, suppressed raw
    //   8. child turn/completed (v2 nested, durationMs) → SubagentUpdated
    //      Completed with duration
    //   9. collabAgentToolCall wait (completed) agentsStates completed →
    //      SubagentUpdated Completed
    //  10. parent turn/completed → parent turn succeeds
    let script = write_script(json!([
        {"after":"turn/start","delay_ms":5,"emit":"notification","method":"turn/started",
         "params":{"threadId":"c-1","turnId":"pt-1"}},
        {"after":"turn/start","delay_ms":5,"emit":"notification","method":"item/completed",
         "params":{"threadId":"c-1","turnId":"pt-1","item":{
            "type":"collabAgentToolCall","id":"call-1","tool":"spawnAgent","status":"inProgress",
            "senderThreadId":"c-1","receiverThreadIds":["c-child"],"model":"gpt-5.4",
            "prompt":"explore the repo","agentsStates":{"c-child":{"status":"pendingInit"}}}}},
        {"after":"turn/start","delay_ms":5,"emit":"notification","method":"item/completed",
         "params":{"threadId":"c-1","turnId":"pt-1","item":{
            "type":"collabAgentToolCall","id":"call-1","tool":"spawnAgent","status":"completed",
            "senderThreadId":"c-1","receiverThreadIds":["c-child"],"model":"gpt-5.4",
            "prompt":"explore the repo",
            "agentsStates":{"c-child":{"status":"running","message":"reading files"}}}}},
        {"after":"turn/start","delay_ms":5,"emit":"notification","method":"thread/started",
         "params":{"thread":{"id":"c-child","parentThreadId":"c-1","agentNickname":"Explore",
            "agentRole":"explore","cwd":"/tmp","status":"running","turns":[]}}},
        {"after":"turn/start","delay_ms":5,"emit":"notification","method":"turn/started",
         "params":{"threadId":"c-child","turn":{"id":"ct-1","status":"inProgress","items":[]}}},
        {"after":"turn/start","delay_ms":5,"emit":"notification","method":"item/completed",
         "params":{"threadId":"c-child","turnId":"ct-1","item":{
            "type":"agentMessage","id":"cm-1","text":"child found the answer"}}},
        {"after":"turn/start","delay_ms":5,"emit":"notification","method":"item/completed",
         "params":{"threadId":"c-1","turnId":"pt-1","item":{
            "type":"subAgentActivity","id":"sa-1","agentThreadId":"c-child",
            "agentPath":"root/explore","kind":"interacted"}}},
        {"after":"turn/start","delay_ms":5,"emit":"notification","method":"turn/completed",
         "params":{"threadId":"c-child","turn":{"id":"ct-1","status":"completed",
            "durationMs":4321,"items":[]}}},
        {"after":"turn/start","delay_ms":5,"emit":"notification","method":"item/completed",
         "params":{"threadId":"c-1","turnId":"pt-1","item":{
            "type":"collabAgentToolCall","id":"call-2","tool":"wait","status":"completed",
            "senderThreadId":"c-1","receiverThreadIds":["c-child"],
            "agentsStates":{"c-child":{"status":"completed","message":"done"}}}}},
        {"after":"turn/start","delay_ms":5,"emit":"notification","method":"turn/completed",
         "params":{"threadId":"c-1","turnId":"pt-1","status":"succeeded"}}
    ]));
    let wrapper = wrapper_with_env(&[("FAKE_CODEX_SCRIPT", &script.to_string_lossy())]);
    let provider = provider_with_fixture_and_binary(wrapper.to_path_buf());
    start_session_resilient(&provider, start_input("t-sub")).await.unwrap();
    let mut stream = provider.event_stream();
    provider
        .send_turn(SendTurnInput {
            thread_id: ThreadId("t-sub".into()),
            text: "delegate".into(),
            images: vec![],
            model_override: None,
            effort_override: None,
            permission_mode_override: None,
            client_nonce: None,
        })
        .await
        .unwrap();

    let mut saw_child_running = false;
    let mut saw_child_model = false;
    let mut saw_child_name = false;
    let mut saw_child_activity = false;
    let mut saw_child_completed_with_duration = false;
    let mut saw_child_transcript_text = false;
    let mut saw_parent_turn_completed = false;
    // Suppression / isolation invariants:
    let mut saw_collab_tool_render = false; // no ToolUse/ToolResult for collab item
    let mut saw_parent_tagged_delta = false; // parent transcript must stay untagged
    let mut collab_warning = false; // collab/subAgentActivity must not warn

    let collected = timeout(Duration::from_secs(6), async {
        while let Some(ev) = stream.next().await {
            match &ev {
                ProviderRuntimeEvent::SubagentUpdated { subagent, .. } => {
                    assert_eq!(subagent.subagent_id, "c-child");
                    assert_eq!(subagent.provider_ref.as_deref(), Some("c-child"));
                    if subagent.status == SubagentStatus::Running {
                        saw_child_running = true;
                    }
                    if subagent.model.as_deref() == Some("gpt-5.4") {
                        saw_child_model = true;
                    }
                    if subagent.name.as_deref() == Some("Explore") {
                        saw_child_name = true;
                    }
                    if subagent.activity.as_deref() == Some("reading files") {
                        saw_child_activity = true;
                    }
                    if subagent.status == SubagentStatus::Completed
                        && subagent.duration_ms == Some(4321)
                    {
                        saw_child_completed_with_duration = true;
                    }
                }
                ProviderRuntimeEvent::ItemCompleted {
                    item,
                    subagent_id,
                    ..
                } => match item {
                    CompletedItem::AssistantText { text }
                        if text == "child found the answer" =>
                    {
                        assert_eq!(
                            subagent_id.as_deref(),
                            Some("c-child"),
                            "child transcript item must be tagged with its subagent_id"
                        );
                        saw_child_transcript_text = true;
                    }
                    // This scenario emits no ordinary tools; any ToolUse /
                    // ToolResult here would mean a collabAgentToolCall or
                    // subAgentActivity item leaked instead of being suppressed.
                    CompletedItem::ToolUse { .. } | CompletedItem::ToolResult { .. } => {
                        saw_collab_tool_render = true;
                    }
                    _ => {}
                },
                ProviderRuntimeEvent::ContentDelta { subagent_id, .. } => {
                    if subagent_id.is_some() {
                        saw_parent_tagged_delta = true;
                    }
                }
                ProviderRuntimeEvent::TurnCompleted { .. } => {
                    saw_parent_turn_completed = true;
                }
                ProviderRuntimeEvent::RuntimeWarning { message, .. } => {
                    if message.contains("collabAgentToolCall")
                        || message.contains("subAgentActivity")
                    {
                        collab_warning = true;
                    }
                }
                _ => {}
            }
            if saw_child_running
                && saw_child_model
                && saw_child_name
                && saw_child_activity
                && saw_child_completed_with_duration
                && saw_child_transcript_text
                && saw_parent_turn_completed
            {
                break;
            }
        }
    })
    .await;
    assert!(collected.is_ok(), "timed out waiting for subagent lifecycle events");
    assert!(saw_child_running, "missing Running SubagentUpdated");
    assert!(saw_child_model, "missing model gpt-5.4 on a subagent snapshot");
    assert!(saw_child_name, "missing nickname Explore from child thread/started");
    assert!(saw_child_activity, "missing agentsStates activity line");
    assert!(
        saw_child_completed_with_duration,
        "missing Completed SubagentUpdated with durationMs 4321"
    );
    assert!(
        saw_child_transcript_text,
        "missing child agentMessage routed into the drill-in transcript"
    );
    assert!(saw_parent_turn_completed, "missing parent TurnCompleted");
    assert!(
        !saw_collab_tool_render,
        "collabAgentToolCall / subAgentActivity items must be suppressed, not rendered as tools"
    );
    assert!(
        !saw_parent_tagged_delta,
        "parent transcript deltas must never be tagged with a subagent_id"
    );
    assert!(
        !collab_warning,
        "collab / subAgentActivity items must not fall through to RuntimeWarning"
    );
    provider.stop_session(ThreadId("t-sub".into())).await.ok();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_during_event_streaming_does_not_panic() {
    let script = write_script(json!([
        {"after":"turn/start","delay_ms":5,"emit":"notification","method":"turn/started",
         "params":{"threadId":"c-1","turnId":"t-race"}},
        {"after":"turn/start","delay_ms":20,"emit":"notification",
         "method":"item/agentMessage/delta",
         "params":{"threadId":"c-1","turnId":"t-race","itemId":"i-r","delta":"."}}
    ]));
    let wrapper = wrapper_with_env(&[
        ("FAKE_CODEX_SCRIPT", &script.to_string_lossy()),
    ]);
    let provider = provider_with_fixture_and_binary(wrapper.to_path_buf());
    start_session_resilient(&provider, start_input("t-race")).await.unwrap();
    // Start a turn that will emit events mid-flight, then stop quickly.
    provider
        .send_turn(SendTurnInput {
            thread_id: ThreadId("t-race".into()),
            text: "go".into(),
            images: vec![],
            model_override: None,
            effort_override: None,
            permission_mode_override: None,
            client_nonce: None,
        })
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(5)).await;
    provider.stop_session(ThreadId("t-race".into())).await.unwrap();
    // If we get here without panic, we're good.
}

