//! Attempt-scoped JSON-RPC bridge for native harness sidecars.
//!
//! Provider authentication and configuration remain in each sidecar. This
//! transport captures tool authority, checks readiness before any prompt and
//! verifies the entire owned process group has stopped before releasing it.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::{broadcast, Mutex};

use super::managed::ManagedSession;
use super::{
    child_exit_events, ContentDelta, ProviderError, ProviderEventStream, ProviderKind,
    ProviderRuntimeEvent, ProviderSession, ProviderSessionId, SendTurnInput, SessionStatus,
    StartSessionInput, ThreadId, TurnId, TurnStartResult, TurnStatus, TurnUsage,
};
use crate::json_rpc_child::{JsonRpcChild, RpcChildError, RpcError, SpawnConfig};

pub const SIDECAR_PATH_ENV: &str = "CODEMUX_MANAGED_WORKFLOW_SIDECAR_PATH";

fn pre_spawn_rejected(message: &str) -> ProviderError {
    ProviderError::ValidationError {
        message: format!("managed-start-rejected: setup_required: {message}"),
    }
}

/// Managed execution cannot recover a lost terminal event by guessing from prose.
pub fn event_stream(
    sender: &broadcast::Sender<ProviderRuntimeEvent>,
    thread_id: &ThreadId,
) -> ProviderEventStream {
    let receiver = sender.subscribe();
    let thread_id = thread_id.clone();
    Box::pin(futures_util::stream::unfold(
        (receiver, thread_id),
        |(mut receiver, thread_id)| async move {
            match receiver.recv().await {
                Ok(event) => Some((event, (receiver, thread_id))),
                Err(broadcast::error::RecvError::Closed) => None,
                Err(broadcast::error::RecvError::Lagged(count)) => Some((
                    ProviderRuntimeEvent::RuntimeWarning {
                        thread_id: Some(thread_id.clone()),
                        message: format!("Managed event stream lagged by {count} events"),
                        original_payload: None,
                    },
                    (receiver, thread_id),
                )),
            }
        },
    ))
}

pub fn resolve_sidecar(resource_dir: Option<&Path>) -> Option<PathBuf> {
    if let Some(path) = std::env::var_os(SIDECAR_PATH_ENV).filter(|value| !value.is_empty()) {
        return Some(path.into());
    }
    let suffix = if cfg!(windows) { ".exe" } else { "" };
    let name = format!(
        "codemux-managed-workflow-sidecar-{}{suffix}",
        super::claude::sidecar_path::target_triple()
    );
    if let Some(resources) = resource_dir {
        let path = resources.join("binaries").join(&name);
        if path.is_file() {
            return Some(path);
        }
    }
    #[cfg(debug_assertions)]
    {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("binaries")
            .join(name);
        if path.is_file() {
            return Some(path);
        }
    }
    None
}

pub fn sidecar_path(provider: ProviderKind) -> Result<PathBuf, ProviderError> {
    resolve_sidecar(None).ok_or_else(|| ProviderError::ValidationError {
        message: format!("managed-start-rejected: setup_required: {provider:?} workflow sidecar is missing. Rebuild CodeMux with scripts/build-managed-workflow-sidecar.sh."),
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Ready {
    protocol_version: u32,
    provider: ProviderKind,
    adapter_version: String,
    session_id: String,
    tools: Vec<String>,
    isolation: Isolation,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Isolation {
    native_tools: Vec<String>,
    native_fanout: bool,
    ambient_config: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Tokens {
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    cache_write_tokens: u64,
    #[serde(default)]
    reasoning_tokens: u64,
    total_tokens: Option<u64>,
}

fn parse_tokens(value: Value) -> Result<Tokens, String> {
    let tokens: Tokens =
        serde_json::from_value(value).map_err(|_| "Invalid managed token counters")?;
    let total = tokens
        .input_tokens
        .checked_add(tokens.output_tokens)
        .and_then(|value| value.checked_add(tokens.cache_read_tokens))
        .and_then(|value| value.checked_add(tokens.cache_write_tokens))
        .ok_or("Managed token count overflow")?;
    if tokens.reasoning_tokens > tokens.output_tokens
        || tokens
            .total_tokens
            .is_some_and(|reported| reported != total)
    {
        return Err("Managed token counters violate non-overlapping usage accounting".into());
    }
    Ok(tokens)
}

fn validate_ready(
    value: Value,
    provider: ProviderKind,
    adapter: &str,
    expected_tools: &[String],
) -> Result<Ready, String> {
    let ready: Ready = serde_json::from_value(value)
        .map_err(|_| "Invalid managed sidecar readiness acknowledgment")?;
    let mut actual = ready.tools.clone();
    let mut expected = expected_tools.to_vec();
    actual.sort();
    expected.sort();
    if ready.protocol_version != 1
        || ready.provider != provider
        || ready.adapter_version != adapter
        || ready.session_id.is_empty()
        || ready.session_id.len() > 256
        || actual != expected
        || !ready.isolation.native_tools.is_empty()
        || ready.isolation.native_fanout
        || ready.isolation.ambient_config
    {
        return Err(
            "Managed sidecar did not confirm the required tool catalog, version and isolation"
                .into(),
        );
    }
    Ok(ready)
}

pub struct ManagedBridgeSession {
    pub thread_id: ThreadId,
    pub provider_session_id: ProviderSessionId,
    pub provider: ProviderKind,
    child: Arc<JsonRpcChild>,
    context: Arc<ManagedSession>,
    active: Arc<Mutex<Option<TurnId>>>,
    stopping: Arc<AtomicBool>,
    state_dir: PathBuf,
    event_tx: broadcast::Sender<ProviderRuntimeEvent>,
}

impl ManagedBridgeSession {
    pub async fn spawn(
        input: StartSessionInput,
        provider: ProviderKind,
        expected_adapter_version: &str,
        context: Arc<ManagedSession>,
        config: SpawnConfig,
        mut provider_options: Value,
        event_tx: broadcast::Sender<ProviderRuntimeEvent>,
    ) -> Result<Arc<Self>, ProviderError> {
        if !cfg!(target_os = "linux")
            || input.resume_cursor.is_some()
            || !input.additional_directories.is_empty()
        {
            return Err(ProviderError::ValidationError { message: "managed-start-rejected: managed attempts require Linux, fresh sessions and a single owned workspace".into() });
        }
        let tools = context.handler.tools();
        let names: Vec<_> = tools.iter().map(|tool| tool.name.clone()).collect();
        if tools.is_empty() || tools.len() > 64 {
            return Err(ProviderError::ValidationError {
                message: "managed-start-rejected: invalid managed tool catalog".into(),
            });
        }
        let options =
            provider_options
                .as_object_mut()
                .ok_or_else(|| ProviderError::ValidationError {
                    message: "managed-start-rejected: provider options must be an object".into(),
                })?;
        if config.program.is_absolute() {
            let metadata = tokio::fs::metadata(&config.program)
                .await
                .map_err(|_| pre_spawn_rejected("Managed workflow executable is unavailable"))?;
            if !metadata.is_file() {
                return Err(pre_spawn_rejected(
                    "Managed workflow executable is not a file",
                ));
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if metadata.permissions().mode() & 0o111 == 0 {
                    return Err(pre_spawn_rejected(
                        "Managed workflow executable lacks execute permission",
                    ));
                }
            }
        }
        let state_dir =
            std::env::temp_dir().join(format!("codemux-managed-attempt-{}", uuid::Uuid::new_v4()));
        tokio::fs::create_dir(&state_dir)
            .await
            .map_err(|_| pre_spawn_rejected("Cannot create owned attempt state"))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if tokio::fs::set_permissions(&state_dir, std::fs::Permissions::from_mode(0o700))
                .await
                .is_err()
            {
                let _ = tokio::fs::remove_dir(&state_dir).await;
                return Err(pre_spawn_rejected("Cannot secure owned attempt state"));
            }
        }
        options.insert("stateDir".into(), json!(state_dir));
        let child = match JsonRpcChild::spawn_managed(config).await {
            Ok(child) => Arc::new(child),
            Err(error) => {
                let _ = tokio::fs::remove_dir_all(&state_dir).await;
                // OS spawn errors can only originate from Command::spawn.
                // Stdio capture failures are Error::other without an OS errno
                // after a child exists.
                // Retain uncertain failures for reconciliation rather than
                // treating every transport error as proof of no execution.
                if matches!(&error, RpcChildError::SpawnFailed(source) if source.raw_os_error().is_some() || matches!(source.kind(), std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied))
                {
                    return Err(pre_spawn_rejected(
                        "Managed workflow executable could not start",
                    ));
                }
                return Err(ProviderError::ProcessError {
                    message: "managed-start-rejected: failed to spawn managed workflow sidecar"
                        .into(),
                    source: Some(error.to_string()),
                });
            }
        };
        let recorded = child
            .managed_evidence()
            .map_err(|error| error.to_string())
            .and_then(|evidence| context.handler.record_runtime(evidence));
        if let Err(error) = recorded {
            Self::reject_start(&child, &context, &state_dir, error).await?;
            unreachable!("reject_start always returns an error");
        }
        // Subscribe before initialize so early child failures cannot disappear.
        let notifications = child.notifications();
        let incoming = child
            .incoming_requests()
            .ok_or_else(|| ProviderError::ProcessError {
                message: "managed incoming request stream unavailable".into(),
                source: None,
            });
        let mut incoming = match incoming {
            Ok(incoming) => incoming,
            Err(error) => {
                Self::reject_start(&child, &context, &state_dir, error.to_string()).await?;
                unreachable!();
            }
        };
        let active: Arc<Mutex<Option<TurnId>>> = Arc::new(Mutex::new(None));
        let stopping = Arc::new(AtomicBool::new(false));
        let weak_child = Arc::downgrade(&child);
        let tool_context = Arc::clone(&context);
        let tool_active = Arc::clone(&active);
        let tool_stopping = Arc::clone(&stopping);
        // Reject requests arriving during initialization immediately. Deferring
        // this queue until after readiness could admit an old request on the
        // first active turn.
        tokio::spawn(async move {
            while let Some(request) = incoming.recv().await {
                let Some(child) = weak_child.upgrade() else {
                    break;
                };
                let result =
                    if tool_stopping.load(Ordering::SeqCst) || tool_active.lock().await.is_none() {
                        Err("Managed attempt has no authorized active turn".to_string())
                    } else if request.method != "managed/tool_call" {
                        Err("Unknown managed host method".to_string())
                    } else {
                        let name = request.params.get("name").and_then(Value::as_str);
                        let arguments = request
                            .params
                            .get("arguments")
                            .filter(|value| value.is_object());
                        match (name, arguments) {
                            (Some(name), Some(arguments)) => {
                                tool_context.call(name, arguments.clone()).await
                            }
                            _ => Err("Invalid managed tool call".into()),
                        }
                    };
                let response = result.map_err(|message| RpcError {
                    code: -32000,
                    message,
                    data: None,
                });
                if child.respond(request.id, response).await.is_err() {
                    break;
                }
            }
        });
        let value = child.request("initialize", json!({
            "provider": provider, "cwd": input.cwd, "model": input.model, "effort": input.effort,
            "tools": tools.iter().map(|tool| json!({"name":tool.name,"description":tool.description,"inputSchema":tool.input_schema})).collect::<Vec<_>>(),
            "options": provider_options,
        })).await;
        let ready = match value
            .map_err(|error| error.to_string())
            .and_then(|value| validate_ready(value, provider, expected_adapter_version, &names))
        {
            Ok(ready) => ready,
            Err(error) => {
                stopping.store(true, Ordering::SeqCst);
                Self::reject_start(&child, &context, &state_dir, error).await?;
                unreachable!();
            }
        };
        let session = Arc::new(Self {
            thread_id: input.thread_id,
            provider_session_id: ProviderSessionId(ready.session_id),
            provider,
            child,
            context,
            active,
            stopping,
            state_dir,
            event_tx,
        });
        let weak = Arc::downgrade(&session);
        tokio::spawn(async move {
            let mut notifications = notifications;
            loop {
                let notification =
                    tokio::time::timeout(Duration::from_millis(250), notifications.recv()).await;
                let Some(session) = weak.upgrade() else {
                    break;
                };
                if session.stopping.load(Ordering::SeqCst) {
                    break;
                }
                match notification {
                    Ok(Ok(notification)) => {
                        if let Err(error) = session
                            .notification(&notification.method, notification.params)
                            .await
                        {
                            session.fail(error).await;
                            break;
                        }
                    }
                    Ok(Err(_)) => {
                        session
                            .fail("Managed notification stream was interrupted".into())
                            .await;
                        break;
                    }
                    Err(_) if !session.child.is_alive() => {
                        session.fail("Managed workflow sidecar exited".into()).await;
                        break;
                    }
                    Err(_) => {}
                }
            }
        });
        let _ = session
            .event_tx
            .send(ProviderRuntimeEvent::SessionConfigured {
                thread_id: session.thread_id.clone(),
                provider_session_id: session.provider_session_id.clone(),
            });
        Ok(session)
    }

    async fn reject_start(
        child: &JsonRpcChild,
        context: &ManagedSession,
        state_dir: &Path,
        error: String,
    ) -> Result<(), ProviderError> {
        child
            .shutdown_managed()
            .await
            .map_err(|stop| ProviderError::ProcessError {
                message: "Managed startup stop unconfirmed".into(),
                source: Some(stop.to_string()),
            })?;
        context
            .handler
            .quiesce()
            .await
            .map_err(|error| ProviderError::ProcessError {
                message: "Managed startup host tools remain active".into(),
                source: Some(error),
            })?;
        let _ = tokio::fs::remove_dir_all(state_dir).await;
        Err(ProviderError::ValidationError {
            message: format!("managed-start-rejected: {error}"),
        })
    }

    pub fn is_dead(&self) -> bool {
        self.stopping.load(Ordering::SeqCst) || !self.child.is_alive()
    }

    pub async fn turn_active(&self) -> bool {
        !self.is_dead() && self.active.lock().await.is_some()
    }

    pub async fn provider_session(&self) -> ProviderSession {
        ProviderSession {
            thread_id: self.thread_id.clone(),
            provider: self.provider,
            session_id: self.provider_session_id.clone(),
            status: if self.is_dead() {
                SessionStatus::Closed
            } else {
                SessionStatus::Ready
            },
            resume_cursor: None,
        }
    }

    pub async fn send_turn(&self, input: SendTurnInput) -> Result<TurnStartResult, ProviderError> {
        if input.thread_id != self.thread_id
            || self.is_dead()
            || !input.images.is_empty()
            || !input.skill_invocations.is_empty()
            || input.model_override.is_some()
            || input.effort_override.is_some()
            || input.permission_mode_override.is_some()
        {
            return Err(ProviderError::ValidationError {
                message: "Managed workflow turn cannot change its captured configuration".into(),
            });
        }
        let turn_id = TurnId(uuid::Uuid::new_v4().to_string());
        {
            let mut active = self.active.lock().await;
            if active.is_some() {
                return Err(ProviderError::ValidationError {
                    message: "Managed workflow attempt already has an active turn".into(),
                });
            }
            *active = Some(turn_id.clone());
        }
        if let Some(checkpoint) = &input.turn_checkpoint {
            checkpoint.prepare().await;
        }
        if let Err(error) = self
            .child
            .request("startTurn", json!({"turnId":turn_id.0,"prompt":input.text}))
            .await
        {
            self.fail(error.to_string()).await;
            return Err(ProviderError::RpcError {
                message: error.to_string(),
            });
        }
        Ok(TurnStartResult {
            steered: false,
            turn_id,
            queued_id: None,
        })
    }

    pub async fn interrupt(&self) -> Result<(), ProviderError> {
        self.child
            .request("cancel", json!({}))
            .await
            .map_err(|error| ProviderError::RpcError {
                message: error.to_string(),
            })?;
        Ok(())
    }

    pub async fn shutdown_managed(&self) -> Result<(), ProviderError> {
        self.stopping.store(true, Ordering::SeqCst);
        self.child
            .shutdown_managed()
            .await
            .map_err(|error| ProviderError::ProcessError {
                message: "Managed process group stop unconfirmed".into(),
                source: Some(error.to_string()),
            })?;
        self.context
            .handler
            .quiesce()
            .await
            .map_err(|error| ProviderError::ProcessError {
                message: "Managed host tools remain active".into(),
                source: Some(error),
            })?;
        *self.active.lock().await = None;
        let _ = tokio::fs::remove_dir_all(&self.state_dir).await;
        Ok(())
    }

    async fn fail(&self, message: String) {
        // Emit a terminal error immediately; the executor separately requires a
        // verified shutdown before releasing ownership or retrying an attempt.
        self.stopping.store(true, Ordering::SeqCst);
        let active = self.active.lock().await.take();
        for event in child_exit_events(self.thread_id.clone(), active, message) {
            let _ = self.event_tx.send(event);
        }
    }

    async fn notification(&self, method: &str, params: Value) -> Result<(), String> {
        let turn_id = params
            .get("turnId")
            .and_then(Value::as_str)
            .map(|value| TurnId(value.into()));
        let mut active = self.active.lock().await;
        if turn_id.as_ref() != active.as_ref() || active.is_none() {
            return Err("Managed notification had an unexpected turn id".into());
        }
        let turn_id = turn_id.ok_or("Managed notification omitted its turn id")?;
        match method {
            "text" => {
                let text = params
                    .get("text")
                    .and_then(Value::as_str)
                    .ok_or("Invalid managed text notification")?;
                let _ = self.event_tx.send(ProviderRuntimeEvent::ContentDelta {
                    thread_id: self.thread_id.clone(),
                    turn_id,
                    delta: ContentDelta::Text { text: text.into() },
                    subagent_id: None,
                });
            }
            "complete" => {
                let status = match params.get("status").and_then(Value::as_str) {
                    Some("success") => TurnStatus::Success,
                    Some("cancelled") => TurnStatus::Error {
                        subtype: "interrupted".into(),
                        message: "Managed turn cancelled".into(),
                    },
                    Some("error") => TurnStatus::Error {
                        subtype: "managed_provider_error".into(),
                        message: params
                            .get("message")
                            .and_then(Value::as_str)
                            .unwrap_or("Managed turn failed")
                            .into(),
                    },
                    _ => return Err("Invalid managed terminal status".into()),
                };
                let usage = if let Some(usage) = params.get("usage") {
                    let duration_ms = usage
                        .get("durationMs")
                        .and_then(Value::as_u64)
                        .ok_or("Invalid managed duration")?;
                    let num_turns = usage
                        .get("numTurns")
                        .and_then(Value::as_u64)
                        .and_then(|value| u32::try_from(value).ok())
                        .ok_or("Invalid managed turn count")?;
                    let total_cost_usd = match usage.get("totalCostUsd") {
                        None | Some(Value::Null) => None,
                        Some(value) => Some(
                            value
                                .as_f64()
                                .filter(|value| value.is_finite() && *value >= 0.0)
                                .ok_or("Invalid managed cost")?,
                        ),
                    };
                    if let Some(tokens) = usage.get("tokens") {
                        let tokens = parse_tokens(tokens.clone())?;
                        let model = match usage.get("model") {
                            None | Some(Value::Null) => None,
                            Some(Value::String(model)) => Some(model.clone()),
                            _ => return Err("Invalid managed usage model".into()),
                        };
                        let _ = self.event_tx.send(ProviderRuntimeEvent::UsageRecorded {
                            thread_id: self.thread_id.clone(),
                            provider: self.provider,
                            model,
                            subagent: false,
                            input_tokens: tokens.input_tokens,
                            output_tokens: tokens.output_tokens,
                            cache_read_tokens: tokens.cache_read_tokens,
                            cache_write_tokens: tokens.cache_write_tokens,
                            reasoning_tokens: tokens.reasoning_tokens,
                            cost_usd: None,
                            cost_source: None,
                        });
                    }
                    Some(TurnUsage {
                        total_cost_usd,
                        duration_ms,
                        num_turns,
                    })
                } else {
                    None
                };
                *active = None;
                let _ = self.event_tx.send(ProviderRuntimeEvent::TurnCompleted {
                    thread_id: self.thread_id.clone(),
                    turn_id,
                    status,
                    usage,
                });
            }
            _ => return Err("Unknown managed notification".into()),
        }
        Ok(())
    }
}

impl Drop for ManagedBridgeSession {
    fn drop(&mut self) {
        let child = Arc::clone(&self.child);
        let state_dir = self.state_dir.clone();
        let context = Arc::clone(&self.context);
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(async move {
                if child.shutdown_managed().await.is_ok() && context.handler.quiesce().await.is_ok()
                {
                    let _ = tokio::fs::remove_dir_all(state_dir).await;
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ack() -> Value {
        json!({"protocolVersion":1,"provider":"cursor","adapterVersion":"cursor-sdk-1.0.37","sessionId":"fresh","tools":["workflow_result"],"isolation":{"nativeTools":[],"nativeFanout":false,"ambientConfig":false}})
    }
    #[test]
    fn managed_bridge_rejects_catalog_version_or_isolation_mismatch() {
        let names = vec!["workflow_result".to_string()];
        assert!(validate_ready(ack(), ProviderKind::Cursor, "cursor-sdk-1.0.37", &names).is_ok());
        for (pointer, value) in [
            ("/protocolVersion", json!(2)),
            ("/provider", json!("codex")),
            ("/adapterVersion", json!("unreviewed")),
            ("/sessionId", json!("")),
            ("/tools", json!(["workflow_result", "shell"])),
            ("/tools", json!(["workflow_result", "workflow_result"])),
            ("/isolation/nativeTools", json!(["edit"])),
            ("/isolation/nativeFanout", json!(true)),
            ("/isolation/ambientConfig", json!(true)),
        ] {
            let mut value_ack = ack();
            *value_ack.pointer_mut(pointer).unwrap() = value;
            assert!(
                validate_ready(value_ack, ProviderKind::Cursor, "cursor-sdk-1.0.37", &names)
                    .is_err(),
                "{pointer}"
            );
        }
    }

    #[tokio::test]
    async fn managed_bridge_event_stream_reports_lag_for_its_captured_thread() {
        use futures_util::StreamExt;
        let (sender, _) = broadcast::channel(2);
        let id = ThreadId("owned-thread".into());
        let mut events = event_stream(&sender, &id);
        for _ in 0..10 {
            let _ = sender.send(ProviderRuntimeEvent::SessionStateChanged {
                thread_id: id.clone(),
                status: SessionStatus::Ready,
            });
        }
        assert!(
            matches!(events.next().await, Some(ProviderRuntimeEvent::RuntimeWarning { thread_id: Some(captured), message, .. })
            if captured == id && message.starts_with("Managed event stream lagged"))
        );
    }

    #[test]
    fn managed_bridge_token_usage_is_disjoint_and_never_infers_a_bill() {
        assert!(parse_tokens(json!({"inputTokens":2,"outputTokens":3,"cacheReadTokens":5,"cacheWriteTokens":7,"reasoningTokens":1,"totalTokens":17})).is_ok());
        for value in [
            json!({"inputTokens":2,"outputTokens":3,"cacheReadTokens":5,"cacheWriteTokens":7,"totalTokens":10}),
            json!({"inputTokens":2,"outputTokens":3,"cacheReadTokens":5,"cacheWriteTokens":7,"reasoningTokens":4}),
            json!({"inputTokens":-1,"outputTokens":3,"cacheReadTokens":5,"cacheWriteTokens":7}),
        ] {
            assert!(parse_tokens(value).is_err());
        }
    }
}
