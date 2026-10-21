//! Codemux's MCP registry as ACP `mcpServers` entries.
//!
//! ACP agents get one server, Codemux's loopback MCP gateway, with a token of
//! the chat's own (see [`ChatMcp`]). Nothing is written to disk or to the
//! agent's config files: the entry rides on `session/new` and `session/load`.

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

    fn chat(registry: &McpRegistry, provider: crate::agent_provider::ProviderKind) -> ChatMcp {
        ChatMcp::new(
            registry.clone(),
            "t1",
            provider,
            Some("ws".into()),
            Some("agent".into()),
            SERVER_NAME.into(),
            None,
            CURSOR_NATIVE_SOURCES.to_vec(),
            false,
        )
    }

    #[tokio::test]
    async fn a_chat_gets_one_gateway_entry_and_one_token_per_session() {
        let registry = McpRegistry::new();
        let chat = chat(&registry, crate::agent_provider::ProviderKind::Cursor);
        let http = chat.servers(Transport::Http).await;
        let entries = http.as_array().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["type"], "http");
        assert_eq!(entries[0]["name"], SERVER_NAME);
        assert!(entries[0]["url"].as_str().unwrap().starts_with("http://127.0.0.1:"));

        let bridge = chat.servers(Transport::StdioBridge).await;
        assert_eq!(bridge[0]["args"], json!(["mcp-bridge"]));
        // Same session, same token, whichever transport carries it.
        assert_eq!(
            bridge[0]["env"][1]["value"],
            entries[0]["headers"][0]["value"]
                .as_str()
                .unwrap()
                .trim_start_matches("Bearer ")
        );
        // Stop does not rotate the credential; a new session gets its own.
        chat.session.authority.invalidate();
        assert_eq!(chat.servers(Transport::Http).await, http);
        let next = self::chat(&registry, crate::agent_provider::ProviderKind::Cursor);
        assert_ne!(next.servers(Transport::Http).await[0]["headers"], entries[0]["headers"]);
    }

    struct Collaboration;
    impl crate::mcp::registry::HostTools for Collaboration {
        fn tools_for(&self, _: &crate::mcp::registry::HostCaller) -> Vec<crate::mcp::runtime::McpTool> {
            vec![crate::mcp::runtime::McpTool {
                name: "thread_send".into(),
                prefixed_name: crate::mcp::runtime::prefix_tool_name("codemux", "thread_send"),
                description: None,
                input_schema: json!({}),
                server_id: "codemux".into(),
            }]
        }
        fn call(&self, _: &crate::mcp::registry::HostCaller, _: &str, _: &Value) -> Option<Value> {
            None
        }
    }

    #[test]
    fn the_cli_hint_is_added_only_while_collaboration_needs_it() {
        use crate::agent_provider::ProviderKind::*;
        let off = McpRegistry::new();
        assert_eq!(chat(&off, Cursor).prompt("Review this"), "Review this", "collaboration off");
        let on = McpRegistry::new();
        on.set_host_tools(std::sync::Arc::new(Collaboration));
        let cursor = chat(&on, Cursor);
        let hinted = cursor.prompt("Review this");
        assert!(hinted.starts_with("Review this\n\n[Codemux: "), "{hinted}");
        assert!(hinted.contains("mcp-call --list"), "{hinted}");
        assert!(!hinted.contains("connection-file"), "no credential path is advertised");
        for command in ["/compact", "  /model fast", "/title Release notes"] {
            assert_eq!(cursor.prompt(command), command, "slash commands stay untouched");
        }
        // Agents that use injected MCP never need the hint.
        cursor.session.mark_contacted();
        assert_eq!(cursor.prompt("Review this"), "Review this");
        for provider in [Hermes, OpenCode] {
            assert_eq!(chat(&on, provider).prompt("Review this"), "Review this", "{provider:?}");
        }
    }

    #[tokio::test]
    async fn a_kept_key_follows_the_reopened_session_with_one_server_entry() {
        let registry = McpRegistry::new();
        let open = |key: &str| {
            ChatMcp::new(
                registry.clone(),
                "hermes-thread",
                crate::agent_provider::ProviderKind::Hermes,
                None,
                Some("default".into()),
                "codemux_fixed".into(),
                Some(key.into()),
                vec![],
                false,
            )
        };
        let first = open("hermes:runtime-a:hermes-thread");
        let before = first.servers(Transport::Http).await;
        drop(first);
        // Same process, same chat: Hermes reuses its connected server by
        // name, so the same token must now act for the reopened session.
        let reopened = open("hermes:runtime-a:hermes-thread");
        reopened.session.bind_session("s2");
        reopened.session.begin("turn", None).unwrap();
        let after = reopened.servers(Transport::Http).await;
        assert_eq!(before, after);
        let reply = reqwest::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .post(after[0]["url"].as_str().unwrap())
            .header("authorization", after[0]["headers"][0]["value"].as_str().unwrap())
            .json(&json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}))
            .send()
            .await
            .unwrap();
        assert_eq!(reply.status(), 200);
        // A new Hermes process gets a new key and token.
        let restarted = open("hermes:runtime-b:hermes-thread");
        assert_ne!(restarted.servers(Transport::Http).await, after);
        registry.started_gateway().unwrap().revoke("hermes:runtime-a:");
        let revoked = reqwest::Client::builder()
            .no_proxy()
            .build()
            .unwrap()
            .post(after[0]["url"].as_str().unwrap())
            .header("authorization", after[0]["headers"][0]["value"].as_str().unwrap())
            .json(&json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}))
            .send()
            .await
            .unwrap();
        assert_eq!(revoked.status(), 401, "an ended Hermes process loses its tokens");
    }

    #[tokio::test]
    async fn only_one_process_per_chat_agents_get_the_shell_fallback_credential() {
        use crate::agent_provider::ProviderKind::*;
        let registry = McpRegistry::new();
        for provider in [Cursor, Grok, Acp] {
            let env = chat(&registry, provider).env().await;
            assert!(env[crate::mcp::gateway::BRIDGE_URL_ENV].starts_with("http://127.0.0.1:"));
            assert_eq!(env[crate::mcp::gateway::BRIDGE_TOKEN_ENV].len(), 48);
        }
        for provider in [Hermes, OpenCode] {
            assert!(chat(&registry, provider).env().await.is_empty());
        }
    }
}

/// One chat session's gateway credential and the server entry carrying it.
///
/// The credential lives as long as the provider session: Stop leaves it in
/// place (the authority's turn fence refuses stopped-turn calls), and closing
/// or replacing the session revokes it. Nothing is written to disk.
///
/// Native subagents (OpenCode child sessions, Hermes delegate children, like
/// Claude Task agents) share their parent's process. Hermes children can use
/// the parent's credential while its turn runs; OpenCode 2 tags each call with
/// its session, so the gateway refuses calls from OpenCode child sessions.
#[derive(Debug)]
pub(crate) struct ChatMcp {
    registry: McpRegistry,
    pub(crate) session: std::sync::Arc<crate::mcp::thread_gateway::GatewaySession>,
    pub(crate) name: String,
    key: String,
    /// A caller-chosen key outlives this session (see [`ChatMcp::new`]).
    keep_key: bool,
    sources: Vec<McpConfigSource>,
    flat: bool,
    gateway: std::sync::OnceLock<std::sync::Arc<crate::mcp::gateway::McpGatewayRuntime>>,
}
impl ChatMcp {
    /// `key` names a credential the provider keeps across sessions (Hermes
    /// keeps a connection per server name for its process lifetime); reissuing
    /// it points the same token at this session. `None` mints a fresh token
    /// that is revoked when this session ends.
    pub(crate) fn new(
        registry: McpRegistry,
        thread: &str,
        provider: crate::agent_provider::ProviderKind,
        workspace: Option<String>,
        mode: Option<String>,
        name: String,
        key: Option<String>,
        sources: Vec<McpConfigSource>,
        flat: bool,
    ) -> Self {
        Self {
            registry,
            session: crate::mcp::thread_gateway::GatewaySession::new(
                thread, provider, workspace, mode,
            ),
            name,
            keep_key: key.is_some(),
            key: key.unwrap_or_else(|| format!("chat:{}", uuid::Uuid::new_v4())),
            sources,
            flat,
            gateway: Default::default(),
        }
    }
    /// Cursor, Grok and custom ACP agents run one process per chat, so their
    /// shell can carry this chat's credential for `codemux mcp-call`.
    fn shell_fallback(&self) -> bool {
        use crate::agent_provider::ProviderKind::*;
        matches!(self.session.caller().provider, Cursor | Grok | Acp)
    }
    /// Agents that never reached the gateway get a one-line hint toward the
    /// CLI fallback while collaboration is on. Slash commands stay untouched.
    pub(crate) fn prompt(&self, text: &str) -> String {
        if !self.shell_fallback()
            || self.session.contacted()
            || text.trim_start().starts_with('/')
            || self.registry.host_tools(&self.session.caller()).is_empty()
        {
            return text.into();
        }
        let program = crate::mcp_server::codemux_mcp_command().replace('\'', "'\"'\"'");
        format!("{text}\n\n[Codemux: if the Codemux MCP tools are not available to you, run '{program}' mcp-call --list, then '{program}' mcp-call <tool> '<JSON arguments>'.]")
    }
    pub(crate) async fn connection(
        &self,
    ) -> Result<crate::mcp::gateway::McpGatewayConnection, String> {
        let gateway = self.registry.gateway_runtime().await?;
        let gateway = self.gateway.get_or_init(|| gateway);
        Ok(gateway.chat_connection(
            &self.key,
            GatewayCaller {
                workspace_id: self.session.caller().workspace_id,
                native_sources: self.sources.clone(),
                flat_tool_names: self.flat,
                native: Some(crate::mcp::thread_gateway::GatewayBinding::new(
                    self.session.clone(),
                )),
            },
        ))
    }
    /// The `mcpServers` array for this session's `session/new` or
    /// `session/load`. Empty when the gateway can't start: the chat then runs
    /// without Codemux's tools rather than not at all.
    pub(crate) async fn servers(&self, transport: Transport) -> Value {
        match self.connection().await {
            Ok(connection) => json!([match transport {
                Transport::Http => connection.acp_http_server(&self.name),
                Transport::StdioBridge => connection
                    .acp_stdio_bridge(&self.name, &crate::mcp_server::codemux_mcp_command()),
            }]),
            Err(error) => {
                eprintln!("[codemux::acp] MCP gateway unavailable; chat starts without Codemux tools: {error}");
                json!([])
            }
        }
    }
    /// Agent process environment for the shell fallback. Same-user processes
    /// can read a process environment, the same exposure the stdio bridge
    /// already has; the credential acts only during this chat's turns.
    pub(crate) async fn env(&self) -> std::collections::HashMap<String, String> {
        if !self.shell_fallback() {
            return Default::default();
        }
        match self.connection().await {
            Ok(connection) => std::collections::HashMap::from([
                (crate::mcp::gateway::BRIDGE_URL_ENV.to_string(), connection.url),
                (
                    crate::mcp::gateway::BRIDGE_TOKEN_ENV.to_string(),
                    connection.bearer_token,
                ),
            ]),
            Err(_) => Default::default(),
        }
    }
}

impl Drop for ChatMcp {
    fn drop(&mut self) {
        self.session.authority.revoke();
        if !self.keep_key {
            if let Some(gateway) = self.gateway.get() {
                gateway.revoke(&self.key);
            }
        }
    }
}
