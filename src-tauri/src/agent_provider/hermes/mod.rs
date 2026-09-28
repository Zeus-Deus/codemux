//! Official Hermes ACP integration. Profile ownership and native history stay in Hermes.
//! No Cursor/Grok authentication, fallback, full-access policy, or idle process reaping.
pub mod binding;
pub mod profile;

use super::*;
use crate::json_rpc_child::{IncomingRequest, JsonRpcChild, Notification, RpcError, SpawnConfig};
use async_trait::async_trait;
use binding::{Binding, BindingStore};
use profile::Profile;
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::Arc,
    time::Duration,
};
use tokio::sync::{broadcast, mpsc, oneshot, Mutex, Notify};
use uuid::Uuid;

/// Serializes committing a cleanup hold with every Codemux worktree deletion surface.
/// Released before launching/waiting on a runtime; foreground queues remain independent.
pub(crate) static WORKTREE_LIFECYCLE: Mutex<()> = Mutex::const_new(());

fn invalid(message: impl Into<String>) -> ProviderError {
    ProviderError::ValidationError {
        message: message.into(),
    }
}
fn rpc(error: impl std::fmt::Display) -> ProviderError {
    ProviderError::RpcError {
        message: format!("Hermes ACP: {error}"),
    }
}
fn text<'a>(v: &'a Value, key: &str) -> Option<&'a str> {
    v.get(key).and_then(Value::as_str)
}
const LIMITS: &str = "Reasoning control unavailable in this Hermes adapter. Background learning has no completion acknowledgment; worktree cleanup remains pending. Desktop can read history, but execution handoff is unsupported.";

/// Deliberately conservative: the catalog has no unambiguous provider metadata in 0.21.3.
/// A custom ID containing further colons may be named-provider routing OR an opaque model;
/// both stay unavailable until a regression test can distinguish them safely.
pub fn durable_model(id: &str) -> Result<(), ProviderError> {
    if id.strip_prefix("custom:").is_some_and(|s| s.contains(':')) {
        return Err(invalid("unsupported: this custom model route cannot be certified for native restart/resume. Configure a supported route in Hermes; no credentials or history will be rewritten."));
    }
    if id.is_empty() {
        return Err(invalid(
            "setup_required: Hermes did not report a configured model",
        ));
    }
    Ok(())
}

/// Hermes 0.21.3 ACP keeps api_mode when switching within a provider. Its
/// currentModelId still changes, so ID confirmation cannot detect a wrong wire
/// protocol (observed DeepSeek -> Muse sending Muse to chat/completions).
/// Keep native history untouched and reject this known incompatible transition.
fn durable_model_change(current: Option<&str>, next: &str) -> Result<(), ProviderError> {
    fn free_api(id: &str) -> Option<&'static str> {
        let model = id.strip_prefix("opencode-free:")?;
        Some(if ["muse-spark", "gpt-", "grok-"].iter().any(|p| model.starts_with(p)) {
            "responses"
        } else if ["claude-", "qwen"].iter().any(|p| model.starts_with(p)) {
            "messages"
        } else {
            "chat/completions"
        })
    }
    if current.and_then(free_api).zip(free_api(next)).is_some_and(|(a, b)| a != b) {
        return Err(invalid("unsupported: official Hermes ACP retains the previous API protocol when switching within OpenCode Free. Switching between these model families cannot be certified; no replacement conversation or history rewrite was attempted."));
    }
    Ok(())
}

/// Routing-only compatibility inspection, separate from layout discovery. Never
/// expose/configure credentials or inspect native history, memories or skills.
/// ACP 0.21.3 flattens a named provider into `custom:MODEL` and may advertise
/// aliases. Until it supplies durable routing metadata, profiles declaring named
/// endpoint inventories are disabled as a whole (including apparently bare IDs).
fn durable_profile(profile: &Profile) -> Result<(), ProviderError> {
    #[derive(serde::Deserialize, Default)]
    struct Routing {
        #[serde(default)]
        providers: HashMap<String, serde::de::IgnoredAny>,
        #[serde(default)]
        custom_providers: Vec<serde::de::IgnoredAny>,
        #[serde(default)]
        model: ModelRouting,
    }
    #[derive(serde::Deserialize, Default)]
    struct ModelRouting {
        #[serde(default)]
        provider: String,
    }
    let config = std::fs::File::open(profile.home.join("config.yaml"))
        .map_err(|_| invalid("setup_required: cannot verify Hermes routing configuration"))?;
    let route: Routing = serde_yaml::from_reader(config)
        .map_err(|_| invalid("unsupported: cannot classify this Hermes routing configuration"))?;
    if !route.providers.is_empty()
        || !route.custom_providers.is_empty()
        || route.model.provider.starts_with("custom:")
    {
        return Err(invalid("unsupported: profiles with named provider routing or endpoint inventories cannot be certified for native restart, even when Hermes reports a bare model ID or alias. Configure a supported profile in Hermes; no credentials or history will be rewritten."));
    }
    Ok(())
}

pub fn durable_catalog(response: &Value) -> Result<(), ProviderError> {
    let current = response
        .pointer("/models/currentModelId")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("unsupported: Hermes did not report its effective model"))?;
    durable_model(current)?;
    // Fail closed on catalog aliases as well; IDs are not proof of effective routing.
    for model in response
        .pointer("/models/availableModels")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if text(model, "modelId")
            .is_some_and(|id| id.starts_with("custom:") && id[7..].contains(':'))
        {
            return Err(invalid(
                "unsupported: ambiguous named custom routes in Hermes catalog",
            ));
        }
    }
    Ok(())
}

struct Pending {
    rpc_id: Value,
    options: Vec<Value>,
    turn: TurnId,
}
struct Chat {
    binding: Mutex<Binding>,
    runtime: Arc<Runtime>,
    state: Mutex<ChatState>,
    finished: Notify,
}
#[derive(Default)]
struct ChatState {
    active: Option<TurnId>,
    cancelled: bool,
    cancel_epoch: u64,
    replaying: bool,
    closed: bool,
    error: Option<String>,
    pending: HashMap<RequestId, Pending>,
    seen_tools: HashSet<String>,
    models: HashSet<String>,
    modes: HashSet<String>,
    prose: String,
    thoughts: String,
}
struct Job {
    id: String,
    cancel_epoch: u64,
    chat: Arc<Chat>,
    input: SendTurnInput,
}
#[derive(Default)]
struct Queue {
    jobs: VecDeque<Job>,
    running: bool,
}
struct Runtime {
    child: JsonRpcChild,
    ownership: std::sync::Mutex<Option<std::fs::File>>,
    installation_stamp: String,
    profile: Profile,
    /// Shared foreground gate also protects new/load and model changes.
    operation: Mutex<()>,
    routes: Mutex<HashMap<String, std::sync::Weak<Chat>>>,
    queue: Mutex<Queue>,
    barrier: mpsc::UnboundedSender<oneshot::Sender<()>>,
}
struct Inner {
    runtimes: Mutex<HashMap<String, Arc<Runtime>>>,
    chats: Mutex<HashMap<ThreadId, Arc<Chat>>>,
    starts: Mutex<HashMap<ThreadId, Arc<Mutex<()>>>>,
    runtime_starts: Mutex<HashMap<String, Arc<Mutex<()>>>>,
    store: Arc<dyn BindingStore>,
    events: broadcast::Sender<ProviderRuntimeEvent>,
}
pub struct HermesProvider {
    inner: Arc<Inner>,
}
impl HermesProvider {
    pub fn new(store: Arc<dyn BindingStore>) -> Self {
        let (events, _) = broadcast::channel(2048);
        Self {
            inner: Arc::new(Inner {
                runtimes: Mutex::new(HashMap::new()),
                chats: Mutex::new(HashMap::new()),
                starts: Mutex::new(HashMap::new()),
                runtime_starts: Mutex::new(HashMap::new()),
                store,
                events,
            }),
        }
    }
    pub fn validate_intent(
        &self,
        thread: &str,
        model: Option<&str>,
        mode: Option<&str>,
    ) -> Result<(), ProviderError> {
        let binding = self
            .inner
            .store
            .load(thread)
            .map_err(invalid)?
            .ok_or_else(|| invalid("repair_required: Hermes binding is missing"))?;
        binding.profile.validate().map_err(invalid)?;
        durable_profile(&binding.profile)?;
        if let Some(model) = model {
            durable_model(model)?;
            durable_model_change(binding.resolved_model.as_deref(), model)?;
            if model == "profile_default" && binding.model_override.is_some() {
                return Err(invalid(
                    "unsupported: cannot reset an existing Hermes conversation to profile default",
                ));
            }
        }
        if mode.is_some_and(|mode| !matches!(mode, "default" | "accept_edits" | "dont_ask")) {
            return Err(invalid("unsupported: unknown Hermes edit policy"));
        }
        Ok(())
    }
    async fn chat(&self, id: &ThreadId) -> Result<Arc<Chat>, ProviderError> {
        self.inner
            .chats
            .lock()
            .await
            .get(id)
            .cloned()
            .ok_or_else(|| ProviderError::SessionNotFound {
                thread_id: id.clone(),
            })
    }
    /// Discovery creates an unprompted native session through the same owned runtime.
    /// Hermes has no standalone model-catalog RPC. It is not imported as a Codemux chat.
    pub async fn catalog(&self, profile: Profile) -> Result<Value, ProviderError> {
        profile.validate().map_err(invalid)?;
        durable_profile(&profile)?;
        let runtime = self.inner.runtime(profile).await?;
        let _guard = runtime.operation.lock().await;
        runtime
            .child
            .request(
                "session/new",
                json!({"cwd":runtime.profile.home,"mcpServers":[]}),
            )
            .await
            .map_err(rpc)
    }
    pub async fn disconnect(&self, profile: Profile) -> Result<(), ProviderError> {
        let key = runtime_key(&profile);
        let creation = self
            .inner
            .runtime_starts
            .lock()
            .await
            .entry(key.clone())
            .or_default()
            .clone();
        let _creation = creation.lock().await;
        let runtime = self.inner.runtimes.lock().await.get(&key).cloned();
        if let Some(runtime) = runtime {
            if runtime.queue.lock().await.running {
                return Err(invalid(
                    "Hermes has active or queued foreground work. Cancel it before disconnecting.",
                ));
            }
            let _operation = runtime.operation.lock().await;
            // Explicit disconnect releases ownership; it does not assert a background flush.
            runtime.child.shutdown().await.map_err(rpc)?;
            runtime.ownership.lock().unwrap().take();
            self.inner.runtimes.lock().await.remove(&key);
        }
        Ok(())
    }
}
fn runtime_key(p: &Profile) -> String {
    format!(
        "{}\0{}\0{}",
        p.host,
        p.installation.display(),
        p.home.display()
    )
}
impl Inner {
    fn emit(&self, event: ProviderRuntimeEvent) {
        let _ = self.events.send(event);
    }
    async fn runtime(self: &Arc<Self>, profile: Profile) -> Result<Arc<Runtime>, ProviderError> {
        let key = runtime_key(&profile);
        let creation = self
            .runtime_starts
            .lock()
            .await
            .entry(key.clone())
            .or_default()
            .clone();
        let _creation = creation.lock().await;
        if let Some(runtime) = self.runtimes.lock().await.get(&key).cloned() {
            if runtime.child.is_alive() {
                if runtime.profile != profile {
                    return Err(invalid("repair_required: profile identity changed while its runtime is active; disconnect the original profile before repairing it"));
                }
                if runtime.installation_stamp != installation_stamp(&profile)? {
                    return Err(invalid("repair_required: Hermes installation changed; disconnect this profile and refresh to probe the new runtime"));
                }
                return Ok(runtime.clone());
            }
            runtime.ownership.lock().unwrap().take();
        }
        let ownership = claim_profile(&profile)?;
        let installation_stamp = installation_stamp(&profile)?;
        // Do not overlay caller credentials, disable configured MCP, or mutate active_profile.
        let child = JsonRpcChild::spawn(SpawnConfig {
            program: profile.installation.clone(), args: vec!["--profile".into(), profile.id.clone(), "acp".into()],
            env: HashMap::from([("HERMES_HOME".into(), profile.root.to_string_lossy().into_owned())]),
            cwd: Some(profile.home.clone()), default_timeout: Duration::from_secs(60),
        }).await.map_err(|e| invalid(format!("setup_required: launch official Hermes with ACP dependencies (`hermes acp --check`): {e}")))?;
        let mut notifications = child.notifications();
        let mut requests = child
            .incoming_requests()
            .ok_or_else(|| invalid("Hermes request routing unavailable"))?;
        let initialized = child.request("initialize", json!({"protocolVersion":1,"clientInfo":{"name":"codemux","version":env!("CARGO_PKG_VERSION")},"clientCapabilities":{}})).await;
        match initialized {
            Ok(v)
                if v.get("protocolVersion").and_then(Value::as_u64) == Some(1)
                    && v.pointer("/agentInfo/name").and_then(Value::as_str)
                        == Some("hermes-agent")
                    && v.pointer("/agentCapabilities/loadSession")
                        .and_then(Value::as_bool)
                        == Some(true) => {}
            Err(error) => {
                let _ = child.shutdown().await;
                return Err(invalid(format!("setup_required: official Hermes ACP could not initialize. Run `hermes acp --check` and install its official ACP dependencies. {error}")));
            }
            other => {
                let _ = child.shutdown().await;
                return Err(invalid(format!("unsupported: official Hermes ACP v1 with native load is required; initialize: {other:?}")));
            }
        }
        let (barrier, mut barriers) = mpsc::unbounded_channel::<oneshot::Sender<()>>();
        let runtime = Arc::new(Runtime {
            child,
            ownership: std::sync::Mutex::new(Some(ownership)),
            installation_stamp,
            profile,
            operation: Mutex::new(()),
            routes: Mutex::new(HashMap::new()),
            queue: Mutex::new(Queue::default()),
            barrier,
        });
        let weak = Arc::downgrade(&runtime);
        let inner = Arc::downgrade(self);
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    biased;
                    request = requests.recv() => {
                        let (Some(r), Some(i), Some(request)) = (weak.upgrade(), inner.upgrade(), request) else { break; };
                        i.permission(&r, request).await;
                    }
                    message = notifications.recv() => {
                        let (Some(r), Some(i)) = (weak.upgrade(), inner.upgrade()) else { break; };
                        match message {
                            Ok(n) => i.notification(&r, n).await,
                            Err(broadcast::error::RecvError::Lagged(_)) => { i.fail_runtime(&r,"Hermes event stream overflowed; reconnect to restore native history").await; },
                            Err(_) => break,
                        }
                    }
                    Some(done) = barriers.recv() => { let _ = done.send(()); }
                    _ = tokio::time::sleep(Duration::from_secs(1)) => {
                        let (Some(r), Some(i)) = (weak.upgrade(), inner.upgrade()) else { break; };
                        if !r.child.is_alive() { i.fail_runtime(&r,"Hermes runtime disconnected. Pending approvals were cancelled; retry resumes the original conversation.").await; break; }
                    }
                }
            }
        });
        self.runtimes.lock().await.insert(key, runtime.clone());
        Ok(runtime)
    }
    async fn fail_runtime(&self, runtime: &Runtime, message: &str) {
        if !runtime.child.is_alive() {
            runtime.ownership.lock().unwrap().take();
        }
        let chats: Vec<_> = runtime
            .routes
            .lock()
            .await
            .values()
            .filter_map(std::sync::Weak::upgrade)
            .collect();
        for chat in chats {
            let mut s = chat.state.lock().await;
            s.error = Some(message.into());
            let thread = ThreadId(chat.binding.lock().await.thread_id.clone());
            for (request_id, _) in s.pending.drain() {
                self.emit(ProviderRuntimeEvent::RequestResolved {
                    thread_id: thread.clone(),
                    request_id,
                    decision: ApprovalDecision::Cancel,
                });
            }
            self.emit(ProviderRuntimeEvent::SessionStateChanged {
                thread_id: thread,
                status: SessionStatus::Error {
                    message: message.into(),
                },
            });
        }
    }
    async fn notification(&self, runtime: &Runtime, n: Notification) {
        if n.method != "session/update" {
            return;
        }
        let Some(sid) = text(&n.params, "sessionId") else {
            return;
        };
        let chat = runtime
            .routes
            .lock()
            .await
            .get(sid)
            .and_then(std::sync::Weak::upgrade);
        let Some(chat) = chat else {
            return;
        }; // Catalog and native load replay are intentionally not imported.
        let Some(update) = n.params.get("update") else {
            return;
        };
        let thread = {
            let mut binding = chat.binding.lock().await;
            if update.pointer("/_meta/hermes/sessionProvenance").is_some() {
                if let Err(e) = binding
                    .provenance(update)
                    .and_then(|_| self.store.save(&binding))
                {
                    drop(binding);
                    chat.state.lock().await.error = Some(e);
                    return;
                }
            }
            ThreadId(binding.thread_id.clone())
        };
        let mut s = chat.state.lock().await;
        if s.replaying {
            return;
        }
        let Some(turn) = s.active.clone() else {
            return;
        };
        let kind = text(update, "sessionUpdate").unwrap_or("");
        match kind {
            "agent_message_chunk" | "agent_thought_chunk" => {
                if let Some(t) = update.pointer("/content/text").and_then(Value::as_str) {
                    let delta = if kind == "agent_thought_chunk" {
                        s.thoughts.push_str(t);
                        ContentDelta::Thinking { text: t.into() }
                    } else {
                        s.prose.push_str(t);
                        ContentDelta::Text { text: t.into() }
                    };
                    self.emit(ProviderRuntimeEvent::ContentDelta {
                        thread_id: thread,
                        turn_id: turn,
                        delta,
                        subagent_id: None,
                    });
                }
            }
            "tool_call" | "tool_call_update" => {
                let Some(id) = text(update, "toolCallId") else {
                    return;
                };
                let completed = matches!(text(update, "status"), Some("completed" | "failed"));
                let key = format!(
                    "{}:{id}:{}",
                    turn.0,
                    if completed { "result" } else { "start" }
                );
                if !completed && kind != "tool_call" {
                    return;
                }
                if !s.seen_tools.insert(key) {
                    return;
                }
                let item = if completed {
                    CompletedItem::ToolResult {
                        tool_use_id: id.into(),
                        content: update
                            .get("rawOutput")
                            .filter(|v| !v.is_null())
                            .or_else(|| update.get("content"))
                            .cloned()
                            .unwrap_or(Value::Null),
                        is_error: text(update, "status") == Some("failed"),
                    }
                } else {
                    CompletedItem::ToolUse {
                        tool_use_id: id.into(),
                        tool_name: text(update, "title")
                            .unwrap_or("Hermes tool")
                            .split(": ")
                            .next()
                            .unwrap_or("Hermes tool")
                            .into(),
                        input: json!({"hermesAcp":true,"title":update.get("title"),"rawInput":update.get("rawInput"),"content":update.get("content")}),
                    }
                };
                self.emit(ProviderRuntimeEvent::ItemCompleted {
                    thread_id: thread,
                    turn_id: turn,
                    item,
                    subagent_id: None,
                });
            }
            "usage_update" => {
                if let Some(used) = update.get("used").and_then(Value::as_u64) {
                    self.emit(ProviderRuntimeEvent::ContextUsageUpdated {
                        thread_id: thread,
                        usage: ContextUsageSnapshot {
                            used_tokens: used,
                            max_tokens: update.get("size").and_then(Value::as_u64),
                            total_processed_tokens: None,
                            last_used_tokens: None,
                            compacts_automatically: None,
                        },
                    });
                }
            }
            "session_info_update"
            | "available_commands_update"
            | "current_mode_update"
            | "config_option_update"
            | "plan" => {}
            _ => self.emit(ProviderRuntimeEvent::RuntimeWarning {
                thread_id: Some(thread),
                message: format!("Unrecognized Hermes ACP update: {kind}"),
                original_payload: None,
            }),
        }
    }
    async fn permission(&self, runtime: &Runtime, mut request: IncomingRequest) {
        if request.method != "session/request_permission" {
            let _ = runtime
                .child
                .respond(
                    request.id,
                    Err(RpcError {
                        code: -32601,
                        message: "Unsupported client method".into(),
                        data: None,
                    }),
                )
                .await;
            return;
        }
        let chat = match text(&request.params, "sessionId") {
            Some(id) => runtime
                .routes
                .lock()
                .await
                .get(id)
                .and_then(std::sync::Weak::upgrade),
            None => None,
        };
        if let Some(chat) = chat {
            let mut s = chat.state.lock().await;
            if let Some(turn) = s
                .active
                .clone()
                .filter(|_| !s.cancelled && !s.closed && s.error.is_none())
            {
                let options = request
                    .params
                    .get("options")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                request.params["_codemuxProvider"] = json!("hermes");
                let id = RequestId(Uuid::new_v4().to_string());
                s.pending.insert(
                    id.clone(),
                    Pending {
                        rpc_id: request.id,
                        options,
                        turn: turn.clone(),
                    },
                );
                let thread = ThreadId(chat.binding.lock().await.thread_id.clone());
                self.emit(ProviderRuntimeEvent::RequestOpened {
                    thread_id: thread.clone(),
                    turn_id: turn,
                    request_id: id.clone(),
                    request_kind: "tool_approval".into(),
                    tool_use_id: request
                        .params
                        .pointer("/toolCall/toolCallId")
                        .and_then(Value::as_str)
                        .map(String::from),
                    payload: request.params,
                    subagent_id: None,
                });
                self.emit(ProviderRuntimeEvent::SessionStateChanged {
                    thread_id: thread,
                    status: SessionStatus::WaitingApproval { request_id: id },
                });
                return;
            }
        }
        let _ = runtime
            .child
            .respond(request.id, Ok(json!({"outcome":{"outcome":"cancelled"}})))
            .await;
    }
    async fn pump(self: Arc<Self>, runtime: Arc<Runtime>) {
        loop {
            let _guard = runtime.operation.lock().await;
            let job = {
                let mut queue = runtime.queue.lock().await;
                match queue.jobs.pop_front() {
                    Some(j) => j,
                    None => {
                        queue.running = false;
                        return;
                    }
                }
            };
            let chat = &job.chat;
            let thread = job.input.thread_id.clone();
            let turn = TurnId(Uuid::new_v4().to_string());
            let mut dispatched = false;
            {
                let mut state = chat.state.lock().await;
                if state.cancel_epoch != job.cancel_epoch {
                    self.emit(ProviderRuntimeEvent::QueuedTurnCancelled {
                        thread_id: thread.clone(),
                        queued_id: job.id.clone(),
                    });
                    continue;
                }
                state.active = Some(turn.clone());
                state.cancelled = false;
            }
            let result = async {
                let binding = chat.binding.lock().await.clone();
                binding.profile.validate().map_err(invalid)?;
                binding.validate_cwd().map_err(invalid)?;
                {
                    let mut state = chat.state.lock().await;
                    if state.closed || state.error.is_some() { return Err(invalid(state.error.clone().unwrap_or("Hermes chat closed".into()))); }
                    state.prose.clear();state.thoughts.clear();state.seen_tools.clear();state.replaying=false;
                }
                if let Some(checkpoint) = job.input.turn_checkpoint.as_ref() { checkpoint.prepare().await; }
                if chat.state.lock().await.cancelled {
                    if let Some(checkpoint) = job.input.turn_checkpoint.as_ref() { checkpoint.abort().await; }
                    return Err(invalid("Hermes turn cancelled before dispatch"));
                }
                if let Some(checkpoint) = job.input.turn_checkpoint.as_ref() { checkpoint.commit().await; }
                dispatched = true;
                self.emit(ProviderRuntimeEvent::QueuedTurnDispatched {thread_id:thread.clone(),queued_id:job.id.clone(),turn_id:turn.clone(),text:job.input.display_text.clone().unwrap_or_else(||job.input.text.clone()),steered:false});
                self.emit(ProviderRuntimeEvent::SessionStateChanged {thread_id:thread.clone(),status:SessionStatus::Running {active_turn:turn.clone()}});
                let response = runtime.child.request_with_timeout("session/prompt",json!({"sessionId":binding.acp_session_id,"prompt":[{"type":"text","text":job.input.text}]}),Duration::from_secs(24*60*60)).await.map_err(rpc)?;
                let (tx,rx) = oneshot::channel(); let _=runtime.barrier.send(tx); let _=rx.await;
                if text(&response,"stopReason") == Some("cancelled") { chat.state.lock().await.cancelled=true; }
                // Empty new sessions are ephemeral upstream. Ask the official load endpoint
                // for provenance once history exists, suppressing its native replay.
                if chat.binding.lock().await.current_native_id.is_none() && text(&response,"stopReason") != Some("cancelled") {
                    chat.state.lock().await.replaying=true;
                    let metadata=runtime.child.request("session/load",json!({"sessionId":binding.acp_session_id,"cwd":binding.cwd,"mcpServers":[]})).await.map_err(rpc)?;
                    let (tx,rx)=oneshot::channel();let _=runtime.barrier.send(tx);let _=rx.await;
                    chat.state.lock().await.replaying=false;
                    let mut b=chat.binding.lock().await;
                    b.provenance(&metadata).map_err(invalid)?;
                    if b.current_native_id.is_none() { return Err(invalid("unsupported: Hermes has not confirmed native history persistence")); }
                    self.store.save(&b).map_err(invalid)?;
                }
                if response.get("stopReason").and_then(Value::as_str).is_none() { return Err(invalid("unsupported: Hermes prompt returned no stop reason")); }
                Ok::<(),ProviderError>(())
            }.await;
            let mut state = chat.state.lock().await;
            for (id, pending) in state.pending.drain() {
                let _ = runtime
                    .child
                    .respond(
                        pending.rpc_id,
                        Ok(json!({"outcome":{"outcome":"cancelled"}})),
                    )
                    .await;
                self.emit(ProviderRuntimeEvent::RequestResolved {
                    thread_id: thread.clone(),
                    request_id: id,
                    decision: ApprovalDecision::Cancel,
                });
            }
            for (thought, body) in [
                (false, std::mem::take(&mut state.prose)),
                (true, std::mem::take(&mut state.thoughts)),
            ] {
                if !body.is_empty() {
                    self.emit(ProviderRuntimeEvent::ItemCompleted {
                        thread_id: thread.clone(),
                        turn_id: turn.clone(),
                        item: if thought {
                            CompletedItem::AssistantThinking { text: body }
                        } else {
                            CompletedItem::AssistantText { text: body }
                        },
                        subagent_id: None,
                    });
                }
            }
            let error = result
                .err()
                .map(|e| e.to_string())
                .or_else(|| state.error.clone());
            let status = if state.cancelled {
                TurnStatus::Error {
                    subtype: "cancelled".into(),
                    message: "Hermes turn cancelled".into(),
                }
            } else if let Some(message) = error.as_ref() {
                TurnStatus::Error {
                    subtype: "hermes".into(),
                    message: message.clone(),
                }
            } else {
                TurnStatus::Success
            };
            if error.is_some() {
                state.error = error.clone();
            }
            state.active = None;
            state.replaying = false;
            if !dispatched {
                self.emit(ProviderRuntimeEvent::QueuedTurnCancelled {
                    thread_id: thread.clone(),
                    queued_id: job.id.clone(),
                });
            }
            self.emit(ProviderRuntimeEvent::TurnCompleted {
                thread_id: thread.clone(),
                turn_id: turn,
                status,
                usage: None,
            });
            self.emit(ProviderRuntimeEvent::SessionStateChanged {
                thread_id: thread,
                status: if let Some(message) = error {
                    SessionStatus::Error { message }
                } else {
                    SessionStatus::Ready
                },
            });
            drop(state);
            chat.finished.notify_waiters();
        }
    }
}

#[async_trait]
impl AgentProvider for HermesProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Hermes
    }
    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities {
            supports_mid_session_model_change: true,
            supports_mid_session_permission_change: true,
            supports_synchronous_tool_approval: true,
            supports_interrupt: true,
            supports_session_resume: true,
            ..Default::default()
        }
    }
    async fn start_session(
        &self,
        input: StartSessionInput,
    ) -> Result<ProviderSession, ProviderError> {
        let start_lock = self
            .inner
            .starts
            .lock()
            .await
            .entry(input.thread_id.clone())
            .or_default()
            .clone();
        let _start = start_lock.lock().await;
        if let Ok(chat) = self.chat(&input.thread_id).await {
            if chat.runtime.child.is_alive() && {
                let s = chat.state.lock().await;
                !s.closed && s.error.is_none()
            } {
                return Err(invalid("Hermes chat is already open"));
            }
        }
        if input.effort.is_some() || input.fast_mode || input.context_window.is_some() {
            return Err(invalid(
                "unsupported: Hermes reasoning, fast mode and context controls are unavailable",
            ));
        }
        let worktree_guard = WORKTREE_LIFECYCLE.lock().await;
        let stored = self.inner.store.load(&input.thread_id.0).map_err(invalid)?;
        let mut binding = if let Some(mut binding) = stored {
            if input.fresh_session {
                return Err(invalid(
                    "repair_required: existing Hermes conversations cannot be silently replaced",
                ));
            }
            if let Some(p) = input.extra.get("hermes_profile") {
                if serde_json::from_value::<Profile>(p.clone()).map_err(rpc)? != binding.profile {
                    return Err(invalid(
                        "repair_required: this conversation is bound to another Hermes profile",
                    ));
                }
            }
            if input.cwd.canonicalize().ok().as_ref() != Some(&binding.cwd) {
                return Err(invalid("repair_required: Hermes conversation cwd changed"));
            }
            // Generic session config is the durable UI intent, including offline edits.
            // Omitted fields preserve bindings from older callers; explicit profile_default
            // cannot reset an existing native conversation to an unrelated model.
            if let Some(model) = input.model.as_ref() {
                if model == "profile_default" {
                    if binding.model_override.is_some() {
                        return Err(invalid("unsupported: cannot reset an existing Hermes conversation to profile default"));
                    }
                } else {
                    binding.model_override = Some(model.clone());
                }
            }
            if let Some(mode) = input.permission_mode.as_ref() {
                binding.permission_mode = Some(mode.clone());
            }
            binding
        } else {
            if input.resume_cursor.is_some() {
                return Err(invalid(
                    "repair_required: native Hermes resume requires its durable profile binding",
                ));
            }
            let profile: Profile =
                serde_json::from_value(
                    input.extra.get("hermes_profile").cloned().ok_or_else(|| {
                        invalid("setup_required: choose an existing Hermes profile")
                    })?,
                )
                .map_err(rpc)?;
            Binding {
                schema_version: 1,
                profile,
                thread_id: input.thread_id.0.clone(),
                workspace_id: input.workspace_id,
                cwd: input
                    .cwd
                    .canonicalize()
                    .map_err(|_| invalid("repair_required: workspace directory is missing"))?,
                acp_session_id: None,
                current_native_id: None,
                root_native_id: None,
                permission_mode: input.permission_mode,
                model_override: input.model.filter(|m| m != "profile_default"),
                resolved_model: None,
                compatibility: "setup_required".into(),
                cleanup_pending: false,
            }
        };
        if binding.schema_version != 1 {
            return Err(invalid("unsupported: Hermes binding version"));
        }
        binding.profile.validate().map_err(invalid)?;
        durable_profile(&binding.profile)?;
        binding.validate_cwd().map_err(invalid)?;
        self.inner.store.save(&binding).map_err(invalid)?;
        drop(worktree_guard);
        let runtime = self.inner.runtime(binding.profile.clone()).await?;
        let _operation = runtime.operation.lock().await;
        if binding
            .current_native_id
            .as_ref()
            .zip(binding.acp_session_id.as_ref())
            .is_some_and(|(head, handle)| head != handle)
        {
            return Err(invalid("unsupported: compressed Hermes history has a new native head; restart recovery needs a verified official continuation contract. History has been retained."));
        }
        let expected_default =
            if binding.acp_session_id.is_some() && binding.model_override.is_none() {
                binding.resolved_model.clone()
            } else {
                None
            };
        // Initialization has no worktree access. Commit the hold only immediately
        // before new/load can access it. Recheck after waiting on the profile gate:
        // deletion may have completed while initialization/another chat was busy.
        {
            let _lifecycle = WORKTREE_LIFECYCLE.lock().await;
            binding.validate_cwd().map_err(invalid)?;
            binding.cleanup_pending = true;
            self.inner.store.save(&binding).map_err(invalid)?;
        }
        let mut res = if let Some(id) = binding.acp_session_id.as_ref() {
            runtime
                .child
                .request(
                    "session/load",
                    json!({"sessionId":id,"cwd":binding.cwd,"mcpServers":[]}),
                )
                .await
        } else {
            runtime
                .child
                .request("session/new", json!({"cwd":binding.cwd,"mcpServers":[]}))
                .await
        }
        .map_err(rpc)?;
        if binding.acp_session_id.is_none() {
            binding.acp_session_id = text(&res, "sessionId").map(String::from);
        }
        // Empty success is the known named-custom restore failure, not a usable session.
        let resolved=res.pointer("/models/currentModelId").and_then(Value::as_str).ok_or_else(||invalid("repair_required: Hermes did not restore a usable native session; no replacement conversation was created"))?.to_string();
        if expected_default
            .as_ref()
            .is_some_and(|expected| expected != &resolved)
        {
            return Err(invalid("repair_required: restored Hermes model differs from the recorded effective model; no fallback was accepted"));
        }
        binding.provenance(&res).map_err(invalid)?;
        binding.resolved_model = Some(resolved.clone());
        binding.compatibility = "unsupported".into();
        self.inner.store.save(&binding).map_err(invalid)?;
        durable_catalog(&res)?;
        durable_model(&resolved)?;
        if let Some(model) = binding.model_override.as_ref() {
            durable_model(model)?;
            durable_model_change(Some(&resolved), model)?;
            if res
                .pointer("/models/availableModels")
                .and_then(Value::as_array)
                .is_none_or(|models| {
                    !models
                        .iter()
                        .any(|m| text(m, "modelId") == Some(model.as_str()))
                })
            {
                return Err(invalid("repair_required: selected Hermes model is no longer advertised; configure it in Hermes"));
            }
            runtime
                .child
                .request(
                    "session/set_model",
                    json!({"sessionId":binding.acp_session_id,"modelId":model}),
                )
                .await
                .map_err(rpc)?;
            res = runtime
                .child
                .request(
                    "session/load",
                    json!({"sessionId":binding.acp_session_id,"cwd":binding.cwd,"mcpServers":[]}),
                )
                .await
                .map_err(rpc)?;
            if res
                .pointer("/models/currentModelId")
                .and_then(Value::as_str)
                != Some(model.as_str())
            {
                return Err(invalid("repair_required: Hermes did not confirm the selected model; no fallback was applied"));
            }
            binding.resolved_model = Some(model.clone());
        }
        if binding.acp_session_id.is_none() {
            return Err(invalid(
                "unsupported: Hermes did not supply native session provenance",
            ));
        }
        let modes: HashSet<String> = res
            .pointer("/modes/availableModes")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|m| text(m, "id").map(String::from))
            .collect();
        if let Some(mode) = binding.permission_mode.as_ref() {
            if !modes.contains(mode) {
                return Err(invalid(
                    "unsupported: Hermes no longer advertises the selected edit policy",
                ));
            }
            runtime
                .child
                .request(
                    "session/set_mode",
                    json!({"sessionId":binding.acp_session_id,"modeId":mode}),
                )
                .await
                .map_err(rpc)?;
            let checked = runtime
                .child
                .request(
                    "session/load",
                    json!({"sessionId":binding.acp_session_id,"cwd":binding.cwd,"mcpServers":[]}),
                )
                .await
                .map_err(rpc)?;
            if checked
                .pointer("/modes/currentModeId")
                .and_then(Value::as_str)
                != Some(mode.as_str())
            {
                return Err(invalid(
                    "repair_required: Hermes did not confirm the selected edit policy",
                ));
            }
        }
        let models: HashSet<String> = res
            .pointer("/models/availableModels")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|m| text(m, "modelId").map(String::from))
            .collect();
        binding.compatibility = "ready".into();
        self.inner.store.save(&binding).map_err(invalid)?;
        let sid = binding.acp_session_id.clone().unwrap();
        let snapshot = ProviderSession {
            thread_id: input.thread_id.clone(),
            provider: ProviderKind::Hermes,
            session_id: ProviderSessionId(sid.clone()),
            status: SessionStatus::Ready,
            resume_cursor: Some(json!({"sessionId":sid,"binding":binding})),
        };
        let chat = Arc::new(Chat {
            binding: Mutex::new(binding),
            runtime: runtime.clone(),
            state: Mutex::new(ChatState {
                models,
                modes,
                ..Default::default()
            }),
            finished: Notify::new(),
        });
        runtime
            .routes
            .lock()
            .await
            .insert(sid.clone(), Arc::downgrade(&chat));
        self.inner
            .chats
            .lock()
            .await
            .insert(input.thread_id.clone(), chat);
        self.inner.emit(ProviderRuntimeEvent::SessionConfigured {
            thread_id: input.thread_id.clone(),
            provider_session_id: ProviderSessionId(sid),
        });
        self.inner.emit(ProviderRuntimeEvent::RuntimeWarning {
            thread_id: Some(input.thread_id),
            message: LIMITS.into(),
            original_payload: None,
        });
        Ok(snapshot)
    }
    async fn send_turn(&self, input: SendTurnInput) -> Result<TurnStartResult, ProviderError> {
        if input.effort_override.is_some()
            || input.permission_mode_override.is_some()
            || !input.images.is_empty()
            || !input.skill_invocations.is_empty()
        {
            return Err(invalid("unsupported: Hermes accepts native text/tools; these turn overrides or attachments are unavailable"));
        }
        let chat = self.chat(&input.thread_id).await?;
        if let Some(model) = input
            .model_override
            .as_ref()
            .filter(|m| m.as_str() != "profile_default")
        {
            if chat.binding.lock().await.resolved_model.as_ref() != Some(model) {
                return Err(invalid(
                    "Change the Hermes session model before queuing a turn",
                ));
            }
        }
        chat.binding.lock().await.validate_cwd().map_err(invalid)?;
        let state = chat.state.lock().await;
        if state.closed || state.error.is_some() || !chat.runtime.child.is_alive() {
            return Err(invalid(
                "repair_required: Hermes session needs reconnecting",
            ));
        }
        let cancel_epoch = state.cancel_epoch;
        drop(state);
        let id = Uuid::new_v4().to_string();
        let runtime = chat.runtime.clone();
        let mut queue = runtime.queue.lock().await;
        let idle = chat.state.lock().await.active.is_none();
        self.inner.emit(ProviderRuntimeEvent::TurnQueued {
            thread_id: input.thread_id.clone(),
            queued_id: id.clone(),
            client_nonce: input.client_nonce.clone(),
            text: input
                .display_text
                .clone()
                .unwrap_or_else(|| input.text.clone()),
        });
        queue.jobs.push_back(Job {
            id: id.clone(),
            cancel_epoch,
            chat,
            input,
        });
        // A profile-wide queue can hold a chat with no active turn of its own.
        // Clear the composer's optimistic streaming state until actual dispatch.
        if idle {
            self.inner.emit(ProviderRuntimeEvent::SessionStateChanged {
                thread_id: ThreadId(queue.jobs.back().unwrap().input.thread_id.0.clone()),
                status: SessionStatus::Ready,
            });
        }
        if !queue.running {
            queue.running = true;
            tokio::spawn(self.inner.clone().pump(runtime.clone()));
        }
        Ok(TurnStartResult {
            turn_id: TurnId(String::new()),
            queued_id: Some(id),
            steered: false,
        })
    }
    async fn cancel_queued_turn(
        &self,
        thread_id: ThreadId,
        queued_id: String,
    ) -> Result<bool, ProviderError> {
        let chat = self.chat(&thread_id).await?;
        let mut q = chat.runtime.queue.lock().await;
        if let Some(index) = q
            .jobs
            .iter()
            .position(|j| j.id == queued_id && j.input.thread_id == thread_id)
        {
            q.jobs.remove(index);
            self.inner.emit(ProviderRuntimeEvent::QueuedTurnCancelled {
                thread_id,
                queued_id,
            });
            Ok(true)
        } else {
            Ok(false)
        }
    }
    async fn interrupt_turn(
        &self,
        thread_id: ThreadId,
        turn_id: Option<TurnId>,
    ) -> Result<(), ProviderError> {
        let chat = self.chat(&thread_id).await?;
        if turn_id.is_none() {
            let mut queue = chat.runtime.queue.lock().await;
            queue.jobs.retain(|job| {
                if job.input.thread_id != thread_id {
                    return true;
                }
                self.inner.emit(ProviderRuntimeEvent::QueuedTurnCancelled {
                    thread_id: thread_id.clone(),
                    queued_id: job.id.clone(),
                });
                false
            });
        }
        let mut s = chat.state.lock().await;
        if turn_id.is_none() {
            s.cancel_epoch += 1;
        }
        if s.active.is_none() {
            self.inner.emit(ProviderRuntimeEvent::SessionStateChanged {
                thread_id,
                status: SessionStatus::Ready,
            });
            return Ok(());
        }
        if turn_id.is_some_and(|id| Some(id) != s.active) {
            return Ok(());
        }
        s.cancelled = true;
        for (id, p) in s.pending.drain() {
            let _ = chat
                .runtime
                .child
                .respond(p.rpc_id, Ok(json!({"outcome":{"outcome":"cancelled"}})))
                .await;
            self.inner.emit(ProviderRuntimeEvent::RequestResolved {
                thread_id: thread_id.clone(),
                request_id: id,
                decision: ApprovalDecision::Cancel,
            });
        }
        let sid = chat.binding.lock().await.acp_session_id.clone();
        drop(s);
        chat.runtime
            .child
            .notify("session/cancel", json!({"sessionId":sid}))
            .await
            .map_err(rpc)?;
        tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                let notified = chat.finished.notified();
                if chat.state.lock().await.active.is_none() {
                    break;
                }
                notified.await;
            }
        })
        .await
        .map_err(|_| {
            invalid("Hermes cancellation has not completed; workspace cleanup remains pending")
        })?;
        Ok(())
    }
    async fn respond_to_request(
        &self,
        thread_id: ThreadId,
        request_id: RequestId,
        decision: ApprovalDecision,
    ) -> Result<(), ProviderError> {
        let chat = self.chat(&thread_id).await?;
        let mut s = chat.state.lock().await;
        let p = s
            .pending
            .get(&request_id)
            .ok_or_else(|| ProviderError::RequestNotPending {
                request_id: request_id.clone(),
            })?;
        if Some(&p.turn) != s.active.as_ref()
            || s.cancelled
            || s.closed
            || !chat.runtime.child.is_alive()
        {
            return Err(ProviderError::RequestNotPending { request_id });
        }
        let kind=match &decision {ApprovalDecision::ProviderOption {..}=>None,ApprovalDecision::Allow {updated_input:None,updated_permissions:None}=>Some("allow_once"),ApprovalDecision::Deny {..}=>Some("reject_once"),ApprovalDecision::AllowForSession=>Some("hermes_session"),ApprovalDecision::Cancel=>None,_=>return Err(invalid("unsupported: Hermes permission scope/input cannot be inferred; select an advertised permission option"))};
        let outcome = if let ApprovalDecision::ProviderOption { option_id } = &decision {
            if !p
                .options
                .iter()
                .any(|o| text(o, "optionId") == Some(option_id.as_str()))
            {
                return Err(invalid("Unknown Hermes permission option"));
            }
            json!({"outcome":"selected","optionId":option_id})
        } else if let Some(kind) = kind {
            let option = p
                .options
                .iter()
                .find(|o| {
                    if kind == "hermes_session" {
                        text(o, "optionId") == Some("allow_session")
                            && text(o, "kind") == Some("allow_always")
                    } else {
                        text(o, "kind") == Some(kind)
                    }
                })
                .and_then(|o| text(o, "optionId"))
                .ok_or_else(|| {
                    invalid("unsupported: permission choice is not advertised by Hermes")
                })?;
            json!({"outcome":"selected","optionId":option})
        } else {
            json!({"outcome":"cancelled"})
        };
        chat.runtime
            .child
            .respond(p.rpc_id.clone(), Ok(json!({"outcome":outcome})))
            .await
            .map_err(rpc)?;
        s.pending.remove(&request_id);
        self.inner.emit(ProviderRuntimeEvent::RequestResolved {
            thread_id,
            request_id,
            decision,
        });
        Ok(())
    }
    async fn set_model(&self, thread_id: ThreadId, model: String) -> Result<(), ProviderError> {
        durable_model(&model)?;
        let chat = self.chat(&thread_id).await?;
        durable_profile(&chat.runtime.profile)?;
        if chat.runtime.queue.lock().await.running {
            return Err(invalid(
                "Wait for this Hermes profile's foreground queue before changing model",
            ));
        }
        let _guard = chat.runtime.operation.lock().await;
        let b = chat.binding.lock().await.clone();
        durable_model_change(b.resolved_model.as_deref(), &model)?;
        if model == "profile_default" {
            return if b.model_override.is_none() {
                Ok(())
            } else {
                Err(invalid(
                    "Use profile default cannot reset an existing native model override",
                ))
            };
        }
        if !chat.state.lock().await.models.contains(&model) {
            return Err(invalid(
                "repair_required: selected Hermes model is not in this session's catalog",
            ));
        }
        chat.runtime
            .child
            .request(
                "session/set_model",
                json!({"sessionId":b.acp_session_id,"modelId":model}),
            )
            .await
            .map_err(rpc)?;
        chat.state.lock().await.replaying = true;
        let response = chat
            .runtime
            .child
            .request(
                "session/load",
                json!({"sessionId":b.acp_session_id,"cwd":b.cwd,"mcpServers":[]}),
            )
            .await;
        // Release binding before the router barrier (provenance updates take that lock).
        let (tx, rx) = oneshot::channel();
        let _ = chat.runtime.barrier.send(tx);
        let _ = rx.await;
        chat.state.lock().await.replaying = false;
        let response = response.map_err(rpc)?;
        durable_catalog(&response)?;
        if response
            .pointer("/models/currentModelId")
            .and_then(Value::as_str)
            != Some(model.as_str())
        {
            chat.state.lock().await.error = Some("Hermes model selection was not confirmed".into());
            return Err(invalid(
                "repair_required: Hermes model selection was not confirmed",
            ));
        }
        let mut b = chat.binding.lock().await;
        b.model_override = Some(model.clone());
        b.resolved_model = Some(model);
        self.inner.store.save(&b).map_err(invalid)
    }
    async fn set_permission_mode(
        &self,
        thread_id: ThreadId,
        mode: String,
    ) -> Result<(), ProviderError> {
        let chat = self.chat(&thread_id).await?;
        if chat.runtime.queue.lock().await.running {
            return Err(invalid(
                "Wait for this Hermes profile's foreground queue before changing edit policy",
            ));
        }
        let _guard = chat.runtime.operation.lock().await;
        if !chat.state.lock().await.modes.contains(&mode) {
            return Err(invalid(
                "unsupported: edit policy is not advertised by Hermes",
            ));
        }
        let mut b = chat.binding.lock().await.clone();
        chat.runtime
            .child
            .request(
                "session/set_mode",
                json!({"sessionId":b.acp_session_id,"modeId":mode}),
            )
            .await
            .map_err(rpc)?;
        chat.state.lock().await.replaying = true;
        let checked = chat
            .runtime
            .child
            .request(
                "session/load",
                json!({"sessionId":b.acp_session_id,"cwd":b.cwd,"mcpServers":[]}),
            )
            .await;
        let (tx, rx) = oneshot::channel();
        let _ = chat.runtime.barrier.send(tx);
        let _ = rx.await;
        chat.state.lock().await.replaying = false;
        if checked
            .as_ref()
            .ok()
            .and_then(|v| v.pointer("/modes/currentModeId"))
            .and_then(Value::as_str)
            != Some(mode.as_str())
        {
            chat.state.lock().await.error = Some("Hermes edit policy was not confirmed".into());
            return Err(invalid(
                "repair_required: Hermes did not confirm the selected edit policy",
            ));
        }
        b.permission_mode = Some(mode);
        *chat.binding.lock().await = b.clone();
        self.inner.store.save(&b).map_err(invalid)
    }
    async fn stop_session(&self, thread_id: ThreadId) -> Result<(), ProviderError> {
        let Ok(chat) = self.chat(&thread_id).await else {
            return Ok(());
        };
        chat.state.lock().await.closed = true;
        {
            let mut q = chat.runtime.queue.lock().await;
            q.jobs.retain(|job| {
                if job.input.thread_id == thread_id {
                    self.inner.emit(ProviderRuntimeEvent::QueuedTurnCancelled {
                        thread_id: thread_id.clone(),
                        queued_id: job.id.clone(),
                    });
                    false
                } else {
                    true
                }
            });
        }
        self.interrupt_turn(thread_id.clone(), None).await?;
        let sid = chat.binding.lock().await.acp_session_id.clone().unwrap();
        chat.runtime.routes.lock().await.remove(&sid);
        self.inner.chats.lock().await.remove(&thread_id);
        self.inner.emit(ProviderRuntimeEvent::SessionStateChanged {
            thread_id,
            status: SessionStatus::Closed,
        });
        Ok(())
    }
    async fn list_sessions(&self) -> Result<Vec<ProviderSession>, ProviderError> {
        let chats: Vec<_> = self.inner.chats.lock().await.values().cloned().collect();
        let mut out = vec![];
        for chat in chats {
            let b = chat.binding.lock().await;
            out.push(ProviderSession {
                thread_id: ThreadId(b.thread_id.clone()),
                provider: ProviderKind::Hermes,
                session_id: ProviderSessionId(b.acp_session_id.clone().unwrap()),
                status: SessionStatus::Ready,
                resume_cursor: Some(serde_json::to_value(&*b).map_err(rpc)?),
            });
        }
        Ok(out)
    }
    async fn has_session(&self, id: &ThreadId) -> bool {
        match self.chat(id).await {
            Ok(c) => {
                c.runtime.child.is_alive() && {
                    let s = c.state.lock().await;
                    !s.closed && s.error.is_none()
                }
            }
            Err(_) => false,
        }
    }
    async fn turn_active(&self, id: &ThreadId) -> bool {
        match self.chat(id).await {
            Ok(c) => c.state.lock().await.active.is_some(),
            Err(_) => false,
        }
    }
    fn event_stream(&self) -> ProviderEventStream {
        let rx = self.inner.events.subscribe();
        Box::pin(futures_util::stream::unfold(rx, |mut rx| async move {
            loop {
                match rx.recv().await {
                    Ok(e) => return Some((e, rx)),
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(_) => return None,
                }
            }
        }))
    }
}

#[cfg(test)]
mod tests;

fn installation_stamp(profile: &Profile) -> Result<String, ProviderError> {
    let m = std::fs::metadata(&profile.installation).map_err(rpc)?;
    Ok(format!("{}:{:?}", m.len(), m.modified().map_err(rpc)?))
}
fn claim_profile(profile: &Profile) -> Result<std::fs::File, ProviderError> {
    use sha2::{Digest, Sha256};
    #[cfg(test)]
    let directory = std::env::temp_dir().join(format!(
        "codemux-hermes-test-ownership-{}",
        std::process::id()
    ));
    #[cfg(not(test))]
    let directory = dirs::data_local_dir()
        .ok_or_else(|| invalid("setup_required: Codemux data directory unavailable"))?
        .join("codemux/hermes-ownership");
    std::fs::create_dir_all(&directory).map_err(rpc)?;
    let key = format!(
        "{:x}",
        Sha256::digest(profile.home.to_string_lossy().as_bytes())
    );
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(directory.join(key))
        .map_err(rpc)?;
    file.try_lock().map_err(|_|invalid("repair_required: another Codemux runtime owns this Hermes profile. Disconnect it there first. Concurrent Hermes Desktop/gateway use is unsupported."))?;
    Ok(file)
}
