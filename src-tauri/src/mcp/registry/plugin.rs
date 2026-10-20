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
        if matches!(self, Self::All(s) if s == "plugin")
            || matches!(other, Self::All(s) if s == "plugin")
        {
            return false;
        }
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
pub struct PluginMcpToolDescription {
    pub append: Option<String>,
    pub replace: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PluginMcpShutdown {
    #[default]
    Graceful,
    Immediate,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginMcpServer {
    pub id: String,
    pub name: String,
    pub tool_prefix: Option<String>,
    #[serde(default)]
    pub tool_descriptions: HashMap<String, PluginMcpToolDescription>,
    pub scope: PluginMcpScope,
    #[serde(default)]
    pub shutdown: PluginMcpShutdown,
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

fn prefix_collision(
    inner: &RegistryInner,
    id: &str,
    prefix: &str,
    scope: &PluginMcpScope,
) -> Option<String> {
    inner
        .plugins
        .iter()
        .find(|(other_id, registration)| {
            other_id.as_str() != id
                && registration.prefix == prefix
                && registration.scope.overlaps(scope)
        })
        .map(|(other_id, _)| format!("MCP tool prefix '{prefix}' overlaps server {other_id}"))
}

pub(super) fn tool_collision(
    inner: &RegistryInner,
    id: &str,
    tools: &[McpTool],
    host_tools: &[McpTool],
) -> Option<String> {
    let candidate = inner.plugins.get(id);
    let mut names = HashSet::new();
    for tool in tools {
        if candidate.is_some()
            && (matches!(
                tool.prefixed_name.as_str(),
                LIST_PLUGIN_TOOLS | CALL_PLUGIN_TOOL
            ) || tool.prefixed_name.starts_with("mcp__codemux__")
                || host_tools
                    .iter()
                    .any(|host| host.prefixed_name == tool.prefixed_name))
        {
            return Some(format!(
                "MCP tool name '{}' is reserved by Codemux",
                tool.prefixed_name
            ));
        }
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
        if prefix.eq_ignore_ascii_case("mcp") || prefix.to_ascii_lowercase().starts_with("codemux")
        {
            return Err("toolPrefix uses a reserved Codemux namespace".into());
        }
        match &self.scope {
            PluginMcpScope::All(s) if s == "all" || s == "plugin" => {}
            PluginMcpScope::Workspaces { workspace_ids }
                if !workspace_ids.is_empty()
                    && workspace_ids.len() <= 256
                    && workspace_ids
                        .iter()
                        .all(|s| !s.is_empty() && s.len() <= 256) => {}
            _ => return Err("scope must be all, plugin, or 1–256 workspace ids".into()),
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
                if parsed.scheme() == "http" && !super::super::http_client::loopback_url(url) {
                    return Err("MCP HTTP URLs require HTTPS unless the host is loopback".into());
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
        if self.tool_descriptions.len() > 128
            || self
                .tool_descriptions
                .iter()
                .map(|(name, value)| {
                    name.len()
                        + value.append.as_ref().map_or(0, String::len)
                        + value.replace.as_ref().map_or(0, String::len)
                })
                .sum::<usize>()
                > 16384
            || self.tool_descriptions.iter().any(|(name, value)| {
                name.is_empty()
                    || name.len() > 512
                    || value.append.as_ref().is_some_and(|s| s.len() > 2000)
                    || value.replace.as_ref().is_some_and(|s| s.len() > 2000)
            })
        {
            return Err(
                "toolDescriptions exceed 128 tools, 2000 bytes per text or 16 KiB total".into(),
            );
        }
        raw["pluginToolDescriptions"] = json!(self
            .tool_descriptions
            .iter()
            .map(|(name, value)| (name, json!({"append":value.append,"replace":value.replace})))
            .collect::<HashMap<_, _>>());
        raw["pluginImmediateShutdown"] =
            json!(matches!(self.shutdown, PluginMcpShutdown::Immediate));
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
    #[cfg(test)]
    pub async fn register_plugin_server<R: Runtime>(
        &self,
        app: Option<&AppHandle<R>>,
        plugin: &str,
        token: String,
        server: PluginMcpServer,
    ) -> Result<(), String> {
        self.register_plugin_server_for_page(app, self.plugin_page_epoch(), plugin, token, server)
            .await
    }
    pub fn plugin_page_epoch(&self) -> u64 {
        self.plugin_epoch.load(std::sync::atomic::Ordering::SeqCst)
    }
    pub async fn register_plugin_server_for_page<R: Runtime>(
        &self,
        app: Option<&AppHandle<R>>,
        epoch: u64,
        plugin: &str,
        token: String,
        server: PluginMcpServer,
    ) -> Result<(), String> {
        let config = server.config(plugin)?;
        if token.is_empty() || token.len() > 128 {
            return Err("Invalid registration token".into());
        }
        let server_lock = self.server_lock(&config.id);
        let _gate = server_lock.lock().await;
        if self.plugin_epoch.load(std::sync::atomic::Ordering::SeqCst) != epoch {
            return Err("Plugin page reloaded during registration".into());
        }
        let prefix = server.tool_prefix.clone().unwrap_or_else(|| plugin.into());
        {
            let inner = self.inner.lock().await;
            if let Some(error) = prefix_collision(&inner, &config.id, &prefix, &server.scope) {
                return Err(error);
            }
        }
        self.remove_plugin_server_locked(app, &config.id, None)
            .await?;
        {
            let mut inner = self.inner.lock().await;
            if self.plugin_epoch.load(std::sync::atomic::Ordering::SeqCst) != epoch {
                return Err("Plugin page reloaded during registration".into());
            }
            if let Some(error) = prefix_collision(&inner, &config.id, &prefix, &server.scope) {
                return Err(error);
            }
            inner.plugins.insert(
                config.id.clone(),
                PluginRegistration {
                    plugin_id: plugin.into(),
                    epoch,
                    token: token.clone(),
                    scope: server.scope,
                    prefix,
                },
            );
            // Keep disabled registrations discoverable and restartable too.
            inner.handles.insert(
                config.id.clone(),
                McpServerHandle::discovered(config.clone()),
            );
        }
        let registry = self.clone();
        let app = app.cloned();
        tokio::spawn(async move {
            registry
                .ensure_started_owned(app.as_ref(), config, Some(&token))
                .await;
        });
        Ok(())
    }
    pub async fn unregister_plugin_server<R: Runtime>(
        &self,
        app: Option<&AppHandle<R>>,
        plugin: &str,
        local: &str,
        token: &str,
    ) -> Result<(), String> {
        self.remove_plugin_server_inner(app, &plugin_server_id(plugin, local), Some(token))
            .await
    }
    async fn remove_plugin_server_inner<R: Runtime>(
        &self,
        app: Option<&AppHandle<R>>,
        id: &str,
        token: Option<&str>,
    ) -> Result<(), String> {
        let server_lock = self.server_lock(id);
        let _gate = server_lock.lock().await;
        self.remove_plugin_server_locked(app, id, token).await
    }
    async fn remove_plugin_server_locked<R: Runtime>(
        &self,
        app: Option<&AppHandle<R>>,
        id: &str,
        token: Option<&str>,
    ) -> Result<(), String> {
        if !self
            .inner
            .lock()
            .await
            .plugins
            .get(id)
            .is_some_and(|r| token.is_none_or(|t| r.token == t))
        {
            return Ok(());
        }
        self.stop_server_locked(app, id).await?;
        let mut inner = self.inner.lock().await;
        inner.plugins.remove(id);
        inner.handles.remove(id);
        Ok(())
    }
    /// Invalidate synchronously at document replacement, before scheduling
    /// cleanup. The old page's delayed work cannot delete new registrations.
    pub fn invalidate_plugin_page(&self) {
        self.plugin_epoch
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
    pub async fn clear_stale_plugin_servers<R: Runtime>(&self, app: Option<&AppHandle<R>>) {
        let epoch = self.plugin_epoch.load(std::sync::atomic::Ordering::SeqCst);
        let ids: Vec<_> = self
            .inner
            .lock()
            .await
            .plugins
            .iter()
            .filter(|(_, r)| r.epoch < epoch)
            .map(|(id, registration)| (id.clone(), registration.token.clone()))
            .collect();
        for (id, token) in ids {
            if let Err(error) = self
                .remove_plugin_server_inner(app, &id, Some(&token))
                .await
            {
                eprintln!("[plugins] MCP cleanup failed: {error}");
            }
        }
    }
    pub async fn clear_plugin_servers<R: Runtime>(&self, app: Option<&AppHandle<R>>) {
        let ids: Vec<_> = self.inner.lock().await.plugins.keys().cloned().collect();
        for id in ids {
            if let Err(error) = self.remove_plugin_server_inner(app, &id, None).await {
                eprintln!("[plugins] MCP cleanup failed: {error}");
            }
        }
    }
    pub async fn plugin_configs(&self) -> Vec<McpServerConfig> {
        let inner = self.inner.lock().await;
        inner
            .plugins
            .keys()
            .filter_map(|id| inner.handles.get(id).map(|h| h.config.clone().redacted()))
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

#[derive(Default)]
pub(super) struct CallTracker {
    count: std::sync::atomic::AtomicUsize,
    changed: tokio::sync::Notify,
}
impl CallTracker {
    pub async fn wait_idle(&self) {
        loop {
            let notified = self.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if self.count.load(std::sync::atomic::Ordering::Acquire) == 0 {
                return;
            }
            notified.await;
        }
    }
}
pub(super) struct CallGuard {
    tracker: Arc<CallTracker>,
    pub activity: Option<ActivityGuard>,
}
impl CallGuard {
    pub fn new(tracker: Arc<CallTracker>, activity: Option<ActivityGuard>) -> Self {
        tracker
            .count
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        Self { tracker, activity }
    }
}
impl Drop for CallGuard {
    fn drop(&mut self) {
        drop(self.activity.take());
        self.tracker
            .count
            .fetch_sub(1, std::sync::atomic::Ordering::AcqRel);
        self.tracker.changed.notify_waiters();
    }
}

pub const LIST_PLUGIN_TOOLS: &str = "codemux_list_plugin_tools";
pub const CALL_PLUGIN_TOOL: &str = "codemux_call_plugin_tool";

/// A short provider-side reminder, never a transcript message. Descriptions
/// are flattened and bounded; arguments and results are never included.
pub const PLUGIN_NOTICE_START: &str = "<codemux_plugin_tools_notice>";
pub const PLUGIN_NOTICE_END: &str = "</codemux_plugin_tools_notice>";

pub fn strip_plugin_catalog_notice(text: &str) -> String {
    if let Some(rest) = text.trim_start().strip_prefix(PLUGIN_NOTICE_START) {
        if let Some((_, user_text)) = rest.split_once(PLUGIN_NOTICE_END) {
            return user_text.trim_start_matches('\n').to_owned();
        }
    }
    text.to_owned()
}

pub fn plugin_catalog_notice(tools: &[McpTool]) -> String {
    let mut sorted: Vec<_> = tools.iter().collect();
    sorted.sort_by(|a, b| a.prefixed_name.cmp(&b.prefixed_name));
    let entries: Vec<_> = sorted
        .iter()
        .take(8)
        .map(|tool| {
            let description: String = tool
                .description
                .as_deref()
                .unwrap_or("")
                .split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .replace('<', "‹")
                .replace('>', "›")
                .chars()
                .take(120)
                .collect();
            format!("{}: {}", tool.prefixed_name, description)
        })
        .collect();
    format!("{PLUGIN_NOTICE_START}\nCodemux: plugin tools changed. Current tools ({} total): {}. For tools absent from your direct tools, use codemux_list_plugin_tools for schemas, then codemux_call_plugin_tool by name. These tools are available for integration tasks now.\n{PLUGIN_NOTICE_END}", tools.len(), if entries.is_empty() {"none".into()} else {entries.join("; ")})
}

pub fn plugin_catalog_key(tools: &[McpTool]) -> String {
    let mut sorted = tools.to_vec();
    sorted.sort_by(|a, b| a.prefixed_name.cmp(&b.prefixed_name));
    use sha2::{Digest, Sha256};
    format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&sorted).unwrap_or_default())
    )
}

/// Stable entry points for clients whose per-thread tool schemas are fixed.
/// They are available before renderer plugins load and always consult the
/// current, caller-scoped catalog.
pub fn plugin_bridge_tools() -> Vec<McpTool> {
    vec![
        McpTool { name: LIST_PLUGIN_TOOLS.into(), prefixed_name: LIST_PLUGIN_TOOLS.into(), description: Some("List tools added by Codemux plugins (for example Unity). Tools can appear after this chat starts or DURING a turn (for example after starting an Editor). Use this when a plugin task needs tools you cannot see directly or ToolSearch cannot find them. A connecting integration may temporarily return an empty list; retry when it connects. Returns current names, descriptions, input schemas, and whether each is directly callable. Then use codemux_call_plugin_tool with a returned name.".into()), input_schema: json!({"type":"object","properties":{},"additionalProperties":false}), server_id: "codemux-plugin-bridge".into() },
        McpTool { name: CALL_PLUGIN_TOOL.into(), prefixed_name: CALL_PLUGIN_TOOL.into(), description: Some("Call a Codemux plugin tool by the name returned by codemux_list_plugin_tools. Use that tool's input schema for arguments. Works for tools added after this chat started and tools already directly callable. If a tool is missing, list plugin tools again; the plugin or Editor may have connected or disconnected.".into()), input_schema: json!({"type":"object","properties":{"name":{"type":"string","description":"Exact current tool name from codemux_list_plugin_tools"},"arguments":{"type":"object","additionalProperties":true}},"required":["name","arguments"],"additionalProperties":false}), server_id: "codemux-plugin-bridge".into() },
    ]
}
impl McpRegistry {
    pub async fn has_visible_plugin_servers(&self, workspace: Option<&str>) -> bool {
        let inner = self.inner.lock().await;
        inner.plugins.iter().any(|(id, registration)| {
            registration.scope.visible(workspace) && !inner.disabled_ids.contains(id)
        })
    }
    pub async fn plugin_catalog(&self, workspace: Option<&str>) -> Vec<McpTool> {
        self.list_tools_for_workspace(&[], workspace)
            .await
            .into_iter()
            .filter(|tool| tool.server_id.starts_with("plugin:"))
            .collect()
    }
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
            tool_descriptions: HashMap::new(),
            shutdown: PluginMcpShutdown::Graceful,
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
    async fn plugin_mcp_review_old_page_cannot_register_after_invalidation() {
        let registry = McpRegistry::new();
        let old_epoch = registry.plugin_page_epoch();
        registry.invalidate_plugin_page();
        registry
            .clear_stale_plugin_servers::<tauri::Wry>(None)
            .await;
        let rejected = registry
            .register_plugin_server_for_page::<tauri::Wry>(
                None,
                old_epoch,
                "demo",
                "old-page".into(),
                server(PluginMcpScope::All("all".into()), vec![]),
            )
            .await
            .is_err();
        registry.clear_plugin_servers::<tauri::Wry>(None).await;
        assert!(rejected, "old-page registration survived invalidation");
        assert!(registry.plugin_configs().await.is_empty());
        registry
            .register_plugin_server::<tauri::Wry>(
                None,
                "demo",
                "new-page".into(),
                server(PluginMcpScope::All("all".into()), vec![]),
            )
            .await
            .unwrap();
        wait_count(&registry, 4).await;
        registry.shutdown_all().await;
    }
    #[tokio::test]
    async fn plugin_mcp_review_slow_stop_does_not_block_another_server() {
        let registry = McpRegistry::new();
        registry
            .register_plugin_server::<tauri::Wry>(
                None,
                "demo",
                "one".into(),
                server(
                    PluginMcpScope::All("all".into()),
                    vec!["--ignore-eof".into()],
                ),
            )
            .await
            .unwrap();
        wait_count(&registry, 4).await;
        let r = registry.clone();
        let stop = tokio::spawn(async move {
            r.unregister_plugin_server::<tauri::Wry>(None, "demo", "fixture", "one")
                .await
        });
        loop {
            if registry
                .plugin_status("plugin:demo:fixture")
                .await
                .is_some_and(|row| matches!(row.status, McpServerStatus::Stopping))
            {
                break;
            }
            tokio::task::yield_now().await;
        }
        let mut other = server(PluginMcpScope::All("all".into()), vec![]);
        other.id = "other".into();
        other.tool_prefix = Some("other".into());
        let result = tokio::time::timeout(Duration::from_millis(500), async {
            registry
                .register_plugin_server::<tauri::Wry>(None, "demo", "two".into(), other)
                .await
                .unwrap();
            while !registry
                .plugin_status("plugin:demo:other")
                .await
                .is_some_and(|row| row.status.is_running())
            {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        stop.await.unwrap().unwrap();
        registry.shutdown_all().await;
        assert!(result.is_ok(), "unrelated server waited for graceful stop");
    }
    #[tokio::test]
    async fn plugin_mcp_review_listing_redacts_secrets() {
        let registry = McpRegistry::new();
        registry
            .set_disabled_ids::<tauri::Wry>(None, vec!["plugin:demo:fixture".into()])
            .await
            .unwrap();
        registry
            .register_plugin_server::<tauri::Wry>(
                None,
                "demo",
                "private-token".into(),
                server(PluginMcpScope::All("all".into()), vec!["arg-secret".into()]),
            )
            .await
            .unwrap();
        let output = serde_json::to_string(&registry.plugin_configs().await).unwrap();
        let native = registry.inner.lock().await.handles["plugin:demo:fixture"]
            .config
            .clone();
        assert!(native.raw.get("pluginRegistrationToken").is_none());
        let mut http = server(PluginMcpScope::All("all".into()), vec![]);
        http.id = "http".into();
        http.tool_prefix = Some("httpfixture".into());
        http.transport = PluginMcpTransport::Http {
            url: "https://example.invalid/mcp?token=query-secret".into(),
            headers: HashMap::from([("authorization".into(), "header-secret".into())]),
        };
        registry
            .set_disabled_ids::<tauri::Wry>(
                None,
                vec!["plugin:demo:fixture".into(), "plugin:demo:http".into()],
            )
            .await
            .unwrap();
        registry
            .register_plugin_server::<tauri::Wry>(None, "demo", "http-token".into(), http)
            .await
            .unwrap();
        let http_output = serde_json::to_string(&registry.plugin_configs().await).unwrap();
        assert!(!http_output.contains("query-secret"), "URL query leaked");
        assert!(!http_output.contains("header-secret"));
        assert!(!http_output.contains("http-token"));
        assert!(!http_output.contains("runtimeGeneration"));
        registry.shutdown_all().await;
        assert!(
            !output.contains("private-token"),
            "registration token leaked"
        );
        assert!(!output.contains("intact"), "environment value leaked");
        assert!(!output.contains("arg-secret"), "stdio arg leaked");
    }
    #[tokio::test]
    async fn plugin_mcp_review_collisions_include_host_and_bridge_tools() {
        let registry = McpRegistry::new();
        let mut inner = registry.inner.lock().await;
        inner.plugins.insert(
            "plugin:demo:fixture".into(),
            PluginRegistration {
                plugin_id: "demo".into(),
                epoch: 0,
                token: "one".into(),
                scope: PluginMcpScope::All("all".into()),
                prefix: "fixture".into(),
            },
        );
        let tool = |name: &str| McpTool {
            name: "x".into(),
            prefixed_name: name.into(),
            description: None,
            input_schema: json!({}),
            server_id: "plugin:demo:fixture".into(),
        };
        for name in [LIST_PLUGIN_TOOLS, CALL_PLUGIN_TOOL, "mcp__codemux__echo"] {
            assert!(tool_collision(&inner, "plugin:demo:fixture", &[tool(name)], &[]).is_some());
        }
        let host = tool("custom_host_name");
        assert!(tool_collision(&inner, "plugin:demo:fixture", &[host.clone()], &[host]).is_some());
    }
    #[test]
    fn plugin_mcp_review_http_requires_tls_outside_loopback() {
        let mut config = server(PluginMcpScope::All("all".into()), vec![]);
        for (url, valid) in [
            ("http://127.0.0.1/mcp", true),
            ("http://[::1]/mcp", true),
            ("https://example.com/mcp", true),
            ("http://example.com/mcp", false),
        ] {
            config.transport = PluginMcpTransport::Http {
                url: url.into(),
                headers: HashMap::new(),
            };
            assert_eq!(config.config("demo").is_ok(), valid, "{url}");
        }
    }
    #[test]
    fn plugin_mcp_review_reserved_prefixes() {
        for prefix in ["mcp", "codemux", "codemux_mcp", "codemux_list_plugin_tools"] {
            let mut config = server(PluginMcpScope::All("all".into()), vec![]);
            config.tool_prefix = Some(prefix.into());
            assert!(
                config.config("demo").is_err(),
                "reserved prefix accepted: {prefix}"
            );
        }
    }
    #[tokio::test]
    async fn plugin_mcp_review_reenable_starts_registration() {
        let registry = McpRegistry::new();
        registry
            .set_disabled_ids::<tauri::Wry>(None, vec!["plugin:demo:fixture".into()])
            .await
            .unwrap();
        registry
            .register_plugin_server::<tauri::Wry>(
                None,
                "demo",
                "one".into(),
                server(PluginMcpScope::All("all".into()), vec![]),
            )
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        registry
            .set_disabled_ids::<tauri::Wry>(None, vec![])
            .await
            .unwrap();
        let running = tokio::time::timeout(Duration::from_secs(2), wait_count(&registry, 4)).await;
        registry.shutdown_all().await;
        assert!(running.is_ok(), "re-enabled plugin never restarted");
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
        // getcwd normalizes separators (and may resolve symlinks). Compare the
        // selected directory rather than its platform-specific spelling.
        assert_eq!(
            std::fs::canonicalize(text["cwd"].as_str().unwrap()).unwrap(),
            std::fs::canonicalize(std::env::temp_dir()).unwrap()
        );
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
            .await
            .unwrap();
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
            .await
            .unwrap();
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
    #[tokio::test]
    async fn plugin_mcp_private_scope_never_reaches_agent_or_bridge() {
        let registry = McpRegistry::new();
        registry
            .register_plugin_server::<tauri::Wry>(
                None,
                "demo",
                "one".into(),
                server(PluginMcpScope::All("plugin".into()), vec![]),
            )
            .await
            .unwrap();
        wait_count(&registry, 4).await;
        assert_eq!(
            registry
                .list_tools_for_server("plugin:demo:fixture")
                .await
                .len(),
            4
        );
        assert!(registry
            .plugin_call("plugin:demo:fixture", "new_tool", json!({}))
            .await
            .is_ok());
        for workspace in [None, Some("a")] {
            assert!(registry
                .list_tools_for_workspace(&[], workspace)
                .await
                .is_empty());
            assert!(registry
                .dispatch_tool_call("fixture__new_tool", json!({}), workspace)
                .await
                .is_err());
            let catalog = registry
                .call_plugin_bridge(LIST_PLUGIN_TOOLS, json!({}), workspace, &[])
                .await
                .unwrap()
                .unwrap();
            assert_eq!(catalog["structuredContent"]["tools"], json!([]));
            assert!(registry
                .call_plugin_bridge(
                    CALL_PLUGIN_TOOL,
                    json!({"name":"fixture__new_tool"}),
                    workspace,
                    &[]
                )
                .await
                .unwrap()
                .is_err());
        }
        registry.shutdown_all().await;
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn plugin_mcp_immediate_stop_halts_effects_and_graceful_keeps_eof_budget() {
        for immediate in [true, false] {
            let dir = tempfile::tempdir().unwrap();
            let effects = dir.path().join("effects");
            let pid_file = dir.path().join("pid");
            let mut config = server(
                PluginMcpScope::All("all".into()),
                vec![
                    "--effects".into(),
                    effects.display().to_string(),
                    "--ignore-eof".into(),
                    "--ignore-term".into(),
                    "--pid-file".into(),
                    pid_file.display().to_string(),
                ],
            );
            if immediate {
                config.shutdown = PluginMcpShutdown::Immediate;
            }
            let registry = McpRegistry::new();
            registry
                .register_plugin_server::<tauri::Wry>(None, "demo", "one".into(), config)
                .await
                .unwrap();
            wait_count(&registry, 4).await;
            let started = std::time::Instant::now();
            let mut clock: libc::timespec = unsafe { std::mem::zeroed() };
            assert_eq!(
                unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut clock) },
                0
            );
            let stop_ns = clock.tv_sec as u128 * 1_000_000_000 + clock.tv_nsec as u128;
            registry
                .unregister_plugin_server::<tauri::Wry>(None, "demo", "fixture", "one")
                .await
                .unwrap();
            let elapsed = started.elapsed();
            let timestamps = std::fs::read_to_string(&effects).unwrap();
            let pid: i32 = std::fs::read_to_string(pid_file).unwrap().parse().unwrap();
            assert_eq!(
                unsafe { libc::kill(pid, 0) },
                -1,
                "disposer confirms process exit"
            );
            tokio::time::sleep(Duration::from_millis(40)).await;
            assert_eq!(std::fs::read_to_string(&effects).unwrap(), timestamps);
            if immediate {
                let last: u128 = timestamps.lines().last().unwrap().parse().unwrap();
                assert!(
                    last <= stop_ns + 10_000_000,
                    "effects continued beyond one interval"
                );
                assert!(
                    elapsed < Duration::from_millis(100),
                    "immediate elapsed {elapsed:?}"
                );
                println!(
                    "immediate stop confirmed in {elapsed:?}; effects after request: {}ns",
                    last.saturating_sub(stop_ns)
                );
            } else {
                assert!(elapsed >= Duration::from_secs(2));
                assert!(timestamps.lines().count() > 100);
            }
        }
    }

    #[tokio::test]
    async fn plugin_mcp_failed_unregister_keeps_ownership_and_retries_to_stopped() {
        let dir = tempfile::tempdir().unwrap();
        let barrier = crate::json_rpc_child::install_exit_drain_test_barrier(dir.path());
        let mut config = server(PluginMcpScope::All("all".into()), vec![]);
        if let PluginMcpTransport::Stdio { cwd, .. } = &mut config.transport {
            *cwd = Some(dir.path().display().to_string());
        }
        let registry = McpRegistry::new();
        registry
            .register_plugin_server::<tauri::Wry>(None, "demo", "one".into(), config)
            .await
            .unwrap();
        wait_count(&registry, 4).await;
        let id = "plugin:demo:fixture";
        let child = registry.inner.lock().await.handles[id]
            .child
            .clone()
            .unwrap();
        let mut events = registry.subscribe_status();
        // Hold the real reap receipt past shutdown's timeout, then allow the
        // same retained watchdog to complete before the next unregister.
        let first = registry
            .unregister_plugin_server::<tauri::Wry>(None, "demo", "fixture", "one")
            .await;
        barrier.release.add_permits(1);
        let error = first.unwrap_err();
        assert!(error.contains("shutdown/reap"), "{error}");
        assert_eq!(
            registry
                .owned_plugin_server("demo", "fixture", "one")
                .await
                .unwrap(),
            id
        );
        {
            let inner = registry.inner.lock().await;
            let handle = &inner.handles[id];
            assert!(matches!(handle.status, McpServerStatus::Errored { .. }));
            assert!(Arc::ptr_eq(handle.child.as_ref().unwrap(), &child));
        }
        while let Ok(row) = events.try_recv() {
            assert!(!matches!(row.status, McpServerStatus::Stopped));
        }
        registry
            .unregister_plugin_server::<tauri::Wry>(None, "demo", "fixture", "one")
            .await
            .unwrap();
        let mut statuses = Vec::new();
        while let Ok(row) = events.try_recv() {
            statuses.push(row.status);
        }
        assert!(matches!(statuses.last(), Some(McpServerStatus::Stopped)));
        assert!(!child.is_alive());
        assert!(registry.plugin_configs().await.is_empty());
        registry
            .unregister_plugin_server::<tauri::Wry>(None, "demo", "fixture", "one")
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn plugin_mcp_startup_dispose_waits_for_child_exit() {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("pid");
        let registry = McpRegistry::new();
        registry
            .register_plugin_server::<tauri::Wry>(
                None,
                "demo",
                "one".into(),
                server(
                    PluginMcpScope::All("all".into()),
                    vec![
                        "--init-delay".into(),
                        "5".into(),
                        "--pid-file".into(),
                        pid_file.display().to_string(),
                    ],
                ),
            )
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            while !pid_file.exists() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        registry
            .unregister_plugin_server::<tauri::Wry>(None, "demo", "fixture", "one")
            .await
            .unwrap();
        #[cfg(unix)]
        {
            let pid: i32 = std::fs::read_to_string(pid_file).unwrap().parse().unwrap();
            assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
        }
        assert!(registry.plugin_configs().await.is_empty());
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
                format!("mcp__codemux__{s}").len() <= 64
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
            tool_descriptions: HashMap::new(),
            shutdown: PluginMcpShutdown::Graceful,
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
