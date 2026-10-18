//! Tauri commands for forking a chat into a new, independent thread.
//!
//! The fork continues the provider's own session (see
//! [`crate::agent_provider::chat_fork`]) in a short-lived provider process,
//! the same dedicated Claude sidecar or `codex app-server` a `/btw` side chat
//! uses. Then one transaction writes the new thread: its session row with
//! the forked native id as resume cursor, its copy of the transcript up to
//! the cut, and its lineage. The fork resumes on its first send like any
//! reopened chat. Providers that can't fork natively are refused.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::json;
use tauri::{AppHandle, Manager, Runtime, State};

use crate::agent_provider::chat_fork::{
    claude_fork_session_id, claude_fork_session_params, codex_persistent_fork_id,
    codex_rollback_turns, fork_error, fork_title, ChatForkCreated, ClaudeForkCut,
};
use crate::agent_provider::codex::protocol::ThreadRollbackParams;
use crate::agent_provider::side_chat::{codex_fork_base_params, provider_label, supports_native_fork};
use crate::agent_provider::{ProviderKind, ThreadId};
use crate::database::chat_fork::{ChatForkOrigin, NewChatFork};
use crate::database::{AgentChatSessionRecord, DatabaseStore};
use crate::json_rpc_child::JsonRpcChild;
use crate::observability::ObservabilityStore;
use crate::state::AppStateStore;

use super::agent_chat::{feature_flag_on, lookup_provider, ProviderRegistry};
use super::side_chat::{codex_initialize, codex_mcp_servers, spawn_claude_sidecar, spawn_codex};

const CLEANUP_TIMEOUT: Duration = Duration::from_secs(3);

/// Where the fork goes and from which point, validated against the source.
struct ForkPlan {
    source: AgentChatSessionRecord,
    provider: ProviderKind,
    native_id: String,
    source_cwd: PathBuf,
    workspace_id: String,
    target_cwd: PathBuf,
    cut: Cut,
}

enum Cut {
    Latest,
    Codex { before: i64, rollback: u32 },
    Claude { before: i64, cut: ClaudeForkCut },
}

impl Cut {
    fn before(&self) -> Option<i64> {
        match self {
            Cut::Latest => None,
            Cut::Codex { before, .. } | Cut::Claude { before, .. } => Some(*before),
        }
    }
}

fn stored_provider(provider: &str) -> Option<ProviderKind> {
    Some(match provider {
        "claude" => ProviderKind::Claude,
        "codex" => ProviderKind::Codex,
        "cursor" => ProviderKind::Cursor,
        "grok" => ProviderKind::Grok,
        "hermes" => ProviderKind::Hermes,
        "acp" => ProviderKind::Acp,
        "opencode" => ProviderKind::OpenCode,
        _ => return None,
    })
}

/// Validate a fork before anything is started. Cheap: database and state
/// reads plus the provider's turn flag.
async fn plan_fork<R: Runtime>(
    app: &AppHandle<R>,
    source_thread_id: &str,
    before_message_id: Option<i64>,
    workspace_id: Option<String>,
) -> Result<ForkPlan, String> {
    let observability: State<'_, ObservabilityStore> = app.state();
    feature_flag_on(&observability)?;
    let db: State<'_, DatabaseStore> = app.state();
    let source = db
        .get_agent_chat_session(source_thread_id)
        .ok_or("This chat no longer exists.")?;
    let provider = stored_provider(&source.provider)
        .ok_or_else(|| format!("Forking isn't available for {} chats.", source.provider))?;
    if !supports_native_fork(provider) {
        return Err(format!(
            "Forking isn't available for {} chats yet.",
            provider_label(provider)
        ));
    }
    if source.imported_from.is_some() {
        return Err("This chat was imported read-only, so it can't be forked.".into());
    }
    let native_id = source
        .sdk_session_id
        .clone()
        .filter(|id| !id.trim().is_empty())
        .ok_or("This chat has no agent session yet. Wait for the first reply, then fork it.")?;
    let source_cwd = source
        .cwd
        .as_deref()
        .filter(|cwd| !cwd.is_empty())
        .map(PathBuf::from)
        .ok_or("This chat has no working directory to fork from.")?;
    if super::agent_chat::thread_runs_on_device_pub(app, source_thread_id) {
        return Err("Forking isn't available for chats running on another device yet.".into());
    }
    let (workspace_id, target_cwd) = match workspace_id {
        None => (source.workspace_id.clone(), source_cwd.clone()),
        Some(id) if id == source.workspace_id => (id, source_cwd.clone()),
        Some(id) => {
            let state: State<'_, AppStateStore> = app.state();
            let workspace = state
                .find_workspace(&id)
                .ok_or("The workspace for the fork no longer exists.")?;
            if workspace.host_id.is_some() {
                return Err("Forks can't run on another device yet.".into());
            }
            (id, PathBuf::from(workspace.cwd))
        }
    };
    if !target_cwd.is_dir() {
        return Err(format!(
            "The fork's folder doesn't exist: {}",
            target_cwd.display()
        ));
    }
    {
        let registry: State<'_, ProviderRegistry> = app.state();
        let live = lookup_provider(&registry, provider).await?;
        if live.turn_active(&ThreadId(source_thread_id.to_string())).await {
            return Err("Wait for the current turn to finish, then fork the chat.".into());
        }
    }
    let cut = match (before_message_id, provider) {
        (None, _) => Cut::Latest,
        (Some(before), ProviderKind::Codex) => Cut::Codex {
            before,
            rollback: codex_rollback_turns(
                &db.list_agent_chat_turn_checkpoints(source_thread_id),
                before,
            )?,
        },
        (Some(before), _) => {
            let (earlier, next) = db.agent_chat_fork_prompts(source_thread_id, before)?;
            Cut::Claude {
                before,
                cut: ClaudeForkCut::new(&earlier, next),
            }
        }
    };
    Ok(ForkPlan {
        source,
        provider,
        native_id,
        source_cwd,
        workspace_id,
        target_cwd,
        cut,
    })
}

/// Check that a fork would be accepted, without starting it. The renderer
/// calls this before it creates a worktree for the fork.
#[tauri::command]
pub async fn agent_chat_fork_check<R: Runtime>(
    app: AppHandle<R>,
    source_thread_id: String,
    before_message_id: Option<i64>,
) -> Result<(), String> {
    plan_fork(&app, &source_thread_id, before_message_id, None)
        .await
        .map(|_| ())
}

/// Fork `source_thread_id` into a new thread. `before_message_id` is the
/// source row of the user message that starts the first turn left out
/// (`None` keeps every turn); `workspace_id` is where the fork runs (`None`
/// is the source's own workspace and folder).
#[tauri::command]
pub async fn agent_chat_fork_thread<R: Runtime>(
    app: AppHandle<R>,
    source_thread_id: String,
    before_message_id: Option<i64>,
    workspace_id: Option<String>,
) -> Result<ChatForkCreated, String> {
    let plan = plan_fork(&app, &source_thread_id, before_message_id, workspace_id).await?;
    let native_fork = match plan.provider {
        ProviderKind::Claude => fork_claude(&plan).await?,
        _ => fork_codex(&plan).await?,
    };
    let thread_id = uuid::Uuid::new_v4().to_string();
    let title = fork_title(plan.source.title.as_deref());
    let cwd = plan.target_cwd.to_string_lossy().into_owned();
    let db: State<'_, DatabaseStore> = app.state();
    db.create_agent_chat_fork(&NewChatFork {
        thread_id: &thread_id,
        source: &plan.source,
        sdk_session_id: &native_fork,
        workspace_id: &plan.workspace_id,
        cwd: &cwd,
        title: &title,
        cut_before: plan.cut.before(),
    })?;
    Ok(ChatForkCreated {
        thread_id,
        provider: plan.source.provider,
        title,
        workspace_id: plan.workspace_id,
        cwd,
    })
}

/// Where a chat was forked from, or `None` for a chat that isn't a fork.
#[tauri::command]
pub async fn agent_chat_fork_origin(
    db: State<'_, DatabaseStore>,
    thread_id: String,
) -> Result<Option<ChatForkOrigin>, String> {
    Ok(db.agent_chat_fork_origin(&thread_id))
}

// ── Claude ──────────────────────────────────────────────────────────────

async fn fork_claude(plan: &ForkPlan) -> Result<String, String> {
    let cut = match &plan.cut {
        Cut::Claude { cut, .. } => Some(cut),
        _ => None,
    };
    let child = spawn_claude_sidecar(&plan.source_cwd)
        .await
        .map_err(|error| format!("Couldn't fork this chat: {error}"))?;
    let reply = child
        .request(
            "fork-session",
            claude_fork_session_params(
                &plan.native_id,
                &plan.source_cwd.to_string_lossy(),
                &plan.target_cwd.to_string_lossy(),
                cut,
            ),
        )
        .await;
    let _ = child.shutdown().await;
    claude_fork_session_id(&reply.map_err(|error| fork_error(&error.to_string()))?)
}

// ── Codex ───────────────────────────────────────────────────────────────

async fn fork_codex(plan: &ForkPlan) -> Result<String, String> {
    if crate::agent_provider::codex::chatgpt::owner().managed_selected_now() {
        // Its threads live in the app-owned Codex home that only the main
        // adapter's token-bound launch uses (see side chats).
        return Err("Forking isn't available while Codex uses your ChatGPT plan connection yet.".into());
    }
    let rollback = match plan.cut {
        Cut::Codex { rollback, .. } => Some(rollback),
        _ => None,
    };
    let child = spawn_codex(&plan.target_cwd, Vec::new())
        .await
        .map_err(|error| format!("Couldn't fork this chat: {error}"))?;
    let result = codex_fork_in(&child, &plan.native_id, &plan.target_cwd, rollback).await;
    let _ = child.shutdown().await;
    result
}

async fn codex_fork_in(
    child: &JsonRpcChild,
    thread_id: &str,
    cwd: &Path,
    rollback: Option<u32>,
) -> Result<String, String> {
    codex_initialize(child)
        .await
        .map_err(|error| format!("Couldn't fork this chat: {error}"))?;
    // The fork is only written here; its MCP servers start when the new
    // chat resumes it.
    let mcp_servers = codex_mcp_servers(child, cwd).await?;
    let fork = child
        .request(
            "thread/fork",
            codex_fork_base_params(thread_id, &cwd.to_string_lossy(), &mcp_servers),
        )
        .await
        .map_err(|error| fork_error(&error.to_string()))?;
    let fork_id = codex_persistent_fork_id(&fork)?;
    if let Some(num_turns) = rollback {
        let params = serde_json::to_value(ThreadRollbackParams {
            thread_id: fork_id.clone(),
            num_turns,
        })
        .map_err(|error| error.to_string())?;
        if let Err(error) = child.request("thread/rollback", params).await {
            // Don't leave a full-length fork behind in Codex's thread list.
            let _ = child
                .request_with_timeout("thread/archive", json!({ "threadId": fork_id }), CLEANUP_TIMEOUT)
                .await;
            return Err(format!("Couldn't cut the fork at that turn: {error}"));
        }
    }
    let _ = child
        .request_with_timeout("thread/unsubscribe", json!({ "threadId": fork_id }), CLEANUP_TIMEOUT)
        .await;
    Ok(fork_id)
}
