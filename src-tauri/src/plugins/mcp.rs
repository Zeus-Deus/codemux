//! The supported plugin MCP IPC surface; the remote denies plugins_*.
use crate::mcp::{
    registry::{McpRegistry, McpServerRuntime, PluginMcpServer},
    runtime::McpTool,
};
use serde_json::Value;
use tauri::{AppHandle, Emitter, Runtime, State};

#[tauri::command]
pub async fn plugins_mcp_register<R: Runtime>(
    app: AppHandle<R>,
    registry: State<'_, McpRegistry>,
    plugin_id: String,
    token: String,
    server: PluginMcpServer,
) -> Result<(), String> {
    super::commands::gui(&app)?;
    let emitter = app.clone();
    registry.set_activity_emitter(std::sync::Arc::new(move |event| {
        let _ = emitter.emit_to("main", "plugin-mcp-activity", event);
    }));
    registry
        .register_plugin_server(Some(&app), &plugin_id, token, server)
        .await
}
#[tauri::command]
pub async fn plugins_mcp_unregister<R: Runtime>(
    app: AppHandle<R>,
    registry: State<'_, McpRegistry>,
    plugin_id: String,
    local_id: String,
    token: String,
) -> Result<(), String> {
    super::commands::gui(&app)?;
    registry
        .unregister_plugin_server(Some(&app), &plugin_id, &local_id, &token)
        .await;
    Ok(())
}
#[tauri::command]
pub async fn plugins_mcp_list_tools<R: Runtime>(
    app: AppHandle<R>,
    registry: State<'_, McpRegistry>,
    plugin_id: String,
    local_id: String,
    token: String,
) -> Result<Vec<McpTool>, String> {
    super::commands::gui(&app)?;
    let id = registry
        .owned_plugin_server(&plugin_id, &local_id, &token)
        .await?;
    Ok(registry.list_tools_for_server(&id).await)
}
#[tauri::command]
pub async fn plugins_mcp_call_tool<R: Runtime>(
    app: AppHandle<R>,
    registry: State<'_, McpRegistry>,
    plugin_id: String,
    local_id: String,
    token: String,
    tool_name: String,
    arguments: Value,
) -> Result<Value, String> {
    super::commands::gui(&app)?;
    let id = registry
        .owned_plugin_server(&plugin_id, &local_id, &token)
        .await?;
    registry.plugin_call(&id, &tool_name, arguments).await
}
#[tauri::command]
pub async fn plugins_mcp_status<R: Runtime>(
    app: AppHandle<R>,
    registry: State<'_, McpRegistry>,
    plugin_id: String,
    local_id: String,
    token: String,
) -> Result<Option<McpServerRuntime>, String> {
    super::commands::gui(&app)?;
    match registry
        .owned_plugin_server(&plugin_id, &local_id, &token)
        .await
    {
        Ok(id) => Ok(registry.plugin_status(&id).await),
        Err(_) => Ok(None),
    }
}
