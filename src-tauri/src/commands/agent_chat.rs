//! Tauri command surface for the agent-chat pane kind.
//!
//! Every command in this module is gated on the `enable_agent_chat`
//! feature flag (see
//! [`FeatureFlags`](crate::observability::FeatureFlags)); when the
//! flag is off callers get a clean `feature_disabled` error string
//! and the backend never mutates the pane tree.
//!
//! Lifecycle commands (`agent_chat_create_pane`,
//! `agent_chat_close_pane`) mutate the existing pane tree via
//! [`AppStateStore`](crate::state::AppStateStore). They do not
//! interact with the provider registry — a pane can exist before
//! any session is bound to it. Provider-session commands and the
//! event bridge land in follow-up commits.

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::{ipc::Channel, AppHandle, Emitter, Manager, Runtime, State};

use crate::agent_provider::{
    AgentProvider, ApprovalDecision, ProviderChatCapabilities, ProviderError, ProviderKind,
    ProviderRuntimeEvent, RequestId, SendTurnInput, SerializableProviderError,
    StartSessionInput, ThreadId, TurnId,
};
use crate::database::{AgentChatSessionRecord, DatabaseStore};
use crate::observability::ObservabilityStore;
use crate::state::AppStateStore;

/// Event name used for the legacy global broadcast. Thread-scoped
/// events — including the high-frequency `content_delta` token stream —
/// now flow over per-thread [`Channel`]s registered via
/// `attach_agent_chat_output` (Tauri's recommended mechanism for
/// streaming data). Only events with NO owning thread (global
/// `RuntimeWarning`s) still go out on this bus, since there is no
/// per-thread subscriber to route them to.
pub const AGENT_CHAT_EVENT: &str = "agent_chat_event";

/// Payload streamed to per-thread chat output channels (and, for
/// threadless events only, emitted on the [`AGENT_CHAT_EVENT`] bus).
///
/// Carries the originating thread id alongside the raw canonical
/// event so subscribers can route without re-parsing the payload.
/// Events that are not scoped to a single thread (global
/// `RuntimeWarning`s) carry an empty `ThreadId`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentChatEventPayload {
    pub thread_id: ThreadId,
    pub event: ProviderRuntimeEvent,
}

/// Per-thread registry of live streaming channels.
///
/// Mirrors the PTY pattern (`SessionRuntime.output_channel` in
/// `terminal/mod.rs`): when a chat pane binds to a thread it invokes
/// `attach_agent_chat_output` with a [`Channel`]; `forward_event`
/// routes every thread-scoped provider event to the channels attached
/// to that thread. This replaces the previous global `app.emit`
/// broadcast, which fanned every token of every thread to every
/// webview listener — Tauri's event system is explicitly not designed
/// for high-throughput streaming, and the frontend had to filter by
/// `thread_id` anyway.
///
/// Multiple panes may attach to the same thread (each gets its own
/// channel); a pane detaches on unmount. Channels whose webview has
/// gone away fail on `send` and are pruned lazily.
#[derive(Default)]
pub struct AgentChatChannelRegistry {
    /// thread_id → attached channels. The channel's IPC id doubles as
    /// the subscription id handed back to the frontend for detach.
    channels: std::sync::Mutex<HashMap<String, Vec<Channel<AgentChatEventPayload>>>>,
}

impl AgentChatChannelRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a channel for a thread. Returns the subscription id
    /// (the channel's IPC id) the frontend passes back on detach.
    pub fn attach(&self, thread_id: &str, channel: Channel<AgentChatEventPayload>) -> u32 {
        let id = channel.id();
        let mut map = self.channels.lock().unwrap();
        map.entry(thread_id.to_string()).or_default().push(channel);
        id
    }

    /// Remove a previously attached channel. Idempotent — detaching
    /// an unknown subscription is a no-op.
    pub fn detach(&self, thread_id: &str, subscription_id: u32) {
        let mut map = self.channels.lock().unwrap();
        if let Some(list) = map.get_mut(thread_id) {
            list.retain(|c| c.id() != subscription_id);
            if list.is_empty() {
                map.remove(thread_id);
            }
        }
    }

    /// Number of channels currently attached to a thread. Test hook.
    pub fn attached_count(&self, thread_id: &str) -> usize {
        self.channels
            .lock()
            .unwrap()
            .get(thread_id)
            .map(|l| l.len())
            .unwrap_or(0)
    }

    /// Send a payload to every channel attached to its thread.
    ///
    /// The subscriber list is cloned out of the lock before sending so
    /// a slow webview eval can never block attach/detach. Channels
    /// that error (webview reloaded/closed without detaching) are
    /// pruned afterwards.
    pub fn route(&self, payload: &AgentChatEventPayload) {
        let subscribers: Vec<Channel<AgentChatEventPayload>> = {
            let map = self.channels.lock().unwrap();
            match map.get(&payload.thread_id.0) {
                Some(list) => list.clone(),
                None => return,
            }
        };
        let mut dead: Vec<u32> = Vec::new();
        for channel in subscribers {
            if let Err(error) = channel.send(payload.clone()) {
                eprintln!(
                    "[codemux::agent_chat] dropping dead chat channel {} for thread {}: {error}",
                    channel.id(),
                    payload.thread_id.0
                );
                dead.push(channel.id());
            }
        }
        if !dead.is_empty() {
            let mut map = self.channels.lock().unwrap();
            if let Some(list) = map.get_mut(&payload.thread_id.0) {
                list.retain(|c| !dead.contains(&c.id()));
                if list.is_empty() {
                    map.remove(&payload.thread_id.0);
                }
            }
        }
    }
}

/// Error string returned when a command runs with
/// `enable_agent_chat` off.
pub const FEATURE_DISABLED_ERROR: &str = "feature_disabled: enable_agent_chat is off";

/// Assert the `enable_agent_chat` flag is on, producing the shared
/// [`FEATURE_DISABLED_ERROR`] string if not. Public so integration
/// tests can exercise the gate without going through a Tauri
/// command.
pub fn feature_flag_on(store: &ObservabilityStore) -> Result<(), String> {
    if store.agent_chat_enabled() {
        Ok(())
    } else {
        Err(FEATURE_DISABLED_ERROR.to_string())
    }
}

/// Shared registry of concrete provider implementations.
///
/// Keyed by [`ProviderKind`]; constructors are expected to populate
/// whichever providers are reachable at startup and skip the rest.
/// Absence of an entry is a recoverable error rather than a crash —
/// the Tauri commands surface it as a provider-not-configured string.
///
/// The OpenCode slot is reserved for Step 12 — Stage 1 lands the
/// schema seam and discovery surface but does not inject a runtime
/// adapter, so `get(OpenCode)` always returns `None` and commands
/// routing to it surface `provider_not_configured` exactly like the
/// existing Claude/Codex paths do when their setup fails.
#[derive(Default)]
pub struct ProviderRegistry {
    claude: tokio::sync::RwLock<Option<Arc<dyn AgentProvider>>>,
    codex: tokio::sync::RwLock<Option<Arc<dyn AgentProvider>>>,
    opencode: tokio::sync::RwLock<Option<Arc<dyn AgentProvider>>>,
}

impl ProviderRegistry {
    /// Construct an empty registry. Providers must be injected via
    /// [`set_claude`](Self::set_claude) / [`set_codex`](Self::set_codex).
    pub fn new() -> Self {
        Self::default()
    }

    /// Inject the Claude provider. Existing handle (if any) is
    /// dropped; the trait's own shutdown path reaps its live
    /// sessions.
    pub async fn set_claude(&self, provider: Arc<dyn AgentProvider>) {
        *self.claude.write().await = Some(provider);
    }

    /// Inject the Codex provider. Same semantics as
    /// [`set_claude`](Self::set_claude).
    pub async fn set_codex(&self, provider: Arc<dyn AgentProvider>) {
        *self.codex.write().await = Some(provider);
    }

    /// Inject the OpenCode provider. Reserved for a later stage; the
    /// Stage 1 scaffold never calls this so the slot stays `None`
    /// in production.
    pub async fn set_opencode(&self, provider: Arc<dyn AgentProvider>) {
        *self.opencode.write().await = Some(provider);
    }

    /// Look up the provider for a given kind. Returns `None` when no
    /// provider has been registered — commands must return a clean
    /// error in that case instead of panicking.
    pub async fn get(&self, kind: ProviderKind) -> Option<Arc<dyn AgentProvider>> {
        match kind {
            ProviderKind::Claude => self.claude.read().await.clone(),
            ProviderKind::Codex => self.codex.read().await.clone(),
            ProviderKind::OpenCode => self.opencode.read().await.clone(),
        }
    }

    /// Snapshot every registered provider. Used at event-bridge
    /// startup time to subscribe every registry member in one pass.
    pub async fn all(&self) -> Vec<(ProviderKind, Arc<dyn AgentProvider>)> {
        let mut out = Vec::new();
        if let Some(p) = self.claude.read().await.clone() {
            out.push((ProviderKind::Claude, p));
        }
        if let Some(p) = self.codex.read().await.clone() {
            out.push((ProviderKind::Codex, p));
        }
        if let Some(p) = self.opencode.read().await.clone() {
            out.push((ProviderKind::OpenCode, p));
        }
        out
    }
}

fn provider_err(err: ProviderError) -> String {
    let ser: SerializableProviderError = err.to_serializable();
    serde_json::to_string(&ser).unwrap_or_else(|_| "provider_error".to_string())
}

async fn lookup_provider(
    registry: &ProviderRegistry,
    kind: ProviderKind,
) -> Result<Arc<dyn AgentProvider>, String> {
    registry
        .get(kind)
        .await
        .ok_or_else(|| format!("provider_not_configured: {kind:?}"))
}

// ── Lifecycle commands ────────────────────────────────────────────────

/// Create a new `agent_chat` pane in the given workspace.
///
/// Returns the new pane id on success. Fails cleanly when the
/// feature flag is off or the workspace does not exist.
///
/// `launch_mode` controls placement when the workspace already has
/// surfaces. `Some(NewTab)` opens a fresh tab; `Some(SplitPane)` (or
/// `None`) splits the active surface — the default matches the
/// historical behaviour relied on by materialise / sidebar / prestart
/// paths, where the chat IS the workspace and a fresh tab/surface is
/// created automatically when surfaces are empty.
#[tauri::command]
pub fn agent_chat_create_pane(
    app: AppHandle,
    state: State<'_, AppStateStore>,
    observability: State<'_, ObservabilityStore>,
    workspace_id: String,
    provider: Option<ProviderKind>,
    cwd: Option<String>,
    launch_mode: Option<crate::presets::LaunchMode>,
) -> Result<String, String> {
    feature_flag_on(&observability)?;
    let pane_id = state.create_agent_chat_pane(&workspace_id, provider, cwd, launch_mode)?;
    crate::state::emit_app_state(&app);
    Ok(pane_id.0)
}

/// Close an `agent_chat` pane.
///
/// Idempotent: calling twice (or on an unknown id) returns `Ok(())`
/// the second time instead of surfacing an error. The intent is to
/// match the frontend's "closing an already-closed pane is fine"
/// expectation; the feature-flag gate still applies.
#[tauri::command]
pub fn agent_chat_close_pane(
    app: AppHandle,
    state: State<'_, AppStateStore>,
    observability: State<'_, ObservabilityStore>,
    pane_id: String,
) -> Result<(), String> {
    feature_flag_on(&observability)?;
    // Capture the chat session bound to this pane *before* the tree
    // mutation so the cleanup path still has provider + thread_id to
    // hand to `stop_session`. Without this the JSON-RPC sidecar and
    // its background tokio tasks live on after the pane disappears.
    let chat_thread = state.agent_chat_pane_thread(&pane_id);
    // close_pane errors when the pane id is unknown — treat that as a
    // no-op to keep the command idempotent.
    let _ = state.close_pane(&pane_id);
    if let Some(pair) = chat_thread {
        shutdown_agent_chat_threads(&app, vec![pair]);
    }
    crate::state::emit_app_state(&app);
    Ok(())
}

// ── Dev-only harness ──────────────────────────────────────────────────

/// Spawn a chat pane in the currently-active workspace.
///
/// Dev builds only (gated behind `cfg!(debug_assertions)`). Also
/// gated on `enable_agent_chat`, since releasing a stub pane onto a
/// user whose flag is off would be surprising. Prints the new pane
/// id to stderr so it's easy to read back from the browser
/// devtools while smoke-testing.
#[tauri::command]
pub fn dev_agent_chat_spawn_test_pane(
    app: AppHandle,
    state: State<'_, AppStateStore>,
    observability: State<'_, ObservabilityStore>,
) -> Result<String, String> {
    if !cfg!(debug_assertions) {
        return Err("dev_only: debug build required".to_string());
    }
    feature_flag_on(&observability)?;
    let snapshot = state.snapshot();
    let workspace_id = snapshot.active_workspace_id.0;
    if workspace_id.is_empty() {
        return Err("no_active_workspace".to_string());
    }
    let pane_id = state.create_agent_chat_pane(&workspace_id, None, None, None)?;
    eprintln!(
        "[codemux::agent_chat] dev_agent_chat_spawn_test_pane created pane {} in workspace {workspace_id}",
        pane_id.0
    );
    crate::state::emit_app_state(&app);
    Ok(pane_id.0)
}

// ── Provider session commands ────────────────────────────────────────
//
// Each command:
// 1. Checks the feature flag.
// 2. Looks up the target provider on the registry.
// 3. Calls the matching trait method.
// 4. Returns the result (serialized errors carry the full
//    `SerializableProviderError` JSON so the UI can inspect subtype).

/// Start a new provider session for the given pane.
///
/// The returned [`ThreadId`] is the identifier the provider itself
/// minted — this command never re-mints a new id. Also writes the
/// thread id back onto the `AgentChat` pane so future look-ups can
/// resolve it without re-consulting the provider, and upserts an
/// `agent_chat_sessions` row so the history dropdown can surface
/// the session after a restart.
#[tauri::command]
pub async fn agent_chat_start_session(
    app: AppHandle,
    pane_id: String,
    provider: ProviderKind,
    input: StartSessionInput,
) -> Result<ThreadId, String> {
    let observability: State<'_, ObservabilityStore> = app.state();
    feature_flag_on(&observability)?;
    let registry: State<'_, ProviderRegistry> = app.state();
    let impl_ = lookup_provider(&registry, provider).await?;
    // Extract the cwd for persistence BEFORE moving input into the
    // provider: StartSessionInput is owned by the provider after
    // start_session().
    let cwd_for_persist = input
        .cwd
        .to_str()
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty());
    // Trace the resume wire so the dev console shows whether a
    // resume_cursor actually reached the provider, and with what
    // shape. Helps distinguish "frontend didn't send it" from
    // "provider didn't honour it".
    match input.resume_cursor.as_ref() {
        Some(cursor) => eprintln!(
            "[codemux::agent_chat] start_session pane={pane_id} provider={provider:?} resume_cursor={cursor}"
        ),
        None => eprintln!(
            "[codemux::agent_chat] start_session pane={pane_id} provider={provider:?} resume_cursor=(none)"
        ),
    }
    // Lazy MCP spawn: first chat session triggers child-process startup
    // for every enabled (non-disabled) MCP server discovered for this
    // workspace. Stage 3 needs the spawn to COMPLETE before
    // `start_session` runs so the tool snapshot the SDK sees actually
    // contains the running MCPs — otherwise the agent boots with zero
    // `mcp__codemux__*` tools and only sees the user's claude.ai
    // managed connectors. We bound the wait so a slow handshake
    // (`npx`-launched server pulling deps on first run) doesn't hang
    // chat-start forever. After the budget elapses tools are
    // registered with whatever has come up so far; servers that
    // finish later only become visible to the agent on the next
    // session start (Stage 4 polish will wire `setMcpServers` for
    // dynamic registration).
    {
        use crate::mcp::registry::McpRegistry;
        use std::time::Duration;
        let mcp_registry: State<'_, McpRegistry> = app.state();
        let registry = mcp_registry.inner().clone_handle();
        let project_path = input.cwd.clone();
        const MCP_PRIME_BUDGET: Duration = Duration::from_secs(8);
        match tokio::time::timeout(
            MCP_PRIME_BUDGET,
            registry.prime_for_chat(Some(&app), Some(&project_path)),
        )
        .await
        {
            Ok(()) => eprintln!(
                "[codemux::agent_chat] mcp prime_for_chat completed within budget"
            ),
            Err(_) => eprintln!(
                "[codemux::agent_chat] mcp prime_for_chat exceeded {}s budget — \
                 starting session with whatever tools are ready",
                MCP_PRIME_BUDGET.as_secs()
            ),
        }
    }
    let session = impl_.start_session(input).await.map_err(provider_err)?;
    let state: State<'_, AppStateStore> = app.state();
    state.set_agent_chat_thread_id(&pane_id, Some(session.thread_id.0.clone()));
    // Persist the session for the history dropdown. Scope is
    // (workspace_id, cwd): workspace lookup goes through the state
    // store because the command layer only knows the pane id.
    if let Some(workspace_id) = state.workspace_id_for_pane(&pane_id) {
        let db: State<'_, DatabaseStore> = app.state();
        let provider_str = match provider {
            ProviderKind::Claude => "claude",
            ProviderKind::Codex => "codex",
            ProviderKind::OpenCode => "opencode",
        };
        if let Err(error) = db.upsert_agent_chat_session(
            &session.thread_id.0,
            &workspace_id,
            cwd_for_persist.as_deref(),
            provider_str,
        ) {
            eprintln!(
                "[codemux::agent_chat] failed to persist session record: {error}"
            );
        }
    }
    // Opt-in rollback checkpoint (issue #80): snapshot the workspace
    // tree so this run can be rolled back. Fire-and-forget on the
    // blocking pool AFTER the session is live — nothing on the
    // first-token path awaits it, so checkpointing adds zero latency
    // to the agent's first response.
    spawn_run_checkpoint(&app, &session.thread_id.0, cwd_for_persist.as_deref());
    crate::state::emit_app_state(&app);
    Ok(session.thread_id)
}

/// Sanitize a thread id into a single git ref path component.
/// Alphanumerics, `-` and `_` pass through; everything else becomes
/// `-` so the result is always a valid `refs/codemux/...` segment.
pub fn checkpoint_ref_component(thread_id: &str) -> String {
    let out: String = thread_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    if out.is_empty() {
        "unknown".to_string()
    } else {
        out
    }
}

/// Fire the background run-start checkpoint when the opt-in
/// `git.agent_checkpoint_enabled` setting is on.
///
/// Everything — including the settings-cache read — runs on the
/// blocking pool so the caller returns immediately. Failures (not a
/// git repo, empty repo, setting off) are logged and never surface
/// as run errors; the checkpoint is best-effort by design.
fn spawn_run_checkpoint<R: Runtime>(app: &AppHandle<R>, thread_id: &str, cwd: Option<&str>) {
    let Some(cwd) = cwd.map(str::to_string) else {
        return;
    };
    let thread_id = thread_id.to_string();
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let enabled = crate::settings_sync::load_cache()
            .unwrap_or_default()
            .git
            .agent_checkpoint_enabled;
        if !enabled {
            return;
        }
        perform_run_checkpoint(&app, &thread_id, &cwd);
    });
}

/// Synchronous body of the run-start checkpoint: snapshot the tree,
/// pin the ref, persist the hashes on the session row. Extracted from
/// [`spawn_run_checkpoint`] (which adds the opt-in gate + background
/// spawn) so integration tests can drive it directly against a real
/// temp repository.
pub fn perform_run_checkpoint<R: Runtime>(app: &AppHandle<R>, thread_id: &str, cwd: &str) {
    let ref_name = format!(
        "refs/codemux/checkpoints/{}",
        checkpoint_ref_component(thread_id)
    );
    match crate::git::git_create_workspace_checkpoint(
        std::path::Path::new(cwd),
        &ref_name,
        "codemux: pre-run checkpoint",
    ) {
        Ok(checkpoint) => {
            let db: State<'_, DatabaseStore> = app.state();
            if let Err(error) =
                db.set_agent_chat_checkpoint(thread_id, &checkpoint.commit, &checkpoint.head)
            {
                eprintln!("[codemux::agent_chat] failed to persist run checkpoint: {error}");
            } else {
                eprintln!(
                    "[codemux::agent_chat] run checkpoint {} recorded for thread {thread_id}",
                    &checkpoint.commit[..12.min(checkpoint.commit.len())]
                );
            }
        }
        Err(error) => {
            // Expected for non-git workspaces / empty repos.
            eprintln!("[codemux::agent_chat] run checkpoint skipped: {error}");
        }
    }
}

/// The recorded run-start rollback checkpoint for a thread, if any.
/// Drives the visibility of the chat pane's "Restore checkpoint"
/// action. Not feature-gated for the same reason as
/// `agent_chat_list_sessions` — absence should render as "nothing to
/// show", not an error string.
#[tauri::command]
pub async fn agent_chat_get_checkpoint(
    db: State<'_, DatabaseStore>,
    thread_id: String,
) -> Result<Option<crate::git::WorkspaceCheckpoint>, String> {
    Ok(db
        .get_agent_chat_checkpoint(&thread_id)
        .map(|(commit, head)| crate::git::WorkspaceCheckpoint { commit, head }))
}

/// Roll the workspace tree back to the thread's run-start checkpoint.
///
/// Tree-only restore — branch refs never move (see
/// `git::git_restore_workspace_checkpoint`). A safety snapshot of the
/// pre-restore state is pinned under `refs/codemux/pre-restore/<thread>`
/// so the action is itself recoverable.
#[tauri::command]
pub async fn agent_chat_restore_checkpoint(
    app: AppHandle,
    thread_id: String,
) -> Result<(), String> {
    let observability: State<'_, ObservabilityStore> = app.state();
    feature_flag_on(&observability)?;
    let db: State<'_, DatabaseStore> = app.state();
    let (commit, _head) = db.get_agent_chat_checkpoint(&thread_id).ok_or_else(|| {
        "no_checkpoint: this thread has no recorded run checkpoint".to_string()
    })?;
    let cwd = db.get_agent_chat_session_cwd(&thread_id).ok_or_else(|| {
        "no_cwd: this session has no recorded working directory".to_string()
    })?;
    let safety_ref = format!(
        "refs/codemux/pre-restore/{}",
        checkpoint_ref_component(&thread_id)
    );
    tokio::task::spawn_blocking(move || {
        crate::git::git_restore_workspace_checkpoint(
            std::path::Path::new(&cwd),
            &commit,
            &safety_ref,
        )
        .map(|_| ())
    })
    .await
    .map_err(|e| format!("restore task join failed: {e}"))?
}

/// Queue a user turn on an existing session.
#[tauri::command]
pub async fn agent_chat_send_turn(
    app: AppHandle,
    provider: ProviderKind,
    input: SendTurnInput,
) -> Result<TurnId, String> {
    let observability: State<'_, ObservabilityStore> = app.state();
    feature_flag_on(&observability)?;
    let registry: State<'_, ProviderRegistry> = app.state();
    let impl_ = lookup_provider(&registry, provider).await?;
    // Capture the inputs we need for persistence before the provider
    // consumes `input`.
    let thread_id_for_persist = input.thread_id.0.clone();
    let user_text_for_persist = input.text.clone();
    let first_line = first_line_title(&input.text);
    let turn = impl_.send_turn(input).await.map_err(provider_err)?;

    // Best-effort: bump last_active_at so the session floats to the
    // top of the dropdown, and set an auto-title from the first user
    // turn if none exists yet. Failures here are non-fatal — a
    // missing persistence row must never block the turn.
    let db: State<'_, DatabaseStore> = app.state();
    let _ = db.touch_agent_chat_session(&thread_id_for_persist);
    if db.get_agent_chat_title(&thread_id_for_persist).is_none() {
        if let Some(title) = first_line {
            let _ = db.set_agent_chat_title(&thread_id_for_persist, &title);
        }
    }
    // Persist the user message envelope. User messages never come
    // back through the provider event stream, so this is the one
    // chance to record them. Best-effort: a failed write means
    // resume will skip this user turn but the live conversation is
    // unaffected.
    let user_msg = serde_json::json!({
        "type": "user_message",
        "thread_id": thread_id_for_persist,
        "text": user_text_for_persist,
    });
    if let Ok(payload) = serde_json::to_string(&user_msg) {
        let _ = db.append_agent_chat_message(&thread_id_for_persist, &payload);
    }
    Ok(turn.turn_id)
}

/// Derive a short dropdown title from the first user turn's text.
/// Takes the first non-empty line, trims, and caps at 60 chars. Returns
/// None for empty input so the DB column stays null.
fn first_line_title(text: &str) -> Option<String> {
    let line = text.lines().find(|l| !l.trim().is_empty())?;
    let trimmed = line.trim();
    let truncated: String = if trimmed.chars().count() > 60 {
        let mut s: String = trimmed.chars().take(57).collect();
        s.push_str("…");
        s
    } else {
        trimmed.to_string()
    };
    if truncated.is_empty() {
        None
    } else {
        Some(truncated)
    }
}

/// Interrupt the currently running turn on a thread.
#[tauri::command]
pub async fn agent_chat_interrupt_turn(
    app: AppHandle,
    provider: ProviderKind,
    thread_id: ThreadId,
    turn_id: Option<TurnId>,
) -> Result<(), String> {
    let observability: State<'_, ObservabilityStore> = app.state();
    feature_flag_on(&observability)?;
    let registry: State<'_, ProviderRegistry> = app.state();
    let impl_ = lookup_provider(&registry, provider).await?;
    impl_
        .interrupt_turn(thread_id, turn_id)
        .await
        .map_err(provider_err)
}

/// Respond to a pending approval / tool / input request.
#[tauri::command]
pub async fn agent_chat_respond_to_request(
    app: AppHandle,
    provider: ProviderKind,
    thread_id: ThreadId,
    request_id: RequestId,
    decision: ApprovalDecision,
) -> Result<(), String> {
    let observability: State<'_, ObservabilityStore> = app.state();
    feature_flag_on(&observability)?;
    let registry: State<'_, ProviderRegistry> = app.state();
    let impl_ = lookup_provider(&registry, provider).await?;
    impl_
        .respond_to_request(thread_id, request_id, decision)
        .await
        .map_err(provider_err)
}

/// Swap a session's active model. `model == None` is a validation
/// error — the trait's `set_model` takes a `String`, and the
/// resulting `ProviderError::ValidationError` is reflected here.
#[tauri::command]
pub async fn agent_chat_set_model(
    app: AppHandle,
    provider: ProviderKind,
    thread_id: ThreadId,
    model: Option<String>,
) -> Result<(), String> {
    let observability: State<'_, ObservabilityStore> = app.state();
    feature_flag_on(&observability)?;
    let registry: State<'_, ProviderRegistry> = app.state();
    let impl_ = lookup_provider(&registry, provider).await?;
    let model = model.ok_or_else(|| "validation_error: model required".to_string())?;
    impl_
        .set_model(thread_id, model)
        .await
        .map_err(provider_err)
}

/// Change a session's permission mode (accept-edits, bypass, plan,
/// etc.). The mode is passed through as a string; providers reject
/// unknown values via `ProviderError::ValidationError`.
#[tauri::command]
pub async fn agent_chat_set_permission_mode(
    app: AppHandle,
    provider: ProviderKind,
    thread_id: ThreadId,
    mode: String,
) -> Result<(), String> {
    let observability: State<'_, ObservabilityStore> = app.state();
    feature_flag_on(&observability)?;
    let registry: State<'_, ProviderRegistry> = app.state();
    let impl_ = lookup_provider(&registry, provider).await?;
    impl_
        .set_permission_mode(thread_id, mode)
        .await
        .map_err(provider_err)
}

/// Return the chat-side capabilities bundle for a provider.
///
/// Claude and Codex ship hand-maintained fallback data — live SDK /
/// `model/list` harvest is a follow-up that would merge live models
/// with the hand-maintained extras (see
/// `agent_provider/claude/capabilities.rs`).
///
/// OpenCode is the live arm: Stage 3 wires
/// [`harvest_opencode_capabilities`](crate::agent_provider::opencode::capabilities::harvest_opencode_capabilities)
/// through `OpenCodeServerManager::ensure_running` (lazy spawn,
/// idempotent) + `OpenCodeClient::list_models`. Failures fall back
/// to the empty placeholder bundle and the error rides on the
/// frontend store's `opencodeError` slot — Stage 6 surfaces it as a
/// "configure OpenCode" hint.
///
/// Originally gated on `enable_agent_chat` (Step 13) so non-Beta users
/// couldn't trigger `opencode serve` or `codex app-server` harvest.
/// That gate was lifted because the same capabilities bundle is now
/// consumed by the merge-resolver settings panel — a non-Beta surface
/// that legitimately needs the model list. The harvest is still safe:
/// Claude returns its static fallback (no spawning); Codex/OpenCode
/// each shell out only if their binary is on PATH and bubble a typed
/// "not installed" / "not authenticated" error otherwise, which the
/// frontend renders as a tooltip in the picker rail.
///
/// The chat session machinery (`agent_chat_send`, `agent_chat_resume`,
/// etc.) remains gated — the gate moved off "data discovery" and onto
/// "actually using the chat", which is the correct level.
#[tauri::command]
pub async fn list_chat_provider_capabilities(
    app: AppHandle,
    provider: ProviderKind,
    opencode_manager: tauri::State<
        '_,
        std::sync::Arc<crate::agent_provider::opencode::OpenCodeServerManager>,
    >,
    codex_cache: tauri::State<
        '_,
        std::sync::Arc<crate::agent_provider::codex::capabilities::CodexCapabilityCache>,
    >,
) -> Result<ProviderChatCapabilities, String> {
    // Note: `feature_flag_on(&observability)?;` was deliberately
    // removed when settings began consuming capabilities. See the
    // function-level comment for the rationale. The `app` handle is
    // still in the signature because removing it would be a public
    // API break; downstream consumers may add it back if they grow a
    // need for app-handle-scoped state.
    let _ = app;
    match provider {
        ProviderKind::Claude => Ok(
            crate::agent_provider::claude::capabilities::claude_fallback_capabilities(),
        ),
        ProviderKind::Codex => {
            // Live harvest from `codex app-server` via `model/list`,
            // memoised in the Tauri-managed cache. The fallback static
            // catalog is gone — drift between the picker and the real
            // SDK is exactly the bug Stage 9 is paying down.
            let binary_path = which::which("codex").map_err(|_| {
                crate::agent_provider::codex::capabilities::HarvestError::NotInstalled {
                    hint: "Install Codex CLI from https://github.com/openai/codex and ensure `codex` is on PATH.".into(),
                }
                .to_command_string()
            })?;
            codex_cache
                .get_or_harvest(&binary_path, None)
                .await
                .map_err(|err| err.to_command_string())
        }
        ProviderKind::OpenCode => {
            crate::agent_provider::opencode::capabilities::harvest_opencode_capabilities(
                opencode_manager.inner().as_ref(),
            )
            .await
        }
    }
}

/// Gracefully terminate a session. Idempotent on the provider side.
#[tauri::command]
pub async fn agent_chat_stop_session(
    app: AppHandle,
    provider: ProviderKind,
    thread_id: ThreadId,
) -> Result<(), String> {
    let observability: State<'_, ObservabilityStore> = app.state();
    feature_flag_on(&observability)?;
    let registry: State<'_, ProviderRegistry> = app.state();
    let impl_ = lookup_provider(&registry, provider).await?;
    impl_.stop_session(thread_id).await.map_err(provider_err)
}

/// Fire-and-forget cleanup for chat sessions whose owning pane/tab/workspace
/// just got removed. Without this, `provider.stop_session` is never called
/// during workspace or tab close — the session keeps its JSON-RPC sidecar
/// child alive and its background tokio tasks hold `Arc<Session>` in a
/// refcycle with the session's own `JoinHandle` vec, so `Drop` never
/// fires. Each closed worktree leaks one sidecar process plus its task
/// graph until the whole app is killed.
///
/// Skips the feature-flag gate intentionally: the gate guards new session
/// creation, but already-running sessions must be reaped regardless of
/// whether the flag has since been flipped off.
pub fn shutdown_agent_chat_threads(
    app: &AppHandle,
    threads: Vec<(ProviderKind, String)>,
) {
    if threads.is_empty() {
        return;
    }
    let app_handle = app.clone();
    tauri::async_runtime::spawn(async move {
        let registry: State<'_, ProviderRegistry> = app_handle.state();
        for (kind, thread_id) in threads {
            let Some(impl_) = registry.get(kind).await else {
                continue;
            };
            if let Err(error) = impl_.stop_session(ThreadId(thread_id.clone())).await {
                eprintln!(
                    "[agent_chat] cleanup stop_session failed for {kind:?} {thread_id}: {error:?}"
                );
            }
        }
    });
}

// ── Session history (for the pane-header dropdown) ──────────────────

/// List persisted chat sessions for the history dropdown.
///
/// Scope is (workspace_id, optional cwd). When `cwd` is present the
/// list is narrowed to sessions opened from that exact directory so
/// the dropdown matches the pane the user is looking at; passing
/// `None` returns every session in the workspace regardless of
/// worktree.
///
/// Not gated on the feature flag: the dropdown UI should render an
/// empty list rather than surface a raw error string when the flag is
/// off.
#[tauri::command]
pub async fn agent_chat_list_sessions(
    db: State<'_, DatabaseStore>,
    workspace_id: String,
    cwd: Option<String>,
    limit: Option<u32>,
) -> Result<Vec<AgentChatSessionRecord>, String> {
    let limit = limit.unwrap_or(50);
    Ok(db.list_agent_chat_sessions(&workspace_id, cwd.as_deref(), limit))
}

/// Rename a persisted chat session. Used by the dropdown's per-row
/// "Rename" affordance.
#[tauri::command]
pub async fn agent_chat_rename_session(
    db: State<'_, DatabaseStore>,
    thread_id: String,
    title: String,
) -> Result<(), String> {
    let trimmed = title.trim();
    if trimmed.is_empty() {
        return Err("validation_error: title cannot be empty".to_string());
    }
    db.set_agent_chat_title(&thread_id, trimmed)
}

/// Delete a persisted chat session. Idempotent. Note this does not
/// stop a live session — the UI should call `agent_chat_stop_session`
/// first if the row being deleted is the current pane's active chat.
/// Persisted messages cascade-delete from the FK on agent_chat_messages.
#[tauri::command]
pub async fn agent_chat_delete_session(
    db: State<'_, DatabaseStore>,
    thread_id: String,
) -> Result<(), String> {
    db.delete_agent_chat_session(&thread_id)
}

/// Return the persisted transcript for a thread.
///
/// Each element is a JSON-encoded envelope — typically a
/// `ProviderRuntimeEvent` (`ItemCompleted`, `TurnCompleted`,
/// `RequestOpened`, `RequestResolved`) or a synthetic
/// `{"type":"user_message","text":...}` record. The frontend's
/// hydrate action parses each payload and dispatches it through the
/// same pure reducer that handles live events, rebuilding the
/// transcript for resume.
///
/// Not gated on the feature flag for the same reason as
/// `agent_chat_list_sessions` — the dropdown should silently render
/// an empty list rather than surfacing a feature-flag error string.
#[tauri::command]
pub async fn agent_chat_list_messages(
    db: State<'_, DatabaseStore>,
    thread_id: String,
) -> Result<Vec<String>, String> {
    Ok(db.list_agent_chat_messages(&thread_id))
}

// ── Streaming channel attach/detach ───────────────────────────────────

/// Attach a per-thread streaming [`Channel`] for live provider events.
///
/// Called by the frontend when a chat pane binds to a thread. Live
/// events (especially the `content_delta` token stream) arrive over
/// this channel only; the pane hydrates the persisted transcript from
/// the DB via `agent_chat_list_messages` on mount, so a late attach
/// never misses completed items — the channel carries live deltas, the
/// DB carries history.
///
/// Returns the subscription id to pass to `detach_agent_chat_output`
/// on unmount.
#[tauri::command]
pub fn attach_agent_chat_output(
    observability: State<'_, ObservabilityStore>,
    channels: State<'_, AgentChatChannelRegistry>,
    thread_id: String,
    channel: Channel<AgentChatEventPayload>,
) -> Result<u32, String> {
    feature_flag_on(&observability)?;
    Ok(channels.attach(&thread_id, channel))
}

/// Detach a previously attached chat output channel.
///
/// Idempotent, and deliberately NOT feature-gated: unmount cleanup
/// must always succeed, even if the beta flag was switched off while
/// the pane was open.
#[tauri::command]
pub fn detach_agent_chat_output(
    channels: State<'_, AgentChatChannelRegistry>,
    thread_id: String,
    subscription_id: u32,
) -> Result<(), String> {
    channels.detach(&thread_id, subscription_id);
    Ok(())
}

// ── Event bridge ──────────────────────────────────────────────────────

/// Start the provider-event forwarding tasks.
///
/// Spawns one Tokio task per registered provider. Each task consumes
/// the provider's canonical event stream and routes each event to the
/// per-thread channels registered in [`AgentChatChannelRegistry`]
/// (threadless events fall back to the [`AGENT_CHAT_EVENT`] bus).
/// When a provider's stream ends (on shutdown) the task exits cleanly;
/// the `event_stream()` helper on each provider already swallows
/// `broadcast::error::RecvError::Lagged` and continues, so slow
/// subscribers never crash this loop.
///
/// Intended to be called once after the registry has been fully
/// populated at startup. Idempotency is not required — call sites
/// guarantee a single call.
pub async fn spawn_event_bridge<R: Runtime>(app: AppHandle<R>) {
    let registry: State<'_, ProviderRegistry> = app.state();
    let providers = registry.all().await;
    for (kind, provider) in providers {
        let app = app.clone();
        let mut stream = provider.event_stream();
        tauri::async_runtime::spawn(async move {
            use futures_util::StreamExt;
            eprintln!(
                "[codemux::agent_chat] event bridge started for {kind:?}"
            );
            while let Some(event) = stream.next().await {
                forward_event(&app, event);
            }
            eprintln!(
                "[codemux::agent_chat] event bridge for {kind:?} ended (provider stream closed)"
            );
        });
    }
}

/// Forward a single provider event to the frontend.
///
/// Thread-scoped events are routed to the per-thread streaming
/// channels in [`AgentChatChannelRegistry`]; only threadless events
/// (global `RuntimeWarning`s) fall back to the legacy
/// [`AGENT_CHAT_EVENT`] broadcast, since they have no owning pane.
///
/// Extracted so tests can exercise the translation without spinning a
/// Tokio task or a real provider. Also used as the inner loop of
/// [`spawn_event_bridge`]. As a side effect, persists the SDK session
/// UUID carried by `ResumeCursorUpdated` so the history dropdown can
/// resume this session after a restart.
pub fn forward_event<R: Runtime>(app: &AppHandle<R>, event: ProviderRuntimeEvent) {
    if let ProviderRuntimeEvent::ResumeCursorUpdated {
        thread_id,
        resume_cursor,
    } = &event
    {
        if let Some(sdk_session_id) = extract_sdk_session_id(resume_cursor) {
            let db: State<'_, DatabaseStore> = app.state();
            if let Err(error) =
                db.set_agent_chat_sdk_session_id(&thread_id.0, &sdk_session_id)
            {
                eprintln!(
                    "[codemux::agent_chat] failed to persist sdk_session_id: {error}"
                );
            }
            // Resume creates a fresh DB row (new thread_id) carrying the
            // same sdk_session_id as the original. Collapse the
            // duplicates so the dropdown doesn't grow unboundedly.
            if let Err(error) =
                db.collapse_duplicate_agent_chat_sessions(&sdk_session_id)
            {
                eprintln!(
                    "[codemux::agent_chat] failed to collapse duplicates: {error}"
                );
            }
        }
    }
    // Best-effort transcript persistence so the SessionSelector resume
    // path can replay the visible conversation. We only persist events
    // that mutate the rendered transcript; partial deltas, lifecycle
    // notices, and runtime warnings are skipped:
    //   - content_delta: replaced by the trailing item_completed
    //   - session_state_changed / session_configured: lifecycle only
    //   - runtime_warning: console-only by design
    //   - resume_cursor_updated: already persisted above
    if should_persist_event(&event) {
        if let Some(thread_id) = thread_id_for_event(&event) {
            if !thread_id.0.is_empty() {
                if let Ok(payload) = serde_json::to_string(&event) {
                    let db: State<'_, DatabaseStore> = app.state();
                    if let Err(error) =
                        db.append_agent_chat_message(&thread_id.0, &payload)
                    {
                        eprintln!(
                            "[codemux::agent_chat] failed to persist message: {error}"
                        );
                    }
                }
            }
        }
    }
    match thread_id_for_event(&event) {
        Some(thread_id) if !thread_id.0.is_empty() => {
            // Thread-scoped: stream over the per-thread channels. If
            // no pane is attached the event is dropped live — the DB
            // persistence above already covers transcript replay, and
            // partial deltas are superseded by their item_completed.
            let channels: State<'_, AgentChatChannelRegistry> = app.state();
            channels.route(&AgentChatEventPayload { thread_id, event });
        }
        _ => {
            // Threadless (global RuntimeWarning): no per-thread
            // subscriber exists, so keep the legacy low-frequency
            // broadcast with an empty ThreadId.
            let payload = AgentChatEventPayload {
                thread_id: ThreadId(String::new()),
                event,
            };
            if let Err(error) = app.emit(AGENT_CHAT_EVENT, &payload) {
                eprintln!("[codemux::agent_chat] Failed to emit {AGENT_CHAT_EVENT}: {error}");
            }
        }
    }
}

/// Whether a canonical provider event should be written to
/// `agent_chat_messages`. Extracted so the policy is unit-testable
/// without an `AppHandle`.
pub fn should_persist_event(event: &ProviderRuntimeEvent) -> bool {
    matches!(
        event,
        ProviderRuntimeEvent::ItemCompleted { .. }
            | ProviderRuntimeEvent::TurnCompleted { .. }
            | ProviderRuntimeEvent::RequestOpened { .. }
            | ProviderRuntimeEvent::RequestResolved { .. }
    )
}

/// Pull the SDK session UUID out of the opaque `resume_cursor` JSON.
///
/// The Claude adapter wraps the id under a `resume` or `sessionId`
/// key (same shape the adapter accepts on the way in — see
/// `agent_provider/claude/session.rs`). Extracted so the logic is
/// unit-testable without spinning up an app handle.
pub fn extract_sdk_session_id(cursor: &serde_json::Value) -> Option<String> {
    cursor
        .get("resume")
        .or_else(|| cursor.get("sessionId"))
        .or_else(|| cursor.get("session_id"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

/// Extract the thread id carried by a provider runtime event, or
/// `None` for events that are not bound to a specific thread
/// (e.g. global `RuntimeWarning`s).
pub fn thread_id_for_event(event: &ProviderRuntimeEvent) -> Option<ThreadId> {
    match event {
        ProviderRuntimeEvent::SessionConfigured { thread_id, .. }
        | ProviderRuntimeEvent::ContentDelta { thread_id, .. }
        | ProviderRuntimeEvent::ItemCompleted { thread_id, .. }
        | ProviderRuntimeEvent::TurnCompleted { thread_id, .. }
        | ProviderRuntimeEvent::RequestOpened { thread_id, .. }
        | ProviderRuntimeEvent::RequestResolved { thread_id, .. }
        | ProviderRuntimeEvent::SessionStateChanged { thread_id, .. }
        | ProviderRuntimeEvent::ResumeCursorUpdated { thread_id, .. } => Some(thread_id.clone()),
        ProviderRuntimeEvent::RuntimeWarning { thread_id, .. } => thread_id.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn first_line_title_takes_first_nonempty_line() {
        assert_eq!(
            first_line_title("Hello there\nsecond line"),
            Some("Hello there".into())
        );
    }

    #[test]
    fn first_line_title_skips_leading_blank_lines() {
        assert_eq!(
            first_line_title("\n\n  First real content  \n"),
            Some("First real content".into())
        );
    }

    #[test]
    fn first_line_title_returns_none_for_empty_or_whitespace() {
        assert_eq!(first_line_title(""), None);
        assert_eq!(first_line_title("   \n  \n"), None);
    }

    #[test]
    fn first_line_title_truncates_long_lines() {
        let long = "x".repeat(200);
        let title = first_line_title(&long).unwrap();
        // 57 chars + ellipsis
        assert_eq!(title.chars().count(), 58);
        assert!(title.ends_with('…'));
    }

    #[test]
    fn first_line_title_handles_multibyte_correctly() {
        // 100 chinese chars → each counts as 1 in .chars(); should
        // truncate to 57 + …
        let multi: String = std::iter::repeat('漢').take(100).collect();
        let title = first_line_title(&multi).unwrap();
        assert_eq!(title.chars().count(), 58);
    }

    #[test]
    fn extract_sdk_session_id_handles_all_three_keys() {
        assert_eq!(
            extract_sdk_session_id(&json!({"resume": "uuid-a"})),
            Some("uuid-a".into())
        );
        assert_eq!(
            extract_sdk_session_id(&json!({"sessionId": "uuid-b"})),
            Some("uuid-b".into())
        );
        assert_eq!(
            extract_sdk_session_id(&json!({"session_id": "uuid-c"})),
            Some("uuid-c".into())
        );
    }

    #[test]
    fn extract_sdk_session_id_prefers_resume_key() {
        let both = json!({"resume": "first", "sessionId": "second"});
        assert_eq!(extract_sdk_session_id(&both), Some("first".into()));
    }

    #[test]
    fn extract_sdk_session_id_returns_none_for_missing_or_non_string() {
        assert_eq!(extract_sdk_session_id(&json!({})), None);
        assert_eq!(extract_sdk_session_id(&json!("just-a-string")), None);
        assert_eq!(extract_sdk_session_id(&json!({"resume": 42})), None);
    }

    // ── should_persist_event policy ──
    //
    // Pinned tests: changing this filter changes the on-disk format
    // contract. Adding a transcript-relevant variant means flipping
    // its case here AND adding a hydrate-side handler in the
    // frontend `replayPayloads` tests. Adding a UI-only variant
    // means adding a `not persisted` row.

    use crate::agent_provider::events::{
        CompletedItem, ContentDelta, TurnStatus, TurnUsage,
    };
    use crate::agent_provider::types::{
        ApprovalDecision, ProviderSessionId, RequestId, SessionStatus,
    };

    fn tid() -> ThreadId {
        ThreadId("t".into())
    }
    fn turn() -> TurnId {
        TurnId("turn-1".into())
    }
    fn req() -> RequestId {
        RequestId("req-1".into())
    }

    #[test]
    fn should_persist_item_completed_assistant_text() {
        let e = ProviderRuntimeEvent::ItemCompleted {
            thread_id: tid(),
            turn_id: turn(),
            item: CompletedItem::AssistantText {
                text: "hi".into(),
            },
        };
        assert!(should_persist_event(&e));
    }

    #[test]
    fn should_persist_item_completed_tool_use_and_result() {
        let tool_use = ProviderRuntimeEvent::ItemCompleted {
            thread_id: tid(),
            turn_id: turn(),
            item: CompletedItem::ToolUse {
                tool_name: "Read".into(),
                input: json!({"path": "/x"}),
                tool_use_id: "tu-1".into(),
            },
        };
        assert!(should_persist_event(&tool_use));
        let tool_result = ProviderRuntimeEvent::ItemCompleted {
            thread_id: tid(),
            turn_id: turn(),
            item: CompletedItem::ToolResult {
                tool_use_id: "tu-1".into(),
                content: json!("ok"),
                is_error: false,
            },
        };
        assert!(should_persist_event(&tool_result));
    }

    #[test]
    fn should_persist_turn_completed() {
        let e = ProviderRuntimeEvent::TurnCompleted {
            thread_id: tid(),
            turn_id: turn(),
            status: TurnStatus::Success,
            usage: Some(TurnUsage {
                total_cost_usd: None,
                duration_ms: 1,
                num_turns: 1,
            }),
        };
        assert!(should_persist_event(&e));
    }

    #[test]
    fn should_persist_request_open_and_resolve() {
        let opened = ProviderRuntimeEvent::RequestOpened {
            thread_id: tid(),
            turn_id: turn(),
            request_id: req(),
            request_kind: "tool".into(),
            payload: json!({}),
            tool_use_id: None,
        };
        assert!(should_persist_event(&opened));
        let resolved = ProviderRuntimeEvent::RequestResolved {
            thread_id: tid(),
            request_id: req(),
            decision: ApprovalDecision::AllowForSession,
        };
        assert!(should_persist_event(&resolved));
    }

    #[test]
    fn should_not_persist_content_delta() {
        // Replaced by the trailing item_completed (assistant_text).
        // Persisting both would produce duplicates on replay.
        let e = ProviderRuntimeEvent::ContentDelta {
            thread_id: tid(),
            turn_id: turn(),
            delta: ContentDelta::Text { text: "hi".into() },
        };
        assert!(!should_persist_event(&e));
    }

    #[test]
    fn should_not_persist_session_state_changed() {
        let e = ProviderRuntimeEvent::SessionStateChanged {
            thread_id: tid(),
            status: SessionStatus::Ready,
        };
        assert!(!should_persist_event(&e));
    }

    #[test]
    fn should_not_persist_session_configured() {
        let e = ProviderRuntimeEvent::SessionConfigured {
            thread_id: tid(),
            provider_session_id: ProviderSessionId("s".into()),
        };
        assert!(!should_persist_event(&e));
    }

    #[test]
    fn should_not_persist_runtime_warning() {
        // Console-only by design (see reducer's `runtime_warning`
        // case — `console.warn` then return state unchanged).
        let e = ProviderRuntimeEvent::RuntimeWarning {
            thread_id: Some(tid()),
            message: "oops".into(),
            original_payload: None,
        };
        assert!(!should_persist_event(&e));
    }

    #[test]
    fn should_not_persist_resume_cursor_updated() {
        // Already persisted via the dedicated agent_chat_sessions
        // sdk_session_id column; persisting it twice would clutter
        // the message replay log without adding any rendered output.
        let e = ProviderRuntimeEvent::ResumeCursorUpdated {
            thread_id: tid(),
            resume_cursor: json!({"resume": "uuid"}),
        };
        assert!(!should_persist_event(&e));
    }
}
