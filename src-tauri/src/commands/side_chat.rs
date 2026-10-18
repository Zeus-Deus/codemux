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

/// One side chat id's lifecycle: claimed while its provider starts, then
/// running. Close removes either; a starter whose claim is gone (or was
/// re-claimed) must reap what it started instead of publishing it.
enum Slot<T> {
    Starting(u64),
    Running(T),
}

/// Side chat slots keyed by renderer-minted id. Generic so the start/close
/// race can be tested without spawning provider processes.
struct SideChatSlots<T> {
    slots: HashMap<String, Slot<T>>,
    next_token: u64,
}

impl<T> Default for SideChatSlots<T> {
    fn default() -> Self {
        Self {
            slots: HashMap::new(),
            next_token: 0,
        }
    }
}

impl<T> SideChatSlots<T> {
    /// Claim `id` before starting its provider. Returns the claim token the
    /// starter hands back to [`Self::finish`] or [`Self::abandon`].
    fn begin(&mut self, id: &str) -> Result<u64, String> {
        if self.slots.contains_key(id) {
            return Err("This side chat is already running.".into());
        }
        self.next_token += 1;
        let token = self.next_token;
        self.slots.insert(id.to_string(), Slot::Starting(token));
        Ok(token)
    }

    /// Publish a started chat. If the claim was cancelled meanwhile, hands
    /// the chat back so the caller reaps it.
    fn finish(&mut self, id: &str, token: u64, chat: T) -> Result<(), T> {
        match self.slots.get(id) {
            Some(Slot::Starting(claim)) if *claim == token => {
                self.slots.insert(id.to_string(), Slot::Running(chat));
                Ok(())
            }
            _ => Err(chat),
        }
    }

    /// Drop a claim whose start failed, unless something else owns `id` now.
    fn abandon(&mut self, id: &str, token: u64) {
        if matches!(self.slots.get(id), Some(Slot::Starting(claim)) if *claim == token) {
            self.slots.remove(id);
        }
    }

    /// Close `id`: cancels a pending start, or returns the running chat.
    fn close(&mut self, id: &str) -> Option<T> {
        match self.slots.remove(id)? {
            Slot::Starting(_) => None,
            Slot::Running(chat) => Some(chat),
        }
    }

    fn running(&self, id: &str) -> Option<&T> {
        match self.slots.get(id)? {
            Slot::Running(chat) => Some(chat),
            Slot::Starting(_) => None,
        }
    }

    /// Remove `id` only if it is running and matches `is_same`.
    fn remove_running_if(&mut self, id: &str, is_same: impl FnOnce(&T) -> bool) {
        if self.running(id).is_some_and(is_same) {
            self.slots.remove(id);
        }
    }

    /// Cancel every pending start and return every running chat.
    fn drain(&mut self) -> Vec<T> {
        self.slots
            .drain()
            .filter_map(|(_, slot)| match slot {
                Slot::Running(chat) => Some(chat),
                Slot::Starting(_) => None,
            })
            .collect()
    }
}

/// Live side chats keyed by renderer-minted id. Managed Tauri state.
#[derive(Default)]
pub struct SideChatStore {
    chats: Mutex<SideChatSlots<SideChat>>,
}

impl SideChatStore {
    /// Reap every side chat (app exit). Starts still in flight are
    /// cancelled and reap themselves when they finish.
    pub async fn shutdown_all(&self) {
        let chats = self.chats.lock().await.drain();
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
    if !matches!(main.provider, ProviderKind::Claude | ProviderKind::Codex) {
        ensure_side_chat_supported(main.provider)?;
        return Err("Side chat isn't available for this agent yet.".into());
    }
    let store: State<'_, SideChatStore> = app.state();
    // Claim the id before spawning, so a close that lands while the
    // provider starts cancels this start instead of finding nothing.
    let token = store.chats.lock().await.begin(&side_id)?;
    let result = match main.provider {
        ProviderKind::Claude => start_claude(&app, &side_id, &main, &question).await,
        _ => start_codex(&app, &side_id, &main, &question).await,
    };
    let (chat, started) = match result {
        Ok(started) => started,
        Err(error) => {
            store.chats.lock().await.abandon(&side_id, token);
            return Err(error);
        }
    };
    let finished = store.chats.lock().await.finish(&side_id, token, chat);
    if let Err(chat) = finished {
        dispose(chat).await;
        return Err(SIDE_CHAT_CANCELLED.into());
    }
    Ok(started)
}

const SIDE_CHAT_CANCELLED: &str = "The side chat was closed before it started.";

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
            .running(&side_id)
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

/// Close a side chat and reap its process, or cancel its pending start.
/// Idempotent.
#[tauri::command]
pub async fn agent_chat_side_chat_close<R: Runtime>(
    app: AppHandle<R>,
    side_id: String,
) -> Result<(), String> {
    let store: State<'_, SideChatStore> = app.state();
    let chat = store.chats.lock().await.close(&side_id);
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
            store
                .chats
                .lock()
                .await
                .remove_running_if(&side_id, |chat| Arc::ptr_eq(&chat.child, &child));
        }
    })
}

#[cfg(test)]
mod tests {
    use super::SideChatSlots;

    #[test]
    fn start_then_finish_publishes_the_chat() {
        let mut slots = SideChatSlots::default();
        let token = slots.begin("a").unwrap();
        assert!(slots.running("a").is_none(), "a pending start is not sendable");
        assert!(slots.finish("a", token, 1).is_ok());
        assert_eq!(slots.running("a"), Some(&1));
        assert_eq!(slots.close("a"), Some(1));
        assert!(slots.running("a").is_none());
    }

    #[test]
    fn close_during_start_hands_the_chat_back_for_reaping() {
        let mut slots = SideChatSlots::default();
        let token = slots.begin("a").unwrap();
        assert_eq!(slots.close("a"), None, "nothing running to reap yet");
        assert_eq!(slots.finish("a", token, 7), Err(7));
        assert!(slots.running("a").is_none(), "a cancelled start is never published");
    }

    #[test]
    fn a_stale_starter_cannot_take_over_a_reclaimed_id() {
        let mut slots = SideChatSlots::default();
        let stale = slots.begin("a").unwrap();
        slots.close("a");
        let fresh = slots.begin("a").unwrap();
        assert_eq!(slots.finish("a", stale, 1), Err(1));
        slots.abandon("a", stale);
        assert!(slots.finish("a", fresh, 2).is_ok());
        assert_eq!(slots.running("a"), Some(&2));
    }

    #[test]
    fn duplicate_starts_are_refused_and_failed_starts_release_the_id() {
        let mut slots: SideChatSlots<u8> = SideChatSlots::default();
        let token = slots.begin("a").unwrap();
        assert!(slots.begin("a").is_err());
        slots.abandon("a", token);
        assert!(slots.begin("a").is_ok());
    }

    #[test]
    fn shutdown_cancels_pending_starts_and_returns_running_chats() {
        let mut slots = SideChatSlots::default();
        let running = slots.begin("a").unwrap();
        slots.finish("a", running, 1).unwrap();
        let pending = slots.begin("b").unwrap();
        assert_eq!(slots.drain(), vec![1]);
        assert_eq!(slots.finish("b", pending, 2), Err(2));
    }

    #[test]
    fn pump_exit_only_removes_its_own_running_chat() {
        let mut slots = SideChatSlots::default();
        let token = slots.begin("a").unwrap();
        slots.remove_running_if("a", |_: &u8| true);
        assert!(slots.finish("a", token, 1).is_ok(), "a pending claim survives");
        slots.remove_running_if("a", |chat| *chat == 2);
        assert_eq!(slots.running("a"), Some(&1));
        slots.remove_running_if("a", |chat| *chat == 1);
        assert!(slots.running("a").is_none());
    }
}
