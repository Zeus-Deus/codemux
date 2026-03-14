/// Agent configuration for an OpenFlow run.
///
/// This is the data contract passed from the frontend AgentConfigPanel when
/// the user starts an orchestration run. It captures exactly what CLI tool,
/// model, provider, thinking mode, and role each agent slot should use.
use serde::{Deserialize, Serialize};

use super::OpenFlowRole;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    /// Logical index within the run (0-based).
    pub agent_index: usize,
    /// CLI tool to use, e.g. `"opencode"`, `"claude"`, `"aider"`.
    pub cli_tool: String,
    /// Full model ID as returned by `list_models_for_tool`, e.g.
    /// `"github-copilot/claude-sonnet-4.6"`.
    pub model: String,
    /// Provider portion of the model ID, e.g. `"github-copilot"`.
    /// May be empty if not applicable.
    pub provider: String,
    /// Thinking mode for models that support it (opencode). Empty string means
    /// the adapter should not set it.
    pub thinking_mode: String,
    /// Agent role in the OpenFlow run.
    pub role: OpenFlowRole,
}

impl AgentConfig {
    /// Returns a short label suitable for terminal titles and log messages.
    pub fn label(&self) -> String {
        format!("{:?}#{}", self.role, self.agent_index)
    }
}

/// State tracked per spawned agent terminal session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSessionState {
    /// The terminal session ID for this agent.
    pub session_id: String,
    /// The OpenFlow run this agent belongs to.
    pub run_id: String,
    /// The config used to spawn this agent.
    pub config: AgentConfig,
    /// Current lifecycle state.
    pub status: AgentSessionStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AgentSessionStatus {
    Spawning,
    Running,
    Done,
    Failed,
}
