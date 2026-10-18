//! Watch, unwatch and read a chat's pull-request watch (`crate::pr_watch`).
use tauri::{AppHandle, Manager, Runtime};

use crate::database::pr_watch::PrWatchRecord;
use crate::database::DatabaseStore;
use crate::pr_watch::{DEFAULT_MAX_ATTEMPTS, MAX_ATTEMPTS_LIMIT};
use crate::state::AppStateStore;

/// Start watching a pull request from a chat, or re-arm the chat's watch.
/// `pr_number` defaults to the pull request linked to the chat's workspace.
#[tauri::command]
pub async fn pr_watch_start<R: Runtime>(
    app: AppHandle<R>,
    thread_id: String,
    pr_number: Option<u32>,
    max_attempts: Option<u32>,
) -> Result<PrWatchRecord, String> {
    let max_attempts = max_attempts.unwrap_or(DEFAULT_MAX_ATTEMPTS);
    if !(1..=MAX_ATTEMPTS_LIMIT).contains(&max_attempts) {
        return Err(format!("max_attempts must be between 1 and {MAX_ATTEMPTS_LIMIT}"));
    }
    let db = app.state::<DatabaseStore>();
    let session = db
        .get_agent_chat_session(&thread_id)
        .ok_or("No chat has this thread ID")?;
    if session.imported_from.is_some() {
        return Err("Imported chats are read-only snapshots".into());
    }
    let workspace = app
        .state::<AppStateStore>()
        .find_workspace(&session.workspace_id)
        .ok_or("The chat's workspace is not open")?;
    if workspace.host_id.is_some() || workspace.remote_cwd.is_some() {
        return Err("Pull request watching runs for local workspaces only".into());
    }
    let pr_number = pr_number
        .or(workspace.pr_number)
        .ok_or("This workspace has no linked pull request")?;
    let known_url = workspace
        .pr_url
        .clone()
        .filter(|_| workspace.pr_number == Some(pr_number));

    // The watch is pinned to the PR's canonical URL: every sweep reads that
    // repository, whatever the checkout points at later.
    let repo = std::path::PathBuf::from(&workspace.cwd);
    let pr_url = tokio::task::spawn_blocking(move || -> Result<String, String> {
        let detected = crate::git_provider::try_detect_provider(&repo)?;
        if detected.kind != crate::git_provider::ProviderKind::GitHub {
            return Err("Pull request watching supports GitHub repositories only".into());
        }
        if let Some(url) = known_url {
            return Ok(url);
        }
        let number = pr_number.to_string();
        let json = crate::github_budget::run(
            &repo,
            &["pr", "view", &number, "--json", "url"],
            std::time::Duration::from_secs(20),
        )?;
        serde_json::from_str::<serde_json::Value>(&json)
            .ok()
            .and_then(|v| v["url"].as_str().map(str::to_string))
            .ok_or_else(|| format!("Could not find pull request #{pr_number}"))
    })
    .await
    .map_err(|e| format!("Pull request lookup failed: {e}"))??;
    if crate::pr_watch::parse_pr_url(&pr_url, pr_number).is_none() {
        return Err(format!("{pr_url} is not the address of pull request #{pr_number}"));
    }

    let record = db.arm_pr_watch(
        &thread_id,
        &session.workspace_id,
        pr_number,
        Some(&pr_url),
        max_attempts,
        crate::pr_watch::now_ms(),
    )?;
    crate::pr_watch::emit_changed(&app, &thread_id, Some(record.clone()));
    crate::pr_watch::kick();
    Ok(record)
}

/// Stop watching and forget what the chat was told. Returns whether a watch
/// existed.
#[tauri::command]
pub async fn pr_watch_stop<R: Runtime>(app: AppHandle<R>, thread_id: String) -> Result<bool, String> {
    let removed = app.state::<DatabaseStore>().delete_pr_watch(&thread_id)?;
    if removed {
        crate::pr_watch::emit_changed(&app, &thread_id, None);
    }
    Ok(removed)
}

/// The chat's watch, including a stopped one and why it stopped.
#[tauri::command]
pub async fn pr_watch_status<R: Runtime>(
    app: AppHandle<R>,
    thread_id: String,
) -> Result<Option<PrWatchRecord>, String> {
    app.state::<DatabaseStore>().get_pr_watch(&thread_id)
}
