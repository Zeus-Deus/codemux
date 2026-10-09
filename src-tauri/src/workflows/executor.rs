use super::{
    artifacts::{checked_path, in_scope, ArtifactStore, WorkspaceArtifact},
    capabilities, Dispatch, ExecutionReport, RunMode, TaskAccess, Usage, WorkflowDriver,
    WorkflowService,
};
use crate::agent_provider::{
    managed::{register_session, ManagedSession, ManagedTool, ManagedToolHandler},
    AgentProvider, ContentDelta, ProviderKind, ProviderRuntimeEvent, SendTurnInput,
    StartSessionInput, ThreadId, TurnStatus,
};
use async_trait::async_trait;
use futures_util::{future::BoxFuture, FutureExt, StreamExt};
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::sync::{watch, Notify};

pub type WorkspaceResolver = Arc<dyn Fn(&str) -> Result<PathBuf, String> + Send + Sync>;
pub type ProviderLookup =
    Arc<dyn Fn(ProviderKind) -> BoxFuture<'static, Option<Arc<dyn AgentProvider>>> + Send + Sync>;
pub type ToolFactory =
    Arc<dyn Fn(Dispatch, WorkflowService) -> Arc<dyn ManagedToolHandler> + Send + Sync>;

pub struct LiveWorkflowDriver {
    resolver: WorkspaceResolver,
    providers: ProviderLookup,
    tool_factory: ToolFactory,
    artifacts: Arc<ArtifactStore>,
    inflight: Mutex<HashMap<String, Arc<AttemptTools>>>,
    shutting_down: AtomicBool,
}

impl LiveWorkflowDriver {
    pub fn new(
        resolver: WorkspaceResolver,
        providers: ProviderLookup,
        tool_factory: ToolFactory,
        artifacts: Arc<ArtifactStore>,
    ) -> Self {
        Self {
            resolver,
            providers,
            tool_factory,
            artifacts,
            inflight: Mutex::new(HashMap::new()),
            shutting_down: AtomicBool::new(false),
        }
    }
    pub fn artifact_store(&self) -> Arc<ArtifactStore> {
        Arc::clone(&self.artifacts)
    }

    /// Called only after the host has checkpointed shutdown and revoked attempt
    /// authority. Aborting a startup future runs the provider's owned-child guard;
    /// observing that guard finish is separate from proving the process stopped.
    pub async fn abort_owned_startups(&self) -> Result<(), String> {
        self.shutting_down.store(true, Ordering::SeqCst);
        let owned = self.owned_tools()?;
        for tools in &owned {
            tools.close();
            if let Some(startup) = tools
                .startup_abort
                .lock()
                .map_err(|_| "startup ownership unavailable")?
                .as_ref()
            {
                startup.abort();
            }
        }
        tokio::time::timeout(Duration::from_secs(3), async {
            futures_util::future::join_all(owned.iter().map(|tools| tools.drain_startup())).await;
        })
        .await
        .map_err(|_| "owned startup cancellation did not finish".to_string())
    }

    fn owned_tools(&self) -> Result<Vec<Arc<AttemptTools>>, String> {
        Ok(self
            .inflight
            .lock()
            .map_err(|_| "attempt ownership unavailable")?
            .values()
            .cloned()
            .collect())
    }

    /// The driver may have returned Unknown or been aborted while a provider was
    /// stopping. Reconcile those owned sessions concurrently before app exit.
    /// Failed proofs retain their durable admission and write holds.
    pub async fn shutdown_owned_runtimes(&self) -> Result<(), String> {
        let owned = self.owned_tools()?;
        let results = futures_util::future::join_all(owned.iter().map(|tools| async move {
            // Native teardown never needs the database, which may be blocked
            // by checkpoint I/O. Identity and process evidence are locally owned.
            let reference = json!({"provider":tools.dispatch.route.provider,
                "thread_id":format!("workflow-{}",tools.dispatch.attempt_id),
                "attempt_id":tools.dispatch.attempt_id,
                "process":tools.runtime_evidence.lock().map_err(|_| "runtime evidence unavailable")?.clone()});
            self.reconcile_owned_runtime(&reference).await
        }))
        .await;
        let errors: Vec<_> = results.into_iter().filter_map(Result::err).collect();
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors.join("; "))
        }
    }

    pub async fn reconcile_owned_runtime(&self, reference: &Value) -> Result<(), String> {
        let kind = capabilities::provider_kind(
            reference
                .get("provider")
                .and_then(Value::as_str)
                .ok_or("provider evidence is missing")?,
        )?;
        let thread = ThreadId(
            reference
                .get("thread_id")
                .and_then(Value::as_str)
                .ok_or("thread evidence is missing")?
                .into(),
        );
        let id = reference
            .get("attempt_id")
            .and_then(Value::as_str)
            .ok_or("attempt evidence is missing")?;
        let tools = self
            .inflight
            .lock()
            .map_err(|_| "attempt ownership unavailable")?
            .get(id)
            .cloned();
        if let Some(tools) = tools.as_ref() {
            tools.close();
            if tools.startup_active.load(Ordering::SeqCst) {
                return Err("owned initialization is still running; a delayed session cannot yet be ruled out".into());
            }
        }
        let mut local_stop = tools
            .as_ref()
            .is_some_and(|tools| tools.process_stopped.load(Ordering::SeqCst));
        if !local_stop {
            if let Some(provider) = (self.providers)(kind).await {
                let stop = match tools.as_ref() {
                    Some(tools) => tools.stop_owned_provider(&provider, thread).await,
                    None => tokio::time::timeout(
                        Duration::from_secs(12),
                        provider.stop_managed_session(thread),
                    )
                    .await
                    .map_err(|_| "owned runtime reconciliation timed out")?,
                };
                match stop {
                    Ok(()) => local_stop = true,
                    Err(crate::agent_provider::ProviderError::SessionNotFound { .. }) => {}
                    Err(error) => return Err(error.to_string()),
                }
            }
        }
        if !local_stop {
            if let Some(tools) = tools.as_ref() {
                tools.quiesce().await?;
            }
            prove_quiescent_inner(reference, tools.is_some())?;
        }
        if let Some(tools) = tools.as_ref() {
            tools.process_stopped.store(true, Ordering::SeqCst);
            // Drain failure retains holds, but cannot prevent native stop.
            tools.quiesce().await?;
        }
        self.inflight
            .lock()
            .map_err(|_| "attempt ownership unavailable")?
            .remove(id);
        Ok(())
    }

    async fn execute_live(
        &self,
        dispatch: Dispatch,
        service: WorkflowService,
        mut cancellation: watch::Receiver<bool>,
    ) -> ExecutionReport {
        if dispatch.mode != RunMode::Live {
            return known_zero(ExecutionReport::failed(
                "live executor cannot simulate provider work",
            ));
        }
        if *cancellation.borrow() {
            return known_zero(ExecutionReport::cancelled());
        }
        if let Err(error) = service.authorize_attempt(&dispatch) {
            return known_zero(ExecutionReport::failed(error));
        }
        let kind = match capabilities::provider_kind(&dispatch.route.provider) {
            Ok(value) => value,
            Err(error) => return known_zero(ExecutionReport::failed(error)),
        };
        let provider = match (self.providers)(kind).await {
            Some(value) => value,
            None => {
                return known_zero(ExecutionReport::failed(
                    "workflow provider is not configured",
                ))
            }
        };
        if let Err(error) =
            capabilities::require(provider.managed_capabilities(), dispatch.task.access)
        {
            return known_zero(ExecutionReport::failed(error));
        }
        let source = match (self.resolver)(&dispatch.workspace_id) {
            Ok(value) => value,
            Err(error) => return known_zero(ExecutionReport::failed(error)),
        };
        let prior = service
            .snapshot(&dispatch.run_id)
            .ok()
            .and_then(|run| {
                run.tasks
                    .into_iter()
                    .find(|task| task.spec.id == dispatch.task_id)
            })
            .and_then(|task| {
                task.attempts.into_iter().rev().find(|attempt| {
                    attempt.id != dispatch.attempt_id
                        && attempt.generation == dispatch.generation
                        && attempt.status == super::AttemptStatus::Waiting
                })
            })
            .map(|attempt| attempt.id);
        let store = Arc::clone(&self.artifacts);
        let run_id = dispatch.run_id.clone();
        let attempt_id = dispatch.attempt_id.clone();
        let scope = dispatch.task.scope.clone();
        let artifact = match tokio::task::spawn_blocking(move || {
            store.prepare_with_seed(&run_id, &source, &attempt_id, scope, prior.as_deref())
        })
        .await
        {
            Ok(Ok(value)) => value,
            Ok(Err(error)) => return known_zero(ExecutionReport::failed(error)),
            Err(error) => return known_zero(ExecutionReport::failed(error.to_string())),
        };
        let run = match service.snapshot(&dispatch.run_id) {
            Ok(run) => run,
            Err(error) => return known_zero(ExecutionReport::failed(error)),
        };
        let limits = run.resolved_limits;
        let routes = run.spec.routes;
        let tools = Arc::new(AttemptTools::new(
            dispatch.clone(),
            service.clone(),
            artifact.clone(),
            (self.tool_factory)(dispatch.clone(), service.clone()),
            Arc::clone(&self.artifacts),
            limits.max_output_bytes,
        ));
        let thread_id = ThreadId(format!("workflow-{}", dispatch.attempt_id));
        let registration = match register_session(
            thread_id.clone(),
            ManagedSession {
                handler: tools.clone(),
                read_only: dispatch.task.access == TaskAccess::ReadOnly,
            },
        ) {
            Ok(value) => Arc::new(value),
            Err(error) => return known_zero(ExecutionReport::failed(error)),
        };
        if let Ok(mut owned) = self.inflight.lock() {
            owned.insert(dispatch.attempt_id.clone(), Arc::clone(&tools));
        } else {
            return known_zero(ExecutionReport::failed(
                "attempt ownership registry unavailable",
            ));
        }
        // Subscribe before start/send; a synchronous fake or fast native
        // session can publish completion before send_turn returns.
        let mut events = provider.managed_event_stream(&thread_id);
        // Durable launch fence: a fenced attempt with no external_ref cannot
        // have started a provider. Keep this commit before every spawn/start.
        if let Err(error)=service.record_external_ref(&dispatch.run_id,&dispatch.attempt_id,json!({
            "provider":dispatch.route.provider,"thread_id":thread_id.0,"attempt_id":dispatch.attempt_id,"checkout":artifact.checkout,
            "phase":"starting"})) { return known_zero(ExecutionReport::failed(error)); }
        if self.shutting_down.load(Ordering::SeqCst) {
            return known_zero(ExecutionReport::cancelled());
        }
        let input = StartSessionInput {
            thread_id: thread_id.clone(),
            cwd: artifact.checkout.clone(),
            model: dispatch.route.model.clone(),
            resume_cursor: None,
            fresh_session: true,
            permission_mode: Some(
                if kind == ProviderKind::Codex {
                    "read-only"
                } else {
                    "default"
                }
                .into(),
            ),
            effort: dispatch.route.effort.clone(),
            context_window: None,
            fast_mode: false,
            additional_directories: vec![],
            env: None,
            workspace_id: Some(dispatch.workspace_id.clone()),
            extra: Value::Null,
            recorded_usage_baseline: None,
        };
        let start_provider = Arc::clone(&provider);
        let start_guard = Arc::clone(&registration);
        let startup_tools = Arc::clone(&tools);
        tools.startup_active.store(true, Ordering::SeqCst);
        // Construct before spawn so aborting a task before its first poll also
        // clears startup_active and releases the managed registration.
        let startup_guard = StartupGuard(startup_tools);
        let mut startup = tokio::spawn(async move {
            let _guard = start_guard;
            let _startup = startup_guard;
            start_provider.start_session(input).await
        });
        *tools
            .startup_abort
            .lock()
            .expect("new attempt startup lock") = Some(startup.abort_handle());
        let session = tokio::select! {
            result=&mut startup=>match result {
                Ok(Ok(session))=>session,
                Ok(Err(crate::agent_provider::ProviderError::ValidationError{message})) if message.starts_with("managed-start-rejected:")=>{tools.close();return known_zero(ExecutionReport::failed(message));},
                Ok(Err(error))=>{tools.close();return ExecutionReport::unknown(format!("provider startup failed; reconcile owned session before retry: {error}"));},
                Err(error)=>{tools.close();return ExecutionReport::unknown(format!("provider startup task failed: {error}"));},
            },
            _=tokio::time::sleep(Duration::from_secs(45))=>{
                tools.close(); return ExecutionReport::unknown("provider initialization timed out; dispatch remains reserved");
            },
            _=cancellation.changed()=>{
                tools.close();
                // Retain registration in startup until lookup has completed.
                match tokio::time::timeout(Duration::from_secs(45),&mut startup).await {
                    Ok(Ok(Ok(_)))=>return stopped(&provider,&thread_id,&tools,ExecutionReport::cancelled()).await,
                    _=>return ExecutionReport::unknown("cancelled during initialization; provider stop remains unconfirmed"),
                }
            }
        };
        let process = service
            .snapshot(&dispatch.run_id)
            .ok()
            .and_then(|run| {
                run.tasks
                    .into_iter()
                    .find(|task| task.spec.id == dispatch.task_id)
            })
            .and_then(|task| task.current_attempt)
            .and_then(|attempt| attempt.external_ref)
            .and_then(|reference| reference.get("process").cloned());
        if let Err(error)=service.record_external_ref(&dispatch.run_id,&dispatch.attempt_id,json!({
            "provider":dispatch.route.provider,"thread_id":thread_id.0,"provider_session_id":session.session_id.0,
            "resume_cursor":session.resume_cursor,"attempt_id":dispatch.attempt_id,"checkout":artifact.checkout,"phase":"ready","process":process})) {
            return stopped(&provider,&thread_id,&tools,ExecutionReport::failed(error)).await;
        }
        if *cancellation.borrow() || service.authorize_attempt(&dispatch).is_err() {
            return stopped(&provider, &thread_id, &tools, ExecutionReport::cancelled()).await;
        }
        let prompt=format!("Managed CodeMux workflow.\nGoal: {}\n\nTask: {}\n{}\n\nThis is a managed CodeMux workflow attempt. Use only the provided workflow file tools and graph tools. All paths are relative to this attempt's copied checkout. Submit the task's JSON output with workflow_submit_result({{output: ...}}); assistant prose does not complete the task. Calling workflow_wait yields this attempt and resumes with child results. Choose child route_id from the configured route catalog; omit route_id to use host routing.\nConfigured routes: {}\nOutput schema: {}\nDependencies: {}\nMessages: {}",
            dispatch.goal,dispatch.task.title,dispatch.task.prompt,json!(routes),dispatch.task.output_schema.as_ref().unwrap_or(&Value::Null),
            json!(dispatch.dependency_results),json!(dispatch.messages));
        let turn_input = SendTurnInput {
            thread_id: thread_id.clone(),
            text: prompt,
            display_text: None,
            images: vec![],
            skill_invocations: vec![],
            // This attempt owns a fresh session already pinned to its route.
            // Repeating model control before the first prompt can block SDK
            // initialization, which itself waits for that prompt.
            model_override: None,
            effort_override: None,
            permission_mode_override: None,
            client_nonce: None,
            turn_checkpoint: None,
        };
        let sent = tokio::select! {
            result=provider.send_turn(turn_input)=>result,
            _=tokio::time::sleep(Duration::from_secs(45))=>return stopped(&provider,&thread_id,&tools,ExecutionReport::failed("provider send-turn acknowledgement timed out")).await,
            _=cancellation.changed()=>return stopped(&provider,&thread_id,&tools,ExecutionReport::cancelled()).await,
        };
        let turn = match sent {
            Ok(turn) => turn.turn_id,
            Err(error) => {
                return stopped(
                    &provider,
                    &thread_id,
                    &tools,
                    ExecutionReport::failed(error.to_string()),
                )
                .await
            }
        };
        let deadline = tokio::time::sleep(Duration::from_millis(limits.wall_time_ms.max(1)));
        tokio::pin!(deadline);
        let mut usage = Usage::default();
        let mut streamed = 0usize;
        let mut saw_usage = false;
        let mut final_turn = false;
        let report = loop {
            tokio::select! {
                _=cancellation.changed()=>break ExecutionReport::cancelled(),
                _=&mut deadline=>break ExecutionReport::failed("provider attempt exceeded workflow wall-time limit"),
                _=tools.wake.notified()=>{
                    if let Some(waiting)=tools.waiting() { break ExecutionReport::waiting(waiting); }
                },
                event=events.next()=>{
                    let Some(event)=event else { break ExecutionReport::unknown("provider event stream closed before completion"); };
                    match event {
                        ProviderRuntimeEvent::ContentDelta{thread_id:id,turn_id,delta,..} if id==thread_id && turn_id==turn=>{
                            streamed=streamed.saturating_add(match delta { ContentDelta::Text{text}|ContentDelta::Thinking{text}=>text.len(),ContentDelta::ToolInput{partial_json,..}=>partial_json.len() });
                            if streamed>limits.max_output_bytes { break ExecutionReport::failed("provider output exceeded workflow byte limit"); }
                        },
                        ProviderRuntimeEvent::UsageRecorded{thread_id:id,input_tokens,output_tokens,cache_read_tokens,cache_write_tokens,cost_usd,..} if id==thread_id=>{
                            saw_usage=true;
                            usage.input_tokens=usage.input_tokens.saturating_add(input_tokens).saturating_add(cache_read_tokens).saturating_add(cache_write_tokens);
                            usage.output_tokens=usage.output_tokens.saturating_add(output_tokens);
                            usage.total_tokens=usage.input_tokens.saturating_add(usage.output_tokens);
                            if let Some(cost)=cost_usd {usage.cost_usd=Some(usage.cost_usd.unwrap_or(0.0)+cost);}
                        },
                        ProviderRuntimeEvent::RequestOpened{thread_id:id,..}|ProviderRuntimeEvent::QuestionsAsked{thread_id:id,..} if id==thread_id=>
                            break ExecutionReport::failed("managed attempt requires approval or user input; no approval was granted"),
                        ProviderRuntimeEvent::SubagentUpdated{thread_id:id,..}|ProviderRuntimeEvent::WorkflowUpdated{thread_id:id,..} if id==thread_id=>
                            break ExecutionReport::unknown("native delegation appeared despite managed policy"),
                        ProviderRuntimeEvent::TurnCompleted{thread_id:id,turn_id,status,usage:turn_usage} if id==thread_id && turn_id==turn=>{
                            final_turn=true;
                            if let Some(cost)=turn_usage.and_then(|u|u.total_cost_usd) {usage.cost_usd=Some(cost);usage.cost_unknown=false;}
                            break match status {
                                TurnStatus::Success=>if let Some(output)=tools.output() {ExecutionReport::success(output)} else if let Some(ids)=tools.waiting() {ExecutionReport::waiting(ids)} else {ExecutionReport::failed("provider finished without an accepted workflow_submit_result call")},
                                TurnStatus::Error{message,..}=>ExecutionReport::failed(message),
                                TurnStatus::MaxBudget=>ExecutionReport::failed("provider budget limit reached"),
                                TurnStatus::MaxTurns=>ExecutionReport::failed("provider turn limit reached"),
                            };
                        },
                        ProviderRuntimeEvent::RuntimeWarning{thread_id:Some(id),message,..} if id==thread_id && message.starts_with("Managed event stream lagged")=>
                            break ExecutionReport::unknown("managed provider events were lost; reconcile before retry"),
                        _=>{},
                    }
                }
            }
        };
        let mut report = stopped(&provider, &thread_id, &tools, report).await;
        // Shutdown can enqueue final accounting while closing the runtime.
        // Read only already-buffered events, with a bound; never await a
        // provider that has claimed to stop and never guess missing tokens.
        for index in 0..2048 {
            let Some(Some(event)) = events.next().now_or_never() else {
                break;
            };
            match event {
                ProviderRuntimeEvent::UsageRecorded {
                    thread_id: id,
                    input_tokens,
                    output_tokens,
                    cache_read_tokens,
                    cache_write_tokens,
                    cost_usd,
                    ..
                } if id == thread_id => {
                    saw_usage = true;
                    usage.input_tokens = usage
                        .input_tokens
                        .saturating_add(input_tokens)
                        .saturating_add(cache_read_tokens)
                        .saturating_add(cache_write_tokens);
                    usage.output_tokens = usage.output_tokens.saturating_add(output_tokens);
                    usage.total_tokens = usage.input_tokens.saturating_add(usage.output_tokens);
                    if let Some(cost) = cost_usd {
                        usage.cost_usd = Some(usage.cost_usd.unwrap_or(0.0) + cost);
                    }
                }
                ProviderRuntimeEvent::TurnCompleted {
                    thread_id: id,
                    turn_id,
                    usage: turn_usage,
                    ..
                } if id == thread_id && turn_id == turn => {
                    final_turn = true;
                    if let Some(cost) = turn_usage.and_then(|u| u.total_cost_usd) {
                        usage.cost_usd = Some(cost);
                        usage.cost_unknown = false;
                    }
                }
                ProviderRuntimeEvent::RuntimeWarning {
                    thread_id: Some(id),
                    message,
                    ..
                } if id == thread_id && message.starts_with("Managed event stream lagged") => {
                    report.disposition = super::ExecutionDisposition::Unknown;
                    report.error = Some(
                        "accounting events were lost; reconcile before releasing reservation"
                            .into(),
                    );
                    saw_usage = false;
                }
                _ => {}
            }
            if index == 2047 {
                report.disposition = super::ExecutionDisposition::Unknown;
                report.error = Some("post-stop event drain exceeded its bound".into());
                saw_usage = false;
            }
        }
        usage.tokens_unknown = !(saw_usage && final_turn);
        report.usage = usage;
        if matches!(
            report.disposition,
            super::ExecutionDisposition::Succeeded | super::ExecutionDisposition::Waiting
        ) {
            let store = Arc::clone(&self.artifacts);
            let attempt_id = dispatch.attempt_id.clone();
            match tokio::task::spawn_blocking(move || store.seal(&attempt_id)).await {
                Ok(Ok(manifest)) => report.artifacts.push(
                    json!({"kind":"workspace_snapshot","run_id":dispatch.run_id,
                    "attempt_id":dispatch.attempt_id,"baseline_digest":artifact.baseline_digest,
                    "digest":manifest.digest,"changed_paths":manifest.changed_paths}),
                ),
                Ok(Err(error)) => {
                    report = ExecutionReport {
                        usage: report.usage,
                        ..ExecutionReport::failed(error)
                    }
                }
                Err(error) => {
                    report = ExecutionReport {
                        usage: report.usage,
                        ..ExecutionReport::failed(error.to_string())
                    }
                }
            }
        }
        drop(registration);
        report
    }
}

#[async_trait]
impl WorkflowDriver for LiveWorkflowDriver {
    async fn execute(
        &self,
        dispatch: Dispatch,
        service: WorkflowService,
        cancellation: watch::Receiver<bool>,
    ) -> ExecutionReport {
        let id = dispatch.attempt_id.clone();
        let report = self.execute_live(dispatch, service, cancellation).await;
        if report.disposition != super::ExecutionDisposition::Unknown {
            if let Ok(mut inflight) = self.inflight.lock() {
                inflight.remove(&id);
            }
        }
        report
    }
}

fn known_zero(mut report: ExecutionReport) -> ExecutionReport {
    report.usage.tokens_unknown = false;
    report.usage.cost_unknown = false;
    report.usage.cost_usd = Some(0.0);
    report
}

async fn stopped(
    provider: &Arc<dyn AgentProvider>,
    thread_id: &ThreadId,
    tools: &AttemptTools,
    mut report: ExecutionReport,
) -> ExecutionReport {
    tools.close();
    let stopped = tools.stop_owned_provider(provider, thread_id.clone()).await;
    let drained = tokio::time::timeout(Duration::from_secs(5), tools.drain()).await;
    if stopped.is_err() || drained.is_err() {
        report.disposition = super::ExecutionDisposition::Unknown;
        report.error=Some("provider/process or host-tool quiescence is unconfirmed; admission and write ownership remain held".into());
    }
    report
}

struct AttemptTools {
    dispatch: Dispatch,
    service: WorkflowService,
    artifact: WorkspaceArtifact,
    delegate: Arc<dyn ManagedToolHandler>,
    artifacts: Arc<ArtifactStore>,
    files: Mutex<()>,
    result: Mutex<Option<Value>>,
    wait: Mutex<Option<Vec<String>>>,
    observed_dependencies: Mutex<HashMap<String, u64>>,
    closed: AtomicBool,
    active: AtomicUsize,
    startup_active: AtomicBool,
    startup_abort: Mutex<Option<tokio::task::AbortHandle>>,
    runtime_evidence: Mutex<Option<Value>>,
    process_stopped: AtomicBool,
    stop_lock: tokio::sync::Mutex<()>,
    idle: Notify,
    wake: Notify,
    max_output_bytes: usize,
}
impl AttemptTools {
    fn new(
        dispatch: Dispatch,
        service: WorkflowService,
        artifact: WorkspaceArtifact,
        delegate: Arc<dyn ManagedToolHandler>,
        artifacts: Arc<ArtifactStore>,
        max_output_bytes: usize,
    ) -> Self {
        Self {
            dispatch,
            service,
            artifact,
            delegate,
            artifacts,
            files: Mutex::new(()),
            result: Mutex::new(None),
            wait: Mutex::new(None),
            observed_dependencies: Mutex::new(HashMap::new()),
            closed: AtomicBool::new(false),
            active: AtomicUsize::new(0),
            startup_active: AtomicBool::new(false),
            startup_abort: Mutex::new(None),
            runtime_evidence: Mutex::new(None),
            process_stopped: AtomicBool::new(false),
            stop_lock: tokio::sync::Mutex::new(()),
            idle: Notify::new(),
            wake: Notify::new(),
            max_output_bytes,
        }
    }
    fn output(&self) -> Option<Value> {
        self.result.lock().ok()?.clone()
    }
    fn waiting(&self) -> Option<Vec<String>> {
        self.wait.lock().ok()?.clone()
    }
    fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
    }
    async fn stop_owned_provider(
        &self,
        provider: &Arc<dyn AgentProvider>,
        thread: ThreadId,
    ) -> Result<(), crate::agent_provider::ProviderError> {
        // Driver cancellation and application exit can meet at the same attempt.
        // Serialize them and retain the actual proof so a removed provider map
        // cannot turn an already verified stop into a spurious Unknown hold.
        tokio::time::timeout(Duration::from_secs(12), async {
            let _stop = self.stop_lock.lock().await;
            if self.process_stopped.load(Ordering::SeqCst) {
                return Ok(());
            }
            provider.stop_managed_session(thread).await?;
            self.process_stopped.store(true, Ordering::SeqCst);
            Ok(())
        })
        .await
        .map_err(|_| crate::agent_provider::ProviderError::ValidationError {
            message: "owned runtime stop timed out".into(),
        })?
    }
    async fn drain(&self) {
        loop {
            let notified = self.idle.notified();
            if self.active.load(Ordering::SeqCst) == 0 {
                return;
            }
            notified.await;
        }
    }
    async fn drain_startup(&self) {
        loop {
            let notified = self.idle.notified();
            if !self.startup_active.load(Ordering::SeqCst) {
                return;
            }
            notified.await;
        }
    }
    fn authority(&self) -> Result<(), String> {
        if self.closed.load(Ordering::SeqCst) {
            return Err("attempt tool authority was closed".into());
        }
        self.service.authorize_attempt(&self.dispatch)
    }
    fn dependency(&self, id: &str) -> Result<WorkspaceArtifact, String> {
        let observed = self
            .observed_dependencies
            .lock()
            .map_err(|_| "dependency observations unavailable")?
            .get(id)
            .copied();
        let dispatched = self
            .dispatch
            .dependency_results
            .iter()
            .find(|(value, result)| {
                value == id && result.get("status").and_then(Value::as_str) == Some("succeeded")
            })
            .and_then(|(_, result)| result.get("generation").and_then(Value::as_u64));
        if observed.is_none()
            && dispatched.is_none()
            && !self
                .dispatch
                .task
                .dependencies
                .iter()
                .any(|value| value == id)
        {
            return Err("artifact access requires a declared or awaited dependency".into());
        }
        let run = self.service.snapshot(&self.dispatch.run_id)?;
        let task = run
            .tasks
            .iter()
            .find(|task| task.spec.id == id)
            .ok_or("unknown dependency")?;
        if task.retired || task.status != super::TaskStatus::Succeeded {
            return Err("dependency is not successful".into());
        }
        let attempt = task
            .current_attempt
            .as_ref()
            .ok_or("dependency attempt is missing")?;
        if attempt.generation != task.generation
            || attempt.cancel_requested
            || attempt.status != super::AttemptStatus::Succeeded
        {
            return Err("dependency artifact generation is stale".into());
        }
        if observed
            .or(dispatched)
            .is_some_and(|generation| generation != task.generation)
        {
            return Err(
                "dependency result observation is stale; wait for its current generation".into(),
            );
        }
        let artifact = self.artifacts.inspect(&attempt.id)?;
        if artifact.run_id != self.dispatch.run_id || artifact.sealed.is_none() {
            return Err("dependency artifact is unavailable".into());
        }
        Ok(artifact)
    }
}
struct ActiveCall<'a>(&'a AttemptTools);
impl Drop for ActiveCall<'_> {
    fn drop(&mut self) {
        if self.0.active.fetch_sub(1, Ordering::SeqCst) == 1 {
            self.0.idle.notify_waiters();
        }
    }
}
struct StartupGuard(Arc<AttemptTools>);
impl Drop for StartupGuard {
    fn drop(&mut self) {
        self.0.startup_active.store(false, Ordering::SeqCst);
        self.0.idle.notify_waiters();
    }
}

#[async_trait]
impl ManagedToolHandler for AttemptTools {
    async fn quiesce(&self) -> Result<(), String> {
        self.close();
        tokio::time::timeout(Duration::from_secs(5), self.drain())
            .await
            .map_err(|_| "host tool calls are still in flight".into())
    }
    fn record_runtime(&self, evidence: Value) -> Result<(), String> {
        *self
            .runtime_evidence
            .lock()
            .map_err(|_| "runtime evidence unavailable")? = Some(evidence.clone());
        self.authority()?;
        self.service.record_external_ref(&self.dispatch.run_id,&self.dispatch.attempt_id,json!({
            "provider":self.dispatch.route.provider,"thread_id":format!("workflow-{}",self.dispatch.attempt_id),
            "attempt_id":self.dispatch.attempt_id,"checkout":self.artifact.checkout,"phase":"initializing","process":evidence,
        })).map(|_|())
    }
    fn tools(&self) -> Vec<ManagedTool> {
        let mut tools = self.delegate.tools();
        tools.push(ManagedTool{name:"workflow_submit_result".into(),description:"Submit the schema-validated JSON task result. Prose alone does not complete an attempt.".into(),input_schema:json!({"type":"object","properties":{"output":self.dispatch.task.output_schema.clone().unwrap_or(json!({}))},"required":["output"],"additionalProperties":false})});
        let path_schema = json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"],"additionalProperties":false});
        tools.push(ManagedTool {
            name: "workflow_read_file".into(),
            description: "Read a UTF-8 file inside this attempt checkout, at most 2 MiB".into(),
            input_schema: path_schema.clone(),
        });
        tools.push(ManagedTool {
            name: "workflow_list_files".into(),
            description: "List files in this attempt checkout".into(),
            input_schema: json!({"type":"object","properties":{},"additionalProperties":false}),
        });
        tools.push(ManagedTool{name:"workflow_read_dependency_file".into(),description:"Read an accepted dependency's file; task_id must be a declared or awaited successful dependency".into(),input_schema:json!({"type":"object","properties":{"task_id":{"type":"string"},"path":{"type":"string"}},"required":["task_id","path"],"additionalProperties":false})});
        if self.dispatch.task.access == TaskAccess::Write {
            tools.push(ManagedTool{name:"workflow_write_file".into(),description:"Write UTF-8 content in the declared task scope; source workspace is unchanged".into(),input_schema:json!({"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},"required":["path","content"],"additionalProperties":false})});
            tools.push(ManagedTool {
                name: "workflow_delete_file".into(),
                description:
                    "Delete one file in the declared task scope from this attempt checkout".into(),
                input_schema: path_schema,
            });
            tools.push(ManagedTool{name:"workflow_import_dependency".into(),description:"Import an accepted dependency's changes into this attempt, rejecting conflicts and writes outside task scope".into(),input_schema:json!({"type":"object","properties":{"task_id":{"type":"string"}},"required":["task_id"],"additionalProperties":false})});
        }
        tools
    }
    async fn call(&self, name: &str, arguments: Value) -> Result<Value, String> {
        self.active.fetch_add(1, Ordering::SeqCst);
        let _active = ActiveCall(self);
        self.authority()?;
        let file = name.starts_with("workflow_")
            && matches!(
                name,
                "workflow_read_file"
                    | "workflow_write_file"
                    | "workflow_delete_file"
                    | "workflow_list_files"
                    | "workflow_read_dependency_file"
                    | "workflow_import_dependency"
            );
        if file {
            let _files = self
                .files
                .lock()
                .map_err(|_| "attempt file tools unavailable")?;
            self.authority()?;
            if name == "workflow_import_dependency" {
                if self.dispatch.task.access != TaskAccess::Write {
                    return Err("read-only attempts cannot import writes".into());
                }
                let id = arguments
                    .get("task_id")
                    .and_then(Value::as_str)
                    .ok_or("task_id is required")?;
                let dependency = self.dependency(id)?;
                let paths = self
                    .artifacts
                    .import_dependency(&self.dispatch.attempt_id, &dependency.attempt_id)?;
                return Ok(json!({"imported_paths":paths}));
            }
            if name == "workflow_list_files" {
                let mut directories = vec![self.artifact.checkout.clone()];
                let mut paths = vec![];
                while let Some(directory) = directories.pop() {
                    for entry in std::fs::read_dir(directory).map_err(|e| e.to_string())? {
                        let entry = entry.map_err(|e| e.to_string())?;
                        let path = entry.path();
                        let metadata =
                            std::fs::symlink_metadata(&path).map_err(|e| e.to_string())?;
                        if metadata.file_type().is_symlink() {
                            continue;
                        }
                        if metadata.is_dir() {
                            directories.push(path);
                        } else {
                            paths.push(
                                path.strip_prefix(&self.artifact.checkout)
                                    .map_err(|e| e.to_string())?
                                    .to_string_lossy()
                                    .to_string(),
                            );
                        }
                        if paths.len() > 10_000 {
                            return Err("file listing exceeds managed limit".into());
                        }
                    }
                }
                paths.sort();
                let result = json!({"paths":paths});
                if result.to_string().len() > self.max_output_bytes {
                    return Err("file listing exceeds output limit".into());
                }
                return Ok(result);
            }
            let relative = arguments
                .get("path")
                .and_then(Value::as_str)
                .ok_or("path is required")?;
            let dependency = if name == "workflow_read_dependency_file" {
                Some(
                    self.dependency(
                        arguments
                            .get("task_id")
                            .and_then(Value::as_str)
                            .ok_or("task_id is required")?,
                    )?,
                )
            } else {
                None
            };
            let path = checked_path(
                dependency
                    .as_ref()
                    .map(|artifact| artifact.checkout.as_path())
                    .unwrap_or(self.artifact.checkout.as_path()),
                relative,
            )?;
            match name {
                "workflow_read_file" | "workflow_read_dependency_file" => {
                    if std::fs::metadata(&path).map_err(|e| e.to_string())?.len() > 2 * 1024 * 1024
                    {
                        return Err("file exceeds managed read limit".into());
                    }
                    let content = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
                    if content.len() > self.max_output_bytes {
                        return Err("file content exceeds output limit".into());
                    }
                    Ok(json!({"content":content}))
                }
                "workflow_write_file" | "workflow_delete_file" => {
                    if self.dispatch.task.access != TaskAccess::Write
                        || !in_scope(relative, &self.dispatch.task.scope)
                    {
                        return Err("write outside declared task scope is forbidden".into());
                    }
                    self.authority()?;
                    if name == "workflow_delete_file" {
                        self.artifacts
                            .delete_file(&self.dispatch.attempt_id, relative)?;
                    } else {
                        let content = arguments
                            .get("content")
                            .and_then(Value::as_str)
                            .ok_or("content is required")?;
                        if content.len() > 2 * 1024 * 1024 {
                            return Err("file exceeds managed write limit".into());
                        }
                        self.artifacts.write_file(
                            &self.dispatch.attempt_id,
                            relative,
                            content.as_bytes(),
                        )?;
                    }
                    Ok(json!({"written":true}))
                }
                _ => Err("unknown file tool".into()),
            }
        } else {
            if name == "workflow_submit_result" {
                let output = arguments.get("output").ok_or("output is required")?.clone();
                if serde_json::to_vec(&output)
                    .map_err(|e| e.to_string())?
                    .len()
                    > self.max_output_bytes
                {
                    return Err("result exceeds managed output limit".into());
                }
                if let Some(schema) = self.dispatch.task.output_schema.as_ref() {
                    let validator = jsonschema::validator_for(schema).map_err(|e| e.to_string())?;
                    if !validator.is_valid(&output) {
                        return Err("result does not satisfy the task output schema".into());
                    }
                }
                let mut captured = self.result.lock().map_err(|_| "result lock unavailable")?;
                if captured
                    .as_ref()
                    .is_some_and(|previous| previous != &output)
                {
                    return Err("attempt already submitted a different result".into());
                }
                *captured = Some(output);
                self.wake.notify_one();
                return Ok(json!({"accepted":true}));
            }
            let result = self.delegate.call(name, arguments.clone()).await?;
            if result.to_string().len() > self.max_output_bytes {
                return Err("tool output exceeds managed limit".into());
            }
            if name == "workflow_wait" {
                // A fresh continuation may wait again for an older completed
                // sibling. Capture only successful results supplied by the
                // host, then fence their accepted generation on file access.
                let mut observed = self
                    .observed_dependencies
                    .lock()
                    .map_err(|_| "dependency observations unavailable")?;
                for result in result
                    .get("results")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    if result.get("status").and_then(Value::as_str) == Some("succeeded") {
                        if let (Some(id), Some(generation)) = (
                            result.get("id").and_then(Value::as_str),
                            result.get("generation").and_then(Value::as_u64),
                        ) {
                            observed.insert(id.into(), generation);
                        }
                    }
                }
            }
            if name == "workflow_wait"
                && result.get("state").and_then(Value::as_str) == Some("waiting")
            {
                let ids = result
                    .get("task_ids")
                    .and_then(Value::as_array)
                    .ok_or("waiting task_ids are required")?
                    .iter()
                    .map(|id| {
                        id.as_str()
                            .map(str::to_owned)
                            .ok_or("task_ids must be strings".to_string())
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                *self.wait.lock().map_err(|_| "wait lock unavailable")? = Some(ids);
                self.wake.notify_one();
            }
            Ok(result)
        }
    }
}

/// Recovery uses OS identity evidence from the durable dispatch, never a
/// model-supplied PID or a cleared adapter map. This function never signals a
/// process. A live prior host could still own an in-flight host file callback.
pub fn prove_quiescent(reference: &Value) -> Result<(), String> {
    prove_quiescent_inner(reference, false)
}

fn prove_quiescent_inner(reference: &Value, local_tools_drained: bool) -> Result<(), String> {
    #[cfg(target_os = "linux")]
    {
        fn stat(pid: u32) -> Result<Option<(u32, u64)>, String> {
            let value = match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
                Ok(value) => value,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
                Err(error) => return Err(error.to_string()),
            };
            let fields = value
                .rsplit_once(") ")
                .ok_or("invalid process identity")?
                .1
                .split_whitespace()
                .collect::<Vec<_>>();
            let group = fields
                .get(2)
                .ok_or("missing process group")?
                .parse()
                .map_err(|_| "invalid process group")?;
            let start = fields
                .get(19)
                .ok_or("missing process start identity")?
                .parse()
                .map_err(|_| "invalid process start identity")?;
            Ok(Some((group, start)))
        }
        let evidence = reference
            .get("process")
            .filter(|value| value.is_object())
            .ok_or("durable process identity is missing; stop remains unconfirmed")?;
        if evidence.get("kind").and_then(Value::as_str) != Some("linux_process_group") {
            return Err("unsupported process evidence".into());
        }
        let boot = std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
            .map_err(|e| e.to_string())?;
        let previous = evidence
            .get("boot_id")
            .and_then(Value::as_str)
            .ok_or("boot identity is missing")?;
        if boot.trim() != previous {
            return Ok(());
        }
        let host = u32::try_from(
            evidence
                .get("host_pid")
                .and_then(Value::as_u64)
                .ok_or("host identity is missing")?,
        )
        .map_err(|_| "invalid host pid")?;
        let host_start = evidence
            .get("host_start_time_ticks")
            .and_then(Value::as_u64)
            .ok_or("host start identity is missing")?;
        if stat(host)?.is_some_and(|(_, start)| start == host_start)
            && !(local_tools_drained && host == std::process::id())
        {
            return Err(
                "prior host is still alive; host-tool quiescence needs local reconciliation".into(),
            );
        }
        let group = u32::try_from(
            evidence
                .get("pid")
                .and_then(Value::as_u64)
                .ok_or("process identity is missing")?,
        )
        .map_err(|_| "invalid pid")?;
        if group <= 1 {
            return Err("invalid managed process group".into());
        }
        let expected = evidence
            .get("start_time_ticks")
            .and_then(Value::as_u64)
            .ok_or("process start identity is missing")?;
        if stat(group)?.is_some_and(|(_, start)| start == expected) {
            return Err("owned provider process is still alive".into());
        }
        for entry in std::fs::read_dir("/proc").map_err(|e| e.to_string())? {
            let entry = entry.map_err(|e| e.to_string())?;
            let Some(pid) = entry
                .file_name()
                .to_str()
                .and_then(|name| name.parse::<u32>().ok())
            else {
                continue;
            };
            if stat(pid)?.is_some_and(|(observed, _)| observed == group) {
                return Err(
                    "owned process group is still present or its identity is ambiguous".into(),
                );
            }
        }
        Ok(())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (reference, local_tools_drained);
        Err("durable process reconciliation is unavailable on this platform".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent_provider::{
        managed::ManagedCapabilities, ApprovalDecision, ProviderCapabilities, ProviderError,
        ProviderEventStream, ProviderSession, ProviderSessionId, RequestId, SessionStatus, TurnId,
        TurnStartResult,
    };
    use tokio::sync::broadcast;
    #[derive(Clone, Copy)]
    enum Behavior {
        Result,
        Prose,
        InvalidResult,
        Approval,
        NoCompletion,
        LateResult,
        WaitWithLateUsage,
        SetupDenied,
        MissingSidecar,
        UnexecutableSidecar,
        InvalidSidecarFormat,
        PendingStartup,
        StopOnce,
    }
    #[derive(Default)]
    struct FakeProviderObservations {
        no_completion_turn_started: Notify,
        starts: AtomicUsize,
        stops: AtomicUsize,
    }
    struct FakeProvider {
        kind: ProviderKind,
        events: broadcast::Sender<ProviderRuntimeEvent>,
        behavior: Behavior,
        stop_confirmed: bool,
        session: Mutex<Option<ProviderSession>>,
        observations: Arc<FakeProviderObservations>,
    }
    #[async_trait]
    impl AgentProvider for FakeProvider {
        fn kind(&self) -> ProviderKind {
            self.kind
        }
        fn capabilities(&self) -> ProviderCapabilities {
            ProviderCapabilities::default()
        }
        fn managed_capabilities(&self) -> ManagedCapabilities {
            ManagedCapabilities {
                scoped_tools: true,
                native_fanout_disabled: true,
                enforced_read_only: true,
                isolated_writes: true,
                verified_stop: true,
            }
        }
        async fn start_session(
            &self,
            input: StartSessionInput,
        ) -> Result<ProviderSession, ProviderError> {
            self.observations.starts.fetch_add(1, Ordering::SeqCst);
            assert!(crate::agent_provider::managed::lookup_session(&input.thread_id).is_some());
            assert!(input.fresh_session);
            assert!(input.resume_cursor.is_none());
            assert_eq!(input.model.as_deref(), Some("fake"));
            assert_eq!(input.effort.as_deref(), Some("high"));
            if matches!(self.behavior, Behavior::PendingStartup) {
                return std::future::pending().await;
            }
            if matches!(self.behavior, Behavior::SetupDenied) {
                return Err(ProviderError::ValidationError {
                    message:
                        "managed-start-rejected: setup_required: prepared Hermes runtime is missing"
                            .into(),
                });
            }
            if matches!(
                self.behavior,
                Behavior::MissingSidecar
                    | Behavior::UnexecutableSidecar
                    | Behavior::InvalidSidecarFormat
            ) {
                let program = input.cwd.join("unavailable-managed-sidecar");
                if matches!(
                    self.behavior,
                    Behavior::UnexecutableSidecar | Behavior::InvalidSidecarFormat
                ) {
                    tokio::fs::write(&program, b"must never execute")
                        .await
                        .unwrap();
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        let mode = if matches!(self.behavior, Behavior::InvalidSidecarFormat) {
                            0o700
                        } else {
                            0o600
                        };
                        tokio::fs::set_permissions(&program, std::fs::Permissions::from_mode(mode))
                            .await
                            .unwrap();
                    }
                }
                let context =
                    crate::agent_provider::managed::lookup_session(&input.thread_id).unwrap();
                let config = crate::json_rpc_child::SpawnConfig {
                    program,
                    args: vec![],
                    env: Default::default(),
                    cwd: Some(input.cwd.clone()),
                    default_timeout: Duration::from_secs(1),
                };
                let session = crate::agent_provider::managed_bridge::ManagedBridgeSession::spawn(
                    input,
                    self.kind,
                    "no-inference-fixture",
                    context,
                    config,
                    json!({}),
                    self.events.clone(),
                )
                .await?;
                return Ok(session.provider_session().await);
            }
            let session = ProviderSession {
                thread_id: input.thread_id,
                provider: self.kind,
                session_id: ProviderSessionId("fake-native".into()),
                status: SessionStatus::Ready,
                resume_cursor: None,
            };
            *self.session.lock().unwrap() = Some(session.clone());
            Ok(session)
        }
        async fn send_turn(&self, input: SendTurnInput) -> Result<TurnStartResult, ProviderError> {
            assert!(input.model_override.is_none());
            assert!(input.effort_override.is_none());
            let context = crate::agent_provider::managed::lookup_session(&input.thread_id).unwrap();
            assert!(context.call("codemux_bash", json!({})).await.is_err());
            if context
                .handler
                .tools()
                .iter()
                .any(|tool| tool.name == "workflow_write_file")
            {
                assert!(context
                    .call(
                        "workflow_write_file",
                        json!({"path":"../outside","content":"unsafe"})
                    )
                    .await
                    .is_err());
                assert!(context
                    .call(
                        "workflow_write_file",
                        json!({"path":"other.txt","content":"unsafe"})
                    )
                    .await
                    .is_err());
                context
                    .call(
                        "workflow_write_file",
                        json!({"path":"tracked.txt","content":"fake patch"}),
                    )
                    .await
                    .unwrap();
            } else {
                assert!(context
                    .call(
                        "workflow_write_file",
                        json!({"path":"tracked.txt","content":"unsafe"})
                    )
                    .await
                    .is_err());
            }
            match self.behavior {
                Behavior::Result | Behavior::LateResult => {
                    context
                        .call("workflow_submit_result", json!({"output":{"ok":true}}))
                        .await
                        .unwrap();
                }
                Behavior::InvalidResult => {
                    assert!(context
                        .call("workflow_submit_result", json!({"output":{"ok":"invalid"}}))
                        .await
                        .is_err());
                }
                Behavior::WaitWithLateUsage => {
                    context.call("workflow_add_tasks",json!({"tasks":[{"id":"child","title":"child","prompt":"fake child"}],"idempotency_key":"children"})).await.unwrap();
                    context
                        .call("workflow_wait", json!({"task_ids":["child"]}))
                        .await
                        .unwrap();
                }
                _ => {}
            }
            let turn = TurnId("fake-turn".into());
            self.events
                .send(ProviderRuntimeEvent::UsageRecorded {
                    thread_id: input.thread_id.clone(),
                    provider: self.kind,
                    model: Some("fake".into()),
                    subagent: false,
                    input_tokens: 2,
                    output_tokens: 3,
                    cache_read_tokens: 1,
                    cache_write_tokens: 0,
                    reasoning_tokens: 0,
                    cost_usd: None,
                    cost_source: None,
                })
                .unwrap();
            if matches!(self.behavior, Behavior::Approval) {
                self.events
                    .send(ProviderRuntimeEvent::RequestOpened {
                        thread_id: input.thread_id.clone(),
                        turn_id: turn.clone(),
                        request_id: RequestId("approval".into()),
                        request_kind: "fake".into(),
                        payload: Value::Null,
                        tool_use_id: None,
                        subagent_id: None,
                    })
                    .unwrap();
            } else if matches!(self.behavior, Behavior::LateResult) {
                let events = self.events.clone();
                let id = input.thread_id.clone();
                let completed_turn = turn.clone();
                let kind = self.kind;
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                    events
                        .send(ProviderRuntimeEvent::UsageRecorded {
                            thread_id: id.clone(),
                            provider: kind,
                            model: None,
                            subagent: false,
                            input_tokens: 20,
                            output_tokens: 30,
                            cache_read_tokens: 0,
                            cache_write_tokens: 0,
                            reasoning_tokens: 0,
                            cost_usd: None,
                            cost_source: None,
                        })
                        .unwrap();
                    events
                        .send(ProviderRuntimeEvent::TurnCompleted {
                            thread_id: id,
                            turn_id: completed_turn,
                            status: TurnStatus::Success,
                            usage: None,
                        })
                        .unwrap();
                });
            } else if !matches!(
                self.behavior,
                Behavior::NoCompletion | Behavior::WaitWithLateUsage | Behavior::StopOnce
            ) {
                self.events
                    .send(ProviderRuntimeEvent::ContentDelta {
                        thread_id: input.thread_id.clone(),
                        turn_id: turn.clone(),
                        delta: ContentDelta::Text {
                            text: "done".into(),
                        },
                        subagent_id: None,
                    })
                    .unwrap();
                self.events
                    .send(ProviderRuntimeEvent::TurnCompleted {
                        thread_id: input.thread_id.clone(),
                        turn_id: turn.clone(),
                        status: TurnStatus::Success,
                        usage: None,
                    })
                    .unwrap();
            }
            if matches!(self.behavior, Behavior::NoCompletion) {
                // Cancellation tests must reach an actual active turn before
                // cancelling, regardless of checkout/startup latency.
                self.observations.no_completion_turn_started.notify_one();
            }
            Ok(TurnStartResult {
                steered: false,
                turn_id: turn,
                queued_id: None,
            })
        }
        async fn stop_managed_session(&self, id: ThreadId) -> Result<(), ProviderError> {
            self.observations.stops.fetch_add(1, Ordering::SeqCst);
            if !self.stop_confirmed {
                return Err(ProviderError::ValidationError {
                    message: "unconfirmed fake process".into(),
                });
            }
            if matches!(self.behavior, Behavior::StopOnce) {
                tokio::time::sleep(Duration::from_millis(20)).await;
                if self.session.lock().unwrap().take().is_none() {
                    return Err(ProviderError::SessionNotFound { thread_id: id });
                }
                return Ok(());
            }
            if matches!(self.behavior, Behavior::WaitWithLateUsage) {
                self.events
                    .send(ProviderRuntimeEvent::UsageRecorded {
                        thread_id: id.clone(),
                        provider: self.kind,
                        model: None,
                        subagent: false,
                        input_tokens: 20,
                        output_tokens: 20,
                        cache_read_tokens: 0,
                        cache_write_tokens: 0,
                        reasoning_tokens: 0,
                        cost_usd: None,
                        cost_source: None,
                    })
                    .unwrap();
                self.events
                    .send(ProviderRuntimeEvent::TurnCompleted {
                        thread_id: id,
                        turn_id: TurnId("fake-turn".into()),
                        status: TurnStatus::Success,
                        usage: None,
                    })
                    .unwrap();
            }
            *self.session.lock().unwrap() = None;
            Ok(())
        }
        async fn interrupt_turn(
            &self,
            _: ThreadId,
            _: Option<TurnId>,
        ) -> Result<(), ProviderError> {
            Ok(())
        }
        async fn respond_to_request(
            &self,
            _: ThreadId,
            _: RequestId,
            _: ApprovalDecision,
        ) -> Result<(), ProviderError> {
            panic!("managed tests must not grant provider approvals")
        }
        async fn set_model(&self, _: ThreadId, _: String) -> Result<(), ProviderError> {
            panic!("fresh managed attempts must pin the model at session startup")
        }
        async fn set_permission_mode(&self, _: ThreadId, _: String) -> Result<(), ProviderError> {
            Ok(())
        }
        async fn stop_session(&self, _: ThreadId) -> Result<(), ProviderError> {
            panic!("unverified regular stop must not release workflow ownership")
        }
        async fn list_sessions(&self) -> Result<Vec<ProviderSession>, ProviderError> {
            Ok(self.session.lock().unwrap().clone().into_iter().collect())
        }
        async fn has_session(&self, _: &ThreadId) -> bool {
            self.session.lock().unwrap().is_some()
        }
        fn event_stream(&self) -> ProviderEventStream {
            let rx = self.events.subscribe();
            Box::pin(futures_util::stream::unfold(rx, |mut rx| async move {
                rx.recv().await.ok().map(|event| (event, rx))
            }))
        }
    }
    fn fixture(
        kind: ProviderKind,
        behavior: Behavior,
        stop_confirmed: bool,
        access: TaskAccess,
    ) -> (
        tempfile::TempDir,
        WorkflowService,
        Dispatch,
        LiveWorkflowDriver,
    ) {
        let (root, service, dispatch, driver, _) =
            observed_fixture(kind, behavior, stop_confirmed, access);
        (root, service, dispatch, driver)
    }
    fn observed_fixture(
        kind: ProviderKind,
        behavior: Behavior,
        stop_confirmed: bool,
        access: TaskAccess,
    ) -> (
        tempfile::TempDir,
        WorkflowService,
        Dispatch,
        LiveWorkflowDriver,
        Arc<FakeProviderObservations>,
    ) {
        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source");
        std::fs::create_dir(&source).unwrap();
        assert!(std::process::Command::new("git")
            .args(["init", "-q"])
            .arg(&source)
            .status()
            .unwrap()
            .success());
        std::fs::write(source.join("tracked.txt"), "dirty source").unwrap();
        let service = WorkflowService::open(root.path().join("state.sqlite"), 4).unwrap();
        let provider_name = serde_json::to_value(kind)
            .unwrap()
            .as_str()
            .unwrap()
            .to_string();
        service.create(super::super::RunSpec{workspace_id:"workspace".into(),title:"fake".into(),goal:"fake only; no provider called".into(),mode:RunMode::Live,allow_writes:access==TaskAccess::Write,
            routes:vec![super::super::RouteSpec{id:"route".into(),provider:provider_name,model:Some("fake".into()),effort:Some("high".into())}],limits:Default::default(),
            tasks:vec![super::super::TaskSpec{id:"task".into(),title:"fake task".into(),prompt:"fake".into(),dependencies:vec![],route_id:None,access,scope:if access==TaskAccess::Write {vec!["tracked.txt".into()]}else{vec![]},output_schema:Some(json!({"type":"object","properties":{"ok":{"type":"boolean"}},"required":["ok"],"additionalProperties":false})),required:true}],script:None},"create").unwrap();
        let dispatch = service.claim_next().unwrap().unwrap();
        service.mark_running(&dispatch).unwrap();
        let (events, _) = broadcast::channel(32);
        let observations = Arc::new(FakeProviderObservations::default());
        let fake: Arc<dyn AgentProvider> = Arc::new(FakeProvider {
            kind,
            events,
            behavior,
            stop_confirmed,
            session: Mutex::new(None),
            observations: Arc::clone(&observations),
        });
        let lookup: ProviderLookup = Arc::new(move |_| {
            let provider = Arc::clone(&fake);
            Box::pin(async move { Some(provider) })
        });
        let resolver: WorkspaceResolver = Arc::new(move |_| Ok(source.clone()));
        let factory: ToolFactory = Arc::new(super::super::tools::graph_tools);
        let driver = LiveWorkflowDriver::new(
            resolver,
            lookup,
            factory,
            Arc::new(ArtifactStore::new(root.path().join("artifacts"))),
        );
        (root, service, dispatch, driver, observations)
    }
    #[tokio::test]
    async fn workflow_executor_exit_and_driver_cancel_share_actual_stop_proof() {
        let (_root, service, dispatch, driver) = fixture(
            ProviderKind::Claude,
            Behavior::StopOnce,
            true,
            TaskAccess::ReadOnly,
        );
        let driver = Arc::new(driver);
        let (cancel, receiver) = watch::channel(false);
        let worker = tokio::spawn({
            let driver = driver.clone();
            let service = service.clone();
            let dispatch = dispatch.clone();
            async move { driver.execute(dispatch, service, receiver).await }
        });
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if service.snapshot(&dispatch.run_id).unwrap().tasks[0]
                    .current_attempt
                    .as_ref()
                    .and_then(|attempt| attempt.external_ref.as_ref())
                    .is_some_and(|reference| reference["phase"] == "ready")
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .unwrap();
        service.shutdown_signal().unwrap();
        cancel.send(true).unwrap();
        driver.abort_owned_startups().await.unwrap();
        let (report, stop) = tokio::join!(worker, driver.shutdown_owned_runtimes());
        stop.unwrap();
        let report = report.unwrap();
        assert_eq!(
            report.disposition,
            super::super::ExecutionDisposition::Cancelled
        );
        service.finish_attempt(&dispatch, report).unwrap();
        assert!(!service.snapshot(&dispatch.run_id).unwrap().tasks[0]
            .current_attempt
            .as_ref()
            .unwrap()
            .status
            .holds_capacity());
        assert!(driver.owned_tools().unwrap().is_empty());
    }

    #[tokio::test]
    async fn workflow_executor_exit_stops_native_owner_despite_blocked_host_callback() {
        struct SlowTool(Arc<Notify>);
        #[async_trait]
        impl ManagedToolHandler for SlowTool {
            fn tools(&self) -> Vec<ManagedTool> {
                vec![ManagedTool {
                    name: "slow".into(),
                    description: "token-free hung callback".into(),
                    input_schema: json!({"type":"object"}),
                }]
            }
            async fn call(&self, _: &str, _: Value) -> Result<Value, String> {
                self.0.notify_one();
                std::future::pending().await
            }
        }
        let (_root, service, dispatch, mut driver) = fixture(
            ProviderKind::Claude,
            Behavior::NoCompletion,
            true,
            TaskAccess::ReadOnly,
        );
        let started = Arc::new(Notify::new());
        driver.tool_factory = Arc::new({
            let started = started.clone();
            move |_, _| Arc::new(SlowTool(started.clone()))
        });
        let driver = Arc::new(driver);
        let (_cancel, receiver) = watch::channel(false);
        let worker = tokio::spawn({
            let driver = driver.clone();
            let service = service.clone();
            let dispatch = dispatch.clone();
            async move { driver.execute(dispatch, service, receiver).await }
        });
        let tools = tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if let Some(tools) = driver.owned_tools().unwrap().first().cloned() {
                    if !tools.startup_active.load(Ordering::SeqCst)
                        && (driver.providers)(ProviderKind::Claude)
                            .await
                            .unwrap()
                            .has_session(&ThreadId(format!("workflow-{}", dispatch.attempt_id)))
                            .await
                    {
                        break tools;
                    }
                }
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .unwrap();
        let callback = tokio::spawn({
            let tools = tools.clone();
            async move { tools.call("slow", json!({})).await }
        });
        started.notified().await;
        service.shutdown_signal().unwrap();
        worker.abort();
        let _ = worker.await;
        driver.abort_owned_startups().await.unwrap();
        let error = driver.shutdown_owned_runtimes().await.unwrap_err();
        assert!(error.contains("host tool calls are still in flight"));
        assert!((driver.providers)(ProviderKind::Claude)
            .await
            .unwrap()
            .list_sessions()
            .await
            .unwrap()
            .is_empty());
        assert!(!driver.owned_tools().unwrap().is_empty());
        assert!(service.snapshot(&dispatch.run_id).unwrap().tasks[0]
            .current_attempt
            .as_ref()
            .unwrap()
            .status
            .holds_capacity());
        callback.abort();
        let _ = callback.await;
        driver.shutdown_owned_runtimes().await.unwrap();
        assert!(driver.owned_tools().unwrap().is_empty());
    }

    #[tokio::test]
    async fn workflow_executor_exit_stops_owned_session_when_snapshot_database_fails() {
        let (_root, service, dispatch, driver) = fixture(
            ProviderKind::Claude,
            Behavior::NoCompletion,
            true,
            TaskAccess::ReadOnly,
        );
        let driver = Arc::new(driver);
        let (_cancel, receiver) = watch::channel(false);
        let worker = tokio::spawn({
            let driver = driver.clone();
            let service = service.clone();
            let dispatch = dispatch.clone();
            async move { driver.execute(dispatch, service, receiver).await }
        });
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let ready = service.snapshot(&dispatch.run_id).unwrap().tasks[0]
                    .current_attempt
                    .as_ref()
                    .and_then(|attempt| attempt.external_ref.as_ref())
                    .is_some_and(|reference| reference["phase"] == "ready");
                if ready {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .unwrap();
        service
            .with_connection(|connection| {
                connection
                    .execute("DROP TABLE workflow_runs", [])
                    .map_err(|error| error.to_string())?;
                Ok(())
            })
            .unwrap();
        assert!(service.shutdown_signal().is_err());
        worker.abort();
        let _ = worker.await;
        driver.abort_owned_startups().await.unwrap();
        driver.shutdown_owned_runtimes().await.unwrap();
        let provider = (driver.providers)(ProviderKind::Claude).await.unwrap();
        assert!(provider.list_sessions().await.unwrap().is_empty());
        assert!(driver.owned_tools().unwrap().is_empty());
    }

    #[tokio::test]
    async fn workflow_executor_exit_aborts_owned_pending_startup_and_preserves_uncertainty() {
        let (_root, service, dispatch, driver) = fixture(
            ProviderKind::Claude,
            Behavior::PendingStartup,
            true,
            TaskAccess::Write,
        );
        let driver = Arc::new(driver);
        let (cancel, receiver) = watch::channel(false);
        let worker = tokio::spawn({
            let driver = driver.clone();
            let service = service.clone();
            let dispatch = dispatch.clone();
            async move { driver.execute(dispatch, service, receiver).await }
        });
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let ready = driver
                    .owned_tools()
                    .unwrap()
                    .iter()
                    .any(|tools| tools.startup_active.load(Ordering::SeqCst));
                if ready {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .unwrap();
        service.shutdown_signal().unwrap();
        cancel.send(true).unwrap();
        driver.abort_owned_startups().await.unwrap();
        assert!(driver
            .owned_tools()
            .unwrap()
            .iter()
            .all(|tools| !tools.startup_active.load(Ordering::SeqCst)));
        let report = tokio::time::timeout(Duration::from_secs(2), worker)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            report.disposition,
            super::super::ExecutionDisposition::Unknown
        );
        service.finish_attempt(&dispatch, report).unwrap();
        driver.shutdown_owned_runtimes().await.unwrap();
        assert!(driver.owned_tools().unwrap().is_empty());
        assert!(service.snapshot(&dispatch.run_id).unwrap().tasks[0]
            .current_attempt
            .as_ref()
            .unwrap()
            .status
            .holds_capacity());
        assert!(service.claim_next().unwrap().is_none());
        assert!(service.authorize_attempt(&dispatch).is_err());
    }

    #[tokio::test]
    async fn workflow_executor_fake_all_provider_labels_enforce_result_and_scope_without_tokens() {
        for kind in [
            ProviderKind::Claude,
            ProviderKind::Codex,
            ProviderKind::Cursor,
            ProviderKind::Grok,
            ProviderKind::Hermes,
            ProviderKind::OpenCode,
        ] {
            let (root, service, dispatch, driver) =
                fixture(kind, Behavior::Result, true, TaskAccess::Write);
            let (_tx, rx) = watch::channel(false);
            let report = driver.execute(dispatch.clone(), service.clone(), rx).await;
            assert_eq!(
                report.disposition,
                super::super::ExecutionDisposition::Succeeded,
                "{kind:?}: {:?}",
                report.error
            );
            assert_eq!(report.usage.total_tokens, 6);
            assert_eq!(report.artifacts.len(), 1);
            assert_eq!(
                std::fs::read_to_string(root.path().join("source/tracked.txt")).unwrap(),
                "dirty source"
            );
            let artifact = driver.artifacts.inspect(&dispatch.attempt_id).unwrap();
            assert!(artifact.sealed.is_some());
            assert_eq!(
                std::fs::read_to_string(artifact.checkout.join("tracked.txt")).unwrap(),
                "fake patch"
            );
        }
    }
    #[tokio::test]
    async fn workflow_executor_prose_invalid_json_approval_and_uncertain_stop_are_not_success() {
        for behavior in [Behavior::Prose, Behavior::InvalidResult, Behavior::Approval] {
            let (_root, service, dispatch, driver) =
                fixture(ProviderKind::Claude, behavior, true, TaskAccess::ReadOnly);
            let (_tx, rx) = watch::channel(false);
            let report = driver.execute(dispatch, service, rx).await;
            assert_eq!(
                report.disposition,
                super::super::ExecutionDisposition::Failed
            );
        }
        let (_root, service, dispatch, driver) = fixture(
            ProviderKind::Claude,
            Behavior::Result,
            false,
            TaskAccess::Write,
        );
        let (_tx, rx) = watch::channel(false);
        let report = driver.execute(dispatch.clone(), service.clone(), rx).await;
        assert_eq!(
            report.disposition,
            super::super::ExecutionDisposition::Unknown
        );
        service.finish_attempt(&dispatch, report).unwrap();
        assert!(service.claim_next().unwrap().is_none());
        assert!(driver
            .artifacts
            .inspect(&dispatch.attempt_id)
            .unwrap()
            .sealed
            .is_none());
    }

    #[tokio::test]
    async fn workflow_executor_pre_spawn_setup_failure_releases_capacity_with_zero_usage() {
        let (_root, service, dispatch, driver) = fixture(
            ProviderKind::Hermes,
            Behavior::SetupDenied,
            false,
            TaskAccess::Write,
        );
        let (_tx, rx) = watch::channel(false);
        let report = driver.execute(dispatch.clone(), service.clone(), rx).await;
        assert_eq!(
            report.disposition,
            super::super::ExecutionDisposition::Failed
        );
        assert_eq!(report.usage.total_tokens, 0);
        assert_eq!(report.usage.cost_usd, Some(0.0));
        assert!(!report.usage.tokens_unknown && !report.usage.cost_unknown);
        service.finish_attempt(&dispatch, report).unwrap();
        let run = service.snapshot(&dispatch.run_id).unwrap();
        let task = &run.tasks[0];
        assert_eq!(task.status, super::super::TaskStatus::Failed);
        assert!(task
            .current_attempt
            .as_ref()
            .unwrap()
            .external_ref
            .as_ref()
            .unwrap()
            .get("process")
            .is_none());
        service.retry(&dispatch.run_id, &dispatch.task_id).unwrap();
        assert!(service.claim_next().unwrap().is_some());
    }
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn workflow_executor_unavailable_sidecar_releases_capacity_without_runtime_evidence() {
        for behavior in [
            Behavior::MissingSidecar,
            Behavior::UnexecutableSidecar,
            Behavior::InvalidSidecarFormat,
        ] {
            let (_root, service, dispatch, driver) =
                fixture(ProviderKind::Hermes, behavior, false, TaskAccess::Write);
            let (_tx, rx) = watch::channel(false);
            let report = driver.execute(dispatch.clone(), service.clone(), rx).await;
            assert_eq!(
                report.disposition,
                super::super::ExecutionDisposition::Failed
            );
            assert_eq!(report.usage.total_tokens, 0);
            assert_eq!(report.usage.cost_usd, Some(0.0));
            assert!(!report.usage.tokens_unknown && !report.usage.cost_unknown);
            service.finish_attempt(&dispatch, report).unwrap();
            let run = service.snapshot(&dispatch.run_id).unwrap();
            assert_eq!(run.tasks[0].status, super::super::TaskStatus::Failed);
            assert!(run.tasks[0]
                .current_attempt
                .as_ref()
                .unwrap()
                .external_ref
                .as_ref()
                .unwrap()
                .get("process")
                .is_none());
            assert_eq!(run.usage.reserved_tokens, 0);
            assert_eq!(run.usage.cost_usd, Some(0.0));
            service.retry(&dispatch.run_id, &dispatch.task_id).unwrap();
            assert!(service.claim_next().unwrap().is_some());
        }
    }
    #[tokio::test]
    async fn workflow_executor_retired_dependency_cannot_be_read_or_imported() {
        let (root, service, parent, driver) = fixture(
            ProviderKind::Claude,
            Behavior::Result,
            true,
            TaskAccess::Write,
        );
        let mut child_spec = parent.task.clone();
        child_spec.id = "child".into();
        service
            .add_tasks_as(&parent, vec![child_spec], "spawn")
            .unwrap();
        service
            .finish_attempt(&parent, ExecutionReport::waiting(vec!["child".into()]))
            .unwrap();
        let child = service.claim_next().unwrap().unwrap();
        let child_artifact = driver
            .artifacts
            .prepare(
                &child.run_id,
                &root.path().join("source"),
                &child.attempt_id,
                child.task.scope.clone(),
            )
            .unwrap();
        driver
            .artifacts
            .write_file(&child.attempt_id, "tracked.txt", b"accepted dependency")
            .unwrap();
        driver.artifacts.seal(&child.attempt_id).unwrap();
        service
            .finish_attempt(&child, ExecutionReport::success(json!({"ok":true})))
            .unwrap();
        let resumed = service.claim_next().unwrap().unwrap();
        service.mark_running(&resumed).unwrap();
        let artifact = driver
            .artifacts
            .prepare(
                &resumed.run_id,
                &root.path().join("source"),
                &resumed.attempt_id,
                resumed.task.scope.clone(),
            )
            .unwrap();
        let tools = AttemptTools::new(
            resumed.clone(),
            service.clone(),
            artifact,
            super::super::tools::graph_tools(resumed, service.clone()),
            driver.artifacts.clone(),
            1_048_576,
        );
        assert_eq!(
            tools
                .call(
                    "workflow_read_dependency_file",
                    json!({"task_id":"child","path":"tracked.txt"})
                )
                .await
                .unwrap()["content"],
            "accepted dependency"
        );
        assert!(child_artifact.checkout.exists());
        service.retire(&child.run_id, "child").unwrap();
        for name in [
            "workflow_read_dependency_file",
            "workflow_import_dependency",
        ] {
            assert!(tools
                .call(name, json!({"task_id":"child","path":"tracked.txt"}))
                .await
                .is_err());
        }
    }
    #[tokio::test]
    async fn workflow_executor_cancellation_requires_verified_stop() {
        for stop_confirmed in [true, false] {
            let (_root, service, dispatch, driver, observations) = observed_fixture(
                ProviderKind::Claude,
                Behavior::NoCompletion,
                stop_confirmed,
                TaskAccess::ReadOnly,
            );
            let provider = (driver.providers)(ProviderKind::Claude).await.unwrap();
            let thread_id = ThreadId(format!("workflow-{}", dispatch.attempt_id));
            let (tx, rx) = watch::channel(false);
            let cancel_active_turn = async {
                observations.no_completion_turn_started.notified().await;
                assert_eq!(observations.starts.load(Ordering::SeqCst), 1);
                assert!(provider.has_session(&thread_id).await);
                assert_eq!(
                    service.snapshot(&dispatch.run_id).unwrap().tasks[0]
                        .current_attempt
                        .as_ref()
                        .unwrap()
                        .external_ref
                        .as_ref()
                        .unwrap()["phase"],
                    "ready"
                );
                service.cancel(&dispatch.run_id).unwrap();
                tx.send(true).unwrap();
            };
            let (report, ()) = tokio::time::timeout(Duration::from_secs(30), async {
                tokio::join!(
                    driver.execute(dispatch.clone(), service.clone(), rx),
                    cancel_active_turn
                )
            })
            .await
            .expect("fake provider must start, cancel, and attempt verified stop");
            assert_eq!(observations.stops.load(Ordering::SeqCst), 1);
            assert_eq!(provider.has_session(&thread_id).await, !stop_confirmed);
            assert_eq!(
                report.disposition,
                if stop_confirmed {
                    super::super::ExecutionDisposition::Cancelled
                } else {
                    super::super::ExecutionDisposition::Unknown
                }
            );
            service.finish_attempt(&dispatch, report).unwrap();
            let run = service.snapshot(&dispatch.run_id).unwrap();
            let attempt = run.tasks[0].current_attempt.as_ref().unwrap();
            assert_eq!(
                attempt.status,
                if stop_confirmed {
                    super::super::AttemptStatus::Cancelled
                } else {
                    super::super::AttemptStatus::Unknown
                }
            );
            assert_eq!(attempt.status.holds_capacity(), !stop_confirmed);
            assert_eq!(run.usage.reserved_tokens > 0, !stop_confirmed);
            assert!(attempt.output.is_none());
            assert!(service.claim_next().unwrap().is_none());
        }
    }
    #[tokio::test]
    async fn workflow_executor_cancellation_before_launch_is_durable_and_known_zero() {
        let (_root, service, dispatch, mut driver, observations) = observed_fixture(
            ProviderKind::Claude,
            Behavior::NoCompletion,
            true,
            TaskAccess::Write,
        );
        let (tx, rx) = watch::channel(false);
        let resolver = Arc::clone(&driver.resolver);
        let cancellation_service = service.clone();
        let run_id = dispatch.run_id.clone();
        driver.resolver = Arc::new(move |workspace| {
            let source = resolver(workspace)?;
            // Reproduce cancellation after initial authorization but before
            // checkout preparation commits any external execution intent.
            cancellation_service.cancel(&run_id)?;
            tx.send(true).unwrap();
            Ok(source)
        });
        let report = driver.execute(dispatch.clone(), service.clone(), rx).await;
        // The revoked launch fence reports its local failure; durable finishing
        // must retain the user's cancellation and its proven zero accounting.
        assert_eq!(
            report.disposition,
            super::super::ExecutionDisposition::Failed
        );
        assert_eq!(report.usage.total_tokens, 0);
        assert_eq!(report.usage.estimated_tokens, 0);
        assert_eq!(report.usage.cost_usd, Some(0.0));
        assert!(!report.usage.tokens_unknown && !report.usage.cost_unknown);
        assert_eq!(observations.starts.load(Ordering::SeqCst), 0);
        assert_eq!(observations.stops.load(Ordering::SeqCst), 0);
        assert!(driver.owned_tools().unwrap().is_empty());
        service.finish_attempt(&dispatch, report).unwrap();
        let run = service.snapshot(&dispatch.run_id).unwrap();
        let task = &run.tasks[0];
        let attempt = task.current_attempt.as_ref().unwrap();
        assert_eq!(run.status, super::super::RunStatus::Cancelled);
        assert_eq!(task.status, super::super::TaskStatus::Cancelled);
        assert_eq!(attempt.status, super::super::AttemptStatus::Cancelled);
        assert!(!attempt.status.holds_capacity());
        assert!(attempt.external_ref.is_none());
        assert!(attempt.output.is_none());
        assert_eq!(run.usage.reserved_tokens, 0);
        assert_eq!(run.usage.total_tokens, 0);
        assert_eq!(run.usage.estimated_tokens, 0);
        assert_eq!(run.usage.cost_usd, Some(0.0));
        assert!(!run.usage.tokens_unknown && !run.usage.cost_unknown);
        assert!(service.claim_next().unwrap().is_none());
    }
    #[tokio::test]
    async fn workflow_executor_collects_late_and_post_stop_usage() {
        let (_root, service, dispatch, driver) = fixture(
            ProviderKind::Codex,
            Behavior::LateResult,
            true,
            TaskAccess::ReadOnly,
        );
        let (_tx, rx) = watch::channel(false);
        let report = driver.execute(dispatch, service, rx).await;
        assert_eq!(
            report.disposition,
            super::super::ExecutionDisposition::Succeeded
        );
        assert_eq!(report.usage.total_tokens, 56);
        assert!(!report.usage.tokens_unknown);
        let (_root, service, dispatch, driver) = fixture(
            ProviderKind::Claude,
            Behavior::WaitWithLateUsage,
            true,
            TaskAccess::ReadOnly,
        );
        let (_tx, rx) = watch::channel(false);
        let report = driver.execute(dispatch, service, rx).await;
        assert_eq!(
            report.disposition,
            super::super::ExecutionDisposition::Waiting
        );
        assert_eq!(report.usage.total_tokens, 46);
        assert!(!report.usage.tokens_unknown);
        assert_eq!(report.waiting_for, vec!["child"]);
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn workflow_executor_recovery_refuses_missing_evidence_and_live_host() {
        assert!(prove_quiescent(&json!({})).is_err());
        let stat = std::fs::read_to_string(format!("/proc/{}/stat", std::process::id())).unwrap();
        let start = stat
            .rsplit_once(") ")
            .unwrap()
            .1
            .split_whitespace()
            .nth(19)
            .unwrap()
            .parse::<u64>()
            .unwrap();
        let boot = std::fs::read_to_string("/proc/sys/kernel/random/boot_id").unwrap();
        let evidence = json!({"process":{"kind":"linux_process_group","pid":u32::MAX,"start_time_ticks":1,
            "boot_id":boot.trim(),"host_pid":std::process::id(),"host_start_time_ticks":start}});
        assert!(prove_quiescent(&evidence)
            .unwrap_err()
            .contains("prior host"));
    }
}
