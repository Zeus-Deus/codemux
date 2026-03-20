/// Agent adapter trait.
///
/// An adapter knows how to produce a spawn-ready command and environment for a
/// given `AgentConfig`. It does NOT actually create the terminal session — that
/// is Codemux integration work done by the spawner command in `commands.rs`.
use crate::openflow::agent::AgentConfig;

/// Description of how to spawn a CLI agent process.
pub struct AgentSpawnSpec {
    /// The executable and its arguments (first element is the binary path or name).
    pub argv: Vec<String>,
    /// Environment variable overrides to set on the spawned process.
    pub env: Vec<(String, String)>,
    /// Generic execution policy for this agent process.
    pub execution_policy: crate::execution::ExecutionPolicy,
    /// Suggested terminal title for this agent's pane.
    pub title: String,
    /// Path to the system prompt file for this agent.
    pub system_prompt_path: Option<String>,
}

/// Pluggable adapter interface for spawning agent CLI tools.
pub trait AgentAdapter: Send + Sync {
    /// Return the spawn spec for this agent config.
    fn spawn_spec(
        &self,
        config: &AgentConfig,
        run_id: &str,
        comm_log_path: &str,
        goal_path: &str,
        app_url: &str,
        working_directory: &str,
    ) -> AgentSpawnSpec;
}

pub mod claude;
pub mod opencode;
