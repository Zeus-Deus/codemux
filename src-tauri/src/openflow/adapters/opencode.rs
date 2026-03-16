/// OpenCode CLI adapter.
///
/// Builds a spawn spec for running `opencode` as an agent in an OpenFlow run.
/// The spawned process runs `opencode` via a wrapper script that reads the
/// system prompt from a file and passes it to opencode.
use super::{AgentAdapter, AgentSpawnSpec};
use crate::openflow::agent::AgentConfig;
use crate::openflow::prompts::SystemPrompts;

pub struct OpenCodeAdapter;

impl AgentAdapter for OpenCodeAdapter {
    fn spawn_spec(
        &self,
        config: &AgentConfig,
        run_id: &str,
        comm_log_path: &str,
        goal_path: &str,
        working_directory: &str,
    ) -> AgentSpawnSpec {
        // Use the wrapper script that reads the system prompt and goal
        let wrapper_path = SystemPrompts::wrapper_script_path();
        let wrapper_str = wrapper_path.to_string_lossy().to_string();
        let argv = vec![wrapper_str.clone()];

        let instance_id = instance_label(config);
        let mut env = vec![
            ("CODEMUX_AGENT_ROLE".into(), role_label(config)),
            ("CODEMUX_AGENT_INSTANCE_ID".into(), instance_id.clone()),
            ("CODEMUX_OPENFLOW_RUN_ID".into(), run_id.to_string()),
            (
                "CODEMUX_COMMUNICATION_LOG".into(),
                comm_log_path.to_string(),
            ),
            ("CODEMUX_GOAL_PATH".into(), goal_path.to_string()),
            ("CODEMUX_WORKING_DIR".into(), working_directory.to_string()),
            ("OPENCODE_MODEL".into(), config.model.clone()),
        ];

        // Inject thinking mode when configured.
        if !config.thinking_mode.is_empty() && config.thinking_mode != "auto" {
            env.push(("OPENCODE_THINKING".into(), config.thinking_mode.clone()));
        }

        // Get system prompt path — instance-specific so each parallel agent has its own file.
        let system_prompt_path =
            SystemPrompts::prompt_path_for_instance(&config.role, config.agent_index);
        let prompt_path_str = system_prompt_path.to_string_lossy().to_string();
        env.push(("CODEMUX_SYSTEM_PROMPT_PATH".into(), prompt_path_str.clone()));

        let title = format!(
            "[{}] {} — {}",
            instance_id,
            short_model(&config.model),
            run_id,
        );

        AgentSpawnSpec {
            argv,
            env,
            title,
            system_prompt_path: Some(prompt_path_str),
        }
    }
}

fn role_label(config: &AgentConfig) -> String {
    format!("{:?}", config.role).to_lowercase()
}

/// Returns a unique per-instance label like `builder-0`, `builder-1`.
/// The orchestrator always keeps its simple role label since there's only one.
fn instance_label(config: &AgentConfig) -> String {
    let role = role_label(config);
    if matches!(config.role, crate::openflow::OpenFlowRole::Orchestrator) {
        role
    } else {
        format!("{}-{}", role, config.agent_index)
    }
}

fn short_model(model: &str) -> &str {
    model.split('/').last().unwrap_or(model)
}
