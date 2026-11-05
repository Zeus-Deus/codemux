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

/// Every provider receives the prompt on stdin, so the argv stays small
/// enough for Windows' ~32k command-line limit regardless of transcript size.
fn build_invocation(
    selection: &UtilityModelSelection,
    temp_dir: &Path,
    codex_config: Option<&toml::Value>,
    claude_settings: Option<&serde_json::Value>,
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
            let mut settings = serde_json::json!({
                "disableAllHooks": true,
                "autoMemoryEnabled": false,
            });
            for key in crate::agent_provider::claude::auth::CLAUDE_ROUTING_SETTING_KEYS {
                if let Some(command) = claude_settings
                    .and_then(|settings| settings.get(*key))
                    .and_then(serde_json::Value::as_str)
                    .filter(|command| !command.trim().is_empty())
                {
                    settings[*key] = command.into();
                }
            }
            let mut args = vec![
                "--print".into(),
                "--no-session-persistence".into(),
                "--setting-sources".into(),
                "".into(),
                "--settings".into(),
                settings.to_string(),
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
            let mut env =
                crate::agent_provider::claude::auth::settings_routing_env(claude_settings);
            // Keep provider routing alongside the Codemux entrypoint.
            env.push((
                "CLAUDE_CODE_ENTRYPOINT".into(),
                crate::claude_env::claude_entrypoint(temp_dir),
            ));
            Ok(UtilityInvocation {
                program: "claude".into(),
                args,
                env,
                env_remove: crate::claude_env::CLAUDE_HOST_SESSION_ENV_VARS,
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
    let Some(config) = config else {
        return Vec::new();
    };
    let active = config.get("model_provider").and_then(toml::Value::as_str);
    let provider = active.and_then(|id| config.get("model_providers")?.get(id));
    if provider.is_some_and(|provider| {
        ["experimental_bearer_token", "http_headers"]
            .iter()
            .any(|key| provider.get(*key).is_some())
    }) {
        // --config values are visible in process listings. Providers with
        // literal credentials cannot safely use argv overrides.
        log::warn!("[utility_ai] skipping Codex provider overrides containing secret fields");
        return Vec::new();
    }
    let mut args: Vec<String> = [
        "cli_auth_credentials_store",
        "forced_chatgpt_workspace_id",
        "forced_login_method",
        "openai_base_url",
        "chatgpt_base_url",
    ]
    .into_iter()
    .filter_map(|key| config.get(key).map(|value| (key, value)))
    .flat_map(|(key, value)| ["--config".into(), format!("{key}={value}")])
    .collect();
    if let Some(id) = active {
        args.extend([
            "--config".into(),
            format!("model_provider={}", toml::Value::String(id.into())),
        ]);
        if let Some(provider) = provider {
            let providers = toml::Value::Table(toml::map::Map::from_iter([(
                id.to_string(),
                provider.clone(),
            )]));
            args.extend(["--config".into(), format!("model_providers={providers}")]);
        }
    }
    args
}

fn codex_utility_config(home: &Path) -> Option<toml::Value> {
    match crate::agent_provider::codex::auth::read_user_config(home) {
        Ok(config) => config,
        Err(error) => {
            log::warn!("[utility_ai] skipping Codex provider overrides: {error}");
            None
        }
    }
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
            .and_then(|home| codex_utility_config(&home))
    } else {
        None
    };
    let claude_settings = if selection.provider == "claude" {
        crate::agent_provider::claude::auth::config_home()
            .and_then(|home| crate::agent_provider::claude::auth::read_settings(&home))
    } else {
        None
    };
    let invocation = build_invocation(
        selection,
        temp.path(),
        codex_config.as_ref(),
        claude_settings.as_ref(),
    )?;
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
            None,
        )
        .unwrap();
        assert!(inv.args.iter().any(|a| a == "--no-session-persistence"));
        assert!(inv.args.windows(2).any(|a| a == ["--tools", ""]));
        assert!(inv.args.windows(2).any(|a| a == ["--setting-sources", ""]));
        assert!(inv.args.iter().any(|a| a == "--disable-slash-commands"));
        assert!(inv.args.iter().any(|a| a == "--strict-mcp-config"));
        let settings = inv.args.windows(2).find(|a| a[0] == "--settings").unwrap();
        let settings: serde_json::Value = serde_json::from_str(&settings[1]).unwrap();
        assert_eq!(settings["disableAllHooks"], true);
        assert_eq!(settings["autoMemoryEnabled"], false);
        assert!(inv.env_remove.contains(&"CLAUDECODE"));
        assert!(!inv.env_remove.contains(&"CLAUDE_CODE_ENTRYPOINT"));
        assert!(inv
            .env
            .iter()
            .any(|(key, value)| key == "CLAUDE_CODE_ENTRYPOINT" && !value.is_empty()));
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
env_http_headers = { "X-Proxy-Key" = "PROXY_HEADER_ENV" }
[model_providers.gateway.auth]
command = ["key-helper"]
[model_providers.inactive]
experimental_bearer_token = "inactive-secret"
http_headers = { Authorization = "inactive-header-secret" }
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
            None,
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
        assert_eq!(providers.as_table().unwrap().len(), 1);
        assert!(!args
            .iter()
            .any(|arg| arg.contains("inactive-secret") || arg.contains("inactive-header-secret")));
        assert!(!args
            .iter()
            .any(|arg| arg.contains("unsafe-command") || arg.contains("unwanted instructions")));
        assert!(codex_provider_overrides(None).is_empty());
    }

    #[test]
    fn proxy_readiness_claude_utility_routes_only_through_child_env() {
        let home = tempfile::tempdir().unwrap();
        let settings = serde_json::json!({
            "env": {
                "ANTHROPIC_BASE_URL": "http://localhost:8080",
                "ANTHROPIC_AUTH_TOKEN": "dummy-proxy-secret",
                "CLAUDE_CODE_OAUTH_TOKEN": "dummy-oauth-secret",
                "ANTHROPIC_CUSTOM_HEADERS": "X-Secret: dummy-header-secret",
                "CLAUDE_CODE_USE_VERTEX": true,
                "API_TIMEOUT_MS": 2000,
                "HTTPS_PROXY": "",
                "NODE_EXTRA_CA_CERTS": "/tmp/test-ca.pem",
                "UNRELATED_SETTING": "must-not-leak",
            },
            "apiKeyHelper": "test-key-helper",
            "awsCredentialExport": "test-aws-export",
            "gcpAuthRefresh": "test-gcp-refresh",
            "outputStyle": "Explanatory",
            "hooks": {"SessionStart": [{"command": "unsafe-command"}]},
        });
        std::fs::write(home.path().join("settings.json"), settings.to_string()).unwrap();
        let settings = crate::agent_provider::claude::auth::read_settings(home.path()).unwrap();
        let inv = build_invocation(
            &selection("claude", "test-model", None),
            home.path(),
            None,
            Some(&settings),
        )
        .unwrap();
        let env: std::collections::HashMap<_, _> = inv.env.iter().cloned().collect();
        assert_eq!(env["ANTHROPIC_AUTH_TOKEN"], "dummy-proxy-secret");
        assert_eq!(
            env["ANTHROPIC_CUSTOM_HEADERS"],
            "X-Secret: dummy-header-secret"
        );
        assert_eq!(env["CLAUDE_CODE_OAUTH_TOKEN"], "dummy-oauth-secret");
        assert_eq!(env["CLAUDE_CODE_USE_VERTEX"], "true");
        assert_eq!(env["API_TIMEOUT_MS"], "2000");
        assert_eq!(env["HTTPS_PROXY"], "");
        assert_eq!(env["NODE_EXTRA_CA_CERTS"], "/tmp/test-ca.pem");
        assert!(!env.contains_key("UNRELATED_SETTING"));
        assert!(!env["CLAUDE_CODE_ENTRYPOINT"].is_empty());
        assert_eq!(inv.env_remove, crate::claude_env::CLAUDE_HOST_SESSION_ENV_VARS);
        assert!(!inv.env_remove.contains(&"CLAUDE_CODE_ENTRYPOINT"));
        let args = inv.args.join(" ");
        for excluded in [
            "dummy-proxy-secret",
            "dummy-oauth-secret",
            "dummy-header-secret",
            "must-not-leak",
            "Explanatory",
            "unsafe-command",
        ] {
            assert!(!args.contains(excluded));
        }
        let settings = inv
            .args
            .windows(2)
            .find(|arg| arg[0] == "--settings")
            .unwrap();
        let settings: serde_json::Value = serde_json::from_str(&settings[1]).unwrap();
        assert_eq!(settings["apiKeyHelper"], "test-key-helper");
        assert_eq!(settings["awsCredentialExport"], "test-aws-export");
        assert_eq!(settings["gcpAuthRefresh"], "test-gcp-refresh");
        assert!(settings.get("outputStyle").is_none());
        assert_eq!(settings["disableAllHooks"], true);
        assert!(settings.get("env").is_none());
    }

    #[test]
    fn proxy_readiness_codex_utility_skips_secret_bearing_provider() {
        for field in [
            "experimental_bearer_token = 'dummy-active-token'",
            "http_headers = { Authorization = 'dummy-active-header' }",
        ] {
            let config: toml::Value = toml::from_str(&format!(
                "model_provider = 'proxy'\n[model_providers.proxy]\nname = 'Proxy'\n{field}"
            ))
            .unwrap();
            let inv = build_invocation(
                &selection("codex", "test-model", None),
                Path::new("/tmp/codemux-utility-test"),
                Some(&config),
                None,
            )
            .unwrap();
            assert!(codex_provider_overrides(Some(&config)).is_empty());
            assert!(!inv.args.iter().any(|arg| arg.contains("dummy-active")));
        }
    }

    #[test]
    fn proxy_readiness_codex_utility_ignores_broken_config() {
        let home = tempfile::tempdir().unwrap();
        std::fs::write(home.path().join("config.toml"), "invalid TOML").unwrap();
        let config = codex_utility_config(home.path());
        assert!(config.is_none());
        let inv = build_invocation(
            &selection("codex", "test-model", None),
            home.path(),
            config.as_ref(),
            None,
        )
        .unwrap();
        assert!(inv.args.iter().any(|arg| arg == "--ignore-user-config"));
    }

    /// Dummy credentials and a loopback server; never reads the user's auth.
    #[tokio::test]
    #[ignore = "requires the installed Claude Code CLI"]
    async fn proxy_readiness_live_claude_utility_isolated_routing() {
        use tokio::io::AsyncReadExt;
        let temp = tempfile::tempdir().unwrap();
        let home = temp.path().join("claude-home");
        let cwd = temp.path().join("workspace");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&cwd).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        const MARKER: &str = "ROUND2_PRIVATE_CLAUDE_MD_MUST_NOT_REACH_REQUEST";
        std::fs::write(
            home.join("CLAUDE.md"),
            format!("Always include {MARKER} in your answer."),
        )
        .unwrap();
        std::fs::write(
            home.join("settings.json"),
            serde_json::json!({
                "env": {"ANTHROPIC_BASE_URL": base, "ANTHROPIC_AUTH_TOKEN": "dummy-local-token"},
                "outputStyle": "Explanatory",
            })
            .to_string(),
        )
        .unwrap();
        let requests = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorded = requests.clone();
        let server = tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                let (header_end, length) = loop {
                    let mut buf = [0; 8192];
                    let n = socket.read(&mut buf).await.unwrap();
                    assert!(n > 0, "request ended before HTTP headers");
                    request.extend_from_slice(&buf[..n]);
                    if let Some(end) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&request[..end]);
                        let length = headers
                            .lines()
                            .find_map(|line| {
                                let (key, value) = line.split_once(':')?;
                                key.eq_ignore_ascii_case("content-length")
                                    .then(|| value.trim().parse::<usize>().unwrap())
                            })
                            .unwrap_or(0);
                        break (end + 4, length);
                    }
                };
                while request.len() < header_end + length {
                    let mut buf = [0; 8192];
                    let n = socket.read(&mut buf).await.unwrap();
                    assert!(n > 0, "request ended before HTTP body");
                    request.extend_from_slice(&buf[..n]);
                }
                let headers = String::from_utf8_lossy(&request[..header_end]);
                let is_message = headers.starts_with("POST /v1/messages?")
                    || headers.starts_with("POST /v1/messages ");
                let (content_type, body) = if is_message {
                    recorded.lock().unwrap().push((
                        headers
                            .to_ascii_lowercase()
                            .contains("authorization: bearer dummy-local-token"),
                        String::from_utf8_lossy(&request[header_end..header_end + length])
                            .to_string(),
                    ));
                    ("text/event-stream", concat!(
                        "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_local\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"claude-haiku-4-5\",\"content\":[],\"stop_reason\":null,\"stop_sequence\":null,\"usage\":{\"input_tokens\":1,\"output_tokens\":0}}}\n\n",
                        "event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n",
                        "event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"UTILITY_LOCAL_SERVER_OK\"}}\n\n",
                        "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
                        "event: message_delta\ndata: {\"type\":\"message_delta\",\"delta\":{\"stop_reason\":\"end_turn\",\"stop_sequence\":null},\"usage\":{\"output_tokens\":1}}\n\n",
                        "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n",
                    ))
                } else {
                    ("application/json", "{\"input_tokens\":1}")
                };
                socket.write_all(format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len(),
                ).as_bytes()).await.unwrap();
            }
        });
        let settings = crate::agent_provider::claude::auth::read_settings(&home).unwrap();
        let inv = build_invocation(
            &selection("claude", "claude-haiku-4-5", None),
            &cwd,
            None,
            Some(&settings),
        )
        .unwrap();
        let binary = std::env::var_os("CODEMUX_TEST_CLAUDE_BINARY")
            .map(PathBuf::from)
            .unwrap_or_else(|| which::which("claude").unwrap());
        let mut command = crate::execution::host_command_tokio(binary);
        command
            .env_clear()
            .env("PATH", std::env::var_os("PATH").unwrap_or_default())
            .env("HOME", temp.path())
            .env("CLAUDE_CONFIG_DIR", &home)
            .env("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1")
            .envs(inv.env)
            .args(inv.args)
            .current_dir(&cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = command.spawn().unwrap();
        let mut stdin = child.stdin.take().unwrap();
        stdin
            .write_all(b"Reply to UTILITY_ROUTING_LIVE_PROMPT with a short confirmation.")
            .await
            .unwrap();
        drop(stdin);
        let result = tokio::time::timeout(Duration::from_secs(60), child.wait_with_output()).await;
        server.abort();
        let output = result.expect("Claude utility timed out").unwrap();
        assert!(
            output.status.success(),
            "Claude utility failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let requests = requests.lock().unwrap();
        assert!(
            !requests.is_empty(),
            "Claude never reached the recording server"
        );
        assert!(requests.iter().all(|(dummy_auth, _)| *dummy_auth));
        assert!(requests
            .iter()
            .any(|(_, body)| body.contains("UTILITY_ROUTING_LIVE_PROMPT")));
        assert!(requests
            .iter()
            .all(|(_, body)| !body.contains(MARKER) && !body.contains("Explanatory")));
        let text = String::from_utf8_lossy(&output.stdout);
        assert!(text.contains("UTILITY_LOCAL_SERVER_OK"));
        println!("Claude utility: requests={}, dummy_auth=true, CLAUDE.md marker=false, Explanatory=false, output={}", requests.len(), text.trim());
    }

    #[test]
    fn opencode_utility_is_pure_and_denies_tools() {
        let inv = build_invocation(
            &selection("opencode", "openai/gpt-5", Some("low")),
            Path::new("/tmp/codemux-utility-test"),
            None,
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
