/// OpenCode CLI adapter.
///
/// Builds a spawn spec for running `opencode` as an agent in an OpenFlow run.
/// The spawned process runs `opencode` interactively in the terminal pane so
/// the user can watch it work.
use super::{AgentAdapter, AgentSpawnSpec};
use crate::openflow::agent::AgentConfig;

pub struct OpenCodeAdapter;

impl AgentAdapter for OpenCodeAdapter {
    fn spawn_spec(
        &self,
        config: &AgentConfig,
        run_id: &str,
        comm_log_path: &str,
    ) -> AgentSpawnSpec {
        // Build argv: `opencode` with optional model flag.
        // opencode does not yet accept a --model CLI flag at spawn time (the
        // user selects the model inside the session), but we inject the
        // preferred model via the OPENCODE_MODEL env var which opencode reads
        // on startup.  If that variable is unsupported it is silently ignored.
        let argv = vec!["opencode".to_string()];

        let mut env = vec![
            ("CODEMUX_AGENT_ROLE".into(), role_label(config)),
            ("CODEMUX_OPENFLOW_RUN_ID".into(), run_id.to_string()),
            (
                "CODEMUX_COMMUNICATION_LOG".into(),
                comm_log_path.to_string(),
            ),
            ("OPENCODE_MODEL".into(), config.model.clone()),
        ];

        // Inject thinking mode when configured.
        if !config.thinking_mode.is_empty() && config.thinking_mode != "auto" {
            env.push(("OPENCODE_THINKING".into(), config.thinking_mode.clone()));
        }

        let title = format!(
            "[{}] {} — {}",
            role_label(config),
            short_model(&config.model),
            run_id,
        );

        AgentSpawnSpec { argv, env, title }
    }
}

fn role_label(config: &AgentConfig) -> String {
    format!("{:?}", config.role).to_lowercase()
}

fn short_model(model: &str) -> &str {
    model.split('/').last().unwrap_or(model)
}
