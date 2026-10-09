use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum RunMode {
    #[default]
    DryRun,
    Live,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum TaskAccess {
    #[default]
    ReadOnly,
    Write,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RouteSpec {
    pub id: String,
    pub provider: String,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub effort: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskSpec {
    pub id: String,
    pub title: String,
    pub prompt: String,
    #[serde(default)]
    pub dependencies: Vec<String>,
    #[serde(default)]
    pub route_id: Option<String>,
    #[serde(default)]
    pub access: TaskAccess,
    #[serde(default)]
    pub scope: Vec<String>,
    #[serde(default)]
    pub output_schema: Option<Value>,
    #[serde(default = "required_default")]
    pub required: bool,
}

fn required_default() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WorkflowLimits {
    pub concurrency: usize,
    pub max_tasks: usize,
    pub max_attempts: usize,
    pub max_depth: usize,
    pub token_budget: Option<u64>,
    pub max_output_bytes: usize,
    pub wall_time_ms: u64,
}

impl Default for WorkflowLimits {
    fn default() -> Self {
        Self {
            concurrency: 4,
            max_tasks: 500,
            max_attempts: 1000,
            max_depth: 3,
            token_budget: None,
            max_output_bytes: 1_048_576,
            wall_time_ms: 3_600_000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunSpec {
    pub workspace_id: String,
    pub title: String,
    pub goal: String,
    #[serde(default)]
    pub mode: RunMode,
    #[serde(default)]
    pub allow_writes: bool,
    pub routes: Vec<RouteSpec>,
    #[serde(default)]
    pub limits: WorkflowLimits,
    #[serde(default)]
    pub tasks: Vec<TaskSpec>,
    #[serde(default)]
    pub script: Option<ScriptSpec>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScriptSpec {
    pub source: String,
    #[serde(default)]
    pub args: Value,
    pub api_version: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScriptStatus {
    Pending,
    Running,
    Paused,
    Completed,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScriptSnapshot {
    pub status: ScriptStatus,
    pub result: Option<Value>,
    pub error: Option<String>,
    pub phase: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunStatus {
    Running,
    Paused,
    Completed,
    Failed,
    Cancelled,
    Stopping,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Queued,
    Running,
    Waiting,
    Succeeded,
    Failed,
    Cancelled,
    Blocked,
    Unknown,
    Stopping,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttemptStatus {
    Dispatching,
    Running,
    Succeeded,
    Failed,
    Cancelled,
    Waiting,
    Unknown,
    Stopping,
}

impl AttemptStatus {
    pub fn holds_capacity(self) -> bool {
        matches!(
            self,
            Self::Dispatching | Self::Running | Self::Unknown | Self::Stopping
        )
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
    pub reserved_tokens: u64,
    pub estimated_tokens: u64,
    pub tokens_unknown: bool,
    pub cost_usd: Option<f64>,
    pub cost_unknown: bool,
}
impl Default for Usage {
    fn default() -> Self {
        Self {
            input_tokens: 0,
            output_tokens: 0,
            total_tokens: 0,
            reserved_tokens: 0,
            estimated_tokens: 0,
            tokens_unknown: true,
            cost_usd: None,
            cost_unknown: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AttemptSnapshot {
    pub id: String,
    pub generation: u64,
    pub operation_id: String,
    pub status: AttemptStatus,
    pub route_id: String,
    pub started_at_ms: i64,
    pub finished_at_ms: Option<i64>,
    pub cancel_requested: bool,
    pub reserved_tokens: u64,
    pub external_ref: Option<Value>,
    pub output: Option<Value>,
    pub error: Option<String>,
    pub usage: Usage,
    pub artifacts: Vec<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskSnapshot {
    pub spec: TaskSpec,
    #[serde(default)]
    pub retired: bool,
    pub generation: u64,
    pub status: TaskStatus,
    pub depth: usize,
    pub parent_task_id: Option<String>,
    pub current_attempt: Option<AttemptSnapshot>,
    pub attempts: Vec<AttemptSnapshot>,
    pub result: Option<Value>,
    pub error: Option<String>,
    pub waiting_for: Vec<String>,
    /// Successful child results consumed by a coordinator, retained after a
    /// yield or completion so replacement revokes downstream acceptance.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub accepted_dependencies: std::collections::BTreeMap<String, u64>,
    pub messages: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunSnapshot {
    pub id: String,
    pub status: RunStatus,
    pub revision: u64,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub spec: RunSpec,
    pub tasks: Vec<TaskSnapshot>,
    pub usage: Usage,
    pub resolved_limits: WorkflowLimits,
    #[serde(default)]
    pub pause_requested: bool,
    #[serde(default)]
    pub script: Option<ScriptSnapshot>,
    #[serde(default)]
    pub cancel_requested: bool,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunSummarySpec {
    pub workspace_id: String,
    pub title: String,
    pub mode: RunMode,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RunSummary {
    pub id: String,
    pub status: RunStatus,
    pub revision: u64,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub spec: RunSummarySpec,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Dispatch {
    pub run_id: String,
    pub task_id: String,
    pub generation: u64,
    pub attempt_id: String,
    pub operation_id: String,
    pub task: TaskSpec,
    pub route: RouteSpec,
    pub mode: RunMode,
    pub workspace_id: String,
    pub goal: String,
    pub messages: Vec<String>,
    pub dependency_results: Vec<(String, Value)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionDisposition {
    Succeeded,
    Failed,
    Cancelled,
    Unknown,
    Waiting,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionReport {
    pub disposition: ExecutionDisposition,
    pub output: Option<Value>,
    pub error: Option<String>,
    #[serde(default)]
    pub usage: Usage,
    #[serde(default)]
    pub artifacts: Vec<Value>,
    #[serde(default)]
    pub waiting_for: Vec<String>,
}
impl ExecutionReport {
    pub fn success(output: Value) -> Self {
        Self {
            disposition: ExecutionDisposition::Succeeded,
            output: Some(output),
            error: None,
            usage: Usage::default(),
            artifacts: vec![],
            waiting_for: vec![],
        }
    }
    pub fn failed(error: impl Into<String>) -> Self {
        Self {
            disposition: ExecutionDisposition::Failed,
            output: None,
            error: Some(error.into()),
            usage: Usage::default(),
            artifacts: vec![],
            waiting_for: vec![],
        }
    }
    pub fn cancelled() -> Self {
        Self {
            disposition: ExecutionDisposition::Cancelled,
            output: None,
            error: None,
            usage: Usage::default(),
            artifacts: vec![],
            waiting_for: vec![],
        }
    }
    pub fn unknown(error: impl Into<String>) -> Self {
        Self {
            disposition: ExecutionDisposition::Unknown,
            output: None,
            error: Some(error.into()),
            usage: Usage::default(),
            artifacts: vec![],
            waiting_for: vec![],
        }
    }
    pub fn waiting(waiting_for: Vec<String>) -> Self {
        Self {
            disposition: ExecutionDisposition::Waiting,
            output: None,
            error: None,
            usage: Usage::default(),
            artifacts: vec![],
            waiting_for,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowEvent {
    pub sequence: i64,
    pub run_id: String,
    pub revision: u64,
    pub kind: String,
    pub timestamp_ms: i64,
}
