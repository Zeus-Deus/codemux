//! Runs the `/workflow` program (the exact source the app ships) through the
//! production script engine and scheduler with a deterministic driver. No
//! model is called.

use async_trait::async_trait;
use codemux_lib::workflows::{
    scripts::{validate_source, WorkflowScriptRuntime, SCRIPT_API_VERSION},
    Dispatch, ExecutionReport, RouteSpec, RunMode, RunSnapshot, RunSpec, RunStatus, ScriptSpec,
    TaskAccess, TaskStatus, WorkflowDriver, WorkflowLimits, WorkflowService,
};
use serde_json::{json, Value};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::watch;

const CHAT_WORKFLOW_SCRIPT: &str =
    include_str!("../../../src/lib/workflows/chat-workflow.script.js");

/// Plays every agent: a fixed plan, one summary per subtask, then a report
/// that echoes what it was given. Records each dispatch for assertions.
struct ScriptedAgents {
    plan: Value,
    fail_task: Option<&'static str>,
    seen: Mutex<Vec<Dispatch>>,
}

#[async_trait]
impl WorkflowDriver for ScriptedAgents {
    async fn execute(
        &self,
        dispatch: Dispatch,
        _service: WorkflowService,
        _cancellation: watch::Receiver<bool>,
    ) -> ExecutionReport {
        self.seen.lock().unwrap().push(dispatch.clone());
        if self.fail_task == Some(dispatch.task_id.as_str()) {
            return ExecutionReport::failed("scripted failure");
        }
        let output = match dispatch.task_id.as_str() {
            "plan" => self.plan.clone(),
            "report" => json!({"summary": format!("Report from: {}", dispatch.task.prompt)}),
            id => json!({"summary": format!("finding from {id} on {}", dispatch.route.id)}),
        };
        let mut report = ExecutionReport::success(output);
        report.usage.tokens_unknown = false;
        report.usage.cost_usd = Some(0.0);
        report.usage.cost_unknown = false;
        report
    }
}

fn spec(allow_writes: bool) -> RunSpec {
    RunSpec {
        workspace_id: "chat-workflow-fixture".into(),
        title: "Review auth".into(),
        goal: "Review the auth module".into(),
        mode: RunMode::DryRun,
        allow_writes,
        routes: vec![
            RouteSpec { id: "claude-opus".into(), provider: "claude".into(), model: Some("opus".into()), effort: None },
            RouteSpec { id: "claude-haiku".into(), provider: "claude".into(), model: Some("haiku".into()), effort: None },
        ],
        limits: WorkflowLimits { concurrency: 4, max_tasks: 32, max_attempts: 64, max_depth: 3, token_budget: None, max_output_bytes: 262_144, wall_time_ms: 60_000 },
        tasks: vec![],
        script: Some(ScriptSpec {
            source: CHAT_WORKFLOW_SCRIPT.into(),
            args: json!({"max_workers": 2, "route_labels": {"claude-opus": "Claude Opus", "claude-haiku": "Claude Haiku"}}),
            api_version: SCRIPT_API_VERSION,
        }),
    }
}

async fn run(driver: Arc<ScriptedAgents>, allow_writes: bool) -> RunSnapshot {
    validate_source(CHAT_WORKFLOW_SCRIPT).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let service = WorkflowService::open(directory.path().join("runs.sqlite"), 4).unwrap();
    let (created, _) = service.create_paused(spec(allow_writes), "chat-workflow").unwrap();
    service.resume(&created.id).unwrap();
    let runtime = tokio::spawn({
        let service = service.clone();
        let driver = driver.clone();
        async move { service.run_driver(driver).await }
    });
    let scripts = WorkflowScriptRuntime::default();
    scripts.start(service.clone(), &created.id).unwrap();
    let finished = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let run = service.snapshot(&created.id).unwrap();
            let script_done = run.script.as_ref().is_some_and(|script| {
                matches!(script.status, codemux_lib::workflows::ScriptStatus::Completed | codemux_lib::workflows::ScriptStatus::Failed)
            });
            if script_done && matches!(run.status, RunStatus::Completed | RunStatus::Failed | RunStatus::Cancelled) {
                break run;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("workflow did not settle");
    service.shutdown_signal().unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(10), runtime).await;
    let _ = tokio::time::timeout(Duration::from_secs(10), scripts.drain_active()).await;
    finished
}

fn plan(tasks: Value) -> Value {
    json!({ "tasks": tasks })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_workflow_plans_fans_out_and_reports_through_the_engine() {
    let driver = Arc::new(ScriptedAgents {
        plan: plan(json!([
            {"title": "Sessions", "prompt": "Check session expiry", "route_id": "claude-opus", "edits": true},
            {"title": "Tests", "prompt": "Find missing tests", "route_id": "claude-haiku", "edits": false},
            {"title": "Over the cap", "prompt": "Never runs", "route_id": "claude-haiku", "edits": false}
        ])),
        fail_task: None,
        seen: Mutex::new(vec![]),
    });
    // max_workers is 2, so the planner's schema allows at most two subtasks
    // and the host rejects this three-item plan.
    let rejected = run(driver, false).await;
    assert_eq!(rejected.status, RunStatus::Failed);
    assert!(rejected.script.as_ref().and_then(|script| script.error.as_deref()).unwrap_or_default().contains("Planning failed"));

    let driver = Arc::new(ScriptedAgents {
        plan: plan(json!([
            {"title": "Sessions", "prompt": "Check session expiry", "route_id": "claude-opus", "edits": true},
            {"title": "Tests", "prompt": "Find missing tests", "route_id": "claude-haiku", "edits": false}
        ])),
        fail_task: None,
        seen: Mutex::new(vec![]),
    });
    let finished = run(driver.clone(), false).await;
    assert_eq!(finished.status, RunStatus::Completed, "run error: {:?} / {:?}", finished.error, finished.script);
    let ids: Vec<&str> = finished.tasks.iter().map(|task| task.spec.id.as_str()).collect();
    assert_eq!(ids, ["plan", "work-1", "work-2", "report"]);
    assert!(finished.tasks.iter().all(|task| task.status == TaskStatus::Succeeded));
    let work: Vec<(&str, &str, TaskAccess)> = finished
        .tasks
        .iter()
        .filter(|task| task.spec.id.starts_with("work-"))
        .map(|task| (task.spec.title.as_str(), task.spec.route_id.as_deref().unwrap(), task.spec.access))
        .collect();
    // A read-only run never grants write access, even when the plan asks for edits.
    assert_eq!(work, [("Sessions", "claude-opus", TaskAccess::ReadOnly), ("Tests", "claude-haiku", TaskAccess::ReadOnly)]);
    let seen = driver.seen.lock().unwrap();
    let planner = seen.iter().find(|dispatch| dispatch.task_id == "plan").unwrap();
    assert!(planner.task.prompt.contains("claude-haiku: Claude Haiku"));
    let report = finished.script.as_ref().unwrap().result.as_ref().unwrap()["summary"].as_str().unwrap().to_owned();
    assert!(report.contains("finding from work-1 on claude-opus"));
    assert!(report.contains("finding from work-2 on claude-haiku"));
    assert_eq!(finished.script.as_ref().unwrap().phase.as_deref(), Some("Report"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_workflow_reports_failed_subtasks_and_scopes_writes() {
    let driver = Arc::new(ScriptedAgents {
        plan: plan(json!([
            {"title": "Fix", "prompt": "Fix the bug", "route_id": "claude-opus", "edits": true},
            {"title": "Read", "prompt": "Read the docs", "route_id": "claude-haiku", "edits": false}
        ])),
        fail_task: Some("work-2"),
        seen: Mutex::new(vec![]),
    });
    let finished = run(driver, true).await;
    // Subtasks are optional, so one failure still reaches the report.
    assert_eq!(finished.status, RunStatus::Completed, "run error: {:?} / {:?}", finished.error, finished.script);
    let fix = finished.tasks.iter().find(|task| task.spec.id == "work-1").unwrap();
    assert_eq!((fix.spec.access, fix.spec.scope.clone()), (TaskAccess::Write, vec![".".to_owned()]));
    let read = finished.tasks.iter().find(|task| task.spec.id == "work-2").unwrap();
    assert_eq!((read.spec.access, read.status), (TaskAccess::ReadOnly, TaskStatus::Failed));
    let report = finished.script.as_ref().unwrap().result.as_ref().unwrap()["summary"].as_str().unwrap().to_owned();
    assert!(report.contains("scripted failure"), "report prompt lacked the failure: {report}");
}
