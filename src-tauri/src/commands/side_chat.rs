//! Tauri commands for `/btw` side chats.
//!
//! Each side chat owns one provider process (Claude sidecar or Codex
//! app-server) that forks the main chat's provider session read-only. The
//! process is separate from the main session's, so the side question runs
//! while the main turn keeps streaming, and closing the side chat (or the app)
//! reaps it. Updates go to the renderer on [`SIDE_CHAT_EVENT`]; nothing is
//! persisted or routed through the main chat event bridge.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, Runtime, State};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

use crate::agent_provider::side_chat::{
    claude_side_chat_params, claude_side_update, codex_enabled_mcp_servers, codex_fork_error,
    codex_fork_params, codex_side_chat_args, codex_side_request_reply, codex_side_update,
    codex_fork_settings, codex_turn_params, codex_validate_fork, ensure_side_chat_supported,
    is_valid_side_id, non_blank, side_chat_question, SideChatEventPayload, SideChatStarted,
    SideChatUpdate, SIDE_CHAT_EVENT,
};
use crate::agent_provider::{ProviderKind, ThreadId};
use crate::database::DatabaseStore;
use crate::json_rpc_child::{JsonRpcChild, RpcError, SpawnConfig};
use crate::observability::ObservabilityStore;

use super::agent_chat::{feature_flag_on, lookup_provider, ProviderRegistry};

const SIDE_RPC_TIMEOUT: Duration = Duration::from_secs(60);

enum SideBackend {
    /// `start-side-chat` session in a dedicated Claude sidecar.
    Claude,
    /// Forked thread id inside a dedicated `codex app-server`, plus the
    /// main thread's model and effort, re-sent on every follow-up turn.
    Codex {
        fork_thread_id: String,
        model: Option<String>,
        effort: Option<String>,
    },
}

struct SideChat {
    child: Arc<JsonRpcChild>,
    backend: SideBackend,
    pump: JoinHandle<()>,
}

/// Live side chats keyed by renderer-minted id. Managed Tauri state.
#[derive(Default)]
pub struct SideChatStore {
    chats: Mutex<HashMap<String, SideChat>>,
}

impl SideChatStore {
    /// Reap every side chat (app exit).
    pub async fn shutdown_all(&self) {
        let chats: Vec<SideChat> = self.chats.lock().await.drain().map(|(_, c)| c).collect();
        for chat in chats {
            dispose(chat).await;
        }
    }
}

async fn dispose(chat: SideChat) {
    if let SideBackend::Codex { fork_thread_id, .. } = &chat.backend {
        // The fork is ephemeral (never written to disk), so there is nothing
        // to archive or delete; unsubscribing unloads it before shutdown.
        let _ = chat
            .child
            .request_with_timeout(
                "thread/unsubscribe",
                json!({ "threadId": fork_thread_id }),
                Duration::from_secs(3),
            )
            .await;
    } else {
        let _ = chat
            .child
            .request_with_timeout("stop-side-chat", json!({}), Duration::from_secs(3))
            .await;
    }
    chat.pump.abort();
    let _ = chat.child.shutdown().await;
}

fn emit<R: Runtime>(app: &AppHandle<R>, side_id: &str, update: SideChatUpdate) {
    let _ = app.emit(
        SIDE_CHAT_EVENT,
        SideChatEventPayload {
            side_id: side_id.to_string(),
            update,
        },
    );
}

/// What the main chat is, read from its persisted row: the provider, its
/// native session id, where it runs, and the model and reasoning effort the
/// user picked for it (the picker persists both on every change).
struct MainSession {
    provider: ProviderKind,
    native_id: String,
    cwd: PathBuf,
    model: Option<String>,
    effort: Option<String>,
    /// Claude's context-window pick (`1m`), so the fork can hold the whole
    /// transcript it reads: a 1M main chat forked onto the 200k default
    /// would fail once the conversation outgrows it.
    context_window: Option<String>,
}

fn main_session<R: Runtime>(app: &AppHandle<R>, thread_id: &str) -> Result<MainSession, String> {
    let db: State<'_, DatabaseStore> = app.state();
    let record = db
        .get_agent_chat_session(thread_id)
        .ok_or("This chat hasn't started a session yet. Send a message first, then ask your side question.")?;
    let provider = match record.provider.as_str() {
        "claude" => ProviderKind::Claude,
        "codex" => ProviderKind::Codex,
        "cursor" => ProviderKind::Cursor,
        "grok" => ProviderKind::Grok,
        "hermes" => ProviderKind::Hermes,
        "acp" => ProviderKind::Acp,
        "opencode" => ProviderKind::OpenCode,
        other => return Err(format!("Side chat isn't available for {other} yet.")),
    };
    ensure_side_chat_supported(provider)?;
    if record.imported_from.is_some() {
        return Err("Side chat needs a live conversation; this one was imported read-only.".into());
    }
    let native_id = record
        .sdk_session_id
        .clone()
        .filter(|id| !id.trim().is_empty())
        .ok_or("This chat has no provider session yet. Wait for the first reply, then ask your side question.")?;
    let cwd = record
        .cwd
        .as_deref()
        .filter(|cwd| !cwd.is_empty())
        .map(PathBuf::from)
        .ok_or("This chat has no working directory to fork from.")?;
    Ok(MainSession {
        provider,
        native_id,
        cwd,
        model: non_blank(record.model.as_deref()),
        effort: non_blank(record.effort.as_deref()),
        context_window: non_blank(record.context_window.as_deref()),
    })
}

fn spawn_error(what: &str, error: impl std::fmt::Display) -> String {
    format!("Couldn't start the side chat: {what}: {error}")
}

/// Start a side chat that forks `thread_id`'s provider session and asks
/// `text`. Returns once the fork exists and the first turn is running, with
/// the model and effort the fork runs on; the answer streams on
/// [`SIDE_CHAT_EVENT`].
#[tauri::command]
pub async fn agent_chat_side_chat_start<R: Runtime>(
    app: AppHandle<R>,
    side_id: String,
    thread_id: ThreadId,
    text: String,
) -> Result<SideChatStarted, String> {
    let observability: State<'_, ObservabilityStore> = app.state();
    feature_flag_on(&observability)?;
    if !is_valid_side_id(&side_id) {
        return Err("invalid side chat id".into());
    }
    let question = side_chat_question(&text)?;
    if super::agent_chat::thread_runs_on_device_pub(&app, &thread_id.0) {
        return Err("Side chat isn't available for chats running on another device yet.".into());
    }
    let main = main_session(&app, &thread_id.0)?;
    // Make sure the provider is configured before spawning anything.
    {
        let registry: State<'_, ProviderRegistry> = app.state();
        lookup_provider(&registry, main.provider).await?;
    }
    let store: State<'_, SideChatStore> = app.state();
    if store.chats.lock().await.contains_key(&side_id) {
        return Err("This side chat is already running.".into());
    }
    let (chat, started) = match main.provider {
        ProviderKind::Claude => start_claude(&app, &side_id, &main, &question).await?,
        ProviderKind::Codex => start_codex(&app, &side_id, &main, &question).await?,
        other => {
            ensure_side_chat_supported(other)?;
            return Err("Side chat isn't available for this agent yet.".into());
        }
    };
    let previous = store.chats.lock().await.insert(side_id, chat);
    if let Some(previous) = previous {
        dispose(previous).await;
    }
    Ok(started)
}

/// Ask a follow-up in an open side chat. It continues the same fork.
#[tauri::command]
pub async fn agent_chat_side_chat_send<R: Runtime>(
    app: AppHandle<R>,
    side_id: String,
    text: String,
) -> Result<(), String> {
    let question = side_chat_question(&text)?;
    let store: State<'_, SideChatStore> = app.state();
    let (child, request) = {
        let chats = store.chats.lock().await;
        let chat = chats
            .get(&side_id)
            .ok_or("This side chat has ended. Start a new one with /btw.")?;
        let request = match &chat.backend {
            SideBackend::Claude => ("side-chat-send", json!({ "text": question })),
            SideBackend::Codex {
                fork_thread_id,
                model,
                effort,
            } => (
                "turn/start",
                codex_turn_params(
                    fork_thread_id,
                    &question,
                    model.as_deref(),
                    effort.as_deref(),
                ),
            ),
        };
        (Arc::clone(&chat.child), request)
    };
    child
        .request_with_timeout(request.0, request.1, SIDE_RPC_TIMEOUT)
        .await
        .map(|_| ())
        .map_err(|error| format!("Couldn't send to the side chat: {error}"))
}

/// Close a side chat and reap its process. Idempotent.
#[tauri::command]
pub async fn agent_chat_side_chat_close<R: Runtime>(
    app: AppHandle<R>,
    side_id: String,
) -> Result<(), String> {
    let store: State<'_, SideChatStore> = app.state();
    let chat = store.chats.lock().await.remove(&side_id);
    if let Some(chat) = chat {
        dispose(chat).await;
    }
    Ok(())
}

// ── Claude ──────────────────────────────────────────────────────────────

async fn start_claude<R: Runtime>(
    app: &AppHandle<R>,
    side_id: &str,
    main: &MainSession,
    question: &str,
) -> Result<(SideChat, SideChatStarted), String> {
    // Never touch Claude's on-disk session here: the SDK reads the main
    // transcript to fork it and, with `persistSession: false`, writes none.
    let sidecar = crate::agent_provider::claude::sidecar_path::resolve_sidecar_path()
        .map_err(|error| spawn_error("Claude sidecar", error))?;
    let child = JsonRpcChild::spawn(SpawnConfig {
        program: sidecar,
        args: vec![],
        env: HashMap::new(),
        cwd: Some(main.cwd.clone()),
        default_timeout: SIDE_RPC_TIMEOUT,
    })
    .await
    .map_err(|error| spawn_error("Claude sidecar", error))?;
    let child = Arc::new(child);
    let pump = spawn_pump(app.clone(), side_id.to_string(), Arc::clone(&child), None);
    let model = main.model.as_deref().map(|model| {
        crate::agent_provider::claude::capabilities::resolve_claude_api_model_id(model, main.context_window.as_deref())
    });
    let params = claude_side_chat_params(
        side_id,
        &main.cwd.to_string_lossy(),
        &main.native_id,
        "claude",
        model.as_deref(),
        main.effort.as_deref(),
        question,
    );
    if let Err(error) = child.request("start-side-chat", params).await {
        pump.abort();
        let _ = child.shutdown().await;
        return Err(format!("Couldn't start the side chat: {error}"));
    }
    Ok((
        SideChat {
            child,
            backend: SideBackend::Claude,
            pump,
        },
        SideChatStarted {
            model: main.model.clone(),
            effort: main.effort.clone(),
        },
    ))
}

// ── Codex ───────────────────────────────────────────────────────────────

async fn spawn_codex(cwd: &std::path::Path, extra: Vec<String>) -> Result<JsonRpcChild, String> {
    let mut args = extra;
    args.extend(codex_side_chat_args());
    JsonRpcChild::spawn(SpawnConfig {
        program: PathBuf::from("codex"),
        args,
        env: HashMap::new(),
        cwd: Some(cwd.to_path_buf()),
        default_timeout: SIDE_RPC_TIMEOUT,
    })
    .await
    .map_err(|error| spawn_error("codex app-server", error))
}

async fn codex_initialize(child: &JsonRpcChild) -> Result<(), String> {
    child
        .request(
            "initialize",
            json!({
                "clientInfo": { "name": "codemux", "title": "Codemux", "version": env!("CARGO_PKG_VERSION") },
                "capabilities": { "experimentalApi": true },
            }),
        )
        .await
        .map_err(|error| format!("Couldn't start the side chat: {error}"))?;
    child
        .notify("initialized", json!({}))
        .await
        .map_err(|error| format!("Couldn't start the side chat: {error}"))
}

async fn start_codex<R: Runtime>(
    app: &AppHandle<R>,
    side_id: &str,
    main: &MainSession,
    question: &str,
) -> Result<(SideChat, SideChatStarted), String> {
    if crate::agent_provider::codex::chatgpt::owner().managed_selected_now() {
        // App-owned ChatGPT-plan routing needs the token-bound launch the
        // main adapter performs; a plain app-server would bill another
        // account or fail. Refuse honestly rather than guess.
        return Err(
            "Side chat isn't available while Codex uses your ChatGPT plan connection yet.".into(),
        );
    }
    let child = Arc::new(spawn_codex(&main.cwd, Vec::new()).await?);
    let result = async {
        codex_initialize(&child).await?;
        let config = child
            .request(
                "config/read",
                json!({ "includeLayers": false, "cwd": main.cwd }),
            )
            .await
            .map_err(|error| format!("Couldn't read Codex configuration: {error}"))?;
        let mcp_servers = codex_enabled_mcp_servers(config.get("config").unwrap_or(&Value::Null));
        let fork = child
            .request(
                "thread/fork",
                codex_fork_params(&main.native_id, &main.cwd.to_string_lossy(), &mcp_servers),
            )
            .await
            .map_err(|error| codex_fork_error(&error.to_string()))?;
        let id = codex_validate_fork(&fork)?;
        Ok::<_, String>((
            id,
            codex_fork_settings(&fork, main.model.as_deref(), main.effort.as_deref()),
        ))
    }
    .await;
    let (fork_thread_id, started) = match result {
        Ok(fork) => fork,
        Err(error) => {
            let _ = child.shutdown().await;
            return Err(error);
        }
    };
    let pump = spawn_pump(
        app.clone(),
        side_id.to_string(),
        Arc::clone(&child),
        Some(fork_thread_id.clone()),
    );
    if let Err(error) = child
        .request(
            "turn/start",
            codex_turn_params(
                &fork_thread_id,
                question,
                main.model.as_deref(),
                main.effort.as_deref(),
            ),
        )
        .await
    {
        pump.abort();
        let _ = child.shutdown().await;
        return Err(format!("Couldn't ask the side question: {error}"));
    }
    Ok((
        SideChat {
            child,
            backend: SideBackend::Codex {
                fork_thread_id,
                model: main.model.clone(),
                effort: main.effort.clone(),
            },
            pump,
        },
        started,
    ))
}

// ── Event pump ──────────────────────────────────────────────────────────

/// Forward the side process's notifications to the renderer, decline every
/// server request that would act, and report the process's exit.
fn spawn_pump<R: Runtime>(
    app: AppHandle<R>,
    side_id: String,
    child: Arc<JsonRpcChild>,
    codex_fork: Option<String>,
) -> JoinHandle<()> {
    let mut notifications = child.notifications();
    let mut requests = child.incoming_requests();
    tokio::spawn(async move {
        let mut alive = tokio::time::interval(Duration::from_millis(250));
        loop {
            tokio::select! {
                note = notifications.recv() => match note {
                    Ok(note) => {
                        let update = match &codex_fork {
                            Some(fork) => codex_side_update(fork, &note.method, &note.params),
                            None => claude_side_update(&side_id, &note.method, &note.params),
                        };
                        if let Some(update) = update {
                            emit(&app, &side_id, update);
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                },
                request = async {
                    match requests.as_mut() {
                        Some(rx) => rx.recv().await,
                        None => std::future::pending().await,
                    }
                } => {
                    let Some(request) = request else { requests = None; continue; };
                    let reply = codex_side_request_reply(&request.method)
                        .map_err(|(code, message)| RpcError { code, message, data: None });
                    let _ = child.respond(request.id, reply).await;
                }
                _ = alive.tick() => {
                    if !child.is_alive() { break; }
                }
            }
        }
        emit(&app, &side_id, SideChatUpdate::Closed);
        // Drop the dead entry so a later send reports the ended chat.
        if let Some(store) = app.try_state::<SideChatStore>() {
            let mut chats = store.chats.lock().await;
            if chats
                .get(&side_id)
                .is_some_and(|chat| Arc::ptr_eq(&chat.child, &child))
            {
                chats.remove(&side_id);
            }
        }
    })
}
