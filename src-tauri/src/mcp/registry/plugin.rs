//! Ephemeral plugin registrations. Stable ids belong to the user-facing
//! server; tokens belong to one incarnation and prevent late cleanup races.
use super::*;
use serde_json::{json, Value};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PluginMcpScope {
    All(String),
    Workspaces {
        #[serde(rename = "workspaceIds")]
        workspace_ids: Vec<String>,
    },
}
impl PluginMcpScope {
    pub fn visible(&self, workspace: Option<&str>) -> bool {
        match self {
            Self::All(s) => s == "all",
            Self::Workspaces { workspace_ids } => {
                workspace.is_some_and(|id| workspace_ids.iter().any(|s| s == id))
            }
        }
    }
    fn overlaps(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::All(_), _) | (_, Self::All(_)) => true,
            (Self::Workspaces { workspace_ids: a }, Self::Workspaces { workspace_ids: b }) => {
                a.iter().any(|id| b.contains(id))
            }
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "transport", rename_all = "lowercase")]
pub enum PluginMcpTransport {
    Stdio {
        command: String,
        #[serde(default)]
        args: Vec<String>,
        #[serde(default)]
        env: HashMap<String, String>,
        cwd: Option<String>,
    },
    Http {
        url: String,
        #[serde(default)]
        headers: HashMap<String, String>,
    },
}
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginMcpServer {
    pub id: String,
    pub name: String,
    pub tool_prefix: Option<String>,
    pub scope: PluginMcpScope,
    #[serde(flatten)]
    pub transport: PluginMcpTransport,
}
#[derive(Clone)]
pub(super) struct PluginRegistration {
    pub plugin_id: String,
    pub epoch: u64,
    pub token: String,
    pub scope: PluginMcpScope,
    pub prefix: String,
}

pub fn plugin_server_id(plugin: &str, local: &str) -> String {
    format!("plugin:{plugin}:{local}")
}

pub(super) fn tool_collision(inner: &RegistryInner, id: &str, tools: &[McpTool]) -> Option<String> {
    let candidate = inner.plugins.get(id);
    let mut names = HashSet::new();
    for tool in tools {
        if !names.insert(&tool.prefixed_name) {
            return Some(format!("Duplicate MCP tool name: {}", tool.prefixed_name));
        }
    }
    for (other_id, handle) in &inner.handles {
        if other_id == id || !handle.status.is_running() {
            continue;
        }
        let other = inner.plugins.get(other_id);
        if candidate.is_none() && other.is_none() {
            continue;
        }
        if matches!((candidate, other), (Some(a), Some(b)) if !a.scope.overlaps(&b.scope)) {
            continue;
        }
        if let Some(tool) = handle
            .tools
            .iter()
            .find(|tool| names.contains(&tool.prefixed_name))
        {
            return Some(format!(
                "MCP tool name '{}' overlaps server {}",
                tool.prefixed_name, other_id
            ));
        }
    }
    None
}

impl PluginMcpServer {
    fn config(&self, plugin: &str) -> Result<McpServerConfig, String> {
        if !crate::plugins::valid_id(plugin) || !crate::plugins::valid_id(&self.id) {
            return Err(
                "Plugin and server ids must be lowercase letters, digits or dashes (1–64)".into(),
            );
        }
        if self.name.trim().is_empty() || self.name.len() > 128 {
            return Err("Server name must have 1–128 bytes".into());
        }
        let prefix = self.tool_prefix.as_deref().unwrap_or(plugin);
        if prefix.is_empty()
            || prefix.len() > 32
            || !prefix
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
        {
            return Err(
                "toolPrefix must have 1–32 ASCII letters, digits, underscores or dashes".into(),
            );
        }
        match &self.scope {
            PluginMcpScope::All(s) if s == "all" => {}
            PluginMcpScope::Workspaces { workspace_ids }
                if !workspace_ids.is_empty()
                    && workspace_ids.len() <= 256
                    && workspace_ids
                        .iter()
                        .all(|s| !s.is_empty() && s.len() <= 256) => {}
            _ => return Err("scope must be all or 1–256 workspace ids".into()),
        }
        let (command, args, env, transport, mut raw) = match &self.transport {
            PluginMcpTransport::Stdio {
                command,
                args,
                env,
                cwd,
            } => {
                if command.is_empty()
                    || command.len() > 4096
                    || command.contains('\0')
                    || args.len() > 256
                    || args.iter().any(|s| s.len() > 16384 || s.contains('\0'))
                {
                    return Err("Invalid command or args".into());
                }
                validate_map(env, false)?;
                if cwd.as_ref().is_some_and(|s| {
                    !std::path::Path::new(s).is_absolute() || s.len() > 4096 || s.contains('\0')
                }) {
                    return Err("cwd must be an absolute path".into());
                }
                (
                    command.clone(),
                    args.clone(),
                    env.clone(),
                    super::super::McpTransport::Stdio,
                    json!({"cwd":cwd}),
                )
            }
            PluginMcpTransport::Http { url, headers } => {
                let parsed =
                    reqwest::Url::parse(url).map_err(|e| format!("Invalid MCP URL: {e}"))?;
                if !matches!(parsed.scheme(), "http" | "https")
                    || parsed.host_str().is_none()
                    || url.len() > 8192
                    || !parsed.username().is_empty()
                    || parsed.password().is_some()
                    || parsed.fragment().is_some()
                {
                    return Err(
                        "MCP URL must be an explicit http(s) URL without credentials or fragment"
                            .into(),
                    );
                }
                validate_map(headers, true)?;
                (
                    url.clone(),
                    vec![],
                    HashMap::new(),
                    super::super::McpTransport::Http,
                    json!({"headers":headers}),
                )
            }
        };
        raw["pluginId"] = json!(plugin);
        raw["pluginToolPrefix"] = json!(prefix);
        Ok(McpServerConfig {
            id: plugin_server_id(plugin, &self.id),
            name: self.name.clone(),
            sources: vec![McpConfigSource::Plugin],
            command,
            args,
            env,
            disabled: false,
            transport,
            raw,
        })
    }
}
fn validate_map(values: &HashMap<String, String>, headers: bool) -> Result<(), String> {
    if values.len() > 128 || values.iter().map(|(k, v)| k.len() + v.len()).sum::<usize>() > 65536 {
        return Err("env/headers exceed 128 entries or 64 KiB".into());
    }
    for (key, value) in values {
        if key.is_empty()
            || key.len() > 256
            || value.len() > 16384
            || key.contains(['\0', '='])
            || value.contains('\0')
        {
            return Err("Invalid env/header entry".into());
        }
        if headers {
            reqwest::header::HeaderName::from_bytes(key.as_bytes()).map_err(|e| e.to_string())?;
            reqwest::header::HeaderValue::from_str(value).map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

impl McpRegistry {
    pub async fn register_plugin_server<R: Runtime>(
        &self,
        app: Option<&AppHandle<R>>,
        plugin: &str,
        token: String,
        server: PluginMcpServer,
    ) -> Result<(), String> {
        let epoch = self.plugin_epoch.load(std::sync::atomic::Ordering::SeqCst);
        let mut config = server.config(plugin)?;
        config.raw["pluginRegistrationToken"] = json!(token);
        if token.is_empty() || token.len() > 128 {
            return Err("Invalid registration token".into());
        }
        let _gate = self.prime_lock.lock().await;
        if self.plugin_epoch.load(std::sync::atomic::Ordering::SeqCst) != epoch {
            return Err("Plugin page reloaded during registration".into());
        }
        let prefix = server.tool_prefix.clone().unwrap_or_else(|| plugin.into());
        {
            let inner = self.inner.lock().await;
            for (id, registration) in &inner.plugins {
                if id != &config.id
                    && registration.prefix == prefix
                    && registration.scope.overlaps(&server.scope)
                {
                    return Err(format!("MCP tool prefix '{prefix}' overlaps server {id}"));
                }
            }
        }
        self.remove_plugin_server_inner(app, &config.id, None).await;
        self.inner.lock().await.plugins.insert(
            config.id.clone(),
            PluginRegistration {
                plugin_id: plugin.into(),
                epoch,
                token,
                scope: server.scope,
                prefix,
            },
        );
        // Keep disabled registrations discoverable and restartable too.
        self.inner.lock().await.handles.insert(
            config.id.clone(),
            McpServerHandle::discovered(config.clone()),
        );
        let registry = self.clone();
        let app = app.cloned();
        tokio::spawn(async move {
            registry.ensure_started(app.as_ref(), config).await;
        });
        Ok(())
    }
    pub async fn unregister_plugin_server<R: Runtime>(
        &self,
        app: Option<&AppHandle<R>>,
        plugin: &str,
        local: &str,
        token: &str,
    ) {
        let _gate = self.prime_lock.lock().await;
        self.remove_plugin_server_inner(app, &plugin_server_id(plugin, local), Some(token))
            .await;
    }
    async fn remove_plugin_server_inner<R: Runtime>(
        &self,
        app: Option<&AppHandle<R>>,
        id: &str,
        token: Option<&str>,
    ) {
        let mut handle = {
            let mut inner = self.inner.lock().await;
            if !inner
                .plugins
                .get(id)
                .is_some_and(|r| token.is_none_or(|t| r.token == t))
            {
                return;
            }
            inner.plugins.remove(id);
            if let Some(task) = inner.watchers.remove(id) {
                task.abort();
            }
            inner.cancel_calls.remove(id); // dropping sender cancels every dispatch
            inner.handles.remove(id)
        };
        if let Some(h) = handle.as_mut() {
            h.status = McpServerStatus::Stopped;
            h.tools.clear();
            emit_status_with_bus(app, &self.status_tx, &McpServerRuntime::from_handle(h));
            if let Some(child) = h.child.take() {
                let _ = child.shutdown().await;
            }
        }
    }
    /// Invalidate synchronously at document replacement, before scheduling
    /// cleanup. The old page's delayed work cannot delete new registrations.
    pub fn invalidate_plugin_page(&self) {
        self.plugin_epoch
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
    pub async fn clear_stale_plugin_servers<R: Runtime>(&self, app: Option<&AppHandle<R>>) {
        let _gate = self.prime_lock.lock().await;
        let epoch = self.plugin_epoch.load(std::sync::atomic::Ordering::SeqCst);
        let ids: Vec<_> = self
            .inner
            .lock()
            .await
            .plugins
            .iter()
            .filter(|(_, r)| r.epoch < epoch)
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            self.remove_plugin_server_inner(app, &id, None).await;
        }
    }
    pub async fn clear_plugin_servers<R: Runtime>(&self, app: Option<&AppHandle<R>>) {
        let _gate = self.prime_lock.lock().await;
        let ids: Vec<_> = self.inner.lock().await.plugins.keys().cloned().collect();
        for id in ids {
            self.remove_plugin_server_inner(app, &id, None).await;
        }
    }
    pub async fn plugin_configs(&self) -> Vec<McpServerConfig> {
        let inner = self.inner.lock().await;
        inner
            .plugins
            .keys()
            .filter_map(|id| inner.handles.get(id).map(|h| h.config.clone()))
            .collect()
    }
    pub async fn owned_plugin_server(
        &self,
        plugin: &str,
        local: &str,
        token: &str,
    ) -> Result<String, String> {
        let id = plugin_server_id(plugin, local);
        if self
            .inner
            .lock()
            .await
            .plugins
            .get(&id)
            .is_some_and(|r| r.plugin_id == plugin && r.token == token)
        {
            Ok(id)
        } else {
            Err("Plugin MCP registration is no longer active".into())
        }
    }
    pub async fn plugin_status(&self, id: &str) -> Option<McpServerRuntime> {
        self.inner
            .lock()
            .await
            .handles
            .get(id)
            .map(McpServerRuntime::from_handle)
    }
    pub async fn plugin_call(
        &self,
        id: &str,
        name: &str,
        arguments: Value,
    ) -> Result<Value, String> {
        let tool = self
            .list_tools_for_server(id)
            .await
            .into_iter()
            .find(|t| t.name == name)
            .ok_or("Unknown plugin tool")?;
        // The owner's UI may inspect its scoped server without a chat workspace.
        self.dispatch_tool_call_inner(&tool.prefixed_name, arguments, None, Some(id))
            .await
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginMcpActivity {
    pub plugin_id: String,
    pub server_id: String,
    pub phase: String,
    pub call_id: String,
    pub tool_name: String,
    pub prefixed_name: String,
    pub workspace_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_error: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}
pub(super) struct ActivityGuard {
    pub event: PluginMcpActivity,
    pub started: std::time::Instant,
    pub emit: Arc<dyn Fn(PluginMcpActivity) + Send + Sync>,
}
impl Drop for ActivityGuard {
    fn drop(&mut self) {
        self.event.phase = "end".into();
        self.event.is_error.get_or_insert(true);
        self.event.duration_ms =
            Some(self.started.elapsed().as_millis().min(u64::MAX as u128) as u64);
        (self.emit)(self.event.clone());
    }
}

pub const LIST_PLUGIN_TOOLS: &str = "codemux_list_plugin_tools";
pub const CALL_PLUGIN_TOOL: &str = "codemux_call_plugin_tool";

/// Stable entry points for clients whose per-thread tool schemas are fixed.
/// They are available before renderer plugins load and always consult the
/// current, caller-scoped catalog.
pub fn plugin_bridge_tools() -> Vec<McpTool> {
    vec![
        McpTool { name: LIST_PLUGIN_TOOLS.into(), prefixed_name: LIST_PLUGIN_TOOLS.into(), description: Some("List tools added by Codemux plugins (for example Unity). Tools can appear after this chat starts or after an Editor connects. Use this when a task needs plugin tools you cannot see directly. Returns current names, descriptions, input schemas, and whether each is directly callable. Then use codemux_call_plugin_tool with a returned name.".into()), input_schema: json!({"type":"object","properties":{},"additionalProperties":false}), server_id: "codemux-plugin-bridge".into() },
        McpTool { name: CALL_PLUGIN_TOOL.into(), prefixed_name: CALL_PLUGIN_TOOL.into(), description: Some("Call a Codemux plugin tool by the name returned by codemux_list_plugin_tools. Use that tool's input schema for arguments. Works for tools added after this chat started and tools already directly callable. If a tool is missing, list plugin tools again; the plugin or Editor may have connected or disconnected.".into()), input_schema: json!({"type":"object","properties":{"name":{"type":"string","description":"Exact current tool name from codemux_list_plugin_tools"},"arguments":{"type":"object","additionalProperties":true}},"required":["name","arguments"],"additionalProperties":false}), server_id: "codemux-plugin-bridge".into() },
    ]
}
impl McpRegistry {
    pub async fn call_plugin_bridge(
        &self,
        name: &str,
        arguments: Value,
        workspace: Option<&str>,
        direct: &[String],
    ) -> Option<Result<Value, String>> {
        match name {
            LIST_PLUGIN_TOOLS => {
                let tools: Vec<_> = self.list_tools_for_workspace(&[], workspace).await.into_iter().filter(|t| t.server_id.starts_with("plugin:")).map(|t| json!({"name":t.prefixed_name,"description":t.description,"inputSchema":t.input_schema,"directlyCallable":direct.contains(&t.prefixed_name)})).collect();
                let data = json!({"tools":tools});
                Some(Ok(
                    json!({"content":[{"type":"text","text":data.to_string()}],"structuredContent":data}),
                ))
            }
            CALL_PLUGIN_TOOL => {
                let Some(name) = arguments.get("name").and_then(Value::as_str) else {
                    return Some(Err("A plugin tool name is required".into()));
                };
                if !self
                    .list_tools_for_workspace(&[], workspace)
                    .await
                    .iter()
                    .any(|t| t.server_id.starts_with("plugin:") && t.prefixed_name == name)
                {
                    return Some(Err(
                        "Plugin tool is not available in this workspace; list plugin tools again"
                            .into(),
                    ));
                }
                let args = arguments
                    .get("arguments")
                    .cloned()
                    .unwrap_or_else(|| json!({}));
                Some(self.dispatch_tool_call(name, args, workspace).await)
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn server(scope: PluginMcpScope, args: Vec<String>) -> PluginMcpServer {
        PluginMcpServer {
            id: "fixture".into(),
            name: "Fixture".into(),
            tool_prefix: Some("fixture".into()),
            scope,
            transport: PluginMcpTransport::Stdio {
                command: "python3".into(),
                args: std::iter::once(format!(
                    "{}/tests/fixtures/plugin_mcp.py",
                    env!("CARGO_MANIFEST_DIR")
                ))
                .chain(args)
                .collect(),
                env: HashMap::from([("PLUGIN_FIXTURE_ENV".into(), "intact".into())]),
                cwd: Some(std::env::temp_dir().display().to_string()),
            },
        }
    }
    async fn wait_count(registry: &McpRegistry, count: usize) {
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if registry
                    .plugin_status("plugin:demo:fixture")
                    .await
                    .is_some_and(|r| r.status.is_running() && r.tools_count == count)
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("fixture reaches expected tool count");
    }
    #[tokio::test]
    async fn plugin_mcp_pagination_refresh_workspace_and_bridge() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("state");
        let registry = McpRegistry::new();
        let mut events = registry.subscribe_status();
        registry
            .register_plugin_server::<tauri::Wry>(
                None,
                "demo",
                "one".into(),
                server(
                    PluginMcpScope::Workspaces {
                        workspace_ids: vec!["a".into()],
                    },
                    vec![
                        "--empty".into(),
                        "--state".into(),
                        state.display().to_string(),
                    ],
                ),
            )
            .await
            .unwrap();
        wait_count(&registry, 0).await;
        std::fs::write(&state, "ready").unwrap();
        wait_count(&registry, 4).await;
        assert!(events.try_recv().is_ok());
        assert_eq!(
            registry
                .list_tools_for_workspace(&[], Some("a"))
                .await
                .len(),
            4
        );
        assert!(registry
            .list_tools_for_workspace(&[], Some("b"))
            .await
            .is_empty());
        assert!(registry
            .dispatch_tool_call("fixture__new_tool", json!({}), Some("b"))
            .await
            .is_err());
        let catalog = registry
            .call_plugin_bridge(
                LIST_PLUGIN_TOOLS,
                json!({}),
                Some("a"),
                &["fixture__echo".into()],
            )
            .await
            .unwrap()
            .unwrap();
        let tools = catalog["structuredContent"]["tools"].as_array().unwrap();
        assert!(tools
            .iter()
            .any(|t| t["name"] == "fixture__echo" && t["directlyCallable"] == true));
        let result = registry
            .call_plugin_bridge(
                CALL_PLUGIN_TOOL,
                json!({"name":"fixture__new_tool","arguments":{}}),
                Some("a"),
                &[],
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result["content"][0]["text"], "ORCHID-742");
        let image = registry
            .plugin_call("plugin:demo:fixture", "image", json!({}))
            .await
            .unwrap();
        assert_eq!(image["content"][0]["mimeType"], "image/png");
        assert!(image["content"][0]["data"]
            .as_str()
            .unwrap()
            .starts_with("iVBOR"));
        assert_eq!(image["structuredContent"]["intact"], true);
        let echo = registry
            .plugin_call("plugin:demo:fixture", "echo", json!({"ok":true}))
            .await
            .unwrap();
        let text: Value =
            serde_json::from_str(echo["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(text["env"], "intact");
        assert_eq!(text["cwd"], std::env::temp_dir().display().to_string());
        registry.shutdown_all().await;
    }
    #[tokio::test]
    async fn plugin_mcp_disposal_cancels_call_and_pairs_activity() {
        let registry = McpRegistry::new();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        registry.set_activity_emitter(Arc::new(move |e| {
            let _ = tx.send(e);
        }));
        registry
            .register_plugin_server::<tauri::Wry>(
                None,
                "demo",
                "one".into(),
                server(PluginMcpScope::All("all".into()), vec![]),
            )
            .await
            .unwrap();
        wait_count(&registry, 4).await;
        let r = registry.clone();
        let call =
            tokio::spawn(
                async move { r.dispatch_tool_call("fixture__slow", json!({}), None).await },
            );
        let start = rx.recv().await.unwrap();
        assert_eq!(start.phase, "start");
        registry
            .unregister_plugin_server::<tauri::Wry>(None, "demo", "fixture", "one")
            .await;
        assert!(tokio::time::timeout(Duration::from_secs(1), call)
            .await
            .unwrap()
            .unwrap()
            .is_err());
        let end = rx.recv().await.unwrap();
        assert_eq!(end.call_id, start.call_id);
        assert_eq!(end.phase, "end");
        assert_eq!(end.is_error, Some(true));
        assert!(rx.try_recv().is_err());
        assert!(registry.plugin_configs().await.is_empty());
        assert!(registry
            .dispatch_tool_call("fixture__slow", json!({}), None)
            .await
            .is_err());
    }
    #[tokio::test]
    async fn plugin_mcp_stable_disable_and_stale_disposer() {
        let registry = McpRegistry::new();
        registry
            .set_disabled_ids::<tauri::Wry>(None, vec!["plugin:demo:fixture".into()])
            .await
            .unwrap();
        for token in ["one", "two"] {
            registry
                .register_plugin_server::<tauri::Wry>(
                    None,
                    "demo",
                    token.into(),
                    server(PluginMcpScope::All("all".into()), vec![]),
                )
                .await
                .unwrap();
        }
        registry
            .unregister_plugin_server::<tauri::Wry>(None, "demo", "fixture", "one")
            .await;
        assert!(registry
            .owned_plugin_server("demo", "fixture", "two")
            .await
            .is_ok());
        assert!(registry
            .owned_plugin_server("other", "fixture", "two")
            .await
            .is_err());
        assert!(registry.is_disabled("plugin:demo:fixture").await);
        tokio::time::sleep(Duration::from_millis(80)).await;
        assert!(registry.list_all_tools().await.is_empty());
        registry.clear_plugin_servers::<tauri::Wry>(None).await;
    }
    #[tokio::test]
    async fn plugin_mcp_rejects_prefix_overlap_but_accepts_disjoint_workspaces() {
        let registry = McpRegistry::new();
        let a = PluginMcpScope::Workspaces {
            workspace_ids: vec!["a".into()],
        };
        registry
            .register_plugin_server::<tauri::Wry>(
                None,
                "demo",
                "one".into(),
                server(a.clone(), vec!["--empty".into()]),
            )
            .await
            .unwrap();
        let mut other = server(a, vec!["--empty".into()]);
        other.id = "other".into();
        assert!(registry
            .register_plugin_server::<tauri::Wry>(None, "demo", "two".into(), other.clone())
            .await
            .is_err());
        other.scope = PluginMcpScope::Workspaces {
            workspace_ids: vec!["b".into()],
        };
        registry
            .register_plugin_server::<tauri::Wry>(None, "demo", "two".into(), other)
            .await
            .unwrap();
        registry.clear_plugin_servers::<tauri::Wry>(None).await;
    }
    #[test]
    fn plugin_mcp_validation_and_names() {
        let scope = PluginMcpScope::All("all".into());
        let mut config = server(scope, vec![]);
        assert!(config.config("demo").is_ok());
        if let PluginMcpTransport::Stdio { cwd, .. } = &mut config.transport {
            *cwd = Some("relative".into());
        }
        assert!(config.config("demo").is_err());
        config.transport = PluginMcpTransport::Http {
            url: "file:///tmp/a".into(),
            headers: HashMap::new(),
        };
        assert!(config.config("demo").is_err());
        assert!(validate_map(&HashMap::from([("x".into(), "a".repeat(65537))]), true).is_err());
        let names = [
            "manage_scene".into(),
            "a tool/画像".into(),
            "x".repeat(500),
            format!("{}y", "x".repeat(500)),
        ];
        let safe: Vec<_> = names
            .iter()
            .map(|s| crate::mcp::runtime::plugin_tool_name("unity", s))
            .collect();
        assert_eq!(safe[0], "unity__manage_scene");
        for s in &safe {
            assert!(
                s.len() <= 64
                    && s.bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
            );
        }
        assert_ne!(safe[2], safe[3]);
        assert_eq!(
            safe[1],
            crate::mcp::runtime::plugin_tool_name("unity", &names[1])
        );
    }
}

#[cfg(test)]
mod cursor_tests {
    use super::*;
    #[tokio::test]
    async fn plugin_mcp_repeated_cursor_fails_instead_of_hanging() {
        let config = PluginMcpServer {
            id: "loop".into(),
            name: "Loop".into(),
            tool_prefix: None,
            scope: PluginMcpScope::All("all".into()),
            transport: PluginMcpTransport::Stdio {
                command: "python3".into(),
                args: vec![
                    format!(
                        "{}/tests/fixtures/plugin_mcp.py",
                        env!("CARGO_MANIFEST_DIR")
                    ),
                    "--loop-cursor".into(),
                ],
                env: HashMap::new(),
                cwd: None,
            },
        }
        .config("demo")
        .unwrap();
        let handle = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            crate::mcp::runtime::start_mcp_server(config),
        )
        .await
        .unwrap();
        assert!(
            matches!(handle.status, McpServerStatus::Errored { message } if message.contains("repeated cursor"))
        );
    }
}
