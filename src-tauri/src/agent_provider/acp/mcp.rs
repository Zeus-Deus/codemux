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
