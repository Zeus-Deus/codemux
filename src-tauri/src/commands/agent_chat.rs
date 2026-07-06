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
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
use tauri::{AppHandle, Emitter, Manager, Runtime, State};

use crate::agent_provider::{
    AgentProvider, ApprovalDecision, ProviderChatCapabilities, ProviderError, ProviderKind,
    ProviderRuntimeEvent, RequestId, SendTurnInput, SerializableProviderError,
    StartSessionInput, ThreadId, TurnId,
};
use crate::database::{AgentChatCheckpointRecord, AgentChatSessionRecord, DatabaseStore};
use crate::observability::ObservabilityStore;
use crate::state::AppStateStore;

/// Event name used for the few provider events that are NOT scoped to
/// a single thread (global `RuntimeWarning`s with no `thread_id`).
///
/// Thread-scoped events — including the high-frequency streaming
/// `content_delta` tokens — do NOT ride this global event bus anymore.
/// They are routed per-thread through a `tauri::ipc::Channel`
/// registered via [`attach_agent_chat_output`], mirroring how PTY
/// output streams through `attach_pty_output` (see
/// `terminal/mod.rs`). Tauri's event system is JSON-broadcast to every
/// listener and not designed for high-throughput streaming; Channels
/// are the documented mechanism for exactly this case.
pub const AGENT_CHAT_EVENT: &str = "agent_chat_event";

/// Payload delivered to the frontend for every provider runtime event.
///
/// Thread-scoped events arrive over the per-thread `Channel` attached
/// via [`attach_agent_chat_output`]; events without a thread id
/// (global `RuntimeWarning`s) are emitted on the [`AGENT_CHAT_EVENT`]
/// event bus with an empty `ThreadId`. The shape is identical on both
/// transports so the frontend reducer does not care which path an
/// event took.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentChatEventPayload {
    pub thread_id: ThreadId,
    pub event: ProviderRuntimeEvent,
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

// ── Per-thread live event channels ───────────────────────────────────

/// A live subscriber for one thread's runtime events.
struct AgentChatChannelEntry {
    channel: Channel<AgentChatEventPayload>,
    /// Attach generation minted by the registry. A detach only removes
    /// the entry when its generation matches, so a stale detach from a
    /// pane that already lost the thread (unmount racing a remount, or
    /// a second pane re-attaching the same thread) can never tear down
    /// the channel a newer subscriber just installed. Same guard idea
    /// as `SessionRuntime::output_channel_generation` on the PTY path.
    generation: u64,
}

/// Registry of per-thread frontend event channels (Tauri-managed
/// state).
///
/// Analogous to `SessionRuntime.output_channel` in `terminal/mod.rs`:
/// when a chat pane mounts with a bound thread it invokes
/// [`attach_agent_chat_output`] with a fresh `Channel`; the event
/// bridge ([`forward_event`]) then routes that thread's live events —
/// crucially the high-frequency `content_delta` token stream — to
/// exactly that channel instead of broadcasting them app-wide.
///
/// Replay split (decided in issue #75): the channel carries **live
/// events only**. Transcript-affecting events are persisted to SQLite
/// by `forward_event` regardless of whether a channel is attached, and
/// a late-attaching / resumed pane rebuilds history through the
/// existing DB hydrate (`agent_chat_list_messages` +
/// `hydrateThread`). Events that fire while no channel is attached and
/// that are not persisted (lifecycle notices like
/// `session_state_changed`) are dropped — exactly the behaviour the
/// event-bus path had for unmounted panes, where no listener was
/// registered to hear them.
#[derive(Default)]
pub struct AgentChatChannelRegistry {
    channels: Mutex<HashMap<String, AgentChatChannelEntry>>,
    next_generation: AtomicU64,
}

impl AgentChatChannelRegistry {
    /// Install `channel` as the live subscriber for `thread_id`,
    /// replacing any previous subscriber (newest pane wins, like a PTY
    /// reattach). Returns the generation token the caller must hand
    /// back to [`detach`](Self::detach).
    pub fn attach(
        &self,
        thread_id: &str,
        channel: Channel<AgentChatEventPayload>,
    ) -> u64 {
        let generation = self.next_generation.fetch_add(1, Ordering::Relaxed) + 1;
        let mut channels = self.channels.lock().expect("channel registry poisoned");
        channels.insert(
            thread_id.to_string(),
            AgentChatChannelEntry {
                channel,
                generation,
            },
        );
        generation
    }

    /// Remove the subscriber for `thread_id`, but only when
    /// `generation` still matches the installed entry. Returns whether
    /// an entry was actually removed. Idempotent: detaching an unknown
    /// thread or a superseded generation is a no-op.
    pub fn detach(&self, thread_id: &str, generation: u64) -> bool {
        let mut channels = self.channels.lock().expect("channel registry poisoned");
        match channels.get(thread_id) {
            Some(entry) if entry.generation == generation => {
                channels.remove(thread_id);
                true
            }
            _ => false,
        }
    }

    /// Clone the live channel for `thread_id`, if any. Cloning is
    /// cheap (the `Channel` is internally ref-counted) and keeps the
    /// lock scope tight on the hot streaming path.
    pub fn channel_for(&self, thread_id: &str) -> Option<Channel<AgentChatEventPayload>> {
        let channels = self.channels.lock().expect("channel registry poisoned");
        channels.get(thread_id).map(|entry| entry.channel.clone())
    }
}

/// Register a per-thread `Channel` that will receive every live
/// runtime event for `thread_id` (see [`AgentChatChannelRegistry`]).
/// Returns the attach generation; the frontend passes it back to
/// [`detach_agent_chat_output`] on unmount so a stale detach cannot
/// clobber a newer attach.
///
/// Not gated on `enable_agent_chat`, same rationale as
/// `agent_chat_list_messages`: a pane bound to a session that predates
/// a flag flip-off must still be able to stream/teardown cleanly, and
/// attaching is side-effect-free beyond the registry entry.
#[tauri::command]
pub fn attach_agent_chat_output(
    channels: State<'_, AgentChatChannelRegistry>,
    thread_id: String,
    channel: Channel<AgentChatEventPayload>,
) -> Result<u64, String> {
    if thread_id.is_empty() {
        return Err("validation_error: thread_id cannot be empty".to_string());
    }
    Ok(channels.attach(&thread_id, channel))
}

/// Tear down the per-thread channel installed by
/// [`attach_agent_chat_output`]. Idempotent; a mismatched generation
/// (a newer pane re-attached first) is a silent no-op.
#[tauri::command]
pub fn detach_agent_chat_output(
    channels: State<'_, AgentChatChannelRegistry>,
    thread_id: String,
    generation: u64,
) -> Result<(), String> {
    channels.detach(&thread_id, generation);
    Ok(())
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

/// Build the workspace-scoped environment overlay a chat agent's child
/// processes need so `codemux browser open` — and any other `codemux` CLI
/// the agent shells out to — routes to the agent's OWN workspace rather
/// than whichever workspace the user happens to be viewing.
///
/// Terminal-spawned agents get this env at PTY spawn time (see
/// `crate::terminal::spawn_pty_for_session_in_process`); the chat sidecar
/// otherwise spawns with a bare env, so its `codemux browser open` calls
/// reach the control layer with an empty `workspace_id` and fall through to
/// the "active workspace" legacy path — the routing bug this overlay fixes.
///
/// Mirrors the `CODEMUX_*` surface a terminal PTY injects and reuses the
/// exact [`crate::terminal::workspace_pty_env`] helper for the
/// workspace-level vars (`CODEMUX_WORKSPACE_NAME`, `CODEMUX_AGENT_CONTEXT`,
/// `CODEMUX_WORKSPACE_PATH`/`ROOT_PATH`/`BRANCH`/`PORT`) so the two surfaces
/// stay in lockstep. `pane_id` is the chat pane the session runs in,
/// surfaced as `CODEMUX_PANE_ID` for hook / browser routing.
///
/// Semantics:
/// * `ws == None` (orphaned pane, no owning workspace) → returns `existing`
///   untouched so no workspace context leaks in, matching the terminal
///   path's behavior for sessions with no owning workspace.
/// * entries already present in `existing` are never overwritten — an env
///   value the caller explicitly supplied always wins over the workspace
///   default.
fn workspace_env_overlay(
    ws: Option<&crate::state::WorkspaceSnapshot>,
    pane_id: &str,
    existing: Option<HashMap<String, String>>,
) -> Option<HashMap<String, String>> {
    let Some(ws) = ws else {
        return existing;
    };
    let mut env = existing.unwrap_or_default();

    // Session-level vars every Codemux surface advertises, followed by the
    // workspace-level vars the terminal path injects. Collected into one
    // list so the insert-if-absent pass below treats them uniformly.
    let mut defaults: Vec<(String, String)> = vec![
        ("CODEMUX".to_string(), "1".to_string()),
        (
            "CODEMUX_VERSION".to_string(),
            env!("CARGO_PKG_VERSION").to_string(),
        ),
        ("CODEMUX_WORKSPACE_ID".to_string(), ws.workspace_id.0.clone()),
        ("CODEMUX_PANE_ID".to_string(), pane_id.to_string()),
        ("CODEMUX_BROWSER_CMD".to_string(), "codemux browser".to_string()),
        ("BROWSER".to_string(), "codemux browser open".to_string()),
    ];
    defaults.extend(crate::terminal::workspace_pty_env(ws));

    for (key, val) in defaults {
        // Insert-if-absent so a caller-provided entry always wins.
        env.entry(key).or_insert(val);
    }
    Some(env)
}

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
    mut input: StartSessionInput,
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
    // Overlay the chat pane's workspace env onto `input.env` BEFORE the
    // provider consumes it, so the agent's browser/CLI subprocesses route to
    // the agent's own workspace instead of the user's active one. Resolve the
    // owning workspace from the pane; an orphaned pane (no workspace) injects
    // nothing, matching the terminal path's behavior.
    {
        let state: State<'_, AppStateStore> = app.state();
        let workspace_id = state.workspace_id_for_pane(&pane_id);
        let snapshot = state.snapshot();
        let ws = workspace_id
            .as_ref()
            .and_then(|id| snapshot.workspaces.iter().find(|w| &w.workspace_id.0 == id));
        input.env = workspace_env_overlay(ws, &pane_id, input.env.take());
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
        // Issue #80 — optional rollback checkpoint. Spawned AFTER the
        // provider session is up and the session row is persisted, so
        // not a single git operation (or even the settings-cache read)
        // sits on the latency-to-first-token path. The opt-in gate is
        // evaluated inside the task.
        if let Some(cwd) = cwd_for_persist.clone() {
            spawn_run_checkpoint(
                &app,
                session.thread_id.0.clone(),
                workspace_id.clone(),
                cwd,
            );
        }
    }
    crate::state::emit_app_state(&app);
    Ok(session.thread_id)
}

// ── Run checkpoints (issue #80) ──────────────────────────────────────
//
// When the (opt-in) setting is on, every session start snapshots the
// working tree in the BACKGROUND via `git_checkpoint_create` — a
// non-destructive shadow-ref commit that leaves the user's index,
// worktree, and stash list untouched. The snapshot is recorded against
// the thread so the pane header can offer "Restore checkpoint".

/// Event emitted when a background checkpoint lands, so the pane
/// header can reveal the restore affordance without polling.
pub const AGENT_CHAT_CHECKPOINT_EVENT: &str = "agent_chat_checkpoint";

/// Payload emitted on [`AGENT_CHAT_CHECKPOINT_EVENT`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentChatCheckpointEventPayload {
    pub thread_id: ThreadId,
    pub checkpoint: AgentChatCheckpointRecord,
}

/// Whether the user opted into run-start checkpoints. Reads the synced
/// settings cache (same pattern as session-restore in
/// `terminal/mod.rs`); default is OFF.
pub fn run_checkpoints_enabled() -> bool {
    crate::settings_sync::load_cache()
        .map(|s| s.agent_chat.checkpoints_enabled)
        .unwrap_or(false)
}

/// Synchronous core of the background checkpoint: snapshot the repo at
/// `repo_path`, persist the bookkeeping row, and prune old shadow refs
/// (dropping their rows too). Blocking — run it on a blocking thread.
///
/// Returns `Ok(None)` when there was nothing to checkpoint (not a git
/// repo / unborn HEAD). Public so integration tests can exercise the
/// full create→restore round trip without a Tauri runtime.
pub fn create_run_checkpoint_blocking(
    db: &DatabaseStore,
    repo_path: &std::path::Path,
    thread_id: &str,
    workspace_id: &str,
) -> Result<Option<AgentChatCheckpointRecord>, String> {
    let ref_name = crate::git::checkpoint_ref_name(thread_id);
    let message = format!("codemux checkpoint: before agent run {thread_id}");
    let Some(snapshot) = crate::git::git_checkpoint_create(repo_path, &ref_name, &message)?
    else {
        return Ok(None);
    };
    let record = AgentChatCheckpointRecord {
        thread_id: thread_id.to_string(),
        workspace_id: workspace_id.to_string(),
        repo_path: repo_path.to_string_lossy().to_string(),
        ref_name: snapshot.ref_name.clone(),
        snapshot_commit: snapshot.snapshot_commit.clone(),
        head_commit: snapshot.head_commit.clone(),
        branch: snapshot.branch.clone(),
        created_at: String::new(), // assigned by SQLite
    };
    db.upsert_agent_chat_checkpoint(&record)?;
    // Cap shadow-ref growth. Best-effort: a prune failure must not
    // fail the checkpoint that already landed.
    for namespace in [
        crate::git::CHECKPOINT_REF_PREFIX,
        crate::git::PRE_RESTORE_REF_PREFIX,
    ] {
        match crate::git::git_checkpoint_prune(
            repo_path,
            namespace,
            crate::git::CHECKPOINT_KEEP_PER_NAMESPACE,
        ) {
            Ok(pruned) => {
                if let Err(error) = db.delete_agent_chat_checkpoints_by_refs(
                    &record.repo_path,
                    &pruned,
                ) {
                    eprintln!(
                        "[codemux::agent_chat] failed to drop pruned checkpoint rows: {error}"
                    );
                }
            }
            Err(error) => eprintln!(
                "[codemux::agent_chat] checkpoint prune failed: {error}"
            ),
        }
    }
    // Re-read so the caller (and the emitted event) sees the
    // SQLite-assigned created_at.
    Ok(db.get_agent_chat_checkpoint(thread_id).or(Some(record)))
}

/// Synchronous core of the restore path. Blocking — run it on a
/// blocking thread. Public for integration tests.
pub fn restore_run_checkpoint_blocking(
    db: &DatabaseStore,
    thread_id: &str,
) -> Result<(), String> {
    let record = db.get_agent_chat_checkpoint(thread_id).ok_or_else(|| {
        "No checkpoint is recorded for this chat.".to_string()
    })?;
    crate::git::git_checkpoint_restore(
        std::path::Path::new(&record.repo_path),
        &record.snapshot_commit,
        &record.head_commit,
        record.branch.as_deref(),
        &crate::git::pre_restore_ref_name(thread_id),
    )
}

/// Fire-and-forget background checkpoint for a freshly started run.
///
/// The whole body — including the settings-cache read that decides
/// whether the feature is even on — runs off the command path:
/// `tauri::async_runtime::spawn` returns immediately and the git work
/// happens on the blocking pool. Failures are logged, never surfaced:
/// a checkpoint must not break (or slow) the chat it protects.
fn spawn_run_checkpoint(
    app: &AppHandle,
    thread_id: String,
    workspace_id: String,
    cwd: String,
) {
    spawn_run_checkpoint_with_gate(app, thread_id, workspace_id, cwd, run_checkpoints_enabled);
}

/// [`spawn_run_checkpoint`] with the opt-in gate injected. Public (and
/// generic over the runtime) so integration tests can drive the REAL
/// background path — spawn, blocking pool, DB write through managed
/// state, event emission — on a `tauri::test::mock_app` without
/// touching the user's settings cache.
pub fn spawn_run_checkpoint_with_gate<R: Runtime>(
    app: &AppHandle<R>,
    thread_id: String,
    workspace_id: String,
    cwd: String,
    gate: fn() -> bool,
) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let blocking_app = app.clone();
        let blocking_thread_id = thread_id.clone();
        let result = tokio::task::spawn_blocking(move || {
            if !gate() {
                return Ok(None);
            }
            let db: State<'_, DatabaseStore> = blocking_app.state();
            create_run_checkpoint_blocking(
                &db,
                std::path::Path::new(&cwd),
                &blocking_thread_id,
                &workspace_id,
            )
        })
        .await;
        match result {
            Ok(Ok(Some(checkpoint))) => {
                eprintln!(
                    "[codemux::agent_chat] checkpoint {} recorded for thread {thread_id}",
                    checkpoint.snapshot_commit
                );
                let payload = AgentChatCheckpointEventPayload {
                    thread_id: ThreadId(thread_id),
                    checkpoint,
                };
                if let Err(error) = app.emit(AGENT_CHAT_CHECKPOINT_EVENT, &payload) {
                    eprintln!(
                        "[codemux::agent_chat] failed to emit {AGENT_CHAT_CHECKPOINT_EVENT}: {error}"
                    );
                }
            }
            Ok(Ok(None)) => { /* feature off, or nothing to snapshot */ }
            Ok(Err(error)) => eprintln!(
                "[codemux::agent_chat] background checkpoint failed for {thread_id}: {error}"
            ),
            Err(join_error) => eprintln!(
                "[codemux::agent_chat] checkpoint task panicked for {thread_id}: {join_error}"
            ),
        }
    });
}

/// Return the rollback checkpoint recorded for a thread, if any.
///
/// Not gated on the feature flag (mirrors `agent_chat_list_sessions`):
/// the header should render "no checkpoint" rather than an error
/// string when the flag is off.
#[tauri::command]
pub async fn agent_chat_get_checkpoint(
    db: State<'_, DatabaseStore>,
    thread_id: String,
) -> Result<Option<AgentChatCheckpointRecord>, String> {
    Ok(db.get_agent_chat_checkpoint(&thread_id))
}

/// Roll the workspace back to the checkpoint taken when this run
/// started. Mutates the working tree — the UI confirms first.
#[tauri::command]
pub async fn agent_chat_restore_checkpoint(
    app: AppHandle,
    thread_id: String,
) -> Result<(), String> {
    let observability: State<'_, ObservabilityStore> = app.state();
    feature_flag_on(&observability)?;
    // Blocking pool, same rationale as commands/git.rs: a wedged git
    // subprocess must not freeze the GTK main thread.
    tokio::task::spawn_blocking(move || {
        let db: State<'_, DatabaseStore> = app.state();
        restore_run_checkpoint_blocking(&db, &thread_id)
    })
    .await
    .map_err(|e| format!("agent_chat_restore_checkpoint task join failed: {e}"))?
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
    claude_cache: tauri::State<
        '_,
        std::sync::Arc<crate::agent_provider::claude::capabilities::ClaudeCapabilityCache>,
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
        ProviderKind::Claude => {
            // Cache + cascade: sidecar `list-models` (SDK
            // `supportedModels()` — works for any Claude Code user) →
            // Anthropic `/v1/models` REST API (requires
            // `ANTHROPIC_API_KEY`) → hand-maintained fallback. The
            // cache wraps the cascade so a successful live harvest is
            // reused; failure at every live tier logs and serves the
            // maintained bundle so the picker is never blank.
            match claude_cache.get_or_harvest().await {
                Ok(caps) => Ok(caps),
                Err(err) => {
                    eprintln!(
                        "[claude] live harvest failed, falling back to maintained: {}",
                        err.to_command_string()
                    );
                    Ok(crate::agent_provider::claude::capabilities::claude_fallback_capabilities())
                }
            }
        }
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

// ── Event bridge ──────────────────────────────────────────────────────

/// Start the provider-event forwarding tasks.
///
/// Spawns one Tokio task per registered provider. Each task consumes
/// the provider's canonical event stream and forwards each event to
/// the frontend wrapped in an [`AgentChatEventPayload`] —
/// thread-scoped events go to the thread's attached `Channel` (see
/// [`AgentChatChannelRegistry`]), thread-less ones to the
/// [`AGENT_CHAT_EVENT`] event bus. When a provider's stream ends (on
/// shutdown) the task exits cleanly; the `event_stream()` helper on
/// each provider already swallows `broadcast::error::RecvError::Lagged`
/// and continues, so slow subscribers never crash this loop.
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
/// Routing:
/// - thread-scoped events → the thread's live `Channel`, when one is
///   attached (high-throughput path; carries the `content_delta`
///   token stream). No channel attached → the event is dropped after
///   persistence; a late-attaching pane recovers transcript state via
///   the DB hydrate.
/// - thread-less events (global `RuntimeWarning`) → the
///   [`AGENT_CHAT_EVENT`] global event bus, unchanged.
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
    let thread_id = thread_id_for_event(&event)
        // Events without a thread_id (e.g. global RuntimeWarning) are
        // forwarded with an empty ThreadId so the frontend at least
        // sees them; a richer global-warning channel is a follow-up.
        .unwrap_or_else(|| ThreadId(String::new()));
    let payload = AgentChatEventPayload {
        thread_id: thread_id.clone(),
        event,
    };
    if thread_id.0.is_empty() {
        // Thread-less lifecycle/warning traffic is low-frequency; the
        // global event bus stays the simplest transport for it.
        if let Err(error) = app.emit(AGENT_CHAT_EVENT, &payload) {
            eprintln!(
                "[codemux::agent_chat] Failed to emit {AGENT_CHAT_EVENT}: {error}"
            );
        }
        return;
    }
    // Thread-scoped events (incl. the content_delta token stream) go
    // over the per-thread Channel only — never the global bus. A
    // missing channel means no pane is currently attached to this
    // thread; transcript events were already persisted above, so the
    // DB hydrate replays them on (re)attach.
    let channels: State<'_, AgentChatChannelRegistry> = app.state();
    if let Some(channel) = channels.channel_for(&thread_id.0) {
        if let Err(error) = channel.send(payload) {
            eprintln!(
                "[codemux::agent_chat] Failed to send event on thread channel {}: {error}",
                thread_id.0
            );
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

    // ── workspace_env_overlay ──
    //
    // Guards the primary fix for the "chat agent's browser lands in the
    // user's workspace" routing bug: the overlay must inject the workspace's
    // CODEMUX_* surface, must never clobber a caller-provided entry, and must
    // inject nothing for an orphaned pane.

    fn test_workspace(id: &str) -> crate::state::WorkspaceSnapshot {
        use crate::state::*;
        WorkspaceSnapshot {
            workspace_id: WorkspaceId(id.to_string()),
            title: "my-feature".to_string(),
            workspace_type: WorkspaceType::Standard,
            cwd: "/home/user/projects/repo".to_string(),
            git_branch: Some("feat/my-feature".to_string()),
            git_ahead: 0,
            git_behind: 0,
            git_additions: 0,
            git_deletions: 0,
            git_changed_files: 0,
            notification_count: 0,
            latest_agent_state: None,
            worktree_path: None,
            project_root: Some("/home/user/projects/repo".to_string()),
            project_uid: None,
            workspace_kind: None,
            protected: false,
            divergent_copy: false,
            pr_number: None,
            pr_state: None,
            pr_url: None,
            linked_issue: None,
            notifications_muted: false,
            tabs: Vec::new(),
            active_tab_id: String::new(),
            active_surface_id: SurfaceId(String::new()),
            surfaces: Vec::new(),
            host_id: None,
            remote_cwd: None,
            attach_only: false,
        }
    }

    #[test]
    fn workspace_env_overlay_injects_workspace_surface() {
        let ws = test_workspace("ws-123");
        let env = workspace_env_overlay(Some(&ws), "pane-7", None)
            .expect("a resolved workspace must yield an env map");

        assert_eq!(env["CODEMUX"], "1");
        assert_eq!(env["CODEMUX_WORKSPACE_ID"], "ws-123");
        assert_eq!(env["CODEMUX_PANE_ID"], "pane-7");
        assert_eq!(env["CODEMUX_BROWSER_CMD"], "codemux browser");
        assert_eq!(env["BROWSER"], "codemux browser open");
        assert_eq!(env["CODEMUX_VERSION"], env!("CARGO_PKG_VERSION"));
        // Workspace-level vars threaded through `workspace_pty_env`.
        assert_eq!(env["CODEMUX_WORKSPACE_NAME"], "my-feature");
        assert_eq!(env["CODEMUX_WORKSPACE_PATH"], "/home/user/projects/repo");
        assert_eq!(env["CODEMUX_ROOT_PATH"], "/home/user/projects/repo");
        assert_eq!(env["CODEMUX_BRANCH"], "feat/my-feature");
        assert!(env.contains_key("CODEMUX_PORT"));
        assert!(env.contains_key("CODEMUX_AGENT_CONTEXT"));
    }

    #[test]
    fn workspace_env_overlay_does_not_clobber_caller_entries() {
        let ws = test_workspace("ws-123");
        let mut existing = HashMap::new();
        existing.insert("BROWSER".to_string(), "custom-browser".to_string());
        existing.insert("MY_VAR".to_string(), "keep-me".to_string());

        let env = workspace_env_overlay(Some(&ws), "pane-7", Some(existing))
            .expect("a resolved workspace must yield an env map");

        // Caller-provided entries win over the workspace defaults …
        assert_eq!(env["BROWSER"], "custom-browser");
        // … and unrelated caller entries survive.
        assert_eq!(env["MY_VAR"], "keep-me");
        // … while un-collided workspace vars are still injected.
        assert_eq!(env["CODEMUX_WORKSPACE_ID"], "ws-123");
    }

    #[test]
    fn workspace_env_overlay_no_workspace_injects_nothing() {
        // Orphaned pane, no existing env → stays None (no injection).
        assert!(workspace_env_overlay(None, "pane-7", None).is_none());

        // Orphaned pane WITH a caller env → returned untouched.
        let mut existing = HashMap::new();
        existing.insert("MY_VAR".to_string(), "keep-me".to_string());
        let env = workspace_env_overlay(None, "pane-7", Some(existing))
            .expect("caller env must be preserved");
        assert_eq!(env.len(), 1);
        assert_eq!(env["MY_VAR"], "keep-me");
        assert!(!env.contains_key("CODEMUX_WORKSPACE_ID"));
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

    // ── AgentChatChannelRegistry ──
    //
    // Pure registry semantics; the end-to-end routing through
    // forward_event (incl. DB persistence side effects and the
    // thread-less event-bus fallback) lives in
    // tests/agent_chat_commands.rs where a mock app handle exists.

    /// Real `tauri::ipc::Channel` whose handler captures every decoded
    /// payload — same pattern as the terminal module's channel tests.
    fn capture_channel() -> (
        Channel<AgentChatEventPayload>,
        Arc<Mutex<Vec<AgentChatEventPayload>>>,
    ) {
        let captured: Arc<Mutex<Vec<AgentChatEventPayload>>> =
            Arc::new(Mutex::new(Vec::new()));
        let captured_handler = captured.clone();
        let channel = Channel::new(move |body| {
            let payload = body
                .deserialize::<AgentChatEventPayload>()
                .expect("decode AgentChatEventPayload");
            captured_handler.lock().unwrap().push(payload);
            Ok(())
        });
        (channel, captured)
    }

    fn delta_payload(thread: &str, text: &str) -> AgentChatEventPayload {
        AgentChatEventPayload {
            thread_id: ThreadId(thread.into()),
            event: ProviderRuntimeEvent::ContentDelta {
                thread_id: ThreadId(thread.into()),
                turn_id: turn(),
                delta: ContentDelta::Text { text: text.into() },
            },
        }
    }

    #[test]
    fn registry_attach_routes_sends_through_channel() {
        let registry = AgentChatChannelRegistry::default();
        let (channel, captured) = capture_channel();
        registry.attach("t1", channel);

        let live = registry.channel_for("t1").expect("channel installed");
        live.send(delta_payload("t1", "hello")).expect("send ok");

        let received = captured.lock().unwrap();
        assert_eq!(received.len(), 1);
        assert_eq!(received[0].thread_id.0, "t1");
        match &received[0].event {
            ProviderRuntimeEvent::ContentDelta { delta, .. } => match delta {
                ContentDelta::Text { text } => assert_eq!(text, "hello"),
                other => panic!("unexpected delta: {other:?}"),
            },
            other => panic!("unexpected event: {other:?}"),
        }
    }

    #[test]
    fn registry_channel_for_unknown_thread_is_none() {
        let registry = AgentChatChannelRegistry::default();
        assert!(registry.channel_for("nope").is_none());
    }

    #[test]
    fn registry_newest_attach_wins() {
        let registry = AgentChatChannelRegistry::default();
        let (first, first_captured) = capture_channel();
        let (second, second_captured) = capture_channel();
        let g1 = registry.attach("t1", first);
        let g2 = registry.attach("t1", second);
        assert_ne!(g1, g2, "each attach mints a fresh generation");

        registry
            .channel_for("t1")
            .expect("channel installed")
            .send(delta_payload("t1", "x"))
            .expect("send ok");

        assert!(
            first_captured.lock().unwrap().is_empty(),
            "superseded channel must not receive events"
        );
        assert_eq!(second_captured.lock().unwrap().len(), 1);
    }

    #[test]
    fn registry_detach_with_stale_generation_is_noop() {
        let registry = AgentChatChannelRegistry::default();
        let (first, _) = capture_channel();
        let (second, second_captured) = capture_channel();
        let stale = registry.attach("t1", first);
        let _current = registry.attach("t1", second);

        // The unmounting pane that owned `first` detaches late — the
        // newer subscriber must survive.
        assert!(!registry.detach("t1", stale));
        registry
            .channel_for("t1")
            .expect("newer channel still installed")
            .send(delta_payload("t1", "still-live"))
            .expect("send ok");
        assert_eq!(second_captured.lock().unwrap().len(), 1);
    }

    #[test]
    fn registry_detach_with_current_generation_removes() {
        let registry = AgentChatChannelRegistry::default();
        let (channel, _) = capture_channel();
        let generation = registry.attach("t1", channel);
        assert!(registry.detach("t1", generation));
        assert!(registry.channel_for("t1").is_none());
        // Idempotent on repeat.
        assert!(!registry.detach("t1", generation));
    }

    #[test]
    fn registry_threads_are_independent() {
        let registry = AgentChatChannelRegistry::default();
        let (a, a_captured) = capture_channel();
        let (b, b_captured) = capture_channel();
        registry.attach("thread-a", a);
        registry.attach("thread-b", b);

        registry
            .channel_for("thread-a")
            .unwrap()
            .send(delta_payload("thread-a", "for-a"))
            .unwrap();

        assert_eq!(a_captured.lock().unwrap().len(), 1);
        assert!(
            b_captured.lock().unwrap().is_empty(),
            "no cross-thread leakage"
        );
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
