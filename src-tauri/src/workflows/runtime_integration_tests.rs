//! Behavioral integration tests: deterministic providers exercise the real
//! scheduler, live executor, scoped tools, artifacts and durable database.
//! These providers never create a model session, subprocess or network call.

use super::*;
use crate::agent_provider::{
    managed::{lookup_session, ManagedCapabilities, ManagedSession},
    AgentProvider, ApprovalDecision, ProviderCapabilities, ProviderError, ProviderEventStream,
    ProviderKind, ProviderRuntimeEvent, ProviderSession, ProviderSessionId, RequestId,
    SendTurnInput, SessionStatus, StartSessionInput, ThreadId, TurnId, TurnStartResult, TurnStatus,
    TurnUsage,
};
use async_trait::async_trait;
use executor::{LiveWorkflowDriver, ProviderLookup};
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
use tokio::sync::{broadcast, watch, Notify};

#[derive(Default)]
struct Evidence {
    dispatches: Mutex<HashMap<String, Dispatch>>,
    turns: Mutex<Vec<Dispatch>>,
    contexts: Mutex<HashMap<String, Arc<ManagedSession>>>,
    calls: Mutex<Vec<(String, u64, String)>>,
    active: AtomicUsize,
    peak: AtomicUsize,
    checkpoint_repair: AtomicBool,
    checkpoint_reached: Notify,
    checkpoint_release: Notify,
    uncertain_started: Notify,
    permit_uncertain_stop: AtomicBool,
    sibling_started: Notify,
    sibling_release: Notify,
    branch_started: Notify,
    branch_stop_started: Notify,
    branch_stop_release: Notify,
}

struct ScriptedProvider {
    kind: ProviderKind,
    evidence: Arc<Evidence>,
    sessions: Mutex<HashMap<ThreadId, ProviderSession>>,
    events: broadcast::Sender<ProviderRuntimeEvent>,
}

impl ScriptedProvider {
    async fn tool(
        &self,
        context: &ManagedSession,
        dispatch: &Dispatch,
        name: &str,
        arguments: Value,
    ) -> Value {
        self.evidence.calls.lock().unwrap().push((
            dispatch.task_id.clone(),
            dispatch.generation,
            name.into(),
        ));
        context.call(name, arguments).await.unwrap_or_else(|error| {
            panic!(
                "{} generation {} {name}: {error}",
                dispatch.task_id, dispatch.generation
            )
        })
    }

    async fn synthesize(&self, context: &ManagedSession, dispatch: &Dispatch) {
        self.tool(
            context,
            dispatch,
            "workflow_import_dependency",
            json!({"task_id":"fragile"}),
        )
        .await;
        let accepted = self
            .tool(
                context,
                dispatch,
                "workflow_read_file",
                json!({"path":"src/accepted.txt"}),
            )
            .await;
        let repaired = self
            .tool(
                context,
                dispatch,
                "workflow_read_file",
                json!({"path":"src/repaired.txt"}),
            )
            .await;
        assert_eq!(accepted["content"], "accepted\n");
        assert_eq!(repaired["content"], "repaired\n");
        self.tool(
            context,
            dispatch,
            "workflow_write_file",
            json!({"path":"src/summary.txt","content":"accepted\nrepaired\n"}),
        )
        .await;
        self.tool(
            context,
            dispatch,
            "workflow_submit_result",
            json!({"output":{"ok":true,"combined":"accepted\nrepaired\n"}}),
        )
        .await;
    }
}

#[async_trait]
impl AgentProvider for ScriptedProvider {
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
        assert!(lookup_session(&input.thread_id).is_some());
        assert!(input.fresh_session && input.resume_cursor.is_none());
        let session = ProviderSession {
            thread_id: input.thread_id,
            provider: self.kind,
            session_id: ProviderSessionId("deterministic-no-model".into()),
            status: SessionStatus::Ready,
            resume_cursor: None,
        };
        assert!(self
            .sessions
            .lock()
            .unwrap()
            .insert(session.thread_id.clone(), session.clone())
            .is_none());
        let active = self.evidence.active.fetch_add(1, Ordering::SeqCst) + 1;
        self.evidence.peak.fetch_max(active, Ordering::SeqCst);
        Ok(session)
    }
    async fn send_turn(&self, input: SendTurnInput) -> Result<TurnStartResult, ProviderError> {
        let attempt = input.thread_id.0.strip_prefix("workflow-").unwrap();
        let dispatch = self
            .evidence
            .dispatches
            .lock()
            .unwrap()
            .get(attempt)
            .unwrap()
            .clone();
        assert_eq!(
            dispatch.route.provider,
            if self.kind == ProviderKind::Claude {
                "claude"
            } else {
                "codex"
            }
        );
        let context = lookup_session(&input.thread_id).unwrap();
        self.evidence
            .contexts
            .lock()
            .unwrap()
            .insert(dispatch.attempt_id.clone(), context.clone());
        self.evidence.turns.lock().unwrap().push(dispatch.clone());
        assert!(context.call("codemux_bash", json!({})).await.is_err());
        for message in &dispatch.messages {
            assert!(input.text.contains(message));
        }
        let mut status = TurnStatus::Success;
        match dispatch.task_id.as_str() {
            "coordinator" if dispatch.dependency_results.is_empty() => {
                let tasks = vec![
                    worker("accepted", "codex", "src/accepted.txt"),
                    worker("fragile", "claude", "src/repaired.txt"),
                ];
                let request = json!({"tasks":tasks,"idempotency_key":"children"});
                self.tool(&context, &dispatch, "workflow_add_tasks", request.clone())
                    .await;
                self.tool(&context, &dispatch, "workflow_add_tasks", request)
                    .await;
                assert!(context.call("workflow_add_tasks",json!({"tasks":[worker("escape","codex","outside.txt")],"idempotency_key":"escape"})).await.is_err());
                self.tool(&context,&dispatch,"workflow_message",json!({"task_id":"fragile","text":"Use only the narrow file scope","idempotency_key":"guidance"})).await;
                let waiting = self
                    .tool(
                        &context,
                        &dispatch,
                        "workflow_wait",
                        json!({"task_ids":["accepted","fragile"]}),
                    )
                    .await;
                assert_eq!(waiting["state"], "waiting");
            }
            "coordinator"
                if dispatch
                    .dependency_results
                    .iter()
                    .any(|(id, result)| id == "fragile" && result["status"] == "failed") =>
            {
                let ready = self
                    .tool(
                        &context,
                        &dispatch,
                        "workflow_wait",
                        json!({"task_ids":["accepted","fragile"]}),
                    )
                    .await;
                assert_eq!(ready["state"], "ready");
                assert!(ready["results"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|result| result["id"] == "fragile" && result["status"] == "failed"));
                let accepted = self
                    .tool(
                        &context,
                        &dispatch,
                        "workflow_read_dependency_file",
                        json!({"task_id":"accepted","path":"src/accepted.txt"}),
                    )
                    .await;
                assert_eq!(accepted["content"], "accepted\n");
                self.tool(
                    &context,
                    &dispatch,
                    "workflow_import_dependency",
                    json!({"task_id":"accepted"}),
                )
                .await;
                self.tool(&context,&dispatch,"workflow_replace",json!({"task_id":"fragile","spec":worker("fragile","codex","src/repaired.txt"),"idempotency_key":"repair"})).await;
                self.tool(&context,&dispatch,"workflow_message",json!({"task_id":"fragile","text":"Repair now with the Codex route","idempotency_key":"repair-guidance"})).await;
                self.tool(
                    &context,
                    &dispatch,
                    "workflow_wait",
                    json!({"task_ids":["fragile"]}),
                )
                .await;
            }
            "coordinator" => {
                // The previous yield waited only for the repaired child. A
                // fresh ready observation must grant access to the older
                // successful sibling as well, within this parent's scope.
                assert!(!dispatch
                    .dependency_results
                    .iter()
                    .any(|(id, _)| id == "accepted"));
                let ready = self
                    .tool(
                        &context,
                        &dispatch,
                        "workflow_wait",
                        json!({"task_ids":["accepted"]}),
                    )
                    .await;
                assert_eq!(ready["state"], "ready");
                let older = self
                    .tool(
                        &context,
                        &dispatch,
                        "workflow_read_dependency_file",
                        json!({"task_id":"accepted","path":"src/accepted.txt"}),
                    )
                    .await;
                assert_eq!(older["content"], "accepted\n");
                self.synthesize(&context, &dispatch).await;
            }
            "accepted" => {
                assert!(context
                    .call(
                        "workflow_write_file",
                        json!({"path":"outside.txt","content":"escape"})
                    )
                    .await
                    .is_err());
                self.tool(
                    &context,
                    &dispatch,
                    "workflow_write_file",
                    json!({"path":"src/accepted.txt","content":"accepted\n"}),
                )
                .await;
                self.tool(
                    &context,
                    &dispatch,
                    "workflow_submit_result",
                    json!({"output":{"ok":true,"file":"src/accepted.txt"}}),
                )
                .await;
            }
            "fragile" if dispatch.generation == 1 => {
                status = TurnStatus::Error {
                    subtype: "deterministic_fixture".into(),
                    message: "Controlled child failure; repair is required".into(),
                };
            }
            "fragile" => {
                assert!(dispatch
                    .messages
                    .iter()
                    .any(|message| message.contains("Repair")));
                self.tool(
                    &context,
                    &dispatch,
                    "workflow_write_file",
                    json!({"path":"src/repaired.txt","content":"repaired\n"}),
                )
                .await;
                self.tool(
                    &context,
                    &dispatch,
                    "workflow_submit_result",
                    json!({"output":{"ok":true,"file":"src/repaired.txt"}}),
                )
                .await;
            }
            "script-synth" => {
                self.tool(
                    &context,
                    &dispatch,
                    "workflow_import_dependency",
                    json!({"task_id":"accepted"}),
                )
                .await;
                self.synthesize(&context, &dispatch).await;
            }
            "uncertain" => {
                self.tool(
                    &context,
                    &dispatch,
                    "workflow_write_file",
                    json!({"path":"src/pending.txt","content":"unaccepted\n"}),
                )
                .await;
                self.evidence.uncertain_started.notify_one();
                return Ok(TurnStartResult {
                    turn_id: TurnId(dispatch.attempt_id),
                    steered: false,
                    queued_id: None,
                });
            }
            "branch-stop" => {
                self.evidence.branch_started.notify_one();
                return Ok(TurnStartResult {
                    turn_id: TurnId(dispatch.attempt_id), steered: false, queued_id: None,
                });
            }
            "active-sibling" => {
                self.evidence.sibling_started.notify_one();
                self.evidence.sibling_release.notified().await;
                self.tool(&context, &dispatch, "workflow_write_file", json!({"path":"src/survives.txt","content":"independent work\n"})).await;
                self.tool(&context, &dispatch, "workflow_submit_result", json!({"output":{"ok":true}})).await;
            }
            "survivor" => {
                assert!(context
                    .call(
                        "workflow_write_file",
                        json!({"path":"src/accepted.txt","content":"forbidden"})
                    )
                    .await
                    .is_err());
                self.tool(
                    &context,
                    &dispatch,
                    "workflow_submit_result",
                    json!({"output":{"ok":true}}),
                )
                .await;
            }
            other => panic!("Unexpected scripted task {other}"),
        }
        let turn = TurnId(dispatch.attempt_id);
        self.events
            .send(ProviderRuntimeEvent::UsageRecorded {
                thread_id: input.thread_id.clone(),
                provider: self.kind,
                model: Some("deterministic-no-model".into()),
                subagent: false,
                input_tokens: 0,
                output_tokens: 0,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                reasoning_tokens: 0,
                cost_usd: Some(0.0),
                cost_source: None,
            })
            .unwrap();
        self.events
            .send(ProviderRuntimeEvent::TurnCompleted {
                thread_id: input.thread_id,
                turn_id: turn.clone(),
                status,
                usage: Some(TurnUsage {
                    total_cost_usd: Some(0.0),
                    duration_ms: 0,
                    num_turns: 0,
                }),
            })
            .unwrap();
        Ok(TurnStartResult {
            turn_id: turn,
            steered: false,
            queued_id: None,
        })
    }
    async fn stop_managed_session(&self, id: ThreadId) -> Result<(), ProviderError> {
        let attempt = id.0.strip_prefix("workflow-").unwrap();
        let dispatch = self
            .evidence
            .dispatches
            .lock()
            .unwrap()
            .get(attempt)
            .unwrap()
            .clone();
        if dispatch.task_id == "branch-stop" {
            self.evidence.branch_stop_started.notify_one();
            self.evidence.branch_stop_release.notified().await;
        }
        if dispatch.task_id == "uncertain"
            && !self.evidence.permit_uncertain_stop.load(Ordering::SeqCst)
        {
            return Err(ProviderError::ValidationError {
                message: "Deterministic execution remains live; stop is unverified".into(),
            });
        }
        if dispatch.task_id == "coordinator"
            && dispatch
                .dependency_results
                .iter()
                .any(|(id, result)| id == "fragile" && result["status"] == "failed")
            && self
                .evidence
                .checkpoint_repair
                .swap(false, Ordering::SeqCst)
        {
            self.evidence.checkpoint_reached.notify_one();
            self.evidence.checkpoint_release.notified().await;
        }
        if self.sessions.lock().unwrap().remove(&id).is_some() {
            self.evidence.active.fetch_sub(1, Ordering::SeqCst);
        }
        Ok(())
    }
    async fn interrupt_turn(&self, _: ThreadId, _: Option<TurnId>) -> Result<(), ProviderError> {
        Ok(())
    }
    async fn respond_to_request(
        &self,
        _: ThreadId,
        _: RequestId,
        _: ApprovalDecision,
    ) -> Result<(), ProviderError> {
        panic!("Fixture must never request or grant approval")
    }
    async fn set_model(&self, _: ThreadId, _: String) -> Result<(), ProviderError> {
        Ok(())
    }
    async fn set_permission_mode(&self, _: ThreadId, _: String) -> Result<(), ProviderError> {
        Ok(())
    }
    async fn stop_session(&self, _: ThreadId) -> Result<(), ProviderError> {
        panic!("An unverified stop must never release capacity")
    }
    async fn list_sessions(&self) -> Result<Vec<ProviderSession>, ProviderError> {
        Ok(self.sessions.lock().unwrap().values().cloned().collect())
    }
    async fn has_session(&self, id: &ThreadId) -> bool {
        self.sessions.lock().unwrap().contains_key(id)
    }
    fn event_stream(&self) -> ProviderEventStream {
        let receiver = self.events.subscribe();
        Box::pin(futures_util::stream::unfold(
            receiver,
            |mut receiver| async move { receiver.recv().await.ok().map(|event| (event, receiver)) },
        ))
    }
}

fn worker(id: &str, route: &str, path: &str) -> TaskSpec {
    TaskSpec {
        id: id.into(),
        title: id.into(),
        prompt: "Deterministic fixture instruction".into(),
        dependencies: vec![],
        route_id: Some(route.into()),
        access: TaskAccess::Write,
        scope: vec![path.into()],
        output_schema: Some(
            json!({"type":"object","required":["ok"],"properties":{"ok":{"type":"boolean"}}}),
        ),
        required: true,
    }
}
fn run_spec(tasks: Vec<TaskSpec>, concurrency: usize) -> RunSpec {
    RunSpec {
        workspace_id: "runtime-fixture".into(),
        title: "Deterministic managed coordination".into(),
        goal: "Exercise real host coordination without model calls".into(),
        mode: RunMode::Live,
        allow_writes: true,
        routes: vec![
            RouteSpec {
                id: "claude".into(),
                provider: "claude".into(),
                model: Some("deterministic-no-model".into()),
                effort: None,
            },
            RouteSpec {
                id: "codex".into(),
                provider: "codex".into(),
                model: Some("deterministic-no-model".into()),
                effort: None,
            },
        ],
        limits: WorkflowLimits {
            concurrency,
            max_tasks: 16,
            max_attempts: 32,
            wall_time_ms: 10_000,
            ..Default::default()
        },
        tasks,
        script: None,
    }
}

struct Fixture {
    _directory: tempfile::TempDir,
    database: PathBuf,
    source: PathBuf,
    service: WorkflowService,
    artifacts: Arc<artifacts::ArtifactStore>,
    evidence: Arc<Evidence>,
    lookup: ProviderLookup,
}
impl Fixture {
    fn new(capacity: usize) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source");
        std::fs::create_dir_all(source.join("src")).unwrap();
        assert!(std::process::Command::new("git")
            .args(["init", "-q"])
            .arg(&source)
            .status()
            .unwrap()
            .success());
        std::fs::write(source.join("src/accepted.txt"), "base-a\n").unwrap();
        std::fs::write(source.join("src/repaired.txt"), "base-b\n").unwrap();
        std::fs::write(source.join("outside.txt"), "outside scope\n").unwrap();
        let database = directory.path().join("workflow.sqlite");
        let service = WorkflowService::open(&database, capacity).unwrap();
        let evidence = Arc::new(Evidence::default());
        let providers: HashMap<ProviderKind, Arc<dyn AgentProvider>> =
            [ProviderKind::Claude, ProviderKind::Codex]
                .into_iter()
                .map(|kind| {
                    let (events, _) = broadcast::channel(256);
                    (
                        kind,
                        Arc::new(ScriptedProvider {
                            kind,
                            evidence: evidence.clone(),
                            sessions: Mutex::new(HashMap::new()),
                            events,
                        }) as Arc<dyn AgentProvider>,
                    )
                })
                .collect();
        let lookup: ProviderLookup = Arc::new(move |kind| {
            let provider = providers.get(&kind).cloned();
            Box::pin(async move { provider })
        });
        let artifacts = Arc::new(artifacts::ArtifactStore::new(
            directory.path().join("artifacts"),
        ));
        Self {
            _directory: directory,
            database,
            source,
            service,
            artifacts,
            evidence,
            lookup,
        }
    }
    fn driver(&self) -> Arc<LiveWorkflowDriver> {
        let source = self.source.clone();
        let evidence = self.evidence.clone();
        Arc::new(LiveWorkflowDriver::new(
            Arc::new(move |id| {
                assert_eq!(id, "runtime-fixture");
                Ok(source.clone())
            }),
            self.lookup.clone(),
            Arc::new(move |dispatch, service| {
                evidence
                    .dispatches
                    .lock()
                    .unwrap()
                    .insert(dispatch.attempt_id.clone(), dispatch.clone());
                tools::graph_tools(dispatch, service)
            }),
            self.artifacts.clone(),
        ))
    }
    fn create(&self, spec: RunSpec, key: &str) -> RunSnapshot {
        let (run, fresh) = self.service.create_paused(spec, key).unwrap();
        assert!(fresh);
        self.artifacts.pin_run(&run.id, &self.source).unwrap();
        self.service.resume(&run.id).unwrap()
    }
    async fn until(&self, run: &str, predicate: impl Fn(&RunSnapshot) -> bool) -> RunSnapshot {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let snapshot = self.service.snapshot(run).unwrap();
                if predicate(&snapshot) {
                    return snapshot;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap_or_else(|_| {
            panic!(
                "Fixture did not settle: {:?}; turns: {:?}",
                self.service.snapshot(run),
                self.evidence
                    .turns
                    .lock()
                    .unwrap()
                    .iter()
                    .map(|turn| (&turn.task_id, turn.generation, &turn.route.id))
                    .collect::<Vec<_>>()
            )
        })
    }
}
#[tokio::test]
async fn workflow_runtime_branch_cancel_preserves_active_sibling_and_script_wait() {
    let fixture = Fixture::new(2);
    let mut cancelled = worker("branch-stop", "claude", "src/pending.txt");
    cancelled.required = false;
    let mut spec = run_spec(vec![cancelled, worker("active-sibling", "codex", "src/survives.txt")], 2);
    spec.script = Some(ScriptSpec {
        source: "const result=await workflow.wait('active-sibling');return {status:result.status};".into(),
        args: json!({}), api_version: scripts::SCRIPT_API_VERSION,
    });
    let run = fixture.create(spec, "independent-branch-stop");
    let scripts = scripts::WorkflowScriptRuntime::default();
    scripts.start(fixture.service.clone(), &run.id).unwrap();
    let runtime = OwnedRuntime::start(fixture.service.clone(), fixture.driver());
    tokio::time::timeout(Duration::from_secs(3), fixture.evidence.sibling_started.notified()).await.unwrap();
    tokio::time::timeout(Duration::from_secs(3), fixture.evidence.branch_started.notified()).await.unwrap();
    fixture.service.cancel_task(&run.id, "branch-stop").unwrap();
    tokio::time::timeout(Duration::from_secs(3), fixture.evidence.branch_stop_started.notified()).await.unwrap();
    assert_eq!(fixture.service.snapshot(&run.id).unwrap().status, RunStatus::Stopping);
    fixture.evidence.sibling_release.notify_one();
    let stopping = fixture.until(&run.id, |run| run.tasks.iter().any(|task| task.spec.id == "active-sibling" && task.status == TaskStatus::Succeeded)).await;
    assert_eq!(stopping.status, RunStatus::Stopping);
    assert_ne!(stopping.script.as_ref().unwrap().status, ScriptStatus::Failed);
    let cancelled = stopping.tasks.iter().find(|task| task.spec.id == "branch-stop").unwrap();
    assert_eq!(cancelled.current_attempt.as_ref().unwrap().status, AttemptStatus::Stopping);
    let sibling = stopping.tasks.iter().find(|task| task.spec.id == "active-sibling").unwrap();
    let artifact = fixture.artifacts.inspect(&sibling.current_attempt.as_ref().unwrap().id).unwrap();
    assert_eq!(std::fs::read_to_string(artifact.checkout.join("src/survives.txt")).unwrap(), "independent work\n");
    fixture.evidence.branch_stop_release.notify_one();
    let completed = fixture.until(&run.id, |run| run.status == RunStatus::Completed).await;
    assert_eq!(completed.script.as_ref().unwrap().status, ScriptStatus::Completed);
    assert_eq!(completed.script.as_ref().unwrap().result, Some(json!({"status":"succeeded"})));
    assert_eq!(fixture.evidence.active.load(Ordering::SeqCst), 0);
    runtime.stop().await;
}

struct OwnedRuntime {
    shutdown: watch::Sender<bool>,
    task: Option<tokio::task::JoinHandle<Result<(), String>>>,
}
impl OwnedRuntime {
    fn start(service: WorkflowService, driver: Arc<LiveWorkflowDriver>) -> Self {
        let (shutdown, receiver) = watch::channel(false);
        Self {
            shutdown,
            task: Some(tokio::spawn(async move {
                service.run_driver_until(driver, receiver).await
            })),
        }
    }
    async fn stop(mut self) {
        let _ = self.shutdown.send(true);
        tokio::time::timeout(Duration::from_secs(3), self.task.take().unwrap())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
}
impl Drop for OwnedRuntime {
    fn drop(&mut self) {
        let _ = self.shutdown.send(true);
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

#[tokio::test]
async fn workflow_runtime_mixed_routes_repair_yield_import_and_reopen_with_capacity_one() {
    let mut fixture = Fixture::new(1);
    fixture
        .evidence
        .checkpoint_repair
        .store(true, Ordering::SeqCst);
    let coordinator = worker("coordinator", "claude", "src");
    let run = fixture.create(run_spec(vec![coordinator], 1), "native-coordinator");
    let runtime = OwnedRuntime::start(fixture.service.clone(), fixture.driver());
    tokio::time::timeout(
        Duration::from_secs(5),
        fixture.evidence.checkpoint_reached.notified(),
    )
    .await
    .unwrap();
    fixture.service.pause(&run.id).unwrap();
    fixture.evidence.checkpoint_release.notify_one();
    let checkpoint = fixture
        .until(&run.id, |snapshot| {
            snapshot.status == RunStatus::Paused
                && snapshot.tasks[0]
                    .current_attempt
                    .as_ref()
                    .is_some_and(|attempt| attempt.status == AttemptStatus::Waiting)
        })
        .await;
    assert_eq!(checkpoint.tasks.len(), 3);
    assert_eq!(
        checkpoint
            .tasks
            .iter()
            .find(|task| task.spec.id == "accepted")
            .unwrap()
            .attempts
            .len(),
        1
    );
    assert_eq!(
        checkpoint
            .tasks
            .iter()
            .find(|task| task.spec.id == "fragile")
            .unwrap()
            .generation,
        2
    );
    runtime.stop().await;
    assert_eq!(fixture.evidence.active.load(Ordering::SeqCst), 0);
    let before = fixture.evidence.turns.lock().unwrap().len();
    fixture.service = WorkflowService::open(&fixture.database, 1).unwrap();
    assert!(fixture.service.claim_next().unwrap().is_none());
    assert_eq!(fixture.evidence.turns.lock().unwrap().len(), before);
    fixture.service.resume(&run.id).unwrap();
    let runtime = OwnedRuntime::start(fixture.service.clone(), fixture.driver());
    let finished = fixture
        .until(&run.id, |snapshot| snapshot.status == RunStatus::Completed)
        .await;
    runtime.stop().await;
    assert_eq!(fixture.evidence.peak.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.evidence.active.load(Ordering::SeqCst), 0);
    let turns = fixture.evidence.turns.lock().unwrap();
    let sequence: Vec<_> = turns
        .iter()
        .map(|turn| {
            (
                turn.task_id.as_str(),
                turn.generation,
                turn.route.id.as_str(),
            )
        })
        .collect();
    assert_eq!(
        sequence,
        vec![
            ("coordinator", 1, "claude"),
            ("accepted", 1, "codex"),
            ("fragile", 1, "claude"),
            ("coordinator", 1, "claude"),
            ("fragile", 2, "codex"),
            ("coordinator", 1, "claude")
        ]
    );
    drop(turns);
    let parent = finished
        .tasks
        .iter()
        .find(|task| task.spec.id == "coordinator")
        .unwrap();
    assert_eq!(
        parent.result.as_ref().unwrap()["combined"],
        "accepted\nrepaired\n"
    );
    assert_eq!(parent.attempts.len(), 3);
    assert_eq!(finished.usage.total_tokens, 0);
    assert!(!finished.usage.tokens_unknown);
    assert_eq!(finished.usage.cost_usd, Some(0.0));
    assert_eq!(
        std::fs::read_to_string(fixture.source.join("src/accepted.txt")).unwrap(),
        "base-a\n"
    );
    let attempt = parent.current_attempt.as_ref().unwrap();
    let digest = attempt.artifacts[0]["digest"].as_str().unwrap();
    let report = fixture
        .service
        .with_accepted_attempt(
            &run.id,
            "coordinator",
            &attempt.id,
            parent.generation,
            |_, _| fixture.artifacts.integrate(&attempt.id, digest),
        )
        .unwrap();
    assert_eq!(
        report.changed_paths,
        vec!["src/accepted.txt", "src/repaired.txt", "src/summary.txt"]
    );
    fixture
        .service
        .with_accepted_attempt(
            &run.id,
            "coordinator",
            &attempt.id,
            parent.generation,
            |_, _| fixture.artifacts.integrate(&attempt.id, digest),
        )
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(fixture.source.join("src/summary.txt")).unwrap(),
        "accepted\nrepaired\n"
    );
    assert_eq!(
        std::fs::read_to_string(fixture.source.join("outside.txt")).unwrap(),
        "outside scope\n"
    );
    for context in fixture.evidence.contexts.lock().unwrap().values() {
        assert!(context
            .call("workflow_submit_result", json!({"output":{"ok":true}}))
            .await
            .is_err());
    }
}

#[tokio::test]
async fn workflow_runtime_unknown_cancel_holds_capacity_across_reopen_until_verified_stop() {
    let mut fixture = Fixture::new(1);
    let uncertain = fixture.create(
        run_spec(vec![worker("uncertain", "claude", "src/pending.txt")], 1),
        "uncertain",
    );
    let mut survivor = worker("survivor", "codex", "src");
    survivor.access = TaskAccess::ReadOnly;
    let queued = fixture.create(run_spec(vec![survivor], 1), "queued");
    let runtime = OwnedRuntime::start(fixture.service.clone(), fixture.driver());
    tokio::time::timeout(
        Duration::from_secs(5),
        fixture.evidence.uncertain_started.notified(),
    )
    .await
    .unwrap();
    fixture
        .service
        .cancel_task(&uncertain.id, "uncertain")
        .unwrap();
    let unknown = fixture
        .until(&uncertain.id, |run| run.status == RunStatus::Unknown)
        .await;
    assert_eq!(unknown.usage.reserved_tokens, 4096);
    assert!(fixture.service.claim_next().unwrap().is_none());
    assert!(fixture.service.retry(&uncertain.id, "uncertain").is_err());
    assert!(fixture.service.snapshot(&queued.id).unwrap().tasks[0]
        .attempts
        .is_empty());
    let dispatch = fixture.evidence.turns.lock().unwrap()[0].clone();
    let context = fixture
        .evidence
        .contexts
        .lock()
        .unwrap()
        .get(&dispatch.attempt_id)
        .unwrap()
        .clone();
    assert!(context
        .call(
            "workflow_write_file",
            json!({"path":"src/pending.txt","content":"late mutation"})
        )
        .await
        .is_err());
    assert!(fixture
        .artifacts
        .inspect(&dispatch.attempt_id)
        .unwrap()
        .sealed
        .is_none());
    assert!(!fixture.source.join("src/pending.txt").exists());
    runtime.stop().await;
    fixture.service = WorkflowService::open(&fixture.database, 1).unwrap();
    assert!(fixture.service.claim_next().unwrap().is_none());
    let reference = fixture.service.snapshot(&uncertain.id).unwrap().tasks[0]
        .current_attempt
        .as_ref()
        .unwrap()
        .external_ref
        .clone()
        .unwrap();
    let driver = fixture.driver();
    assert!(driver.reconcile_owned_runtime(&reference).await.is_err());
    assert!(fixture.service.claim_next().unwrap().is_none());
    fixture
        .evidence
        .permit_uncertain_stop
        .store(true, Ordering::SeqCst);
    driver.reconcile_owned_runtime(&reference).await.unwrap();
    let reconciled = fixture
        .service
        .reconcile_attempt(&dispatch, ExecutionReport::cancelled())
        .unwrap();
    assert_eq!(reconciled.usage.reserved_tokens, 0);
    assert_eq!(reconciled.tasks[0].status, TaskStatus::Cancelled);
    let runtime = OwnedRuntime::start(fixture.service.clone(), driver);
    fixture
        .until(&queued.id, |run| run.status == RunStatus::Completed)
        .await;
    runtime.stop().await;
    assert_eq!(fixture.evidence.turns.lock().unwrap().len(), 2);
    assert_eq!(fixture.evidence.peak.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.evidence.active.load(Ordering::SeqCst), 0);
    assert!(context
        .call("workflow_submit_result", json!({"output":{"ok":true}}))
        .await
        .is_err());
}

#[tokio::test]
async fn workflow_runtime_quickjs_controls_live_tool_routes_and_repairs_failed_child() {
    let fixture = Fixture::new(2);
    let mut spec = run_spec(vec![], 2);
    spec.script=Some(ScriptSpec{api_version:scripts::SCRIPT_API_VERSION,args:json!({"marker":"durable-live-fixture"}),source:r#"
        await workflow.phase('Inspect');
        const initial=await parallel([
          {id:'accepted',title:'Accepted',prompt:'Inspect',route_id:'codex',access:'write',scope:['src/accepted.txt']},
          {id:'fragile',title:'Fragile',prompt:'Inspect',route_id:'claude',access:'write',scope:['src/repaired.txt']}
        ],async spec=>{const id=await workflow.addTask(spec);return workflow.wait(id);});
        if(initial.find(task=>task.id==='fragile').status!=='failed')throw Error('Expected controlled failure');
        await workflow.message('fragile','Repair now through the Codex route');
        await workflow.replace('fragile',{title:'Repair',prompt:'Repair',route_id:'codex',access:'write',scope:['src/repaired.txt']});
        const repaired=await workflow.wait('fragile');
        if(repaired.status!=='succeeded')throw Error('Repair did not succeed');
        await workflow.phase('Synthesize');
        const summary=await agent('Synthesize',{id:'script-synth',route_id:'codex',access:'write',scope:['src'],dependencies:['accepted','fragile']});
        return {marker:args.marker,summary};
    "#.into()});
    let run = fixture.create(spec.clone(), "quickjs-live");
    let runtime = OwnedRuntime::start(fixture.service.clone(), fixture.driver());
    let scripts = scripts::WorkflowScriptRuntime::default();
    scripts.start(fixture.service.clone(), &run.id).unwrap();
    let completed = fixture
        .until(&run.id, |run| run.status == RunStatus::Completed)
        .await;
    runtime.stop().await;
    assert_eq!(
        completed.script.as_ref().unwrap().result.as_ref().unwrap()["marker"],
        "durable-live-fixture"
    );
    assert_eq!(completed.tasks.len(), 3);
    assert_eq!(
        completed
            .tasks
            .iter()
            .find(|task| task.spec.id == "accepted")
            .unwrap()
            .attempts
            .len(),
        1
    );
    let fragile = completed
        .tasks
        .iter()
        .find(|task| task.spec.id == "fragile")
        .unwrap();
    assert_eq!(fragile.generation, 2);
    assert_eq!(fragile.attempts.len(), 2);
    let synthesis = completed
        .tasks
        .iter()
        .find(|task| task.spec.id == "script-synth")
        .unwrap();
    assert_eq!(
        synthesis.result.as_ref().unwrap()["combined"],
        "accepted\nrepaired\n"
    );
    assert_eq!(completed.usage.total_tokens, 0);
    assert_eq!(completed.usage.cost_usd, Some(0.0));
    assert!(fixture.evidence.peak.load(Ordering::SeqCst) <= 2);
    let count = fixture.evidence.turns.lock().unwrap().len();
    assert_eq!(count, 4);
    assert_eq!(
        fixture
            .service
            .create_paused(spec, "quickjs-live")
            .unwrap()
            .0
            .id,
        run.id
    );
    scripts.start(fixture.service.clone(), &run.id).unwrap();
    assert_eq!(fixture.evidence.turns.lock().unwrap().len(), count);
    let persisted = WorkflowService::open(&fixture.database, 2)
        .unwrap()
        .snapshot(&run.id)
        .unwrap();
    assert_eq!(persisted.status, RunStatus::Completed);
    assert_eq!(persisted.tasks.len(), 3);
    assert_eq!(
        std::fs::read_to_string(fixture.source.join("src/repaired.txt")).unwrap(),
        "base-b\n"
    );
}
