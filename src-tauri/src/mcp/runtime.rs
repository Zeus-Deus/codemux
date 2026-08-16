// MCP server child process runtime.
//
// Step 9 Stage 2 — spawns a stdio MCP child via the existing
// `JsonRpcChild` helper, walks the MCP 2024-11-05 handshake, queries
// `tools/list`, and returns a handle the registry stores. Headless: no
// agent integration here. Stage 3 wires Claude SDK to actually USE the
// tools registered through this path.
//
// Wire sequence (per MCP spec):
//   client → initialize         { protocolVersion, capabilities, clientInfo }
//   server → response           { protocolVersion, capabilities, serverInfo }
//   client → notifications/initialized
//   client → tools/list
//   server → response           { tools: [{ name, description?, inputSchema }] }
//
// Errors from any step short-circuit to `McpServerStatus::Errored` with
// the underlying message and (when available) the child's stderr tail.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::json_rpc_child::{JsonRpcChild, RpcChildError, SpawnConfig};

use super::{McpConfigSource, McpServerConfig, McpTransport};
use super::http_client::HttpMcpClient;

/// Per the spec — clients SHOULD send the highest version they support.
/// The Codemux MCP server (`mcp_server.rs:411`) speaks the same version,
/// so user-installed servers should agree.
pub const MCP_PROTOCOL_VERSION: &str = "2024-11-05";

/// Handshake budget. Generous because npx-launched servers may pull
/// dependencies on first run (network-bound).
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(60);

/// Default per-request timeout once the handshake completes. Stage 3
/// callers (`tools/call`) override this per call.
pub const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// Tool descriptor as stored in the registry. The unprefixed `name` is
/// what we send back to the server in `tools/call`; `prefixed_name` is
/// what the agent sees so collisions across MCP servers are impossible.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpTool {
    /// Raw name as the server returned it.
    pub name: String,
    /// `mcp__<server-id>__<name>`. Used as the agent-facing identifier.
    pub prefixed_name: String,
    pub description: Option<String>,
    pub input_schema: Value,
    /// `McpServerConfig.id` of the server this tool came from.
    pub server_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerInfo {
    pub name: String,
    pub version: String,
}

/// Lifecycle state. Mirrors what the Settings UI surfaces as a status
/// dot. `Errored` carries a short user-facing message; the registry
/// keeps the detailed stderr tail separately for tooltips.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", rename_all_fields = "camelCase")]
pub enum McpServerStatus {
    /// Config loaded, child not yet spawned. The default after parsing.
    Discovered,
    /// Child spawned, handshake in progress.
    Starting,
    /// `initialize` + `tools/list` succeeded; ready for `tools/call`.
    Running { tool_count: usize },
    /// Spawn or handshake failed. `message` is short; `stderr_tail` (on
    /// `McpServerHandle`) carries the details.
    Errored { message: String },
    /// Gracefully shut down via `stop_server`.
    Stopped,
}

impl McpServerStatus {
    pub fn is_running(&self) -> bool {
        matches!(self, McpServerStatus::Running { .. })
    }
}

/// One running (or once-running) MCP child plus everything we need to
/// dispatch tool calls and surface UI state. Handles are kept in
/// `McpRegistry`'s map keyed by `config.id`.
pub struct McpServerHandle {
    pub config: McpServerConfig,
    pub status: McpServerStatus,
    pub tools: Vec<McpTool>,
    pub server_info: Option<McpServerInfo>,
    /// epoch-ms when this handle entered `Running`. `None` while the
    /// handshake is still in flight or after errors.
    pub started_at_ms: Option<i64>,
    /// `Some` when the child is alive. `None` after `stop_server` or
    /// when the spawn itself failed.
    pub child: Option<Arc<JsonRpcChild>>,
    /// Streamable-HTTP client for remote MCP servers.
    pub remote: Option<Arc<HttpMcpClient>>,
    /// Last 8 KB of the child's stderr captured before exit (or while
    /// alive when JsonRpcChild surfaces it via ChildExited). Used for
    /// the Settings-row error tooltip.
    pub stderr_tail: Option<String>,
}

impl McpServerHandle {
    pub fn discovered(config: McpServerConfig) -> Self {
        Self {
            config,
            status: McpServerStatus::Discovered,
            tools: Vec::new(),
            server_info: None,
            started_at_ms: None,
            child: None,
            remote: None,
            stderr_tail: None,
        }
    }
}

/// Spawn a single MCP server and walk the full handshake. On success the
/// returned handle has `status = Running { tool_count }` and `tools`
/// populated. On failure status is `Errored { message }` and `child` is
/// `None` (we drop the child handle so the watchdog reaps it).
///
pub async fn start_mcp_server(config: McpServerConfig) -> McpServerHandle {
    if matches!(config.transport, McpTransport::Http) {
        return start_http_mcp_server(config).await;
    }

    if config.command.is_empty() {
        return McpServerHandle {
            stderr_tail: None,
            child: None,
            remote: None,
            tools: Vec::new(),
            server_info: None,
            started_at_ms: None,
            status: McpServerStatus::Errored {
                message: "no command in config".into(),
            },
            config,
        };
    }

    // Build SpawnConfig. We deliberately don't sanitize the GUI env because
    // MCP servers are the user's own installs and may legitimately need
    // DISPLAY etc. for headed use cases. If this becomes a problem the
    // registry can grow a per-server option.
    let spawn = SpawnConfig {
        program: PathBuf::from(&config.command),
        args: config.args.clone(),
        env: config.env.clone(),
        cwd: None,
        default_timeout: DEFAULT_REQUEST_TIMEOUT,
    };

    let child = match JsonRpcChild::spawn(spawn).await {
        Ok(c) => Arc::new(c),
        Err(err) => {
            return McpServerHandle {
                stderr_tail: stderr_tail_from(&err),
                child: None,
                remote: None,
                tools: Vec::new(),
                server_info: None,
                started_at_ms: None,
                status: McpServerStatus::Errored {
                    message: format!("spawn failed: {err}"),
                },
                config,
            };
        }
    };

    match handshake(&child, &config).await {
        Ok((server_info, tools)) => {
            let tool_count = tools.len();
            McpServerHandle {
                config,
                status: McpServerStatus::Running { tool_count },
                tools,
                server_info: Some(server_info),
                started_at_ms: Some(now_ms()),
                child: Some(child),
                remote: None,
                stderr_tail: None,
            }
        }
        Err(err) => {
            // Tear down the child so its watchdog reaps the process.
            let stderr_tail = stderr_tail_from(&err);
            let child_clone = Arc::clone(&child);
            tokio::spawn(async move {
                let _ = child_clone.shutdown().await;
            });
            McpServerHandle {
                stderr_tail,
                child: None,
                remote: None,
                tools: Vec::new(),
                server_info: None,
                started_at_ms: None,
                status: McpServerStatus::Errored {
                    message: format!("handshake failed: {err}"),
                },
                config,
            }
        }
    }
}

async fn start_http_mcp_server(config: McpServerConfig) -> McpServerHandle {
    if config.command.is_empty() {
        return McpServerHandle {
            stderr_tail: None,
            child: None,
            remote: None,
            tools: Vec::new(),
            server_info: None,
            started_at_ms: None,
            status: McpServerStatus::Errored {
                message: "no URL in config".into(),
            },
            config,
        };
    }
    let connected = tokio::time::timeout(HANDSHAKE_TIMEOUT, HttpMcpClient::connect(&config)).await;
    match connected {
        Err(_) => McpServerHandle {
            stderr_tail: None,
            child: None,
            remote: None,
            tools: Vec::new(),
            server_info: None,
            started_at_ms: None,
            status: McpServerStatus::Errored {
                message: "handshake timed out".into(),
            },
            config,
        },
        Ok(Ok(client)) => {
            let client = Arc::new(client);
            match tokio::time::timeout(
                HANDSHAKE_TIMEOUT,
                client.request("tools/list", json!({})),
            )
            .await
            {
                Ok(Ok(response)) => {
                    let tools = parse_tools_list(&response, &config.id, &config.name);
                    let tool_count = tools.len();
                    McpServerHandle {
                        server_info: Some(McpServerInfo {
                            name: config.name.clone(),
                            version: String::new(),
                        }),
                        config,
                        status: McpServerStatus::Running { tool_count },
                        tools,
                        started_at_ms: Some(now_ms()),
                        child: None,
                        remote: Some(client),
                        stderr_tail: None,
                    }
                }
                Ok(Err(error)) => McpServerHandle {
                    stderr_tail: None,
                    child: None,
                    remote: None,
                    tools: Vec::new(),
                    server_info: None,
                    started_at_ms: None,
                    status: McpServerStatus::Errored {
                        message: format!("tools/list failed: {error}"),
                    },
                    config,
                },
                Err(_) => McpServerHandle {
                    stderr_tail: None,
                    child: None,
                    remote: None,
                    tools: Vec::new(),
                    server_info: None,
                    started_at_ms: None,
                    status: McpServerStatus::Errored {
                        message: "tools/list timed out".into(),
                    },
                    config,
                },
            }
        }
        Ok(Err(error)) => McpServerHandle {
            stderr_tail: None,
            child: None,
            remote: None,
            tools: Vec::new(),
            server_info: None,
            started_at_ms: None,
            status: McpServerStatus::Errored {
                message: format!("handshake failed: {error}"),
            },
            config,
        },
    }
}

async fn handshake(
    child: &Arc<JsonRpcChild>,
    config: &McpServerConfig,
) -> Result<(McpServerInfo, Vec<McpTool>), RpcChildError> {
    // 1. initialize
    let init_response = child
        .request_with_timeout(
            "initialize",
            json!({
                "protocolVersion": MCP_PROTOCOL_VERSION,
                "capabilities": {},
                "clientInfo": {
                    "name": "codemux",
                    "version": env!("CARGO_PKG_VERSION"),
                },
            }),
            HANDSHAKE_TIMEOUT,
        )
        .await?;

    let server_info = parse_server_info(&init_response);

    // 2. notifications/initialized — fire-and-forget; spec REQUIRES it
    //    before any further requests.
    child.notify("notifications/initialized", json!({})).await?;

    // 3. tools/list — paginated per spec but we read one page for v1.
    //    Servers with > 100 tools paginate via `nextCursor` which we
    //    deliberately don't follow yet (no real-world server has needed
    //    a second page so far).
    let list_response = child
        .request_with_timeout("tools/list", json!({}), HANDSHAKE_TIMEOUT)
        .await?;

    let tools = parse_tools_list(&list_response, &config.id, &config.name);

    Ok((server_info, tools))
}

fn parse_server_info(response: &Value) -> McpServerInfo {
    let info = response.get("serverInfo");
    McpServerInfo {
        name: info
            .and_then(|i| i.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        version: info
            .and_then(|i| i.get("version"))
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
    }
}

fn parse_tools_list(response: &Value, server_id: &str, server_name: &str) -> Vec<McpTool> {
    let arr = match response.get("tools").and_then(Value::as_array) {
        Some(a) => a,
        None => return Vec::new(),
    };

    let mut out: Vec<McpTool> = Vec::with_capacity(arr.len());
    for raw in arr {
        let obj = match raw.as_object() {
            Some(o) => o,
            None => continue,
        };
        let name = match obj.get("name").and_then(Value::as_str) {
            Some(n) if !n.is_empty() => n.to_string(),
            _ => continue,
        };
        let description = obj
            .get("description")
            .and_then(Value::as_str)
            .map(str::to_string);
        let input_schema = obj
            .get("inputSchema")
            .cloned()
            .unwrap_or_else(|| json!({"type": "object"}));

        out.push(McpTool {
            prefixed_name: prefix_tool_name(server_name, &name),
            name,
            description,
            input_schema,
            server_id: server_id.to_string(),
        });
    }
    out
}

/// `mcp__<server>__<tool>` — same convention the Anthropic SDK uses
/// internally so approval rules persisted by the SDK match what
/// Codemux exposes.
pub fn prefix_tool_name(server_name: &str, tool_name: &str) -> String {
    format!("mcp__{}__{}", server_name, tool_name)
}

/// Aggregate enabled servers' tools across the whole registry. The 50
/// cap per `step-9` research lives at the registry layer; here we
/// just yield everything we have.
pub fn aggregate_tools(handles: &HashMap<String, McpServerHandle>) -> Vec<McpTool> {
    let mut out = Vec::new();
    for h in handles.values() {
        if h.status.is_running() {
            out.extend(h.tools.iter().cloned());
        }
    }
    out
}

fn stderr_tail_from(err: &RpcChildError) -> Option<String> {
    if let RpcChildError::ChildExited { stderr_tail, .. } = err {
        if !stderr_tail.is_empty() {
            return Some(stderr_tail.clone());
        }
    }
    None
}

fn now_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

// ── Helper: resolve a non-Codemux source name to a stable id key for
//    the prefix. We use the user-facing alias (e.g. "github") so the
//    SDK's persisted approval rules remain readable.
pub fn server_alias_for_prefix(config: &McpServerConfig) -> &str {
    let _ = McpConfigSource::Codemux; // silence unused-import lint when sources change
    &config.name
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_config(id: &str, name: &str, transport: McpTransport) -> McpServerConfig {
        McpServerConfig {
            id: id.into(),
            name: name.into(),
            sources: vec![McpConfigSource::CodemuxUser],
            command: "/bin/true".into(),
            args: Vec::new(),
            env: HashMap::new(),
            disabled: false,
            transport,
            raw: Value::Null,
        }
    }

    #[test]
    fn prefix_tool_name_format() {
        assert_eq!(prefix_tool_name("github", "create_issue"), "mcp__github__create_issue");
        assert_eq!(prefix_tool_name("codemux", "browser_click"), "mcp__codemux__browser_click");
    }

    #[test]
    fn parse_tools_list_handles_missing_fields() {
        let response = json!({
            "tools": [
                { "name": "search", "description": "Search.", "inputSchema": {"type": "object"} },
                { "name": "noschema" },
                { "no_name_field": true },
                { "name": "" }
            ]
        });
        let tools = parse_tools_list(&response, "srv-1", "github");
        // Two valid: "search" and "noschema". The empty-name and
        // no-name entries are dropped.
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].prefixed_name, "mcp__github__search");
        assert_eq!(tools[0].description.as_deref(), Some("Search."));
        assert_eq!(tools[1].prefixed_name, "mcp__github__noschema");
        assert!(tools[1].description.is_none());
        assert_eq!(tools[1].input_schema, json!({"type": "object"}));
    }

    #[test]
    fn parse_tools_list_returns_empty_when_no_tools_array() {
        let response = json!({});
        let tools = parse_tools_list(&response, "id", "n");
        assert!(tools.is_empty());
    }

    #[test]
    fn parse_server_info_defaults_to_empty_strings() {
        let response = json!({});
        let info = parse_server_info(&response);
        assert_eq!(info.name, "");
        assert_eq!(info.version, "");
    }

    #[test]
    fn parse_server_info_extracts_name_and_version() {
        let response = json!({
            "serverInfo": { "name": "test-server", "version": "1.2.3" }
        });
        let info = parse_server_info(&response);
        assert_eq!(info.name, "test-server");
        assert_eq!(info.version, "1.2.3");
    }

    #[tokio::test]
    async fn http_transport_with_invalid_url_has_clear_error() {
        let cfg = make_config("h1", "linear", McpTransport::Http);
        let h = start_mcp_server(cfg).await;
        match h.status {
            McpServerStatus::Errored { message } => {
                assert!(message.contains("HTTP"));
            }
            _ => panic!("expected Errored, got {:?}", h.status),
        }
        assert!(h.child.is_none());
        assert!(h.remote.is_none());
    }

    #[tokio::test]
    async fn empty_command_is_rejected_before_spawn() {
        let mut cfg = make_config("e1", "empty", McpTransport::Stdio);
        cfg.command = String::new();
        let h = start_mcp_server(cfg).await;
        assert!(matches!(h.status, McpServerStatus::Errored { .. }));
        assert!(h.child.is_none());
    }

    #[tokio::test]
    async fn nonexistent_command_yields_errored() {
        let mut cfg = make_config("ne", "nope", McpTransport::Stdio);
        cfg.command = "/this/path/does/not/exist/codemux-mcp".into();
        let h = start_mcp_server(cfg).await;
        assert!(matches!(h.status, McpServerStatus::Errored { .. }));
        assert!(h.child.is_none());
    }

    #[test]
    fn discovered_handle_has_no_child() {
        let cfg = make_config("d1", "n", McpTransport::Stdio);
        let h = McpServerHandle::discovered(cfg);
        assert!(matches!(h.status, McpServerStatus::Discovered));
        assert!(h.child.is_none());
        assert!(h.tools.is_empty());
    }

    #[test]
    fn aggregate_skips_non_running_servers() {
        let mut handles: HashMap<String, McpServerHandle> = HashMap::new();
        let mut h1 = McpServerHandle::discovered(make_config("a", "a", McpTransport::Stdio));
        h1.status = McpServerStatus::Running { tool_count: 1 };
        h1.tools.push(McpTool {
            name: "t".into(),
            prefixed_name: "mcp__a__t".into(),
            description: None,
            input_schema: json!({}),
            server_id: "a".into(),
        });
        handles.insert("a".into(), h1);

        let mut h2 = McpServerHandle::discovered(make_config("b", "b", McpTransport::Stdio));
        h2.status = McpServerStatus::Errored { message: "x".into() };
        // Even with phantom tools, errored servers contribute nothing.
        h2.tools.push(McpTool {
            name: "phantom".into(),
            prefixed_name: "mcp__b__phantom".into(),
            description: None,
            input_schema: json!({}),
            server_id: "b".into(),
        });
        handles.insert("b".into(), h2);

        let agg = aggregate_tools(&handles);
        assert_eq!(agg.len(), 1);
        assert_eq!(agg[0].prefixed_name, "mcp__a__t");
    }

}
