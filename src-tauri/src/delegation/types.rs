use crate::agent_provider::ProviderKind;
use crate::remote::tasks::{LaunchRequest, TaskEvent, TaskSnapshot, TaskStatus};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Grant {
    pub id: String,
    pub host_id: i64,
    pub host_name: String,
    pub ssh_target: String,
    pub workspace_path: String,
    pub workspace_name: String,
    pub provider: ProviderKind,
    pub permission_mode: String,
    pub enabled: bool,
    pub created_at: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthorizeDelegationInput {
    pub host_id: i64,
    pub workspace_path: String,
    pub provider: ProviderKind,
    pub permission_mode: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct DelegateTaskInput {
    pub target_id: String,
    pub prompt: String,
    pub title: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub client_request_id: String,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WakeState {
    Pending,
    Delivering,
    Delivered,
    Held,
    Suppressed,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalTask {
    pub id: String,
    pub parent_thread_id: String,
    pub parent_provider: ProviderKind,
    pub parent_workspace_id: String,
    pub parent_event_id: Option<i64>,
    pub target_host_id: i64,
    pub target_host_name: String,
    pub target_ssh_target: String,
    pub target_workspace_id: String,
    pub target_workspace_path: String,
    pub title: String,
    pub prompt: String,
    pub provider: ProviderKind,
    pub model: Option<String>,
    pub permission_mode: String,
    pub created_at: String,
    pub updated_at: String,
    pub status: TaskStatus,
    pub connection_error: Option<String>,
    pub cancel_requested: bool,
    pub remote: Option<TaskSnapshot>,
    pub wake_state: WakeState,
    pub wake_error: Option<String>,
    /// Read-time authority projection; never trusted for dispatch admission.
    #[serde(default)]
    pub delivery: DeliveryProjection,
    /// Only current pending requests in this owned task; projected from the
    /// durable ledger, never renderer-supplied dispatch authority.
    #[serde(default)]
    pub responses: Vec<ResponseProjection>,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all="snake_case")]
pub enum DeliveryOutcome { NotAttempted, BeforeSend, InFlight, Accepted, #[default] Unknown }
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all="snake_case")]
pub enum DeliveryReason { Cancelled, NoResult, NotTerminal, InFlight, Accepted, Unknown, AlreadyDelivered, TailPending, AuthorityUnavailable }
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeliveryProjection { pub outcome:DeliveryOutcome, pub eligible:bool, pub reason:Option<DeliveryReason> }
impl Default for DeliveryProjection {
    fn default()->Self { Self {outcome:DeliveryOutcome::Unknown,eligible:false,reason:Some(DeliveryReason::AuthorityUnavailable)} }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseProjection {
    pub request_id: String,
    pub outcome: DeliveryOutcome,
    pub decision: Option<crate::agent_provider::ApprovalDecision>,
    pub eligible: bool,
    pub reason: Option<ResponseReason>,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all="snake_case")]
pub enum ResponseReason { Cancelled, InFlight, Accepted, Unknown, TailPending, AuthorityUnavailable, PayloadChanged }
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskRead {
    pub task: LocalTask,
    pub events: Vec<TaskEvent>,
    pub next_cursor: i64,
    pub has_more: bool,
}
#[derive(Debug, Clone)]
pub struct Parent {
    pub thread_id: String,
    pub provider: ProviderKind,
    pub workspace_id: String,
    pub label: String,
    pub permission_mode: String,
    pub event_id: Option<i64>,
}
impl LocalTask {
    /// Authority is the admitted tuple, not mutable display names or grant id.
    pub(crate) fn validate_target(&self, grant:&Grant)->Result<(),String> {
        if grant.host_id!=self.target_host_id || grant.ssh_target!=self.target_ssh_target || grant.workspace_path!=self.target_workspace_path || grant.provider!=self.provider || grant.permission_mode!=self.permission_mode {return Err("Original remote target authority changed".into());}
        Ok(())
    }
    pub fn new(
        id: String,
        parent: &Parent,
        grant: &Grant,
        workspace_id: String,
        input: &DelegateTaskInput,
    ) -> Self {
        let now = chrono::Utc::now().to_rfc3339();
        Self {
            id,
            parent_thread_id: parent.thread_id.clone(),
            parent_provider: parent.provider,
            parent_workspace_id: parent.workspace_id.clone(),
            parent_event_id: parent.event_id,
            target_host_id: grant.host_id,
            target_host_name: grant.host_name.clone(),
            target_ssh_target: grant.ssh_target.clone(),
            target_workspace_id: workspace_id,
            target_workspace_path: grant.workspace_path.clone(),
            title: input
                .title
                .clone()
                .filter(|v| !v.trim().is_empty())
                .unwrap_or_else(|| {
                    input
                        .prompt
                        .lines()
                        .next()
                        .unwrap_or("Remote task")
                        .chars()
                        .take(120)
                        .collect()
                }),
            prompt: input.prompt.clone(),
            provider: grant.provider,
            model: input.model.clone(),
            permission_mode: grant.permission_mode.clone(),
            created_at: now.clone(),
            updated_at: now,
            status: TaskStatus::Starting,
            connection_error: None,
            cancel_requested: false,
            remote: None,
            wake_state: WakeState::Pending,
            wake_error: None,
            delivery: DeliveryProjection::default(),
            responses: Vec::new(),
        }
    }
    pub fn request(&self, effort: Option<String>) -> LaunchRequest {
        LaunchRequest {
            id: self.id.clone(),
            parent_thread_id: self.parent_thread_id.clone(),
            parent_label: self.title.clone(),
            workspace_path: self.target_workspace_path.clone(),
            prompt: self.prompt.clone(),
            provider: self.provider,
            model: self.model.clone(),
            permission_mode: self.permission_mode.clone(),
            effort,
        }
    }
}
