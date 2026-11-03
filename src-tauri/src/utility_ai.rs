//! Small, isolated text-generation runner shared by non-agentic Codemux
//! features. Unlike the merge resolver this path never grants workspace
//! writes: conversation text is untrusted input and summarisation must not be
//! able to turn an old instruction into a shell/edit action.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncWriteExt;

const UTILITY_TIMEOUT: Duration = Duration::from_secs(180);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UtilityModelSelection {
    pub provider: String,
    pub model: String,
    #[serde(default)]
    pub effort: Option<String>,
}

#[derive(Debug)]
struct UtilityInvocation {
    program: String,
    args: Vec<String>,
    env: Vec<(String, String)>,
    env_remove: &'static [&'static str],
    output_file: Option<PathBuf>,
}

/// Nested-session markers Claude Code exports. When Codemux itself runs under
/// a Claude session they would make the utility CLI treat itself as that
/// session's child and pull in the parent's context.
const CLAUDE_NESTED_SESSION_ENV_VARS: &[&str] = &[
    "CLAUDECODE",
    "CLAUDE_CODE_SESSION_ID",
    "CLAUDE_CODE_ENTRYPOINT",
    "CLAUDE_CODE_CHILD_SESSION",
    "CLAUDE_CODE_SSE_PORT",
];

/// Every provider receives the prompt on stdin, so the argv stays small
/// enough for Windows' ~32k command-line limit regardless of transcript size.
fn build_invocation(
    selection: &UtilityModelSelection,
    temp_dir: &Path,
    codex_config: Option<&toml::Value>,
) -> Result<UtilityInvocation, String> {
    if selection.model.trim().is_empty() {
        return Err("utility_model_required".into());
    }
    match selection.provider.as_str() {
        "codex" => {
            let output_file = temp_dir.join("last-message.md");
            let mut args = vec![
                "exec".into(),
                "--ephemeral".into(),
                "--skip-git-repo-check".into(),
                "--ignore-user-config".into(),
                "--ignore-rules".into(),
                "--sandbox".into(),
                "read-only".into(),
                "--color".into(),
                "never".into(),
                "--model".into(),
                selection.model.clone(),
            ];
            // Keep provider routing, but not hooks, MCP servers, tools or
            // instructions from the user's full agent configuration.
            args.extend(codex_provider_overrides(codex_config));
            if let Some(effort) = selection.effort.as_deref().filter(|v| !v.is_empty()) {
                args.extend([
                    "--config".into(),
                    format!("model_reasoning_effort=\"{effort}\""),
                ]);
            }
            args.extend([
                "--output-last-message".into(),
                output_file.to_string_lossy().to_string(),
                "-".into(),
            ]);
            Ok(UtilityInvocation {
                program: "codex".into(),
                args,
                env: Vec::new(),
                env_remove: &[],
                output_file: Some(output_file),
            })
        }
        "claude" => {
            let mut args = vec![
                "--print".into(),
                "--no-session-persistence".into(),
                // User settings carry proxy env and apiKeyHelper. Disable
                // execution customizations while preserving that routing.
                "--setting-sources".into(),
                "user".into(),
                "--settings".into(),
                r#"{"disableAllHooks":true,"autoMemoryEnabled":false}"#.into(),
                "--disable-slash-commands".into(),
                "--output-format".into(),
                "text".into(),
                "--strict-mcp-config".into(),
                "--tools".into(),
                "".into(),
                "--model".into(),
                selection.model.clone(),
            ];
            if let Some(effort) = selection.effort.as_deref().filter(|v| !v.is_empty()) {
                args.extend(["--effort".into(), effort.into()]);
            }
            Ok(UtilityInvocation {
                program: "claude".into(),
                args,
                env: Vec::new(),
                env_remove: CLAUDE_NESTED_SESSION_ENV_VARS,
                output_file: None,
            })
        }
        "opencode" => {
            let mut args = vec![
                "run".into(),
                "--pure".into(),
                "--format".into(),
                "default".into(),
                "--model".into(),
                selection.model.clone(),
                "--dir".into(),
                temp_dir.to_string_lossy().to_string(),
            ];
            if let Some(effort) = selection.effort.as_deref().filter(|v| !v.is_empty()) {
                args.extend(["--variant".into(), effort.into()]);
            }
            // `opencode run` reads a piped stdin as the message when no
            // positional message is given. Passing the transcript on argv
            // instead would blow past Windows' ~32k CreateProcess limit for
            // any conversation worth summarising.
            //
            // `--pure` removes plugins. The ephemeral config also denies
            // every tool/permission so a transcript cannot ask OpenCode to
            // inspect or mutate the machine while it is being summarized.
            Ok(UtilityInvocation {
                program: "opencode".into(),
                args,
                env: vec![(
                    "OPENCODE_CONFIG_CONTENT".into(),
                    r#"{"permission":{"*":"deny"},"tools":{"*":false},"mcp":{}}"#.into(),
                )],
                env_remove: &[],
                output_file: None,
            })
        }
        _ => Err(format!(
            "utility_provider_unsupported: {}",
            selection.provider
        )),
    }
}

fn codex_provider_overrides(config: Option<&toml::Value>) -> Vec<String> {
    [
        "model_provider",
        "model_providers",
        "cli_auth_credentials_store",
        "forced_chatgpt_workspace_id",
        "forced_login_method",
        "openai_base_url",
        "chatgpt_base_url",
    ]
    .into_iter()
    .filter_map(|key| config?.get(key).map(|value| (key, value)))
    .flat_map(|(key, value)| ["--config".into(), format!("{key}={value}")])
    .collect()
}

fn strip_ansi(value: &str) -> String {
    let mut clean = String::with_capacity(value.len());
    let mut chars = value.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' && chars.peek() == Some(&'[') {
            chars.next();
            for code in chars.by_ref() {
                if ('@'..='~').contains(&code) {
                    break;
                }
            }
        } else {
            clean.push(ch);
        }
    }
    clean
}

pub async fn generate_utility_text(
    selection: &UtilityModelSelection,
    prompt: &str,
) -> Result<String, String> {
    let temp = tempfile::tempdir().map_err(|e| format!("utility_temp_dir_failed: {e}"))?;
    let codex_config = if selection.provider == "codex" {
        crate::agent_provider::codex::auth::config_home()
            .map(|home| crate::agent_provider::codex::auth::read_user_config(&home))
            .transpose()?
            .flatten()
    } else {
        None
    };
    let invocation = build_invocation(selection, temp.path(), codex_config.as_ref())?;
    let mut command = crate::execution::host_command_tokio(&invocation.program);
    command
        .args(&invocation.args)
        .current_dir(temp.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    for (key, value) in &invocation.env {
        command.env(key, value);
    }
    for key in invocation.env_remove {
        command.env_remove(key);
    }
    let mut child = command
        .spawn()
        .map_err(|e| format!("utility_spawn_failed: {}: {e}", selection.provider))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| "utility_stdin_unavailable".to_string())?;
    stdin
        .write_all(prompt.as_bytes())
        .await
        .map_err(|e| format!("utility_prompt_write_failed: {e}"))?;
    drop(stdin);
    let output = tokio::time::timeout(UTILITY_TIMEOUT, child.wait_with_output())
        .await
        .map_err(|_| "utility_generation_timeout".to_string())?
        .map_err(|e| format!("utility_generation_io_failed: {e}"))?;
    if !output.status.success() {
        let stderr = strip_ansi(&String::from_utf8_lossy(&output.stderr));
        let detail = stderr.trim().chars().take(600).collect::<String>();
        return Err(format!(
            "utility_generation_failed: {}{}",
            selection.provider,
            if detail.is_empty() {
                String::new()
            } else {
                format!(": {detail}")
            }
        ));
    }
    let raw = if let Some(path) = invocation.output_file {
        tokio::fs::read_to_string(path)
            .await
            .map_err(|e| format!("utility_output_read_failed: {e}"))?
    } else {
        String::from_utf8_lossy(&output.stdout).to_string()
    };
    let text = strip_ansi(&raw).trim().to_string();
    if text.is_empty() {
        Err("utility_generation_empty".into())
    } else {
        Ok(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn selection(provider: &str, model: &str, effort: Option<&str>) -> UtilityModelSelection {
        UtilityModelSelection {
            provider: provider.into(),
            model: model.into(),
            effort: effort.map(str::to_string),
        }
    }

    #[test]
    fn codex_utility_is_ephemeral_and_read_only() {
        let inv = build_invocation(
            &selection("codex", "gpt-5.6-luna", Some("low")),
            Path::new("/tmp/codemux-utility-test"),
            None,
        )
        .unwrap();
        assert!(inv.args.iter().any(|a| a == "--ephemeral"));
        assert!(inv.args.windows(2).any(|a| a == ["--sandbox", "read-only"]));
        assert!(!inv.args.iter().any(|a| a.contains("dangerously")));
    }

    #[test]
    fn claude_utility_disables_tools_and_persistence() {
        let inv = build_invocation(
            &selection("claude", "claude-haiku-4-5", None),
            Path::new("/tmp/codemux-utility-test"),
            None,
        )
        .unwrap();
        assert!(inv.args.iter().any(|a| a == "--no-session-persistence"));
        assert!(inv.args.windows(2).any(|a| a == ["--tools", ""]));
        assert!(inv
            .args
            .windows(2)
            .any(|a| a == ["--setting-sources", "user"]));
        assert!(inv.args.iter().any(|a| a == "--disable-slash-commands"));
        assert!(inv.args.iter().any(|a| a == "--strict-mcp-config"));
        let settings = inv.args.windows(2).find(|a| a[0] == "--settings").unwrap();
        let settings: serde_json::Value = serde_json::from_str(&settings[1]).unwrap();
        assert_eq!(settings["disableAllHooks"], true);
        assert_eq!(settings["autoMemoryEnabled"], false);
        assert!(inv.env_remove.contains(&"CLAUDECODE"));
        assert!(!inv.args.iter().any(|a| a.contains("dangerously")));
    }

    #[test]
    fn proxy_readiness_codex_utility_preserves_only_provider_config() {
        let config: toml::Value = toml::from_str(
            r#"
model_provider = "gateway"
cli_auth_credentials_store = "file"
notify = ["unsafe-command"]
developer_instructions = "unwanted instructions"
[model_providers.gateway]
name = "My gateway"
base_url = "http://localhost:8080/v1"
env_key = "PROXY_KEY"
[model_providers.gateway.auth]
command = ["key-helper"]
[mcp_servers.unwanted]
command = "unsafe-command"
[hooks]
SessionStart = []
"#,
        )
        .unwrap();
        let inv = build_invocation(
            &selection("codex", "test-model", None),
            Path::new("/tmp/codemux-utility-test"),
            Some(&config),
        )
        .unwrap();
        let args = inv.args;
        assert!(args.iter().any(|arg| arg == "--ignore-user-config"));
        assert!(args.iter().any(|arg| arg == "--ignore-rules"));
        assert!(args.contains(&"model_provider=\"gateway\"".into()));
        let providers = args
            .iter()
            .find_map(|arg| arg.strip_prefix("model_providers="))
            .unwrap();
        let providers: toml::Value = providers.parse().unwrap();
        assert_eq!(providers["gateway"], config["model_providers"]["gateway"]);
        assert!(!args
            .iter()
            .any(|arg| arg.contains("unsafe-command") || arg.contains("unwanted instructions")));
        assert!(codex_provider_overrides(None).is_empty());
    }

    #[test]
    fn opencode_utility_is_pure_and_denies_tools() {
        let inv = build_invocation(
            &selection("opencode", "openai/gpt-5", Some("low")),
            Path::new("/tmp/codemux-utility-test"),
            None,
        )
        .unwrap();
        assert!(inv.args.iter().any(|a| a == "--pure"));
        assert!(inv.env.iter().any(|(_, value)| value.contains("deny")));
        assert!(!inv.args.iter().any(|a| a == "--auto"));
        // No positional message: `opencode run` takes it from stdin instead.
        // On argv a summary chunk would blow past Windows' ~32k limit.
        assert_eq!(inv.args.last().map(String::as_str), Some("low"));
    }
}
