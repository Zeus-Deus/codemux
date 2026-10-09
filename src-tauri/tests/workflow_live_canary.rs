//! Manual paid canary. Normal test runs never start a real model.
#![cfg(target_os = "linux")]

use async_trait::async_trait;
use codemux_lib::{
    agent_provider::{
        claude::{ClaudeAgentProvider, ClaudeProviderConfig},
        managed::{lookup_session, ManagedCapabilities},
        AgentProvider, ApprovalDecision, CompletedItem, ProviderCapabilities, ProviderError,
        ProviderEventStream, ProviderKind, ProviderRuntimeEvent, ProviderSession, RequestId,
        SendTurnInput, StartSessionInput, ThreadId, TurnId, TurnStartResult,
    },
    workflows::{
        artifacts::ArtifactStore,
        executor::{LiveWorkflowDriver, ProviderLookup},
        tools::graph_tools,
        AttemptStatus, RouteSpec, RunMode, RunSpec, RunStatus, TaskAccess, TaskSpec, TaskStatus,
        WorkflowLimits, WorkflowService,
    },
};
use futures_util::StreamExt;
use serde_json::{json, Value};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

struct BoundedClaude {
    inner: ClaudeAgentProvider,
    starts: AtomicUsize,
    native_starts: AtomicUsize,
    metadata_only: bool,
    trace: Arc<Mutex<Vec<Value>>>,
}

const REPORT_SANITIZATION_NOTE: &str = "Sanitized observed_output_fields contain only fixture numeric fields. observer.matches_expected is a host-generated exact comparison, not a model-submitted field. Recorded run outcomes and accounting are unchanged.";

fn bounded_text(text: &str, limit: usize) -> String {
    text.chars().take(limit).collect()
}

fn diagnostic_task_id(id: &str) -> &'static str {
    match id {
        "coordinator" => "coordinator",
        "reader_one" => "reader_one",
        "reader_two" => "reader_two",
        _ => "unexpected_task",
    }
}

fn diagnostic_error(error: Option<&str>) -> Value {
    match error {
        None => Value::Null,
        Some(error) => json!({
            "category":if error.starts_with("managed-start-rejected: metadata-only") {
                "metadata_only_start_rejected"
            } else if error.starts_with("managed-start-rejected:") {
                "managed_start_rejected"
            } else {
                "provider_or_execution_error"
            },
            "message_bytes":error.len()
        }),
    }
}

fn diagnostic_output(id: &str, output: Option<&Value>) -> Value {
    let Some(output) = output else {
        return Value::Null;
    };
    match id {
        "coordinator" => {
            json!({"sum":output.get("sum").and_then(Value::as_i64),"child_count":output.get("child_count").and_then(Value::as_i64)})
        }
        "reader_one" | "reader_two" => {
            json!({"value":output.get("value").and_then(Value::as_i64)})
        }
        _ => Value::Null,
    }
}

fn diagnostic_output_matches(id: &str, output: Option<&Value>) -> Option<bool> {
    let output = output?;
    let expected = match id {
        "coordinator" => json!({"sum":42,"child_count":2}),
        "reader_one" => json!({"value":17}),
        "reader_two" => json!({"value":25}),
        _ => return None,
    };
    Some(output == &expected)
}

fn observe_event(trace: &Mutex<Vec<Value>>, event: &ProviderRuntimeEvent) {
    let mut trace = trace.lock().unwrap();
    if trace.len() >= 64 {
        return;
    }
    let item = match event {
        ProviderRuntimeEvent::ItemCompleted { item, .. } => match item {
            CompletedItem::AssistantText { text } => {
                let used: usize = trace
                    .iter()
                    .filter_map(|event| event.get("text")?.as_str())
                    .map(|text| text.chars().count())
                    .sum();
                if used >= 2048 {
                    return;
                }
                json!({"kind":"assistant_text","text":bounded_text(text,2048-used)})
            }
            CompletedItem::ToolUse { tool_name, .. } => {
                json!({"kind":"tool_use","name":bounded_text(tool_name,128)})
            }
            CompletedItem::ToolResult { is_error, .. } => {
                json!({"kind":"tool_result","is_error":is_error})
            }
            CompletedItem::AssistantThinking { .. } => return,
        },
        ProviderRuntimeEvent::RuntimeWarning {
            message,
            original_payload: Some(payload),
            ..
        } if message == "Managed native tool catalog" => {
            let tools: Vec<String> = payload
                .get("tools")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .take(128)
                .map(|tool| bounded_text(tool, 128))
                .collect();
            let servers:Vec<Value>=payload.get("mcp_servers").and_then(Value::as_array).into_iter().flatten().take(32).filter_map(|server|Some(json!({"name":bounded_text(server.get("name")?.as_str()?,128),"status":bounded_text(server.get("status")?.as_str()?,32)}))).collect();
            json!({"kind":"native_tool_catalog","tools":tools,"mcp_servers":servers})
        }
        _ => return,
    };
    trace.push(item);
}

#[test]
fn managed_canary_observer_is_bounded_and_redacts_payloads() {
    let trace = Mutex::new(Vec::new());
    let completed = |item| ProviderRuntimeEvent::ItemCompleted {
        thread_id: ThreadId("fictional-session".into()),
        turn_id: TurnId("fictional-turn".into()),
        item,
        subagent_id: None,
    };
    observe_event(
        &trace,
        &completed(CompletedItem::AssistantThinking {
            text: "private-thinking-fixture".into(),
        }),
    );
    observe_event(
        &trace,
        &completed(CompletedItem::ToolUse {
            tool_name: "workflow_read_file".into(),
            input: json!({"prompt":"private-prompt-fixture"}),
            tool_use_id: "private-identifier".into(),
        }),
    );
    observe_event(
        &trace,
        &completed(CompletedItem::ToolResult {
            tool_use_id: "private-identifier".into(),
            content: json!({"auth":"private-auth-fixture"}),
            is_error: true,
        }),
    );
    observe_event(
        &trace,
        &ProviderRuntimeEvent::RuntimeWarning {
            thread_id: None,
            message: "Managed native tool catalog".into(),
            original_payload: Some(
                json!({"tools":["workflow_read_file"],"mcp_servers":[{"name":"codemux_workflow","status":"connected","auth":"private-auth-fixture"}],"prompt":"private-prompt-fixture"}),
            ),
        },
    );
    observe_event(
        &trace,
        &completed(CompletedItem::AssistantText {
            text: "🦀".repeat(3000),
        }),
    );
    observe_event(
        &trace,
        &completed(CompletedItem::AssistantText {
            text: "ignored-after-limit".into(),
        }),
    );
    let observed = trace.lock().unwrap();
    assert_eq!(observed.len(), 4);
    assert_eq!(observed[3]["text"].as_str().unwrap().chars().count(), 2048);
    let serialized = serde_json::to_string(&*observed).unwrap();
    assert!(!serialized.contains("private-"));
    assert!(!serialized.contains("ignored-after-limit"));
    assert_eq!(observed[1]["is_error"], true);
    assert_eq!(
        observed[2]["mcp_servers"],
        json!([{"name":"codemux_workflow","status":"connected"}])
    );
    drop(observed);
    for _ in 0..100 {
        observe_event(
            &trace,
            &completed(CompletedItem::ToolUse {
                tool_name: "x".repeat(200),
                input: Value::Null,
                tool_use_id: String::new(),
            }),
        );
    }
    let observed = trace.lock().unwrap();
    assert_eq!(observed.len(), 64);
    assert_eq!(
        observed.last().unwrap()["name"]
            .as_str()
            .unwrap()
            .chars()
            .count(),
        128
    );
    assert!(!diagnostic_error(Some("private-auth-fixture"))
        .to_string()
        .contains("private-auth"));
    assert_eq!(
        diagnostic_output(
            "reader_one",
            Some(&json!({"value":17,"private":"private-auth-fixture"}))
        ),
        json!({"value":17})
    );
    assert_eq!(
        diagnostic_output_matches(
            "reader_one",
            Some(&json!({"value":17,"private":"private-auth-fixture"}))
        ),
        Some(false)
    );
    assert_eq!(
        diagnostic_output_matches("reader_one", Some(&json!({"value":17}))),
        Some(true)
    );
    assert_eq!(
        diagnostic_output_matches("coordinator", Some(&json!({"sum":42,"child_count":2}))),
        Some(true)
    );
    assert_eq!(
        diagnostic_output(
            "unexpected-private-id",
            Some(&json!({"private":"private-prompt-fixture"}))
        ),
        Value::Null
    );
    assert_eq!(
        diagnostic_output_matches(
            "unexpected-private-id",
            Some(&json!({"private":"private-prompt-fixture"}))
        ),
        None
    );
}
#[async_trait]
impl AgentProvider for BoundedClaude {
    fn kind(&self) -> ProviderKind {
        self.inner.kind()
    }
    fn capabilities(&self) -> ProviderCapabilities {
        self.inner.capabilities()
    }
    fn managed_capabilities(&self) -> ManagedCapabilities {
        self.inner.managed_capabilities()
    }
    async fn start_session(
        &self,
        mut input: StartSessionInput,
    ) -> Result<ProviderSession, ProviderError> {
        let context =
            lookup_session(&input.thread_id).ok_or_else(|| ProviderError::ValidationError {
                message: "managed-start-rejected: canary managed context missing".into(),
            })?;
        let report_dir = PathBuf::from("/tmp/codemux-workflow-design-review");
        std::fs::create_dir_all(&report_dir).unwrap();
        std::fs::write(
            report_dir.join("native-host-catalog.json"),
            serde_json::to_vec_pretty(&context.handler.tools()).unwrap(),
        )
        .unwrap();
        std::fs::write(report_dir.join("native-host-capabilities.json"),serde_json::to_vec_pretty(&json!({"kind":"configured_host_declarations","managed_capabilities":self.managed_capabilities(),"read_only":context.read_only})).unwrap()).unwrap();
        if self.starts.fetch_add(1, Ordering::SeqCst) >= 4 {
            return Err(ProviderError::ValidationError {
                message: "managed-start-rejected: manual canary four-session budget exhausted"
                    .into(),
            });
        }
        if self.metadata_only {
            return Err(ProviderError::ValidationError {
                message:
                    "managed-start-rejected: metadata-only host catalog exported; no native start"
                        .into(),
            });
        }
        input.extra = json!({"maxBudgetUsd":0.20,"maxTurns":6,"managedDiagnostics":true});
        input.env = Some(HashMap::from([
            ("CLAUDE_CODE_MAX_OUTPUT_TOKENS".into(), "1024".into()),
            ("CLAUDE_CODE_MAX_RETRIES".into(), "1".into()),
            ("CLAUDE_CODE_RESUME_INTERRUPTED_TURN".into(), "0".into()),
        ]));
        self.native_starts.fetch_add(1, Ordering::SeqCst);
        self.inner.start_session(input).await
    }
    async fn send_turn(&self, input: SendTurnInput) -> Result<TurnStartResult, ProviderError> {
        self.inner.send_turn(input).await
    }
    async fn interrupt_turn(
        &self,
        id: ThreadId,
        turn: Option<TurnId>,
    ) -> Result<(), ProviderError> {
        self.inner.interrupt_turn(id, turn).await
    }
    async fn respond_to_request(
        &self,
        id: ThreadId,
        request: RequestId,
        decision: ApprovalDecision,
    ) -> Result<(), ProviderError> {
        self.inner.respond_to_request(id, request, decision).await
    }
    async fn set_model(&self, id: ThreadId, model: String) -> Result<(), ProviderError> {
        self.inner.set_model(id, model).await
    }
    async fn set_permission_mode(&self, id: ThreadId, mode: String) -> Result<(), ProviderError> {
        self.inner.set_permission_mode(id, mode).await
    }
    async fn stop_session(&self, id: ThreadId) -> Result<(), ProviderError> {
        self.inner.stop_session(id).await
    }
    async fn stop_managed_session(&self, id: ThreadId) -> Result<(), ProviderError> {
        self.inner.stop_managed_session(id).await
    }
    async fn list_sessions(&self) -> Result<Vec<ProviderSession>, ProviderError> {
        self.inner.list_sessions().await
    }
    async fn has_session(&self, id: &ThreadId) -> bool {
        self.inner.has_session(id).await
    }
    fn event_stream(&self) -> ProviderEventStream {
        self.inner.event_stream()
    }
    fn managed_event_stream(&self, id: &ThreadId) -> ProviderEventStream {
        let trace = self.trace.clone();
        Box::pin(
            self.inner
                .managed_event_stream(id)
                .inspect(move |event| observe_event(&trace, event)),
        )
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "Paid native canary: requires explicit user authorization and CODEMUX_PAID_CANARY=yes"]
async fn workflow_live_native_coordination() {
    let metadata_only = std::env::var("CODEMUX_CANARY_METADATA_ONLY").as_deref() == Ok("yes");
    assert!(
        metadata_only || std::env::var("CODEMUX_PAID_CANARY").as_deref() == Ok("yes"),
        "paid canary not authorized"
    );
    let sidecar = PathBuf::from(
        std::env::var("CODEMUX_CANARY_SIDECAR").expect("explicit built sidecar path"),
    );
    let cli =
        PathBuf::from(std::env::var("CODEMUX_CANARY_CLAUDE").expect("explicit installed CLI path"));
    let report_path = PathBuf::from(
        std::env::var("CODEMUX_CANARY_REPORT").expect("explicit sanitized report path"),
    );
    let directory = tempfile::tempdir().unwrap();
    let workspace = directory.path().join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    std::fs::write(workspace.join("first.txt"), "17\n").unwrap();
    std::fs::write(workspace.join("second.txt"), "25\n").unwrap();
    assert!(std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(&workspace)
        .status()
        .unwrap()
        .success());
    let baseline_status = std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(&workspace)
        .output()
        .unwrap();
    assert!(baseline_status.status.success());
    let service = WorkflowService::open(directory.path().join("run.sqlite"), 1).unwrap();
    let provider = Arc::new(BoundedClaude {
        inner: ClaudeAgentProvider::new(ClaudeProviderConfig {
            sidecar_binary: Some(sidecar),
            claude_binary: Some(cli),
            event_channel_capacity: 4096,
            mcp_registry: None,
        })
        .await
        .unwrap(),
        starts: AtomicUsize::new(0),
        native_starts: AtomicUsize::new(0),
        metadata_only,
        trace: Arc::new(Mutex::new(Vec::new())),
    });
    let lookup: ProviderLookup = Arc::new({
        let provider = provider.clone();
        move |kind| {
            let provider = provider.clone();
            Box::pin(async move {
                if kind == ProviderKind::Claude {
                    Some(provider as Arc<dyn AgentProvider>)
                } else {
                    None
                }
            })
        }
    });
    let artifacts = Arc::new(ArtifactStore::new(directory.path().join("artifacts")));
    let driver = Arc::new(LiveWorkflowDriver::new(
        Arc::new({
            let workspace = workspace.clone();
            move |_| Ok(workspace.clone())
        }),
        lookup,
        Arc::new(graph_tools),
        artifacts.clone(),
    ));
    let prompt="Follow this exact coordination procedure. If Dependencies is empty: call workflow_add_tasks once with idempotency_key spawn and exactly two tasks. Task reader_one must read first.txt with workflow_read_file, then call workflow_submit_result with output {value: the integer read}; task reader_two does the same with second.txt. Both use route_id haiku, access read_only, required true, and output_schema an object requiring integer value. Child prompts must tell them to submit the JSON result and immediately end their turn. Then call workflow_wait with both IDs and end your turn immediately, without submitting your own result. If Dependencies contains both successful child outputs, submit your own result {sum: their two values added, child_count: 2}, then end your turn. Do not read the files yourself, create other tasks, or perform extra work.";
    let run=service.create(RunSpec{workspace_id:"isolated-canary".into(),title:"Native coordination canary".into(),goal:"Prove dynamic two-child fanout, capacity-one yield, typed child results and aggregation.".into(),mode:RunMode::Live,allow_writes:false,routes:vec![RouteSpec{id:"haiku".into(),provider:"claude".into(),model:Some("claude-haiku-5-5".into()),effort:None}],limits:WorkflowLimits{concurrency:1,max_tasks:3,max_attempts:4,max_depth:2,token_budget:Some(250_000),max_output_bytes:64*1024,wall_time_ms:90_000},tasks:vec![TaskSpec{id:"coordinator".into(),title:"Coordinator".into(),prompt:prompt.into(),dependencies:vec![],route_id:Some("haiku".into()),access:TaskAccess::ReadOnly,scope:vec![],output_schema:Some(json!({"type":"object","properties":{"sum":{"type":"integer","const":42},"child_count":{"type":"integer","const":2}},"required":["sum","child_count"],"additionalProperties":false})),required:true}],script:None},"create").unwrap();
    artifacts.pin_run(&run.id, &workspace).unwrap();
    let runtime = tokio::spawn({
        let service = service.clone();
        async move { service.run_driver(driver).await }
    });
    let terminal = tokio::time::timeout(Duration::from_secs(110), async {
        loop {
            let run = service.snapshot(&run.id).unwrap();
            if matches!(
                run.status,
                RunStatus::Completed
                    | RunStatus::Failed
                    | RunStatus::Cancelled
                    | RunStatus::Unknown
            ) {
                break run;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    if terminal.is_err() {
        service.cancel(&run.id).unwrap();
    }
    service.shutdown_signal().unwrap();
    let runtime_stopped = matches!(
        tokio::time::timeout(Duration::from_secs(20), runtime).await,
        Ok(Ok(Ok(())))
    );
    let final_run = service.snapshot(&run.id).unwrap();
    let remaining = provider.list_sessions().await.unwrap();
    let mut cleanup_errors = 0;
    for session in &remaining {
        if !matches!(
            tokio::time::timeout(
                Duration::from_secs(15),
                provider.stop_managed_session(session.thread_id.clone())
            )
            .await,
            Ok(Ok(()))
        ) {
            cleanup_errors += 1;
        }
    }
    let sessions = provider.list_sessions().await.unwrap();
    let incomplete_sessions = final_run
        .tasks
        .iter()
        .flat_map(|task| &task.attempts)
        .filter(|attempt| attempt.usage.cost_unknown)
        .count();
    let observed_cost = final_run.usage.cost_usd.unwrap_or(0.0);
    let conservative_cost_envelope = observed_cost + incomplete_sessions as f64 * 0.20;
    let attempts:Vec<Value>=final_run.tasks.iter().flat_map(|task|task.attempts.iter().map(move|attempt|json!({"task_id":diagnostic_task_id(&task.spec.id),"generation":attempt.generation,"status":attempt.status,"usage":attempt.usage,"error":diagnostic_error(attempt.error.as_deref())}))).collect();
    let typed_results:Vec<Value>=final_run.tasks.iter().map(|task|json!({"task_id":diagnostic_task_id(&task.spec.id),"status":task.status,"generation":task.generation,"observed_output_fields":diagnostic_output(&task.spec.id,task.result.as_ref()),"observer":{"matches_expected":diagnostic_output_matches(&task.spec.id,task.result.as_ref())}})).collect();
    let final_status = std::process::Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(&workspace)
        .output()
        .unwrap();
    let source_unchanged = final_status.status.success()
        && final_status.stdout == baseline_status.stdout
        && std::fs::read(workspace.join("first.txt")).unwrap() == b"17\n"
        && std::fs::read(workspace.join("second.txt")).unwrap() == b"25\n";
    let trace = provider.trace.lock().unwrap().clone();
    let report = json!({"case":"native_claude_dynamic_children_capacity_one","sanitization_note":REPORT_SANITIZATION_NOTE,"mode":if metadata_only{"metadata_only"}else{"paid_native_canary"},"provider":"claude","model":"claude-haiku-5-5","run_status":final_run.status,"budget_controls":{"authorized_total_usd":2.0,"native_session_limit":4,"sdk_max_budget_usd_per_session":0.20,"sdk_max_turns_per_session":6,"max_output_tokens_per_request":1024,"concurrency":1,"wall_time_ms":90_000,"token_admission_budget":250_000,"thinking":"native default; no override"},"native_start_requests":provider.starts.load(Ordering::SeqCst),"native_starts":provider.native_starts.load(Ordering::SeqCst),"host_capability_declarations":provider.managed_capabilities(),"host_catalog_file":"native-host-catalog.json","native_trace":trace,"source_unchanged":source_unchanged,"attempts":attempts,"typed_results":typed_results,"quiescence":{"driver_stopped":runtime_stopped,"live_provider_sessions":sessions.len(),"extra_cleanup_sessions":remaining.len(),"cleanup_errors":cleanup_errors,"all_attempts_terminal":final_run.tasks.iter().flat_map(|task|&task.attempts).all(|attempt|matches!(attempt.status,AttemptStatus::Succeeded|AttemptStatus::Failed|AttemptStatus::Cancelled|AttemptStatus::Waiting))},"accounting":final_run.usage,"spending_envelope":{"observed_sdk_cost_usd":observed_cost,"incomplete_session_count":incomplete_sessions,"reserved_usd_per_incomplete_session":0.20,"observed_plus_incomplete_reserve_usd":conservative_cost_envelope},"notes":["Cost is the sum of observed SDK counters; interrupted yield may remain explicitly unknown. SDK budget can overshoot by a final API call; session, turn, output and wall caps provide conservative margin.","Configured host tool schemas and capability declarations do not prove the native model's effective tool catalog. Only the separately captured SDK diagnostic records its reported catalog.","Paid mode exercises native Claude coordination only. Mixed Claude/Codex coordination is separately verified with deterministic providers. Metadata-only mode rejects before any native startup."]});
    std::fs::write(&report_path, serde_json::to_vec_pretty(&report).unwrap()).unwrap();
    assert!(runtime_stopped, "native runtime failed to stop");
    assert!(sessions.is_empty(), "native session still registered");
    assert_eq!(
        cleanup_errors, 0,
        "owned native cleanup could not be proved"
    );
    assert!(source_unchanged, "isolated source workspace changed");
    if metadata_only {
        assert_eq!(provider.native_starts.load(Ordering::SeqCst), 0);
        assert_eq!(provider.starts.load(Ordering::SeqCst), 1);
        assert_eq!(final_run.status, RunStatus::Failed);
        assert_eq!(final_run.usage.total_tokens, 0);
        assert_eq!(final_run.usage.reserved_tokens, 0);
        assert_eq!(final_run.usage.cost_usd, Some(0.0));
        assert!(!final_run.usage.tokens_unknown);
        assert!(!final_run.usage.cost_unknown);
        assert!(provider.trace.lock().unwrap().is_empty());
        return;
    }
    assert_eq!(
        final_run.status,
        RunStatus::Completed,
        "native coordination failed; sanitized report retained"
    );
    assert_eq!(final_run.tasks.len(), 3);
    assert!(final_run
        .tasks
        .iter()
        .all(|task| task.status == TaskStatus::Succeeded));
    let root = final_run
        .tasks
        .iter()
        .find(|task| task.spec.id == "coordinator")
        .unwrap();
    assert_eq!(
        root.attempts.len(),
        2,
        "coordinator duplicated or did not resume"
    );
    assert_eq!(
        root.attempts[0].status,
        AttemptStatus::Waiting,
        "root never yielded at capacity one"
    );
    assert_eq!(
        root.attempts[1].status,
        AttemptStatus::Succeeded,
        "root did not resume successfully"
    );
    assert_eq!(root.result, Some(json!({"sum":42,"child_count":2})));
    for (id, value) in [("reader_one", 17), ("reader_two", 25)] {
        let child = final_run
            .tasks
            .iter()
            .find(|task| task.spec.id == id)
            .unwrap();
        assert_eq!(
            child.result,
            Some(json!({"value":value})),
            "native child submitted the wrong value"
        );
        assert_eq!(
            child.attempts.len(),
            1,
            "native child executed more than once"
        );
        assert_eq!(child.generation, 1);
    }
    assert_eq!(provider.native_starts.load(Ordering::SeqCst), 4);
    assert!(
        final_run.usage.cost_usd.unwrap_or(0.0) < 2.0,
        "observed charge exceeded authorization"
    );
}
