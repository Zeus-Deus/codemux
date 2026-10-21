//! Durable identity and attribution for application-owned input.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CreatedBy {
    #[default]
    User,
    Agent,
    System,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CreationSource {
    #[default]
    Unknown,
    Desktop,
    Web,
    Phone,
    Mcp,
    Provider,
    Codemux,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputIntent {
    #[default]
    TurnStart,
    QueuedTurn,
    Steer,
    PromotedQueuedToSteer,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationKind {
    DelegatedTask,
    RemoteTask,
    Subagent,
    Automation,
    System,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationOutcome {
    Completed,
    Failed,
    Cancelled,
    Updated,
    Unknown,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationSource {
    pub kind: NotificationKind,
    #[serde(default)]
    pub task_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child_thread_id: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NotificationTask {
    pub provider: String,
    pub title: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageNotification {
    pub source: NotificationSource,
    pub outcome: NotificationOutcome,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tasks: Vec<NotificationTask>,
}

/// Missing legacy metadata means human input from an unknown source. Text is
/// never evidence of who sent a message.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct MessageProvenance {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<TurnOrigin>,
    pub created_by: CreatedBy,
    pub creation_source: CreationSource,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sender_thread_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sender_title: Option<String>,
    pub input_intent: InputIntent,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notification: Option<MessageNotification>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub automation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub automation_run_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delivery_id: Option<String>,
}

impl MessageProvenance {
    /// Provider admission waited for an occupied turn slot. A transport
    /// queue alone does not imply that the input waited behind another turn.
    pub(crate) fn queued_turn() -> Self {
        Self { input_intent: InputIntent::QueuedTurn, ..Self::default() }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnOrigin {
    /// Authenticated outside input; never synthetic human activity.
    Controller,
    /// The user typed and sent it. Counts as user activity: any pending
    /// usage-limit resume on the thread is forgotten.
    User,
    /// A usage-limit resume (scheduled or the manual "Resume now"). Must
    /// NOT count as user activity, or the scheduler's own dispatch would
    /// reset the attempt cap it is bounded by.
    UsageResume,
    /// Cross-provider delegation: a delegated child's task, or a parent's
    /// round of results. Not user activity either — it must not start a new
    /// delegation round or forget a pending usage resume.
    Delegation,
    /// App-owned, correlated remote result; idle-only and non-interrupting.
    RemoteDelegation,
    /// A watched pull request changed (`crate::pr_watch`). Not user
    /// activity: it must not start a delegation round or forget a pending
    /// usage resume, and it always queues behind a busy turn.
    PrWatch,
    /// The resume of a turn the app stopped mid-flight
    /// (`commands::restart_resume`). Not user activity, so it never resets
    /// the cap on resumes in a row. The caller holds the activity lock.
    RestartResume,
}
