use super::catalog::{
    catalog_from, namespaced_session, permission_outcome, semantic_option, text, validate_value,
    Negotiation,
};
use super::{AcpBinding, AcpCatalog, AcpLaunchConfig};
use crate::agent_provider::*;
use crate::json_rpc_child::{
    IncomingRequest, JsonRpcChild, SequencedNotification, RpcChildError, RpcError, SpawnConfig,
};
use async_trait::async_trait;
use base64::Engine;
use serde::Serialize;
use serde_json::{json, Value};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};
use tokio::sync::{broadcast, mpsc, oneshot, Mutex, Notify};
use uuid::Uuid;

const RPC_TIMEOUT: Duration = Duration::from_secs(30);
#[cfg(test)]
#[path = "ordering_tests.rs"]
mod ordering_tests;
#[cfg(test)]
#[path = "transport_tests.rs"]
mod transport_tests;
#[cfg(test)]
#[path = "start_command_tests.rs"]
mod start_command_tests;
const CANCEL_TIMEOUT: Duration = Duration::from_secs(5);
const QUEUE_LIMIT: usize = 32;
const PENDING_LIMIT: usize = 64;
const PROMPT_LIMIT: usize = 16 * 1024 * 1024;

pub trait AcpStore: Send + Sync {
    fn launch_config(&self, id: &str) -> Result<AcpLaunchConfig, String>;
    fn binding(&self, thread: &str) -> Result<Option<AcpBinding>, String>;
    fn save_binding(&self, binding: &AcpBinding) -> Result<(), String>;
    fn update_binding(&self, binding: &AcpBinding) -> Result<(), String> {
        let old = self.binding(&binding.thread_id)?.ok_or("ACP live binding is missing")?;
        if old.thread_id != binding.thread_id || old.agent_id != binding.agent_id
            || old.revision != binding.revision || old.cwd != binding.cwd
            || old.session_id != binding.session_id || binding.session_id.is_none() {
            return Err("ACP live binding identity changed".into());
        }
        self.save_binding(binding)
    }
}
#[derive(Clone, Serialize)]
pub struct AcpCatalogChanged {
    pub thread_id: String,
    pub catalog: AcpCatalog,
}

fn invalid(message: impl Into<String>) -> ProviderError {
    ProviderError::ValidationError {
        message: message.into(),
    }
}
fn protocol(message: impl Into<String>) -> ProviderError {
    ProviderError::RpcError {
        message: message.into(),
    }
}
fn rpc_error(method: &str, error: &RpcChildError) -> ProviderError {
    // Never forward stderr, raw protocol lines, auth errors or arbitrary agent messages.
    match error {
        RpcChildError::RpcError(e) if e.code == -32000 => ProviderError::NotAuthenticated { provider:ProviderKind::Acp, hint:"setup_required: authenticate this agent in its own CLI, or configure an explicitly advertised agent auth method.".into() },
        RpcChildError::Timeout { .. } => ProviderError::Timeout { operation:format!("ACP {method}"),elapsed_ms:RPC_TIMEOUT.as_millis() as u64 },
        RpcChildError::SpawnFailed(_) => ProviderError::NotInstalled { provider:ProviderKind::Acp,hint:"Check the saved executable and literal arguments; the ACP agent could not be launched.".into() },
        RpcChildError::RpcError(e) => protocol(format!("ACP {method} rejected (code {})",e.code)),
        _ => protocol(format!("ACP {method} transport failed")),
    }
}
fn launch_environment(
    definition: &HashMap<String, String>,
    host: Option<&HashMap<String, String>>,
) -> HashMap<String, String> {
    // The command layer clears caller env before resolving this pane-owned
    // overlay. Saved literal values win over defaults, except these routing
    // identities, which must come from the backend's actual pane binding.
    let mut environment = host.cloned().unwrap_or_default();
    environment.extend(definition.clone());
    for key in ["CODEMUX_WORKSPACE_ID", "CODEMUX_PANE_ID"] {
        if let Some(value) = host.and_then(|env| env.get(key)) {
            environment.insert(key.into(), value.clone());
        }
    }
    environment
}
fn host_policy(mode: Option<&str>) -> Result<(), ProviderError> {
    if mode.is_some_and(|m| m != "supervised") {
        return Err(invalid("Custom ACP uses supervised host approvals only; agent session modes do not bypass permission callbacks."));
    }
    Ok(())
}
fn cursor(binding: &AcpBinding) -> Value {
    let scoped = binding
        .session_id
        .as_deref()
        .map(|native| namespaced_session(&binding.agent_id, &binding.revision, native).0);
    json!({"schemaVersion":1,"agentId":binding.agent_id,"revision":binding.revision,"sessionId":scoped})
}
fn validate_cursor(binding: &AcpBinding, value: &Value) -> Result<(), ProviderError> {
    let expected = cursor(binding);
    if value == &expected {
        return Ok(());
    }
    if let Some(encoded) = value
        .as_str()
        .or_else(|| value.get("resume").and_then(Value::as_str))
    {
        if let Some(native) = &binding.session_id {
            if encoded == namespaced_session(&binding.agent_id, &binding.revision, native).0 {
                return Ok(());
            }
        }
    }
    Err(invalid("unavailable: ACP resume cursor does not match the durable instance, revision and native session binding"))
}

struct Queued {
    id: String,
    input: SendTurnInput,
}
struct Pending {
    rpc_id: Value,
    options: Vec<Value>,
}
struct State {
    status: SessionStatus,
    active: Option<TurnId>,
    interrupted: bool,
    pending: HashMap<RequestId, Pending>,
    queued: VecDeque<Queued>,
    prose: String,
    thoughts: String,
    tools: HashSet<String>,
}
struct Session {
    thread: ThreadId,
    launch: AcpLaunchConfig,
    child: Arc<JsonRpcChild>,
    caps: Mutex<Negotiation>,
    binding: Mutex<AcpBinding>,
    // Accessed under the binding owner. Only catalog updates are deferred;
    // callbacks stay live and inactive-turn load history remains suppressed.
    startup_catalog_updates: Mutex<Option<Vec<SequencedNotification>>>,
    catalog_sequence: AtomicU64,
    state: Mutex<State>,
    control: Mutex<()>,
    callbacks: Mutex<()>,
    barrier: mpsc::Sender<oneshot::Sender<()>>,
    #[cfg(test)]
    prompt_settled_gate: Mutex<()>,
    cancel: Notify,
    ended: AtomicBool,
    durable: AtomicBool,
    visible: bool,
    store: Arc<dyn AcpStore>,
    events: broadcast::Sender<ProviderRuntimeEvent>,
    catalogs: broadcast::Sender<AcpCatalogChanged>,
}
struct Inner {
    sessions: Mutex<HashMap<ThreadId, Arc<Session>>>,
    lifecycle: Mutex<()>,
    store: Arc<dyn AcpStore>,
    events: broadcast::Sender<ProviderRuntimeEvent>,
    catalogs: broadcast::Sender<AcpCatalogChanged>,
}
pub struct GenericAcpProvider {
    inner: Arc<Inner>,
}
#[cfg(test)]
mod parsing_tests {
    use super::*;
    #[test]
    fn saved_literal_environment_wins_except_backend_owned_workspace_routes() {
        let definition = HashMap::from([
            ("FAKE_LITERAL".into(), " literal $(value) ".into()),
            ("CODEMUX_WORKSPACE_ID".into(), "wrong-workspace".into()),
        ]);
        let host = HashMap::from([
            ("CODEMUX_WORKSPACE_ID".into(), "owned-workspace".into()),
            ("CODEMUX_PANE_ID".into(), "owned-pane".into()),
            ("FAKE_LITERAL".into(), "wrong-default".into()),
            ("HOST_CONTEXT".into(), "backend".into()),
        ]);
        let env = launch_environment(&definition, Some(&host));
        assert_eq!(env["CODEMUX_WORKSPACE_ID"], "owned-workspace");
        assert_eq!(env["CODEMUX_PANE_ID"], "owned-pane");
        assert_eq!(env["FAKE_LITERAL"], " literal $(value) ");
        assert_eq!(env["HOST_CONTEXT"], "backend");
        assert_eq!(launch_environment(&definition, None), definition);
    }
    #[test]
    fn command_resume_cursor_uses_durable_namespaced_id() {
        let binding = AcpBinding {
            thread_id: "thread".into(),
            agent_id: "agent".into(),
            revision: "revision".into(),
            cwd: "/unused".into(),
            session_id: Some("same native ID".into()),
            catalog: catalog_from("agent", "Agent", &Negotiation::default(), &json!({})).unwrap(),
            config_values: HashMap::new(),
        };
        let encoded = namespaced_session("agent", "revision", "same native ID").0;
        assert_eq!(cursor(&binding)["sessionId"], json!(encoded), "the canonical SDK-id extractor must never receive a raw, colliding native ACP session ID");
        assert!(
            validate_cursor(&binding, &json!({"resume":encoded,"requireOriginal":true})).is_ok()
        );
        assert!(validate_cursor(&binding, &json!({"resume":"same native ID"})).is_err());
        assert!(validate_cursor(
            &binding,
            &json!({"resume":namespaced_session("other-agent","revision","same native ID").0})
        )
        .is_err());
    }
    #[test]
    fn standard_auth_code_does_not_require_english_messages_or_expose_secrets() {
        let error = RpcChildError::RpcError(RpcError {
            code: -32000,
            message: "private token NON_ENGLISH_DETAIL".into(),
            data: Some(json!({"secret":"hidden"})),
        });
        let mapped = rpc_error("session/new", &error);
        assert!(matches!(mapped, ProviderError::NotAuthenticated { .. }));
        assert!(!mapped.to_string().contains("private token"));
        let exited = rpc_error(
            "session/prompt",
            &RpcChildError::ChildExited {
                code: Some(1),
                stderr_tail: "PRIVATE_STDERR".into(),
            },
        );
        assert!(!exited.to_string().contains("PRIVATE_STDERR"));
    }
}
impl GenericAcpProvider {
    pub fn new(store: Arc<dyn AcpStore>) -> Self {
        let (events, _) = broadcast::channel(2048);
        let (catalogs, _) = broadcast::channel(128);
        Self {
            inner: Arc::new(Inner {
                sessions: Mutex::new(HashMap::new()),
                lifecycle: Mutex::new(()),
                store,
                events,
                catalogs,
            }),
        }
    }
    pub fn catalog_updates(&self) -> broadcast::Receiver<AcpCatalogChanged> {
        self.inner.catalogs.subscribe()
    }
    fn launch(&self, id: &str) -> Result<AcpLaunchConfig, ProviderError> {
        let launch = self.inner.store.launch_config(id).map_err(|_| {
            invalid("unavailable: saved ACP definition or its environment could not be loaded")
        })?;
        if launch.agent.id != id || !launch.agent.enabled {
            return Err(invalid(
                "unavailable: this ACP definition is missing or disabled",
            ));
        }
        Ok(launch)
    }
    async fn session(&self, thread: &ThreadId) -> Result<Arc<Session>, ProviderError> {
        let session = self
            .inner
            .sessions
            .lock()
            .await
            .get(thread)
            .cloned()
            .ok_or_else(|| ProviderError::SessionNotFound {
                thread_id: thread.clone(),
            })?;
        if !session.alive() {
            return Err(ProviderError::SessionClosed {
                thread_id: thread.clone(),
            });
        }
        Ok(session)
    }
    pub async fn probe(&self, agent_id: &str, cwd: PathBuf) -> Result<AcpCatalog, ProviderError> {
        let _lifecycle = self.inner.lifecycle.lock().await;
        let launch = self.launch(agent_id)?;
        let session = Session::open(
            &self.inner,
            ThreadId(format!("acp-probe-{}", Uuid::new_v4())),
            launch,
            cwd,
            None,
            None,
            None,
            false,
        )
        .await?;
        let catalog = session.binding.lock().await.catalog.clone();
        session.close().await?;
        Ok(catalog)
    }
    pub async fn live_catalog(&self, thread: &ThreadId) -> Option<AcpCatalog> {
        let session = self.inner.sessions.lock().await.get(thread).cloned()?;
        let binding = session.binding.lock().await;
        let stored = self.inner.store.binding(&thread.0).ok().flatten()?;
        if stored.thread_id != binding.thread_id || stored.agent_id != binding.agent_id
            || stored.revision != binding.revision || stored.cwd != binding.cwd
            || stored.session_id != binding.session_id || binding.session_id.is_none() {
            return None;
        }
        session.alive().then(|| binding.catalog.clone())
    }
    pub async fn catalog(&self, thread: &ThreadId) -> Result<AcpCatalog, ProviderError> {
        if let Some(session) = self.inner.sessions.lock().await.get(thread).cloned() {
            if session.alive() {
                return Ok(session.binding.lock().await.catalog.clone());
            }
        }
        self.inner
            .store
            .binding(&thread.0)
            .map_err(|_| invalid("ACP binding could not be read"))?
            .map(|binding| binding.catalog)
            .ok_or_else(|| ProviderError::SessionNotFound {
                thread_id: thread.clone(),
            })
    }
    pub async fn set_config(
        &self,
        thread: ThreadId,
        id: String,
        value: Value,
    ) -> Result<AcpCatalog, ProviderError> {
        self.session(&thread).await?.set_config(&id, value).await
    }
    pub async fn disconnect_agent(&self, id: &str) -> Result<(), ProviderError> {
        let _lifecycle = self.inner.lifecycle.lock().await;
        let sessions = {
            let mut sessions = self.inner.sessions.lock().await;
            let threads: Vec<_> = sessions
                .iter()
                .filter(|(_, session)| session.launch.agent.id == id)
                .map(|(thread, _)| thread.clone())
                .collect();
            threads
                .into_iter()
                .filter_map(|thread| sessions.remove(&thread))
                .collect::<Vec<_>>()
        };
        let mut first_error = None;
        for session in sessions {
            if let Err(error) = session.close().await {
                if first_error.is_none() {
                    first_error = Some(error);
                }
            }
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

impl Session {
    fn alive(&self) -> bool {
        !self.ended.load(Ordering::SeqCst) && self.child.is_alive()
    }
    fn emit(&self, event: ProviderRuntimeEvent) {
        if self.visible {
            let _ = self.events.send(event);
        }
    }
    fn warning(&self, message: &str) {
        self.emit(ProviderRuntimeEvent::RuntimeWarning {
            thread_id: Some(self.thread.clone()),
            message: message.into(),
            original_payload: None,
        });
    }
    async fn status(&self, status: SessionStatus) {
        let mut state = self.state.lock().await;
        if self.ended.load(Ordering::SeqCst)
            && !matches!(status, SessionStatus::Closed | SessionStatus::Error { .. })
        {
            return;
        }
        state.status = status.clone();
        self.emit(ProviderRuntimeEvent::SessionStateChanged {
            thread_id: self.thread.clone(),
            status,
        });
    }
    async fn publish_catalog(&self) -> Result<(), ProviderError> {
        let binding = self.binding.lock().await;
        if self.durable.load(Ordering::SeqCst) {
            self.store
                .update_binding(&binding)
                .map_err(|_| protocol("ACP durable binding could not be saved"))?;
            let _ = self.catalogs.send(AcpCatalogChanged {
                thread_id: self.thread.0.clone(),
                catalog: binding.catalog.clone(),
            });
        }
        Ok(())
    }
    async fn snapshot(&self) -> ProviderSession {
        let binding = self.binding.lock().await;
        ProviderSession {
            thread_id: self.thread.clone(),
            provider: ProviderKind::Acp,
            session_id: namespaced_session(
                &binding.agent_id,
                &binding.revision,
                binding.session_id.as_deref().unwrap_or(""),
            ),
            status: self.state.lock().await.status.clone(),
            resume_cursor: Some(cursor(&binding)),
        }
    }
    #[allow(clippy::too_many_arguments)]
    async fn open(
        inner: &Arc<Inner>,
        thread: ThreadId,
        launch: AcpLaunchConfig,
        cwd: PathBuf,
        saved: Option<AcpBinding>,
        model: Option<String>,
        effort: Option<String>,
        durable: bool,
    ) -> Result<Arc<Self>, ProviderError> {
        if !cwd.is_absolute() || !cwd.is_dir() || cwd.to_str().is_none() {
            return Err(invalid(
                "ACP requires an existing absolute working directory",
            ));
        }
        let child = Arc::new(
            JsonRpcChild::spawn_strict(SpawnConfig {
                program: PathBuf::from(&launch.agent.executable),
                args: launch.agent.args.clone(),
                env: launch.environment.clone(),
                cwd: Some(cwd.clone()),
                default_timeout: RPC_TIMEOUT,
            })
            .await
            .map_err(|e| rpc_error("launch", &e))?,
        );
        let notifications = child.sequenced_notifications();
        let requests = match child.incoming_requests() {
            Some(rx) => rx,
            None => {
                let _ = child.shutdown().await;
                return Err(protocol("ACP callback receiver unavailable"));
            }
        };
        let (barrier, barriers) = mpsc::channel(8);
        let initial = catalog_from(
            &launch.agent.id,
            &launch.agent.name,
            &Negotiation::default(),
            &json!({}),
        )
        .map_err(protocol)?;
        let binding = saved.unwrap_or_else(|| AcpBinding {
            thread_id: thread.0.clone(),
            agent_id: launch.agent.id.clone(),
            revision: launch.agent.revision.clone(),
            cwd: cwd.to_string_lossy().into_owned(),
            session_id: None,
            catalog: initial,
            config_values: HashMap::new(),
        });
        let session = Arc::new(Self {
            thread,
            launch,
            child,
            caps: Mutex::new(Negotiation::default()),
            binding: Mutex::new(binding),
            startup_catalog_updates: Mutex::new(Some(Vec::new())),
            catalog_sequence: AtomicU64::new(0),
            state: Mutex::new(State {
                status: SessionStatus::Starting,
                active: None,
                interrupted: false,
                pending: HashMap::new(),
                queued: VecDeque::new(),
                prose: String::new(),
                thoughts: String::new(),
                tools: HashSet::new(),
            }),
            control: Mutex::new(()),
            callbacks: Mutex::new(()),
            barrier,
            #[cfg(test)]
            prompt_settled_gate: Mutex::new(()),
            cancel: Notify::new(),
            ended: AtomicBool::new(false),
            durable: AtomicBool::new(false),
            visible: durable,
            store: inner.store.clone(),
            events: inner.events.clone(),
            catalogs: inner.catalogs.clone(),
        });
        Self::pump(&session, notifications, requests, barriers);
        let setup = async {
            let initialized=session.request("initialize",json!({"protocolVersion":1,"clientInfo":{"name":"codemux","version":env!("CARGO_PKG_VERSION")},"clientCapabilities":{"session":{"configOptions":{"boolean":{}}}}})).await?;
            let caps=Negotiation::parse(&initialized).map_err(protocol)?;
            if let Some(method) = session.launch.agent.auth_method.as_deref() {
                let advertised=caps.auth_methods.iter().find(|auth|auth.id == method).ok_or_else(||invalid("setup_required: configured authentication method is not advertised"))?;
                if advertised.kind != "agent" { return Err(invalid("setup_required: only explicitly advertised agent-managed authentication is supported; terminal authentication must be completed in the agent CLI")); }
                let authenticated=session.request("authenticate",json!({"methodId":method})).await?;
                if !authenticated.is_object() {return Err(protocol("ACP authenticate response is malformed"));}
            }
            *session.caps.lock().await=caps.clone();
            let (native,intent,legacy_model)={let b=session.binding.lock().await;
                let legacy=if b.session_id.is_some() && semantic_option(&b.catalog.config_options,"model").is_none() && !b.catalog.capabilities.models.is_empty() {b.catalog.current_model.clone()} else {None};
                (b.session_id.clone(),b.config_values.clone(),legacy)};
            let mut params=json!({"cwd":cwd,"mcpServers":[]});
            let method=if let Some(native)=&native {params["sessionId"]=json!(native);caps.resume_method().map_err(invalid)?} else {"session/new"};
            let (response,sequence)=session.request_ordered(method,params).await?;
            // Pumps are running before new/load/resume. Active=None suppresses replay.
            session.drain().await?;
            if !response.is_object() { return Err(protocol("ACP session response must be an object")); }
            let native=match native {Some(native)=>{
                if text(&response,"sessionId").is_some_and(|reported|reported != native) {return Err(protocol("ACP resumed session identity changed"));}
                native
            },None=>text(&response,"sessionId").ok_or_else(||protocol("ACP new session did not return a native session ID"))?.to_owned()};
            let startup_updates={
                let mut binding=session.binding.lock().await;
                let catalog=catalog_from(&binding.agent_id,&session.launch.agent.name,&caps,&response).map_err(protocol)?;
                binding.session_id=Some(native);
                if sequence > session.catalog_sequence.load(Ordering::SeqCst) {
                    binding.catalog=catalog;
                    session.catalog_sequence.store(sequence,Ordering::SeqCst);
                }
                session.startup_catalog_updates.lock().await.take().unwrap_or_default()
            };
            for update in startup_updates {session.notification(update).await;}
            session.drain().await?;
            if let Some(model)=legacy_model {session.set_model(model).await?;}
            // Restore exact persisted intent, model first because its change can replace controls.
            let model_id={let b=session.binding.lock().await;semantic_option(&b.catalog.config_options,"model").map(|o|o.id.clone())};
            let mut intent:Vec<_>=intent.into_iter().collect();
            intent.sort_by(|(a,_),(b,_)|(Some(a) != model_id.as_ref()).cmp(&(Some(b) != model_id.as_ref())).then(a.cmp(b)));
            for (id,value) in intent {
                let advertised = {let b=session.binding.lock().await;
                    b.catalog.config_options.iter().find(|o|o.id==id)
                        .is_some_and(|o|validate_value(o,&value).is_ok())};
                if advertised {session.set_config(&id,value).await?;}
            }
            {session.binding.lock().await.reconcile_config_values();}
            if let Some(model)=model {session.set_model(model).await?;}
            if let Some(effort)=effort {session.set_effort(effort).await?;}
            if !session.alive() {return Err(protocol("ACP transport ended during setup"));}
            if durable {
                {let binding=session.binding.lock().await;session.store.save_binding(&binding).map_err(|_|protocol("ACP durable binding could not be saved"))?;}
                session.durable.store(true,Ordering::SeqCst);
                session.publish_catalog().await?;
                let snapshot=session.snapshot().await;
                session.emit(ProviderRuntimeEvent::SessionConfigured {thread_id:session.thread.clone(),provider_session_id:snapshot.session_id});
                session.emit(ProviderRuntimeEvent::ResumeCursorUpdated {thread_id:session.thread.clone(),resume_cursor:snapshot.resume_cursor.unwrap()});
                session.warning("Custom ACP uses the agent's own cwd tools. Codemux-specific MCP injection is unavailable; saved definition edits affect future launches, not this running process.");
            }
            session.status(SessionStatus::Ready).await;
            Ok::<(),ProviderError>(())
        }.await;
        if let Err(error) = setup {
            session.fail("ACP session setup failed").await;
            return Err(error);
        }
        Ok(session)
    }
    async fn request(&self, method: &str, params: Value) -> Result<Value, ProviderError> {
        self.request_ordered(method, params).await.map(|(value, _)| value)
    }
    async fn request_ordered(&self, method: &str, params: Value) -> Result<(Value, u64), ProviderError> {
        if !self.alive() {
            return Err(ProviderError::SessionClosed {
                thread_id: self.thread.clone(),
            });
        }
        let response = tokio::time::timeout(RPC_TIMEOUT, self.child.request_with_sequence(method, params, RPC_TIMEOUT)).await;
        let response = response.unwrap_or_else(|_| {
            Err(RpcChildError::Timeout {
                method: method.into(),
                elapsed: RPC_TIMEOUT,
            })
        });
        match response {
            Ok(value) => Ok(value),
            Err(error) => {
                let exposed = if self.child.is_alive() {
                    rpc_error(method, &error)
                } else {
                    protocol(format!("ACP {method} transport failed"))
                };
                if !matches!(error, RpcChildError::RpcError(_)) || !self.child.is_alive() {
                    self.fail(&format!("ACP {method} transport failed")).await;
                }
                Err(exposed)
            }
        }
    }
    fn pump(
        this: &Arc<Self>,
        mut notifications: broadcast::Receiver<SequencedNotification>,
        mut requests: mpsc::Receiver<IncomingRequest>,
        mut barriers: mpsc::Receiver<oneshot::Sender<()>>,
    ) {
        let weak = Arc::downgrade(this);
        let child = this.child.clone();
        let mut failures = child.transport_failures();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_millis(50));
            let mut health_open = true;
            let mut requests_open = true;
            let mut exit_cleanup_started = false;
            if failures.borrow_and_update().is_some() {
                if let Some(session) = weak.upgrade() { session.fail("ACP transport protocol failed").await; }
                return;
            }
            loop {
                tokio::select! {
                    biased;
                    changed=failures.changed(), if health_open=>{
                        if changed.is_err() { health_open=false; }
                        else if failures.borrow_and_update().is_some() {
                            if let Some(session)=weak.upgrade() { session.fail("ACP transport protocol failed").await; }
                            break;
                        }
                    },
                    event=notifications.recv()=>{
                        let Some(session)=weak.upgrade() else {break;};
                        match event {Ok(event)=>session.notification(event).await,Err(broadcast::error::RecvError::Lagged(_))=>{session.fail("ACP notification stream overflowed").await;break;},Err(_)=>break}
                    },
                    request=requests.recv(), if requests_open=>{
                        let Some(session)=weak.upgrade() else {break;};
                        // Closure follows transport routing, not consumption by
                        // the awakened prompt worker. Keep its barrier alive.
                        match request {Some(request)=>session.callback(request).await,None=>requests_open=false}
                    },
                    barrier=barriers.recv()=>{match barrier {Some(ack)=>{let _=ack.send(());},None=>break}},
                    _=tick.tick()=>{
                        let Some(session)=weak.upgrade() else {break;};
                        if session.ended.load(Ordering::SeqCst) {break;}
                        if !child.is_alive() {
                            if session.state.lock().await.active.is_none() {
                                session.fail("ACP process exited").await;
                                break;
                            }
                            // EOF may precede actual process exit. Start bounded
                            // owned cleanup so unanswered prompts also settle,
                            // but let the active worker consume its result and
                            // queued updates before classifying that turn.
                            if !exit_cleanup_started {
                                exit_cleanup_started=true;
                                let child=child.clone();
                                tokio::spawn(async move {let _=child.shutdown().await;});
                            }
                        }
                    }
                }
            }
            // Do not race the owning close operation and its negotiated close RPC.
            if weak.upgrade().is_none() {
                let _ = child.shutdown().await;
            }
        });
    }
    async fn drain(&self) -> Result<(), ProviderError> {
        let (tx, rx) = oneshot::channel();
        let result = tokio::time::timeout(CANCEL_TIMEOUT, async {
            self.barrier.send(tx).await.map_err(|_| ())?;
            rx.await.map_err(|_| ())
        })
        .await;
        if matches!(result, Ok(Ok(()))) {
            Ok(())
        } else {
            self.fail("ACP message drain failed").await;
            Err(protocol("ACP message drain failed"))
        }
    }
    fn parse_catalog_update(base: &AcpCatalog, caps: &Negotiation, response: &Value, legacy_model: Option<&str>) -> Result<AcpCatalog, ProviderError> {
        let mut source = json!({"configOptions":base.config_options.iter().map(|o|json!({"id":o.id,"name":o.name,"description":o.description,"category":o.category,"type":o.kind,"currentValue":o.current_value,"options":o.options})).collect::<Vec<_>>(),"models":{"currentModelId":base.current_model,"availableModels":base.capabilities.models.iter().map(|m|json!({"modelId":m.id,"name":m.label,"description":m.description})).collect::<Vec<_>>()}});
        if let Some(model) = legacy_model { source["models"]["currentModelId"] = json!(model); }
        if let Some(options) = response.get("configOptions") { source["configOptions"] = options.clone(); }
        if let Some(models) = response.get("models") { source["models"] = models.clone(); }
        catalog_from(&base.agent_id, &base.agent_name, caps, &source).map_err(protocol)
    }
    async fn apply_catalog(&self, response: &Value, sequence: u64, legacy_model: Option<&str>, response_base: Option<&AcpCatalog>) -> Result<AcpCatalog, ProviderError> {
        let caps = self.caps.lock().await.clone();
        {
            let mut binding = self.binding.lock().await;
            // Intrinsic ACK validation uses request-era membership even when
            // a later notification has retired the acknowledged model.
            let catalog = Self::parse_catalog_update(response_base.unwrap_or(&binding.catalog), &caps, response, legacy_model)?;
            if response_base.is_some() && !self.alive() {
                return Err(protocol("ACP transport ended during configuration"));
            }
            // A later notification may already have been consumed while this
            // RPC caller awaited its drain barrier. Position comes from the
            // actual reader, not from task scheduling or peer-supplied data.
            if sequence <= self.catalog_sequence.load(Ordering::SeqCst) {
                return Ok(binding.catalog.clone());
            }
            // A fresh ACK owns only its present groups and the acknowledged
            // legacy model. Omitted fields inherit the latest owner state, not
            // the request snapshot. Re-parse this merge so a newer ACK cannot
            // publish B if an earlier D retired B from the current membership.
            binding.catalog = if response_base.is_some() {
                Self::parse_catalog_update(&binding.catalog, &caps, response, legacy_model)?
            } else { catalog };
            self.catalog_sequence.store(sequence, Ordering::SeqCst);
            binding.reconcile_config_values();
        }
        self.publish_catalog().await?;
        Ok(self.binding.lock().await.catalog.clone())
    }
    async fn set_config(&self, id: &str, value: Value) -> Result<AcpCatalog, ProviderError> {
        let _control = self.control.lock().await;
        if !self.alive() {
            return Err(ProviderError::SessionClosed {
                thread_id: self.thread.clone(),
            });
        }
        let (native, kind, response_base) = {
            let b = self.binding.lock().await;
            let o = b
                .catalog
                .config_options
                .iter()
                .find(|o| o.id == id)
                .ok_or_else(|| invalid("ACP configuration option is not advertised"))?;
            validate_value(o, &value).map_err(invalid)?;
            (
                b.session_id
                    .clone()
                    .ok_or_else(|| protocol("ACP session identity is unavailable"))?,
                o.kind.clone(),
                b.catalog.clone(),
            )
        };
        let mut params = json!({"sessionId":native,"configId":id,"value":value});
        if kind == "boolean" {
            params["type"] = json!("boolean");
        }
        let (response, sequence) = self.request_ordered("session/set_config_option", params).await?;
        self.drain().await?;
        if !response.get("configOptions").is_some_and(Value::is_array) {
            self.fail("ACP set_config_option returned an invalid catalog")
                .await;
            return Err(protocol(
                "ACP set_config_option returned an invalid catalog",
            ));
        }
        let catalog = match self.apply_catalog(&response, sequence, None, Some(&response_base)).await {
            Ok(catalog) => catalog,
            Err(error) => {
                self.fail("ACP configuration response was invalid").await;
                return Err(error);
            }
        };
        // Save the acknowledged intent, not merely a requested value.
        {
            let mut b = self.binding.lock().await;
            if let Some(option) = b.catalog.config_options.iter().find(|o| o.id == id) {
                if validate_value(option, &option.current_value).is_ok() {
                    let acknowledged = option.current_value.clone();
                    b.config_values.insert(id.into(), acknowledged);
                }
            }
        }
        self.publish_catalog().await?;
        Ok(catalog)
    }
    async fn set_model(&self, model: String) -> Result<(), ProviderError> {
        let selector = {
            let b = self.binding.lock().await;
            if !b.catalog.capabilities.models.iter().any(|m| m.id == model) {
                return Err(invalid(
                    "ACP model is not advertised; leave model unset to use the agent's default",
                ));
            }
            semantic_option(&b.catalog.config_options, "model").map(|o| o.id.clone())
        };
        if let Some(id) = selector {
            self.set_config(&id, json!(model)).await?;
            return Ok(());
        }
        let _control = self.control.lock().await;
        let (native, response_base) = {
            let binding = self.binding.lock().await;
            if !binding.catalog.capabilities.models.iter().any(|m| m.id == model) {
                return Err(invalid("ACP model is not advertised"));
            }
            (binding.session_id.clone().ok_or_else(|| protocol("ACP session identity unavailable"))?, binding.catalog.clone())
        };
        let (response, sequence) = self
            .request_ordered(
                "session/set_model",
                json!({"sessionId":native,"modelId":model}),
            )
            .await?;
        self.drain().await?;
        if !response.is_object() {
            self.fail("ACP set_model response was invalid").await;
            return Err(protocol("ACP set_model response was invalid"));
        }
        if let Err(error) = self.apply_catalog(&response, sequence, Some(&model), Some(&response_base)).await {
            self.fail("ACP model response was invalid").await;
            return Err(error);
        }
        // Legacy model intent cannot masquerade as a config ID. Native resume
        // restores it; subsequent startup model changes remain explicitly validated.
        self.publish_catalog().await?;
        Ok(())
    }
    async fn set_effort(&self, value: String) -> Result<(), ProviderError> {
        let id = {
            let b = self.binding.lock().await;
            semantic_option(&b.catalog.config_options, "thought_level")
                .map(|o| o.id.clone())
                .ok_or_else(|| invalid("ACP agent did not advertise a reasoning control"))?
        };
        self.set_config(&id, json!(value)).await?;
        Ok(())
    }
    async fn answer(&self, id: Value, value: Result<Value, RpcError>) -> bool {
        matches!(
            tokio::time::timeout(CANCEL_TIMEOUT, self.child.respond(id, value)).await,
            Ok(Ok(()))
        )
    }
    async fn callback(&self, request: IncomingRequest) {
        let _callbacks = self.callbacks.lock().await;
        if request.method != "session/request_permission" {
            if !self
                .answer(
                    request.id,
                    Err(RpcError {
                        code: -32601,
                        message: "Client method is not supported by Codemux".into(),
                        data: None,
                    }),
                )
                .await
            {
                self.fail_without_callbacks("ACP callback response transport failed")
                    .await;
            }
            return;
        }
        let native = self.binding.lock().await.session_id.clone();
        let cancelled = json!({"outcome":{"outcome":"cancelled"}});
        if self.ended.load(Ordering::SeqCst)
            || native
                .as_deref()
                .is_none_or(|id| Some(id) != text(&request.params, "sessionId"))
        {
            if !self.answer(request.id, Ok(cancelled)).await {
                self.fail_without_callbacks("ACP callback response transport failed")
                    .await;
            }
            return;
        }
        let options = match request.params.get("options").and_then(Value::as_array) {
            Some(options) if !options.is_empty() => options.clone(),
            _ => {
                let delivered = self
                    .answer(
                        request.id,
                        Err(RpcError {
                            code: -32602,
                            message: "Invalid permission options".into(),
                            data: None,
                        }),
                    )
                    .await;
                if !delivered {
                    self.fail_without_callbacks("ACP callback response transport failed")
                        .await;
                }
                return;
            }
        };
        if options.iter().enumerate().any(|(index, o)| {
            text(o, "optionId").is_none()
                || text(o, "kind").is_none()
                || options[..index]
                    .iter()
                    .any(|other| text(other, "optionId") == text(o, "optionId"))
        }) {
            if !self
                .answer(
                    request.id,
                    Err(RpcError {
                        code: -32602,
                        message: "Invalid permission option identifiers".into(),
                        data: None,
                    }),
                )
                .await
            {
                self.fail_without_callbacks("ACP callback response transport failed")
                    .await;
            }
            return;
        }
        let mut state = self.state.lock().await;
        let Some(turn) = state.active.clone().filter(|_| !state.interrupted) else {
            drop(state);
            if !self.answer(request.id, Ok(cancelled)).await {
                self.fail_without_callbacks("ACP callback response transport failed")
                    .await;
            }
            return;
        };
        if state.pending.len() >= PENDING_LIMIT {
            drop(state);
            let _ = self.answer(request.id, Ok(cancelled)).await;
            self.fail_without_callbacks("ACP permission callback limit exceeded")
                .await;
            return;
        }
        let request_id = RequestId(format!("acp-request-{}", Uuid::new_v4()));
        state.pending.insert(
            request_id.clone(),
            Pending {
                rpc_id: request.id,
                options,
            },
        );
        state.status = SessionStatus::WaitingApproval {
            request_id: request_id.clone(),
        };
        self.emit(ProviderRuntimeEvent::RequestOpened {
            thread_id: self.thread.clone(),
            turn_id: turn,
            request_id: request_id.clone(),
            request_kind: "tool_approval".into(),
            tool_use_id: request
                .params
                .pointer("/toolCall/toolCallId")
                .and_then(Value::as_str)
                .map(str::to_owned),
            payload: request.params,
            subagent_id: None,
        });
        self.emit(ProviderRuntimeEvent::SessionStateChanged {
            thread_id: self.thread.clone(),
            status: state.status.clone(),
        });
    }
    async fn cancel_callbacks_locked(&self) -> bool {
        let pending = { self.state.lock().await.pending.drain().collect::<Vec<_>>() };
        let deadline = tokio::time::Instant::now() + CANCEL_TIMEOUT;
        let mut all_delivered = true;
        for (id, pending) in pending {
            let delivered = matches!(
                tokio::time::timeout_at(
                    deadline,
                    self.child.respond(
                        pending.rpc_id,
                        Ok(json!({"outcome":{"outcome":"cancelled"}}))
                    )
                )
                .await,
                Ok(Ok(()))
            );
            all_delivered &= delivered;
            if delivered {
                self.emit(ProviderRuntimeEvent::RequestResolved {
                    thread_id: self.thread.clone(),
                    request_id: id,
                    decision: ApprovalDecision::Cancel,
                });
            } else {
                self.emit(ProviderRuntimeEvent::RequestResponseFailed {
                    thread_id: self.thread.clone(),
                    request_id: id,
                    reason: RequestResponseFailureReason::StaleProviderCallback,
                    message: "ACP process-local callback is no longer reachable".into(),
                });
            }
        }
        all_delivered
    }
    async fn respond(
        &self,
        id: RequestId,
        decision: ApprovalDecision,
    ) -> Result<(), ProviderError> {
        let _callbacks = self.callbacks.lock().await;
        let (pending, outcome) = {
            let mut s = self.state.lock().await;
            // Interrupt/close publish cancellation before waiting on callbacks.
            // Recheck after acquiring the gate; leave the pending callback for
            // their cancellation drain instead of delivering a queued approval.
            if self.ended.load(Ordering::SeqCst) || s.interrupted {
                return Err(ProviderError::RequestNotPending { request_id: id });
            }
            let pending = s
                .pending
                .get(&id)
                .ok_or_else(|| ProviderError::RequestNotPending {
                    request_id: id.clone(),
                })?;
            let outcome = permission_outcome(&pending.options, &decision).map_err(invalid)?;
            (s.pending.remove(&id).unwrap(), outcome)
        };
        if !self.answer(pending.rpc_id, Ok(outcome)).await {
            self.emit(ProviderRuntimeEvent::RequestResponseFailed {
                thread_id: self.thread.clone(),
                request_id: id,
                reason: RequestResponseFailureReason::StaleProviderCallback,
                message: "ACP approval response could not be delivered".into(),
            });
            self.fail_without_callbacks("ACP approval response transport failed")
                .await;
            return Err(protocol("ACP approval response transport failed"));
        }
        if matches!(decision, ApprovalDecision::Cancel) {
            self.state.lock().await.interrupted = true;
            self.cancel.notify_one();
            if !self.cancel_callbacks_locked().await {
                self.fail_without_callbacks("ACP cancellation callback transport failed")
                    .await;
                return Err(protocol("ACP cancellation callback transport failed"));
            }
        }
        self.emit(ProviderRuntimeEvent::RequestResolved {
            thread_id: self.thread.clone(),
            request_id: id,
            decision,
        });
        let status = {
            let mut s = self.state.lock().await;
            let status = if let Some(id) = s.pending.keys().next() {
                SessionStatus::WaitingApproval {
                    request_id: id.clone(),
                }
            } else if let Some(turn) = &s.active {
                SessionStatus::Running {
                    active_turn: turn.clone(),
                }
            } else {
                SessionStatus::Ready
            };
            s.status = status.clone();
            status
        };
        self.emit(ProviderRuntimeEvent::SessionStateChanged {
            thread_id: self.thread.clone(),
            status,
        });
        Ok(())
    }
    async fn notification(&self, incoming: SequencedNotification) {
        if self.ended.load(Ordering::SeqCst) {
            return;
        }

        if incoming.notification.method != "session/update" {
            self.warning("Unknown ACP notification was not translated");
            return;
        }
        let native = {
            let binding = self.binding.lock().await;
            // Cold resume/load already owns N. Foreign or missing identities
            // cannot consume N's startup budget, even before its RPC settles.
            // New-session identity is unknown, so its queue remains bounded.
            if binding.session_id.as_deref().is_some_and(|id| Some(id) != text(&incoming.notification.params, "sessionId")) {
                return;
            }
            if incoming.notification.params.pointer("/update/sessionUpdate").and_then(Value::as_str) == Some("config_option_update") {
                let mut startup = self.startup_catalog_updates.lock().await;
                if let Some(updates) = startup.as_mut() {
                    if updates.len() >= PENDING_LIMIT {
                        drop(startup);
                        drop(binding);
                        self.fail("ACP startup catalog stream overflowed").await;
                    } else {
                        updates.push(incoming);
                    }
                    return;
                }
            }
            binding.session_id.clone()
        };
        let SequencedNotification { sequence, notification } = incoming;
        if native
            .as_deref()
            .is_none_or(|id| Some(id) != text(&notification.params, "sessionId"))
        {
            return;
        }
        let update = match notification.params.get("update").filter(|u| u.is_object()) {
            Some(update) => update,
            None => {
                self.fail("ACP session update is malformed").await;
                return;
            }
        };
        let Some(kind) = text(update, "sessionUpdate") else {
            self.fail("ACP session update has no type").await;
            return;
        };
        if kind == "config_option_update" {
            if !update.get("configOptions").is_some_and(Value::is_array)
                || self.apply_catalog(update, sequence, None, None).await.is_err()
            {
                self.fail("ACP configuration update is malformed").await;
            }
            return;
        }
        if kind == "current_mode_update" {
            let Some(mode) = text(update, "currentModeId") else {
                self.fail("ACP mode update is malformed").await;
                return;
            };
            {
                let mut b = self.binding.lock().await;
                for option in &mut b.catalog.config_options {
                    if option.category.as_deref() == Some("mode") {
                        option.current_value = json!(mode);
                    }
                }
            }
            if self.publish_catalog().await.is_err() {
                self.fail("ACP catalog persistence failed").await;
            }
            return;
        }
        let mut s = self.state.lock().await;
        let Some(turn) = s.active.clone() else {
            return;
        }; // Do not replay native load history into Codemux history.
        match kind {
            "agent_message_chunk" | "agent_thought_chunk" => {
                if update.pointer("/content/type").and_then(Value::as_str) != Some("text") {
                    drop(s);
                    self.warning("ACP non-text assistant content is not rendered");
                    return;
                }
                let Some(chunk) = update.pointer("/content/text").and_then(Value::as_str) else {
                    drop(s);
                    self.fail("ACP text update is malformed").await;
                    return;
                };
                let thinking = kind == "agent_thought_chunk";
                let buffer = if thinking {
                    &mut s.thoughts
                } else {
                    &mut s.prose
                };
                if buffer.len().saturating_add(chunk.len()) > PROMPT_LIMIT {
                    drop(s);
                    self.fail("ACP assistant output limit exceeded").await;
                    return;
                }
                buffer.push_str(chunk);
                self.emit(ProviderRuntimeEvent::ContentDelta {
                    thread_id: self.thread.clone(),
                    turn_id: turn,
                    delta: if thinking {
                        ContentDelta::Thinking { text: chunk.into() }
                    } else {
                        ContentDelta::Text { text: chunk.into() }
                    },
                    subagent_id: None,
                });
            }
            "tool_call" | "tool_call_update" => {
                let Some(id) = text(update, "toolCallId") else {
                    drop(s);
                    self.fail("ACP tool update is malformed").await;
                    return;
                };
                if kind == "tool_call" && s.tools.insert(id.into()) {
                    self.emit(ProviderRuntimeEvent::ItemCompleted {
                        thread_id: self.thread.clone(),
                        turn_id: turn.clone(),
                        item: CompletedItem::ToolUse {
                            tool_name: text(update, "title").unwrap_or("ACP tool").into(),
                            input: update.get("rawInput").cloned().unwrap_or(Value::Null),
                            tool_use_id: id.into(),
                        },
                        subagent_id: None,
                    });
                }
                if matches!(text(update, "status"), Some("completed" | "failed")) {
                    self.emit(ProviderRuntimeEvent::ItemCompleted {
                        thread_id: self.thread.clone(),
                        turn_id: turn,
                        item: CompletedItem::ToolResult {
                            tool_use_id: id.into(),
                            content: update
                                .get("rawOutput")
                                .or_else(|| update.get("content"))
                                .cloned()
                                .unwrap_or(Value::Null),
                            is_error: text(update, "status") == Some("failed"),
                        },
                        subagent_id: None,
                    });
                }
            }
            "plan" => {
                let tasks = update
                    .get("entries")
                    .and_then(Value::as_array)
                    .map(|entries| {
                        entries
                            .iter()
                            .enumerate()
                            .filter_map(|(index, entry)| {
                                Some(TaskSnapshotItem {
                                    task_id: format!("acp-plan-{index}"),
                                    title: text(entry, "content")?.into(),
                                    status: match text(entry, "status") {
                                        Some("completed") => TaskStatus::Completed,
                                        Some("in_progress") => TaskStatus::InProgress,
                                        _ => TaskStatus::Pending,
                                    },
                                    detail: None,
                                    blocked_by: vec![],
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default();
                self.emit(ProviderRuntimeEvent::TasksUpdated {
                    thread_id: self.thread.clone(),
                    tasks: TasksSnapshot {
                        explanation: None,
                        tasks,
                    },
                });
            }
            "user_message_chunk" => {}
            _ => {
                drop(s);
                self.warning("Unsupported ACP session update was not translated");
            }
        }
    }
    fn flush(&self, state: &mut State, turn: &TurnId) {
        for (thinking, value) in [
            (true, std::mem::take(&mut state.thoughts)),
            (false, std::mem::take(&mut state.prose)),
        ] {
            if !value.is_empty() {
                self.emit(ProviderRuntimeEvent::ItemCompleted {
                    thread_id: self.thread.clone(),
                    turn_id: turn.clone(),
                    item: if thinking {
                        CompletedItem::AssistantThinking { text: value }
                    } else {
                        CompletedItem::AssistantText { text: value }
                    },
                    subagent_id: None,
                });
            }
        }
        state.tools.clear();
    }
    async fn fail(&self, message: &str) {
        let _callbacks = self.callbacks.lock().await;
        self.fail_without_callbacks(message).await;
    }
    async fn fail_without_callbacks(&self, message: &str) {
        if self.ended.swap(true, Ordering::SeqCst) {
            return;
        }
        self.cancel.notify_one();
        self.cancel_callbacks_locked().await;
        let mut s = self.state.lock().await;
        if let Some(turn) = s.active.take() {
            self.flush(&mut s, &turn);
            self.emit(ProviderRuntimeEvent::TurnCompleted {
                thread_id: self.thread.clone(),
                turn_id: turn,
                status: TurnStatus::Error {
                    subtype: CHILD_EXITED_SUBTYPE.into(),
                    message: message.into(),
                },
                usage: None,
            });
        }
        for queued in s.queued.drain(..) {
            self.emit(ProviderRuntimeEvent::QueuedTurnCancelled {
                thread_id: self.thread.clone(),
                queued_id: queued.id,
            });
        }
        s.status = SessionStatus::Error {
            message: message.into(),
        };
        self.emit(ProviderRuntimeEvent::SessionStateChanged {
            thread_id: self.thread.clone(),
            status: s.status.clone(),
        });
        drop(s);
        let _ = self.child.shutdown().await;
    }
    async fn close(&self) -> Result<(), ProviderError> {
        if self.ended.swap(true, Ordering::SeqCst) {
            let _ = self.child.shutdown().await;
            return Ok(());
        }
        self.cancel.notify_one();
        {
            let _callbacks = self.callbacks.lock().await;
            self.cancel_callbacks_locked().await;
        }
        {
            let mut s = self.state.lock().await;
            if let Some(turn) = s.active.take() {
                self.flush(&mut s, &turn);
                self.emit(ProviderRuntimeEvent::TurnCompleted {
                    thread_id: self.thread.clone(),
                    turn_id: turn,
                    status: TurnStatus::Error {
                        subtype: "cancelled".into(),
                        message: "ACP session stopped".into(),
                    },
                    usage: None,
                });
            }
            for queued in s.queued.drain(..) {
                self.emit(ProviderRuntimeEvent::QueuedTurnCancelled {
                    thread_id: self.thread.clone(),
                    queued_id: queued.id,
                });
            }
        }
        let native = self.binding.lock().await.session_id.clone();
        let supports_close = self.caps.lock().await.close;
        let mut result = Ok(());
        if supports_close && self.child.is_alive() {
            if let Some(native) = native {
                match tokio::time::timeout(
                    CANCEL_TIMEOUT,
                    self.child.request_with_timeout(
                        "session/close",
                        json!({"sessionId":native}),
                        CANCEL_TIMEOUT,
                    ),
                )
                .await
                {
                    Ok(Ok(value)) if value.is_object() => {}
                    Ok(Ok(_)) => result = Err(protocol("ACP close response was invalid")),
                    Ok(Err(e)) => result = Err(rpc_error("session/close", &e)),
                    Err(_) => result = Err(protocol("ACP close timed out")),
                }
            }
        }
        let _ = self.child.shutdown().await;
        self.status(SessionStatus::Closed).await;
        result
    }
    async fn interrupt(&self, turn: Option<TurnId>) -> Result<(), ProviderError> {
        {
            let mut s = self.state.lock().await;
            if s.active.is_none()
                || turn
                    .as_ref()
                    .is_some_and(|turn| Some(turn) != s.active.as_ref())
            {
                return Ok(());
            }
            s.interrupted = true;
        }
        self.cancel.notify_one();
        // Resolve callbacks immediately, before waiting for prompt settlement.
        // A batch shares one deadline, not 64 independent five-second waits.
        {
            let _callbacks = self.callbacks.lock().await;
            if !self.cancel_callbacks_locked().await {
                self.fail_without_callbacks("ACP cancellation callback transport failed")
                    .await;
                return Err(protocol("ACP cancellation callback transport failed"));
            }
        }
        Ok(())
    }
    async fn validate_turn(&self, input: &SendTurnInput) -> Result<(), ProviderError> {
        host_policy(input.permission_mode_override.as_deref())?;
        if !input.skill_invocations.is_empty() {
            return Err(invalid("Custom ACP does not support Codemux skill injection; use the agent's own cwd skills"));
        }
        let b = self.binding.lock().await;
        if !input.images.is_empty() && !b.catalog.supports_images {
            return Err(invalid("ACP agent did not advertise image prompt support"));
        }
        let size = input.images.iter().fold(input.text.len(), |sum, image| {
            sum.saturating_add(image.data.len())
        });
        if size > PROMPT_LIMIT {
            return Err(invalid("ACP prompt exceeds the attachment/text size limit"));
        }
        if input
            .images
            .iter()
            .any(|i| !i.media_type.starts_with("image/") || i.data.is_empty())
        {
            return Err(invalid(
                "ACP image attachment has no image MIME type or content",
            ));
        }
        if input
            .model_override
            .as_ref()
            .is_some_and(|model| !b.catalog.capabilities.models.iter().any(|m| &m.id == model))
        {
            return Err(invalid("ACP per-turn model is not advertised"));
        }
        if let Some(value) = &input.effort_override {
            let option = semantic_option(&b.catalog.config_options, "thought_level")
                .ok_or_else(|| invalid("ACP reasoning control is not advertised"))?;
            validate_value(option, &json!(value)).map_err(invalid)?;
        }
        Ok(())
    }
    async fn send(
        self: &Arc<Self>,
        input: SendTurnInput,
    ) -> Result<TurnStartResult, ProviderError> {
        self.validate_turn(&input).await?;
        let mut s = self.state.lock().await;
        if !self.alive() {
            return Err(ProviderError::SessionClosed {
                thread_id: self.thread.clone(),
            });
        }
        if s.active.is_some() {
            if s.queued.len() >= QUEUE_LIMIT {
                return Err(invalid(
                    "ACP follow-up queue is full (32 turns); cancel a queued turn first",
                ));
            }
            let id = format!("acp-queued-{}", Uuid::new_v4());
            self.emit(ProviderRuntimeEvent::TurnQueued {
                thread_id: self.thread.clone(),
                queued_id: id.clone(),
                client_nonce: input.client_nonce.clone(),
                text: input
                    .display_text
                    .clone()
                    .unwrap_or_else(|| input.text.clone()),
            });
            s.queued.push_back(Queued {
                id: id.clone(),
                input,
            });
            return Ok(TurnStartResult {
                steered: false,
                turn_id: TurnId(String::new()),
                queued_id: Some(id),
            });
        }
        let turn = TurnId(format!("acp-turn-{}", Uuid::new_v4()));
        s.active = Some(turn.clone());
        s.interrupted = false;
        drop(s);
        let session = self.clone();
        let worker_turn = turn.clone();
        tokio::spawn(async move {
            session.worker(worker_turn, input).await;
        });
        Ok(TurnStartResult {
            steered: false,
            turn_id: turn,
            queued_id: None,
        })
    }
    async fn worker(self: Arc<Self>, mut turn: TurnId, mut input: SendTurnInput) {
        loop {
            if let Some(checkpoint) = &input.turn_checkpoint {
                checkpoint.prepare().await;
            }
            let configured = async {
                self.validate_turn(&input).await?;
                if let Some(model) = input.model_override.clone() {
                    self.set_model(model).await?;
                }
                if let Some(effort) = input.effort_override.clone() {
                    self.set_effort(effort).await?;
                }
                Ok::<(), ProviderError>(())
            }
            .await;
            let mut transport_failed = false;
            let result = if configured.is_err() {
                if let Some(checkpoint) = &input.turn_checkpoint {
                    checkpoint.abort().await;
                }
                Err(protocol("ACP turn configuration could not be applied"))
            } else if !self.alive() || self.state.lock().await.interrupted {
                if let Some(checkpoint) = &input.turn_checkpoint {
                    checkpoint.abort().await;
                }
                Ok(json!({"stopReason":"cancelled"}))
            } else {
                self.status(SessionStatus::Running {
                    active_turn: turn.clone(),
                })
                .await;
                if let Some(checkpoint) = &input.turn_checkpoint {
                    checkpoint.commit().await;
                }
                let native = self.binding.lock().await.session_id.clone().unwrap();
                let mut prompt = vec![json!({"type":"text","text":input.text})];
                for image in &input.images {
                    prompt.push(json!({"type":"image","mimeType":image.media_type,"data":base64::engine::general_purpose::STANDARD.encode(&image.data)}));
                }
                let request = self.child.request_with_timeout(
                    "session/prompt",
                    json!({"sessionId":native,"prompt":prompt}),
                    Duration::from_secs(24 * 60 * 60),
                );
                tokio::pin!(request);
                // Poll the prompt first so cancellation never precedes its stdin
                // write. Spawn the cancel write to keep that request polled while
                // it owns JsonRpcChild's FIFO writer lock.
                let cancel_at = tokio::time::sleep(Duration::from_secs(24 * 60 * 60));
                tokio::pin!(cancel_at);
                let mut sent_cancel = false;
                loop {
                    tokio::select! {
                        biased;
                        response=&mut request=>break match response {Ok(value)=>Ok(value),Err(e)=>{
                            transport_failed = !matches!(e,RpcChildError::RpcError(_)) || !self.child.is_alive();
                            Err(if self.child.is_alive() { rpc_error("session/prompt",&e) } else { protocol("ACP prompt transport failed") })
                        }},
                        _=self.cancel.notified()=>{
                            if self.state.lock().await.interrupted || self.ended.load(Ordering::SeqCst) {
                                if !sent_cancel {sent_cancel=true;cancel_at.as_mut().reset(tokio::time::Instant::now()+CANCEL_TIMEOUT);
                                    let child=self.child.clone();let native=native.clone();tokio::spawn(async move {let _=tokio::time::timeout(CANCEL_TIMEOUT,child.notify("session/cancel",json!({"sessionId":native}))).await;});
                                }
                            }
                        },
                        _=&mut cancel_at=>{transport_failed=true;break Err(protocol("ACP prompt did not settle after cancellation"));}
                    }
                }
            };
            // The pinned request is now dropped; teardown cannot deadlock on
            // the stdin writer mutex held by an unpolled request future.
            #[cfg(test)]
            let _schedule = self.prompt_settled_gate.lock().await;
            if transport_failed {
                self.fail("ACP prompt transport or cancellation failed")
                    .await;
                return;
            }
            if self.ended.load(Ordering::SeqCst) {
                return;
            }
            if self.drain().await.is_err() {
                return;
            }
            {
                let _callbacks = self.callbacks.lock().await;
                if !self.cancel_callbacks_locked().await {
                    self.fail_without_callbacks("ACP terminal callback transport failed")
                        .await;
                    return;
                }
            }
            let mut state = self.state.lock().await;
            if state.active.as_ref() != Some(&turn) || self.ended.load(Ordering::SeqCst) {
                return;
            }
            self.flush(&mut state, &turn);
            let status = match result {
                Ok(value) => match text(&value, "stopReason") {
                    Some("end_turn" | "max_tokens" | "max_turn_requests" | "refusal") => {
                        TurnStatus::Success
                    }
                    Some("cancelled") => TurnStatus::Error {
                        subtype: "cancelled".into(),
                        message: "ACP turn cancelled".into(),
                    },
                    _ => {
                        drop(state);
                        self.fail("ACP prompt response is malformed").await;
                        return;
                    }
                },
                Err(_) => TurnStatus::Error {
                    subtype: "acp_error".into(),
                    message: "ACP prompt or turn configuration failed".into(),
                },
            };
            self.emit(ProviderRuntimeEvent::TurnCompleted {
                thread_id: self.thread.clone(),
                turn_id: turn.clone(),
                status,
                usage: None,
            });
            state.active = None;
            state.interrupted = false;
            if !self.child.is_alive() {
                drop(state);
                self.fail("ACP process exited").await;
                return;
            }
            state.status = SessionStatus::Ready;
            self.emit(ProviderRuntimeEvent::SessionStateChanged {
                thread_id: self.thread.clone(),
                status: SessionStatus::Ready,
            });
            let next = state.queued.pop_front();
            if let Some(queued) = next {
                turn = TurnId(format!("acp-turn-{}", Uuid::new_v4()));
                state.active = Some(turn.clone());
                self.emit(ProviderRuntimeEvent::QueuedTurnDispatched {
                    steered: false,
                    thread_id: self.thread.clone(),
                    queued_id: queued.id,
                    turn_id: turn.clone(),
                    text: queued
                        .input
                        .display_text
                        .clone()
                        .unwrap_or_else(|| queued.input.text.clone()),
                });
                input = queued.input;
                drop(state);
            } else {
                return;
            }
        }
    }
}

#[async_trait]
impl AgentProvider for GenericAcpProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Acp
    }
    fn capabilities(&self) -> ProviderCapabilities {
        // Dynamic model/resume support belongs to the instance catalog, not a
        // guessed static promise for every configured binary. These two are ACP baseline.
        ProviderCapabilities {
            supports_synchronous_tool_approval: true,
            supports_interrupt: true,
            ..ProviderCapabilities::default()
        }
    }
    async fn start_session(
        &self,
        input: StartSessionInput,
    ) -> Result<ProviderSession, ProviderError> {
        let _lifecycle = self.inner.lifecycle.lock().await;
        host_policy(input.permission_mode.as_deref())?;
        if input.fast_mode
            || input.context_window.is_some()
            || !input.additional_directories.is_empty()
        {
            return Err(invalid("Custom ACP does not support fast mode, context-window overrides or additional directories"));
        }
        let saved = self
            .inner
            .store
            .binding(&input.thread_id.0)
            .map_err(|_| invalid("ACP binding could not be read"))?;
        let explicit = match input.extra.get("acp_agent_id") {
            None | Some(Value::Null) => None,
            Some(Value::String(id)) => Some(id.as_str()),
            _ => return Err(invalid("acp_agent_id must be an exact saved definition ID")),
        };
        if let Some(saved) = &saved {
            if saved.thread_id != input.thread_id.0
                || explicit.is_some_and(|id| id != saved.agent_id)
            {
                return Err(invalid("unavailable: this thread is bound to a different ACP agent; create a new conversation to change agents"));
            }
            if let Some(value) = &input.resume_cursor {
                validate_cursor(saved, value)?;
            }
        } else if input.resume_cursor.is_some() {
            return Err(invalid(
                "unavailable: ACP resume requires its durable instance binding",
            ));
        }
        if let Some(session) = self
            .inner
            .sessions
            .lock()
            .await
            .get(&input.thread_id)
            .cloned()
        {
            if session.alive() {
                if input.fresh_session {
                    return Err(invalid("Stop the live ACP session before explicitly starting a fresh native session"));
                }
                return Ok(session.snapshot().await);
            }
            session.close().await?;
        }
        let id = saved
            .as_ref()
            .map(|s| s.agent_id.as_str())
            .or(explicit)
            .ok_or_else(|| invalid("Select a saved ACP agent before starting a conversation"))?;
        let mut launch = self.launch(id)?;
        launch.environment = launch_environment(&launch.environment, input.env.as_ref());
        if saved.as_ref().is_some_and(|saved| {
            saved.revision != launch.agent.revision || saved.cwd != input.cwd.to_string_lossy()
        }) {
            return Err(invalid("unavailable: the ACP launch revision or working directory changed; this thread cannot be silently rebound"));
        }
        let mut saved = saved;
        if input.fresh_session {
            if let Some(saved) = &mut saved {
                saved.session_id = None;
            }
        }
        let session = Session::open(
            &self.inner,
            input.thread_id.clone(),
            launch,
            input.cwd,
            saved,
            input.model,
            input.effort,
            true,
        )
        .await?;
        let snapshot = session.snapshot().await;
        self.inner
            .sessions
            .lock()
            .await
            .insert(input.thread_id, session);
        Ok(snapshot)
    }
    async fn send_turn(&self, input: SendTurnInput) -> Result<TurnStartResult, ProviderError> {
        self.session(&input.thread_id).await?.send(input).await
    }
    async fn interrupt_turn(
        &self,
        thread: ThreadId,
        turn: Option<TurnId>,
    ) -> Result<(), ProviderError> {
        self.session(&thread).await?.interrupt(turn).await
    }
    async fn respond_to_request(
        &self,
        thread: ThreadId,
        request: RequestId,
        decision: ApprovalDecision,
    ) -> Result<(), ProviderError> {
        self.session(&thread)
            .await?
            .respond(request, decision)
            .await
    }
    async fn set_model(&self, thread: ThreadId, model: String) -> Result<(), ProviderError> {
        self.session(&thread).await?.set_model(model).await
    }
    async fn set_permission_mode(&self, _: ThreadId, mode: String) -> Result<(), ProviderError> {
        host_policy(Some(&mode))
    }
    async fn stop_session(&self, thread: ThreadId) -> Result<(), ProviderError> {
        let _lifecycle = self.inner.lifecycle.lock().await;
        let session = self.inner.sessions.lock().await.remove(&thread);
        if let Some(session) = session {
            session.close().await?;
        }
        Ok(())
    }
    async fn list_sessions(&self) -> Result<Vec<ProviderSession>, ProviderError> {
        let sessions = self
            .inner
            .sessions
            .lock()
            .await
            .values()
            .cloned()
            .collect::<Vec<_>>();
        let mut snapshots = Vec::new();
        for session in sessions {
            if session.alive() {
                snapshots.push(session.snapshot().await);
            }
        }
        Ok(snapshots)
    }
    async fn has_session(&self, thread: &ThreadId) -> bool {
        self.inner
            .sessions
            .lock()
            .await
            .get(thread)
            .is_some_and(|s| s.alive())
    }
    async fn turn_active(&self, thread: &ThreadId) -> bool {
        match self.session(thread).await {
            Ok(s) => s.state.lock().await.active.is_some(),
            Err(_) => false,
        }
    }
    async fn cancel_queued_turn(
        &self,
        thread: ThreadId,
        id: String,
    ) -> Result<bool, ProviderError> {
        let session = self.session(&thread).await?;
        let mut s = session.state.lock().await;
        if let Some(index) = s.queued.iter().position(|q| q.id == id) {
            s.queued.remove(index);
            session.emit(ProviderRuntimeEvent::QueuedTurnCancelled {
                thread_id: thread,
                queued_id: id,
            });
            Ok(true)
        } else {
            Ok(false)
        }
    }
    async fn send_queued_turn_now(
        &self,
        thread: ThreadId,
        id: String,
    ) -> Result<(), ProviderError> {
        let session = self.session(&thread).await?;
        let turn = {
            let mut s = session.state.lock().await;
            if let Some(index) = s.queued.iter().position(|q| q.id == id) {
                let queued = s.queued.remove(index).unwrap();
                s.queued.push_front(queued);
                s.active.clone()
            } else {
                return Ok(());
            }
        };
        session.interrupt(turn).await
    }
    fn event_stream(&self) -> ProviderEventStream {
        let rx = self.inner.events.subscribe();
        Box::pin(futures_util::stream::unfold(rx, |mut rx| async move {
            match rx.recv().await {
            Ok(event)=>Some((event,rx)),
            Err(broadcast::error::RecvError::Lagged(_))=>Some((ProviderRuntimeEvent::RuntimeWarning {thread_id:None,message:"Custom ACP event subscriber overflowed; refresh the authoritative thread state".into(),original_payload:None},rx)),
            Err(_)=>None,
        }
        }))
    }
}
