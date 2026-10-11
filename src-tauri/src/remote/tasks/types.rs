use crate::agent_provider::{ApprovalDecision, ProviderKind};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LaunchRequest {
    pub id: String,
    pub parent_thread_id: String,
    pub parent_label: String,
    pub workspace_path: String,
    pub prompt: String,
    pub provider: ProviderKind,
    pub model: Option<String>,
    pub permission_mode: String,
    pub effort: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Starting,
    Running,
    AwaitingApproval,
    Stopping,
    Completed,
    Failed,
    Cancelled,
    Interrupted,
}
impl TaskStatus {
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Cancelled | Self::Interrupted
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RemoteApproval {
    pub request_id: String,
    pub request_kind: String,
    pub payload: Value,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskSnapshot {
    pub id: String,
    pub request: LaunchRequest,
    pub child_thread_id: String,
    pub provider_session_id: Option<String>,
    pub turn_id: Option<String>,
    pub status: TaskStatus,
    pub activity: Option<String>,
    pub result: Option<String>,
    pub error: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub cancel_requested: bool,
    pub pending_requests: Vec<RemoteApproval>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CancelFence {
    pub id: String,
    pub absent_and_fenced: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CancelReceipt {
    Task(TaskRead),
    Absent(CancelFence),
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskEvent {
    pub sequence: i64,
    pub event: Value,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskRead {
    #[serde(default)]
    pub approval_receipts: Vec<ApprovalReceipt>,
    pub task: TaskSnapshot,
    pub events: Vec<TaskEvent>,
    pub next_cursor: i64,
    pub has_more: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalReceipt {
    #[serde(default)]
    pub original_request: Option<RemoteApproval>,
    pub request_id: String,
    pub decision: ApprovalDecision,
    pub state: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RespondRequest {
    /// Explicit original consent, never reconstructed from a later pending request.
    #[serde(default)]
    pub original_request: Option<RemoteApproval>,
    pub request_id: String,
    pub decision: ApprovalDecision,
}
