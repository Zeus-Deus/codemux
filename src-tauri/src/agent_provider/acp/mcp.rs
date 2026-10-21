//! Codemux's MCP registry as ACP `mcpServers` entries.
//!
//! ACP agents get one server, Codemux's loopback MCP gateway, with a token of
//! the chat's own (see [`GatewayCaller`]). Nothing is written to the agent's
//! config files: the entry rides on `session/new` and `session/load`.

use serde_json::{json, Value};

use crate::mcp::gateway::GatewayCaller;
use crate::mcp::registry::McpRegistry;
use crate::mcp::McpConfigSource;

/// Server name agents see. Agents prefix tool names with it.
pub const SERVER_NAME: &str = "codemux";

/// Sources Cursor Agent loads itself (`cursor-agent mcp list`, 2026.10.14).
pub const CURSOR_NATIVE_SOURCES: &[McpConfigSource] =
    &[McpConfigSource::CursorUser, McpConfigSource::CursorProject];

/// Sources Grok Build loads itself (`grok inspect`, 1.0.52): Claude's user and
/// local scopes in `~/.claude.json`, the project `.mcp.json`, and both Cursor
/// files. Its own `~/.grok/config.toml` is not a Codemux source.
pub const GROK_NATIVE_SOURCES: &[McpConfigSource] = &[
    McpConfigSource::ClaudeUser,
    McpConfigSource::ClaudeLocal,
    McpConfigSource::ClaudeProject,
    McpConfigSource::CursorUser,
    McpConfigSource::CursorProject,
];

/// Hosts an agent must reach directly: the gateway listens on loopback.
const LOOPBACK_HOSTS: &str = "127.0.0.1,localhost,::1";

/// `env` plus `NO_PROXY`/`no_proxy` that exempt loopback, for a local agent
/// that gets the gateway. HTTP clients such as Hermes's httpx 0.28.1 send
/// even loopback requests through an ambient `HTTP_PROXY`, which breaks
/// every tool call and hands the proxy the chat's token. An existing value,
/// from `env` or this process, is kept.
pub fn with_loopback_no_proxy(
    mut env: std::collections::HashMap<String, String>,
) -> std::collections::HashMap<String, String> {
    let existing = ["NO_PROXY", "no_proxy"]
        .iter()
        .find_map(|name| env.get(*name).cloned().or_else(|| std::env::var(name).ok()))
        .filter(|value| !value.trim().is_empty());
    let value = match existing {
        Some(existing) => format!("{existing},{LOOPBACK_HOSTS}"),
        None => LOOPBACK_HOSTS.to_string(),
    };
    env.insert("NO_PROXY".into(), value.clone());
    env.insert("no_proxy".into(), value);
    env
}

/// How the agent reaches the gateway.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    /// ACP `McpServerHttp`.
    Http,
    /// ACP `McpServerStdio` running `codemux mcp-bridge`, for agents that
    /// don't advertise `mcpCapabilities.http`.
    StdioBridge,
}

impl Transport {
    /// HTTP when `initialize` advertised it, else the stdio bridge.
    pub fn negotiated(initialize: &Value) -> Self {
        if initialize
            .pointer("/agentCapabilities/mcpCapabilities/http")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            Self::Http
        } else {
            Self::StdioBridge
        }
    }
}

/// The `mcpServers` array for one chat's `session/new` or `session/load`.
///
/// Empty without a registry (sessions on a device, tests) or when the
/// gateway can't start: the chat then runs without Codemux's tools rather
/// than not at all.
pub async fn session_mcp_servers(
    registry: Option<&McpRegistry>,
    thread_id: &str,
    caller: GatewayCaller,
    name: &str,
    transport: Transport,
) -> Value {
    let Some(registry) = registry else {
        return json!([]);
    };
    match registry.gateway_chat_connection(thread_id, caller).await {
        Ok(connection) => json!([match transport {
            Transport::Http => connection.acp_http_server(name),
            Transport::StdioBridge => {
                connection.acp_stdio_bridge(name, &crate::mcp_server::codemux_mcp_command())
            }
        }]),
        Err(error) => {
            eprintln!("[codemux::acp] MCP gateway unavailable; chat starts without Codemux tools: {error}");
            json!([])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_is_exempted_from_proxies_and_existing_entries_are_kept() {
        let env = with_loopback_no_proxy(std::collections::HashMap::from([
            ("NO_PROXY".to_string(), "corp.example".to_string()),
            ("OTHER".to_string(), "kept".to_string()),
        ]));
        assert_eq!(env["NO_PROXY"], "corp.example,127.0.0.1,localhost,::1");
        assert_eq!(env["no_proxy"], env["NO_PROXY"]);
        assert_eq!(env["OTHER"], "kept");
        let env = with_loopback_no_proxy(std::collections::HashMap::from([(
            "no_proxy".to_string(),
            " ".to_string(),
        )]));
        assert!(env["NO_PROXY"].ends_with("127.0.0.1,localhost,::1"));
    }

    #[test]
    fn http_only_when_advertised() {
        let advertised = json!({"agentCapabilities":{"mcpCapabilities":{"http":true,"sse":true}}});
        assert_eq!(Transport::negotiated(&advertised), Transport::Http);
        let declined = json!({"agentCapabilities":{"mcpCapabilities":{"http":false}}});
        assert_eq!(Transport::negotiated(&declined), Transport::StdioBridge);
        // Hermes 0.21.5 advertises no mcpCapabilities at all.
        let silent = json!({"agentCapabilities":{"loadSession":true}});
        assert_eq!(Transport::negotiated(&silent), Transport::StdioBridge);
    }

    #[tokio::test]
    async fn no_registry_means_no_servers() {
        let servers =
            session_mcp_servers(None, "t", GatewayCaller::default(), SERVER_NAME, Transport::Http)
                .await;
        assert_eq!(servers, json!([]));
    }

    #[tokio::test]
    async fn a_registry_yields_one_gateway_entry_per_chat() {
        let registry = McpRegistry::new();
        let caller = GatewayCaller {
            workspace_id: Some("ws".into()),
            native_sources: CURSOR_NATIVE_SOURCES.to_vec(),
            flat_tool_names: false,
            native: None,
        };
        let http =
            session_mcp_servers(Some(&registry), "t1", caller.clone(), SERVER_NAME, Transport::Http)
                .await;
        let entries = http.as_array().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["type"], "http");
        assert_eq!(entries[0]["name"], SERVER_NAME);
        assert!(entries[0]["url"].as_str().unwrap().starts_with("http://127.0.0.1:"));

        let bridge = session_mcp_servers(
            Some(&registry),
            "t1",
            caller,
            SERVER_NAME,
            Transport::StdioBridge,
        )
        .await;
        assert_eq!(bridge[0]["args"], json!(["mcp-bridge"]));
        // Same chat, same token, whichever transport carries it.
        assert_eq!(
            bridge[0]["env"][1]["value"],
            entries[0]["headers"][0]["value"]
                .as_str()
                .unwrap()
                .trim_start_matches("Bearer ")
        );
    }
}

/// One injected server bound to one app session, including shared-profile runtimes.
#[derive(Debug)]
pub(crate) struct ChatMcp {
    registry: McpRegistry,
    pub(crate) session: std::sync::Arc<crate::mcp::thread_gateway::GatewaySession>,
    name: String,
    transport: Transport,
    sources: Vec<McpConfigSource>,
    flat: bool,
    published_epoch: std::sync::atomic::AtomicU64,
    fallback: std::sync::Mutex<Option<tempfile::NamedTempFile>>,
}
impl ChatMcp {
    pub(crate) fn new(
        registry: McpRegistry,
        thread: &str,
        provider: crate::agent_provider::ProviderKind,
        workspace: Option<String>,
        mode: Option<String>,
        name: String,
        transport: Transport,
        sources: Vec<McpConfigSource>,
        flat: bool,
    ) -> Self {
        Self {
            registry,
            session: crate::mcp::thread_gateway::GatewaySession::new(
                thread, provider, workspace, mode,
            ),
            name: format!("{name}_{}", &uuid::Uuid::new_v4().simple().to_string()[..8]),
            transport,
            sources,
            flat,
            fallback: Default::default(),
            published_epoch: std::sync::atomic::AtomicU64::new(u64::MAX),
        }
    }
    pub(crate) fn needs_refresh(&self) -> bool {
        self.published_epoch
            .load(std::sync::atomic::Ordering::SeqCst)
            != self.session.authority.token_epoch()
    }
    pub(crate) fn prompt(&self, text: &str) -> String {
        let file = self.fallback.lock().unwrap();
        let Some(file) = file.as_ref() else {
            return text.into();
        };
        let quote = |s: &str| format!("'{}'", s.replace('\'', "'\"'\"'"));
        format!("{text}\n\n[Codemux host guidance: use the injected Codemux MCP tools for orchestration. If your harness does not expose them, run {} mcp-call --connection-file {} --list to discover the same tools, then mcp-call --connection-file {} <tool> '<JSON arguments>'. This connection belongs only to this chat. Do not poll after async delegation; end the turn and wait for its result. When asked to report to a coordinator, use thread_send.]",quote(&crate::mcp_server::codemux_mcp_command()),quote(&file.path().to_string_lossy()),quote(&file.path().to_string_lossy()))
    }
    pub(crate) async fn connection(
        &self,
    ) -> Result<crate::mcp::gateway::McpGatewayConnection, String> {
        use std::io::{Seek, Write};
        let binding = crate::mcp::thread_gateway::GatewayBinding::new(self.session.clone());
        let epoch = binding.epoch;
        let connection = self
            .registry
            .gateway_chat_connection(
                &self.session.caller().thread_id,
                GatewayCaller {
                    workspace_id: self.session.caller().workspace_id,
                    native_sources: self.sources.clone(),
                    flat_tool_names: self.flat,
                    native: Some(binding),
                },
            )
            .await?;
        let mut file = self.fallback.lock().unwrap();
        if file.is_none() {
            let dir = dirs::runtime_dir().unwrap_or_else(std::env::temp_dir);
            *file = Some(
                tempfile::Builder::new()
                    .prefix("codemux-mcp-")
                    .tempfile_in(dir)
                    .map_err(|e| e.to_string())?,
            );
        }
        let file = file.as_mut().unwrap();
        file.as_file_mut().set_len(0).map_err(|e| e.to_string())?;
        file.as_file_mut().rewind().map_err(|e| e.to_string())?;
        serde_json::to_writer(file.as_file_mut(), &connection).map_err(|e| e.to_string())?;
        file.flush().map_err(|e| e.to_string())?;
        self.published_epoch
            .store(epoch, std::sync::atomic::Ordering::SeqCst);
        Ok(connection)
    }
    pub(crate) async fn servers(&self) -> Value {
        match self.connection().await {
            Ok(connection) => {
                let epoch = self
                    .published_epoch
                    .load(std::sync::atomic::Ordering::SeqCst);
                let name = format!("{}_{epoch}", self.name);
                json!([match self.transport {
                    Transport::Http => connection.acp_http_server(&name),
                    Transport::StdioBridge => connection
                        .acp_stdio_bridge(&name, &crate::mcp_server::codemux_mcp_command()),
                }])
            }
            Err(error) => {
                eprintln!("[codemux::acp] chat MCP unavailable: {error}");
                json!([])
            }
        }
    }
}

impl Drop for ChatMcp {
    fn drop(&mut self) {
        self.session.authority.revoke();
    }
}
