//! Desktop/local-controller commands. These never establish a CodeMux cloud session.
//! The web-remote allowlist deliberately denies both metadata and mutations.
use crate::agent_provider::codex::chatgpt::{owner, ChatGptStatus};
use tauri::{Emitter, Manager, Runtime};

pub fn install<R: Runtime>(app: &tauri::AppHandle<R>) {
    let mut changes = owner().changes();
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let mut quota_key: Option<String> = None;
        loop {
            match changes.recv().await {
                Ok(()) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    let cache: tauri::State<
                        '_,
                        std::sync::Arc<
                            crate::agent_provider::codex::capabilities::CodexCapabilityCache,
                        >,
                    > = app.state();
                    cache.invalidate().await;
                    let owner = owner();
                    let key = owner.cache_key().await.ok();
                    let managed = owner.managed_selected_now();
                    if key != quota_key
                        && (managed
                            || quota_key
                                .as_ref()
                                .is_some_and(|k| k.starts_with("managed:")))
                    {
                        let connected = owner.status().await.is_ok_and(|s| {
                            s.profiles
                                .iter()
                                .any(|p| Some(&p.id) == s.active_profile_id.as_ref() && p.connected)
                        });
                        let quota: tauri::State<'_, crate::commands::usage::PlanQuotaStore> =
                            app.state();
                        quota.replace_chatgpt_route(
                            managed && connected,
                            chrono::Utc::now().timestamp_millis(),
                        );
                    }
                    quota_key = key;
                    // Only an invalidation signal: never an auth URL or credential DTO.
                    let _ = app.emit("chatgpt-status-changed", serde_json::Value::Null);
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}

#[tauri::command]
pub async fn get_chatgpt_status() -> Result<ChatGptStatus, String> {
    owner().status().await
}

#[tauri::command]
pub async fn start_chatgpt_login(profile_id: Option<String>) -> Result<ChatGptStatus, String> {
    let owner = owner();
    let launch = owner.begin(profile_id).await?;
    // Deliberately do not include the opener's diagnostic: it may contain the URL/hint.
    if tauri_plugin_opener::open_url(&launch.url, None::<&str>).is_err() {
        if let Some(id) = launch.status.attempt_id.as_deref() {
            let _ = owner.cancel(id).await;
        }
        return Err("Cannot open the system browser. Try ChatGPT sign-in again.".into());
    }
    Ok(launch.status)
}

#[tauri::command]
pub async fn cancel_chatgpt_login(attempt_id: String) -> Result<ChatGptStatus, String> {
    owner().cancel(&attempt_id).await
}
#[tauri::command]
pub async fn disconnect_chatgpt() -> Result<ChatGptStatus, String> {
    owner().disconnect().await
}
#[tauri::command]
pub async fn acknowledge_chatgpt_welcome() -> Result<ChatGptStatus, String> {
    owner().acknowledge_welcome().await
}
#[tauri::command]
pub async fn get_local_workbench() -> Result<bool, String> {
    owner().local_workbench().await
}
#[tauri::command]
pub async fn set_local_workbench(enabled: bool) -> Result<(), String> {
    owner().set_local_workbench(enabled).await
}
