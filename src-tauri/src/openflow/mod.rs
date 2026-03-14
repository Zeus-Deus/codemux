pub mod adapters;
pub mod agent;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use agent::{AgentSessionState, AgentSessionStatus};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OpenFlowRole {
    Orchestrator,
    Planner,
    Builder,
    Reviewer,
    Tester,
    Debugger,
    Researcher,
}

impl OpenFlowRole {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "orchestrator" => Some(Self::Orchestrator),
            "planner" => Some(Self::Planner),
            "builder" => Some(Self::Builder),
            "reviewer" => Some(Self::Reviewer),
            "tester" => Some(Self::Tester),
            "debugger" => Some(Self::Debugger),
            "researcher" => Some(Self::Researcher),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OpenFlowRunStatus {
    Draft,
    Planning,
    Executing,
    Verifying,
    Reviewing,
    AwaitingApproval,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OpenFlowTaskStatus {
    Pending,
    Ready,
    InProgress,
    Blocked,
    Passed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpenFlowArtifactKind {
    Plan,
    Log,
    Screenshot,
    Diff,
    ReviewNote,
    TestResult,
    BrowserEvidence,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpenFlowApprovalKind {
    RunStart,
    MajorChange,
    RiskyAction,
    FinalApply,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpenFlowPermission {
    ReadRepo,
    WriteRepo,
    RunCommands,
    BrowserControl,
    NetworkAccess,
    DestructiveAction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OpenFlowTimelineLevel {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenFlowWorkerContract {
    pub role: OpenFlowRole,
    pub responsibilities: Vec<String>,
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    pub required_permissions: Vec<OpenFlowPermission>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenFlowTaskNode {
    pub task_id: String,
    pub title: String,
    pub description: String,
    pub role: OpenFlowRole,
    pub status: OpenFlowTaskStatus,
    pub depends_on: Vec<String>,
    pub success_criteria: Vec<String>,
    pub produced_artifacts: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenFlowArtifact {
    pub artifact_id: String,
    pub kind: OpenFlowArtifactKind,
    pub title: String,
    pub location: Option<String>,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenFlowApprovalCheckpoint {
    pub checkpoint_id: String,
    pub kind: OpenFlowApprovalKind,
    pub title: String,
    pub required: bool,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenFlowStopPolicy {
    pub max_phases: u32,
    pub max_replans: u32,
    pub max_runtime_minutes: u32,
    pub require_tests_before_completion: bool,
    pub require_review_before_completion: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenFlowRunModel {
    pub run_type: String,
    pub status_model: Vec<OpenFlowRunStatus>,
    pub worker_contracts: Vec<OpenFlowWorkerContract>,
    pub phase_loop: Vec<String>,
    pub stop_policy: OpenFlowStopPolicy,
    pub approval_checkpoints: Vec<OpenFlowApprovalCheckpoint>,
    pub artifact_kinds: Vec<OpenFlowArtifactKind>,
    pub permission_model: Vec<OpenFlowPermission>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenFlowDesignSpec {
    pub product_definition: String,
    pub workspace_integration: String,
    pub task_graph_model: Vec<String>,
    pub shared_memory_model: Vec<String>,
    pub handoff_workflow_prompt: String,
    pub run_model: OpenFlowRunModel,
    pub implementation_notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenFlowTimelineEntry {
    pub entry_id: String,
    pub level: OpenFlowTimelineLevel,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenFlowWorkerState {
    pub role: OpenFlowRole,
    pub assigned_task_ids: Vec<String>,
    pub status: String,
    pub last_output: Option<String>,
    pub model: Option<String>,
    pub thinking_mode: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenFlowRetryPolicy {
    pub max_attempts: u32,
    pub current_attempt: u32,
    pub backoff_seconds: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenFlowRunRecord {
    pub run_id: String,
    pub title: String,
    pub goal: String,
    pub status: OpenFlowRunStatus,
    pub current_phase: String,
    pub replan_count: u32,
    pub assigned_roles: Vec<OpenFlowRole>,
    pub task_graph: Vec<OpenFlowTaskNode>,
    pub artifacts: Vec<OpenFlowArtifact>,
    pub approvals: Vec<OpenFlowApprovalCheckpoint>,
    pub timeline: Vec<OpenFlowTimelineEntry>,
    pub workers: Vec<OpenFlowWorkerState>,
    pub retry_policy: OpenFlowRetryPolicy,
    pub resumable: bool,
    pub verification_required: bool,
    pub browser_validation_required: bool,
    pub command_validation_required: bool,
    pub reviewer_score: Option<u8>,
    pub stop_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenFlowRuntimeSnapshot {
    pub active_runs: Vec<OpenFlowRunRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenFlowCreateRunRequest {
    pub title: String,
    pub goal: String,
    pub agent_roles: Vec<String>,
}

pub struct OpenFlowRuntimeStore {
    inner: Arc<Mutex<OpenFlowRuntimeSnapshot>>,
}

impl Default for OpenFlowRuntimeStore {
    fn default() -> Self {
        Self {
            inner: Arc::new(Mutex::new(OpenFlowRuntimeSnapshot {
                active_runs: vec![],
            })),
        }
    }
}

impl OpenFlowRuntimeStore {
    pub fn snapshot(&self) -> OpenFlowRuntimeSnapshot {
        self.inner.lock().unwrap().clone()
    }

    pub fn create_run(&self, request: OpenFlowCreateRunRequest) -> OpenFlowRunRecord {
        let mut snapshot = self.inner.lock().unwrap();
        let run_id = format!(
            "openflow-run-{}",
            uuid::Uuid::new_v4().to_string()[..8].to_uppercase()
        );
        let assigned_roles: Vec<OpenFlowRole> = {
            let mut seen = std::collections::HashSet::new();
            request
                .agent_roles
                .iter()
                .filter_map(|r| OpenFlowRole::from_str(r))
                // Orchestrator must appear exactly once; all other roles can repeat
                .filter(|role| {
                    if matches!(role, OpenFlowRole::Orchestrator) {
                        seen.insert("orchestrator")
                    } else {
                        true
                    }
                })
                .collect()
        };

        let workers = assigned_roles
            .iter()
            .map(|role| {
                let status = match role {
                    OpenFlowRole::Orchestrator => "active",
                    OpenFlowRole::Planner => "ready",
                    _ => "pending",
                };
                let last_output = match role {
                    OpenFlowRole::Orchestrator => {
                        Some("Run created and waiting on planner".to_string())
                    }
                    _ => None,
                };
                let task_id = match role {
                    OpenFlowRole::Planner => vec!["task-plan".to_string()],
                    OpenFlowRole::Builder => vec!["task-build".to_string()],
                    OpenFlowRole::Reviewer => vec!["task-review".to_string()],
                    _ => vec![],
                };

                OpenFlowWorkerState {
                    role: role.clone(),
                    assigned_task_ids: task_id,
                    status: status.to_string(),
                    last_output,
                    model: None,
                    thinking_mode: None,
                }
            })
            .collect();

        let run = OpenFlowRunRecord {
            run_id,
            title: request.title,
            goal: request.goal,
            status: OpenFlowRunStatus::Planning,
            current_phase: "plan".into(),
            replan_count: 0,
            assigned_roles,
            task_graph: vec![
                OpenFlowTaskNode {
                    task_id: "task-plan".into(),
                    title: "Plan the run".into(),
                    description: "Break the user goal into phases and success criteria".into(),
                    role: OpenFlowRole::Planner,
                    status: OpenFlowTaskStatus::Ready,
                    depends_on: vec![],
                    success_criteria: vec!["A multi-phase plan exists".into()],
                    produced_artifacts: vec!["artifact-plan".into()],
                },
                OpenFlowTaskNode {
                    task_id: "task-build".into(),
                    title: "Execute the implementation phase".into(),
                    description: "Make the requested changes in the workspace".into(),
                    role: OpenFlowRole::Builder,
                    status: OpenFlowTaskStatus::Pending,
                    depends_on: vec!["task-plan".into()],
                    success_criteria: vec!["Requested work is implemented".into()],
                    produced_artifacts: vec!["artifact-diff".into()],
                },
                OpenFlowTaskNode {
                    task_id: "task-review".into(),
                    title: "Review and verify outputs".into(),
                    description: "Validate results, tests, and browser evidence".into(),
                    role: OpenFlowRole::Reviewer,
                    status: OpenFlowTaskStatus::Pending,
                    depends_on: vec!["task-build".into()],
                    success_criteria: vec!["Review passes or issues are identified".into()],
                    produced_artifacts: vec!["artifact-review".into()],
                },
            ],
            artifacts: vec![
                OpenFlowArtifact {
                    artifact_id: "artifact-plan".into(),
                    kind: OpenFlowArtifactKind::Plan,
                    title: "Initial run plan".into(),
                    location: None,
                    summary: "Planner output placeholder".into(),
                },
                OpenFlowArtifact {
                    artifact_id: "artifact-diff".into(),
                    kind: OpenFlowArtifactKind::Diff,
                    title: "Implementation diff".into(),
                    location: None,
                    summary: "Builder output placeholder".into(),
                },
            ],
            approvals: default_openflow_spec().run_model.approval_checkpoints,
            timeline: vec![
                OpenFlowTimelineEntry {
                    entry_id: "timeline-1".into(),
                    level: OpenFlowTimelineLevel::Info,
                    message: "Run created".into(),
                },
                OpenFlowTimelineEntry {
                    entry_id: "timeline-2".into(),
                    level: OpenFlowTimelineLevel::Info,
                    message: "Entered planning phase".into(),
                },
            ],
            workers,
            retry_policy: OpenFlowRetryPolicy {
                max_attempts: 3,
                current_attempt: 1,
                backoff_seconds: 5,
            },
            resumable: true,
            verification_required: true,
            browser_validation_required: true,
            command_validation_required: true,
            reviewer_score: None,
            stop_reason: None,
        };
        snapshot.active_runs.push(run.clone());
        run
    }

    pub fn advance_run_phase(&self, run_id: &str) -> Result<OpenFlowRunRecord, String> {
        let mut snapshot = self.inner.lock().unwrap();
        let run = snapshot
            .active_runs
            .iter_mut()
            .find(|run| run.run_id == run_id)
            .ok_or_else(|| format!("No OpenFlow run found for {run_id}"))?;

        let next = match run.current_phase.as_str() {
            "plan" => {
                promote_task(&mut run.task_graph, "task-plan", OpenFlowTaskStatus::Passed);
                promote_task(&mut run.task_graph, "task-build", OpenFlowTaskStatus::Ready);
                update_worker_status(
                    &mut run.workers,
                    OpenFlowRole::Planner,
                    "done",
                    Some("Plan complete"),
                );
                update_worker_status(&mut run.workers, OpenFlowRole::Builder, "ready", None);
                (
                    "execute",
                    OpenFlowRunStatus::Executing,
                    "Advanced to execute phase",
                )
            }
            "execute" => {
                promote_task(
                    &mut run.task_graph,
                    "task-build",
                    OpenFlowTaskStatus::Passed,
                );
                promote_task(
                    &mut run.task_graph,
                    "task-review",
                    OpenFlowTaskStatus::Ready,
                );
                update_worker_status(
                    &mut run.workers,
                    OpenFlowRole::Builder,
                    "done",
                    Some("Execution complete"),
                );
                update_worker_status(&mut run.workers, OpenFlowRole::Reviewer, "ready", None);
                (
                    "verify",
                    OpenFlowRunStatus::Verifying,
                    "Advanced to verify phase",
                )
            }
            "verify" => (
                "review",
                OpenFlowRunStatus::Reviewing,
                "Advanced to review phase after verification",
            ),
            "review" => {
                promote_task(
                    &mut run.task_graph,
                    "task-review",
                    OpenFlowTaskStatus::Passed,
                );
                update_worker_status(
                    &mut run.workers,
                    OpenFlowRole::Reviewer,
                    "done",
                    Some("Review complete"),
                );
                run.reviewer_score = Some(92);
                (
                    "complete",
                    OpenFlowRunStatus::Completed,
                    "Run marked completed",
                )
            }
            _ => (
                "complete",
                OpenFlowRunStatus::Completed,
                "Run remained completed",
            ),
        };

        run.current_phase = next.0.into();
        run.status = next.1;
        if run.current_phase == "review" {
            run.verification_required = false;
            run.browser_validation_required = false;
            run.command_validation_required = false;
        }
        run.timeline.push(OpenFlowTimelineEntry {
            entry_id: format!("timeline-{}", run.timeline.len() + 1),
            level: OpenFlowTimelineLevel::Info,
            message: next.2.into(),
        });
        Ok(run.clone())
    }

    pub fn retry_run(&self, run_id: &str) -> Result<OpenFlowRunRecord, String> {
        let mut snapshot = self.inner.lock().unwrap();
        let run = snapshot
            .active_runs
            .iter_mut()
            .find(|run| run.run_id == run_id)
            .ok_or_else(|| format!("No OpenFlow run found for {run_id}"))?;

        if run.retry_policy.current_attempt >= run.retry_policy.max_attempts {
            return Err("Retry limit reached for OpenFlow run".into());
        }

        run.retry_policy.current_attempt += 1;
        run.status = OpenFlowRunStatus::Planning;
        run.current_phase = "plan".into();
        run.replan_count += 1;
        run.verification_required = true;
        run.browser_validation_required = true;
        run.command_validation_required = true;
        run.reviewer_score = None;
        run.timeline.push(OpenFlowTimelineEntry {
            entry_id: format!("timeline-{}", run.timeline.len() + 1),
            level: OpenFlowTimelineLevel::Warning,
            message: format!(
                "Retrying run; attempt {} of {}",
                run.retry_policy.current_attempt, run.retry_policy.max_attempts
            ),
        });
        Ok(run.clone())
    }

    pub fn run_autonomous_loop(&self, run_id: &str) -> Result<OpenFlowRunRecord, String> {
        loop {
            let snapshot = self.snapshot();
            let run = snapshot
                .active_runs
                .iter()
                .find(|run| run.run_id == run_id)
                .cloned()
                .ok_or_else(|| format!("No OpenFlow run found for {run_id}"))?;

            if matches!(
                run.status,
                OpenFlowRunStatus::Completed
                    | OpenFlowRunStatus::Failed
                    | OpenFlowRunStatus::Cancelled
                    | OpenFlowRunStatus::AwaitingApproval
            ) {
                return Ok(run);
            }

            let advanced = self.advance_run_phase(run_id)?;

            if advanced.current_phase == "review" {
                let reviewed = self.apply_review_result(run_id, 92, true, None)?;
                if matches!(reviewed.status, OpenFlowRunStatus::Completed) {
                    return Ok(reviewed);
                }
            }
        }
    }

    pub fn apply_review_result(
        &self,
        run_id: &str,
        reviewer_score: u8,
        accepted: bool,
        issue: Option<String>,
    ) -> Result<OpenFlowRunRecord, String> {
        let mut snapshot = self.inner.lock().unwrap();
        let run = snapshot
            .active_runs
            .iter_mut()
            .find(|run| run.run_id == run_id)
            .ok_or_else(|| format!("No OpenFlow run found for {run_id}"))?;

        run.reviewer_score = Some(reviewer_score);
        if accepted {
            run.status = OpenFlowRunStatus::Completed;
            run.current_phase = "complete".into();
            run.stop_reason = Some("Reviewer accepted the run output".into());
            run.timeline.push(OpenFlowTimelineEntry {
                entry_id: format!("timeline-{}", run.timeline.len() + 1),
                level: OpenFlowTimelineLevel::Info,
                message: format!("Reviewer accepted run with score {reviewer_score}"),
            });
        } else {
            run.status = OpenFlowRunStatus::Planning;
            run.current_phase = "plan".into();
            run.replan_count += 1;
            run.timeline.push(OpenFlowTimelineEntry {
                entry_id: format!("timeline-{}", run.timeline.len() + 1),
                level: OpenFlowTimelineLevel::Warning,
                message: issue.unwrap_or_else(|| {
                    format!("Reviewer requested fixes with score {reviewer_score}")
                }),
            });
        }

        Ok(run.clone())
    }

    pub fn stop_run(
        &self,
        run_id: &str,
        status: OpenFlowRunStatus,
        reason: String,
    ) -> Result<OpenFlowRunRecord, String> {
        let mut snapshot = self.inner.lock().unwrap();
        let run = snapshot
            .active_runs
            .iter_mut()
            .find(|run| run.run_id == run_id)
            .ok_or_else(|| format!("No OpenFlow run found for {run_id}"))?;

        run.status = status;
        run.stop_reason = Some(reason.clone());
        run.timeline.push(OpenFlowTimelineEntry {
            entry_id: format!("timeline-{}", run.timeline.len() + 1),
            level: OpenFlowTimelineLevel::Warning,
            message: reason,
        });
        Ok(run.clone())
    }
}

pub fn default_openflow_spec() -> OpenFlowDesignSpec {
    let worker_contracts = vec![
        OpenFlowWorkerContract {
            role: OpenFlowRole::Orchestrator,
            responsibilities: vec![
                "Own the overall run state".into(),
                "Assign tasks to specialized workers".into(),
                "Decide when to replan or request approval".into(),
            ],
            inputs: vec![
                "User goal".into(),
                "Project memory".into(),
                "Index search".into(),
            ],
            outputs: vec![
                "Phase plan".into(),
                "Task assignments".into(),
                "Run decisions".into(),
            ],
            required_permissions: vec![
                OpenFlowPermission::ReadRepo,
                OpenFlowPermission::RunCommands,
            ],
        },
        OpenFlowWorkerContract {
            role: OpenFlowRole::Planner,
            responsibilities: vec![
                "Break goals into phases".into(),
                "Define success criteria".into(),
                "Propose checkpoints and review gates".into(),
            ],
            inputs: vec![
                "User goal".into(),
                "Project memory".into(),
                "Index search".into(),
            ],
            outputs: vec!["Task graph".into(), "Success criteria".into()],
            required_permissions: vec![OpenFlowPermission::ReadRepo],
        },
        OpenFlowWorkerContract {
            role: OpenFlowRole::Builder,
            responsibilities: vec![
                "Implement code changes".into(),
                "Run commands for build/test cycles".into(),
            ],
            inputs: vec!["Assigned tasks".into(), "Code context".into()],
            outputs: vec!["Diffs".into(), "Build results".into()],
            required_permissions: vec![
                OpenFlowPermission::ReadRepo,
                OpenFlowPermission::WriteRepo,
                OpenFlowPermission::RunCommands,
            ],
        },
        OpenFlowWorkerContract {
            role: OpenFlowRole::Reviewer,
            responsibilities: vec![
                "Review outputs and diffs".into(),
                "Request fixes when quality is insufficient".into(),
            ],
            inputs: vec!["Diffs".into(), "Test results".into(), "Artifacts".into()],
            outputs: vec!["Review notes".into(), "Approval recommendation".into()],
            required_permissions: vec![OpenFlowPermission::ReadRepo],
        },
        OpenFlowWorkerContract {
            role: OpenFlowRole::Tester,
            responsibilities: vec![
                "Run builds, tests, lints".into(),
                "Use browser evidence where relevant".into(),
            ],
            inputs: vec!["Task graph".into(), "Browser state".into()],
            outputs: vec!["Test artifacts".into(), "Browser evidence".into()],
            required_permissions: vec![
                OpenFlowPermission::RunCommands,
                OpenFlowPermission::BrowserControl,
            ],
        },
        OpenFlowWorkerContract {
            role: OpenFlowRole::Debugger,
            responsibilities: vec![
                "Investigate failures".into(),
                "Propose fixes and recovery paths".into(),
            ],
            inputs: vec!["Logs".into(), "Errors".into(), "Artifacts".into()],
            outputs: vec!["Debug notes".into(), "Fix strategy".into()],
            required_permissions: vec![
                OpenFlowPermission::ReadRepo,
                OpenFlowPermission::RunCommands,
            ],
        },
        OpenFlowWorkerContract {
            role: OpenFlowRole::Researcher,
            responsibilities: vec![
                "Gather external or internal project context".into(),
                "Support planner and builder with targeted knowledge".into(),
            ],
            inputs: vec!["Project memory".into(), "Index search".into()],
            outputs: vec!["Research summaries".into()],
            required_permissions: vec![
                OpenFlowPermission::ReadRepo,
                OpenFlowPermission::NetworkAccess,
            ],
        },
    ];

    OpenFlowDesignSpec {
        product_definition: "OpenFlow is an embeddable orchestration engine that runs inside Codemux as a first-class workspace/run type. It coordinates specialized agents through a phased execution loop while preserving direct user access to the underlying terminal and browser panes.".into(),
        workspace_integration: "Each OpenFlow run owns a visual Flow View and an underlying Workspace View. The Flow View shows run status, role communication, blockers, approvals, and artifacts. The Workspace View exposes the real terminal/browser panes created for the run.".into(),
        task_graph_model: vec![
            "Directed acyclic task graph with task dependencies".into(),
            "Each task is owned by a worker role".into(),
            "Tasks define success criteria and produced artifacts".into(),
            "Graph can be expanded during replanning".into(),
        ],
        shared_memory_model: vec![
            "Use project memory as the shared context baseline".into(),
            "Use code index as retrieval layer for repo facts".into(),
            "Store run-local summaries, decisions, and artifacts separately from long-term project memory".into(),
        ],
        handoff_workflow_prompt: "When continuing an OpenFlow-related session, first read WORKFLOW.md, PROJECT.md, PLAN.md, and any generated project handoff. Use project memory and code index retrieval instead of replaying long raw chats. Update PLAN.md checkboxes as phases complete and refresh README/docs when user-facing workflows change.".into(),
        run_model: OpenFlowRunModel {
            run_type: "workspace_embedded_orchestration_engine".into(),
            status_model: vec![
                OpenFlowRunStatus::Draft,
                OpenFlowRunStatus::Planning,
                OpenFlowRunStatus::Executing,
                OpenFlowRunStatus::Verifying,
                OpenFlowRunStatus::Reviewing,
                OpenFlowRunStatus::AwaitingApproval,
                OpenFlowRunStatus::Completed,
                OpenFlowRunStatus::Failed,
                OpenFlowRunStatus::Cancelled,
            ],
            worker_contracts,
            phase_loop: vec![
                "plan".into(),
                "execute".into(),
                "verify".into(),
                "review".into(),
                "replan_if_needed".into(),
            ],
            stop_policy: OpenFlowStopPolicy {
                max_phases: 24,
                max_replans: 12,
                max_runtime_minutes: 240,
                require_tests_before_completion: true,
                require_review_before_completion: true,
            },
            approval_checkpoints: vec![
                OpenFlowApprovalCheckpoint {
                    checkpoint_id: "approval-run-start".into(),
                    kind: OpenFlowApprovalKind::RunStart,
                    title: "Run start approval".into(),
                    required: true,
                    reason: "User confirms the top-level goal and permission envelope".into(),
                },
                OpenFlowApprovalCheckpoint {
                    checkpoint_id: "approval-major-change".into(),
                    kind: OpenFlowApprovalKind::MajorChange,
                    title: "Major change checkpoint".into(),
                    required: true,
                    reason: "Large refactors or risky repo-wide changes require confirmation".into(),
                },
                OpenFlowApprovalCheckpoint {
                    checkpoint_id: "approval-final-apply".into(),
                    kind: OpenFlowApprovalKind::FinalApply,
                    title: "Final completion checkpoint".into(),
                    required: true,
                    reason: "Final output should be approved before declaring the run complete".into(),
                },
            ],
            artifact_kinds: vec![
                OpenFlowArtifactKind::Plan,
                OpenFlowArtifactKind::Log,
                OpenFlowArtifactKind::Screenshot,
                OpenFlowArtifactKind::Diff,
                OpenFlowArtifactKind::ReviewNote,
                OpenFlowArtifactKind::TestResult,
                OpenFlowArtifactKind::BrowserEvidence,
            ],
            permission_model: vec![
                OpenFlowPermission::ReadRepo,
                OpenFlowPermission::WriteRepo,
                OpenFlowPermission::RunCommands,
                OpenFlowPermission::BrowserControl,
                OpenFlowPermission::NetworkAccess,
                OpenFlowPermission::DestructiveAction,
            ],
        },
        implementation_notes: vec![
            "Keep the OpenFlow core runtime separate from Codemux UI integration".into(),
            "Make run state resumable and observable from the beginning".into(),
            "Use structured artifacts instead of raw transcript replay".into(),
            "Require bounded permissions and explicit approval gates".into(),
        ],
    }
}

fn promote_task(tasks: &mut [OpenFlowTaskNode], task_id: &str, status: OpenFlowTaskStatus) {
    if let Some(task) = tasks.iter_mut().find(|task| task.task_id == task_id) {
        task.status = status;
    }
}

fn update_worker_status(
    workers: &mut [OpenFlowWorkerState],
    role: OpenFlowRole,
    status: &str,
    last_output: Option<&str>,
) {
    if let Some(worker) = workers.iter_mut().find(|worker| worker.role == role) {
        worker.status = status.into();
        worker.last_output = last_output.map(str::to_string);
    }
}

// ─── Agent session tracking (Phase 2) ────────────────────────────────────────

/// Stores the per-agent-session state for all active OpenFlow runs.
/// Keyed by terminal session ID.
#[derive(Default)]
pub struct AgentSessionStore {
    inner: Arc<Mutex<HashMap<String, AgentSessionState>>>,
}

impl AgentSessionStore {
    pub fn insert(&self, session_id: String, state: AgentSessionState) {
        self.inner.lock().unwrap().insert(session_id, state);
    }

    pub fn update_status(&self, session_id: &str, status: AgentSessionStatus) {
        if let Some(entry) = self.inner.lock().unwrap().get_mut(session_id) {
            entry.status = status;
        }
    }

    pub fn get(&self, session_id: &str) -> Option<AgentSessionState> {
        self.inner.lock().unwrap().get(session_id).cloned()
    }

    pub fn for_run(&self, run_id: &str) -> Vec<AgentSessionState> {
        self.inner
            .lock()
            .unwrap()
            .values()
            .filter(|s| s.run_id == run_id)
            .cloned()
            .collect()
    }

    pub fn all(&self) -> Vec<AgentSessionState> {
        self.inner.lock().unwrap().values().cloned().collect()
    }
}
