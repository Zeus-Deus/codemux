//! Portable wire contract; receiver execution remains in the Unix-only remote module.
use crate::agent_provider::{ApprovalDecision, ProviderChatCapabilities, ProviderKind};
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderAvailability {
    pub provider: ProviderKind,
    pub capabilities: Option<ProviderChatCapabilities>,
    pub error: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskCapabilities {
    pub protocol_version: u32,
    pub providers: Vec<ProviderAvailability>,
    pub workspaces: Vec<Workspace>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workspace {
    /// UUID v4. Globally unique across hosts so the desktop can
    /// import without collisions.
    pub id: String,
    /// Human-readable name. Defaults to the basename of `path` when
    /// the caller doesn't supply one.
    pub name: String,
    /// Absolute path to the worktree on this host.
    pub path: String,
    /// Git branch checked out in the worktree. May be `None` for
    /// non-git workspaces.
    pub branch: Option<String>,
    /// Originating project root if this workspace was created from
    /// an existing repo (so we know where it was cloned/worktreed
    /// from). May be `None` for blank workspaces.
    pub project_root: Option<String>,
    /// Stable, deterministic project identity — `UUIDv5(canonical
    /// remote ?? project_root)`. Shared by a project's main checkout
    /// and all its worktrees, and identical on every host/device that
    /// has the same repo, so the desktop can group siblings without
    /// coordination. See `crate::project_identity`.
    #[serde(default)]
    pub project_uid: Option<String>,
    /// Human-readable project name (basename of `project_root`).
    #[serde(default)]
    pub project_name: Option<String>,
    /// `"main"` for the repo root checkout, `"worktree"` for a
    /// per-branch git worktree. Lets the desktop label and group
    /// without inferring from path shape.
    #[serde(default)]
    pub kind: Option<String>,
    /// Canonical git remote (`canonical_remote(...)`) when the repo has
    /// one. Null for local-only repos. Lets other devices converge on
    /// the same `project_uid` for an independently-cloned copy.
    #[serde(default)]
    pub repo_remote: Option<String>,
    /// The repo's default branch (`origin/HEAD` → main/master → current),
    /// resolved at registration via `crate::git::resolve_default_branch`.
    /// Lets a pulling device land the repo ROOT on the right branch and
    /// classify it as a protected root even when `branch` (the per-checkout
    /// branch) is null — which it routinely is for a root an agent never
    /// named. Advisory: the desktop re-resolves locally before protecting.
    #[serde(default)]
    pub default_branch: Option<String>,
    /// RFC 3339 UTC timestamp.
    pub created_at: String,
    /// RFC 3339 UTC timestamp.
    pub updated_at: String,
    /// Always `None` in v1 — reserved for a future cloud relay
    /// integration to record the authenticated user that created
    /// the workspace. Nullable column today so populating it later
    /// is not a destructive migration.
    pub owner_id: Option<String>,
    /// Hostname-derived id of the host that created this workspace.
    /// Recorded so an imported workspace can show "from <host>" in
    /// the desktop UI.
    pub origin_host_id: String,
    /// Free-form notes. Reserved for the desktop to attach context
    /// when it pulls — empty in v1.
    pub notes: Option<String>,
}

/// Bounded retrieval of the complete JSON behind a remote_event_blob reference.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventBlobChunk {
    pub task_id: String,
    pub sequence: i64,
    pub offset: usize,
    pub next_offset: usize,
    pub bytes: usize,
    pub has_more: bool,
    pub data: String,
}
