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
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};
use tauri::ipc::Channel;
use tauri::{AppHandle, Emitter, Manager, Runtime, State};

use crate::agent_provider::{
    AgentProvider, ApprovalDecision, ProviderChatCapabilities, ProviderError, ProviderKind,
    ProviderRuntimeEvent, RequestId, SendTurnInput, SerializableProviderError,
    StartSessionInput, ThreadId, TurnId,
};
use crate::database::{
    AgentChatCheckpointRecord, AgentChatSessionConfig, AgentChatSessionRecord, DatabaseStore,
};
use crate::observability::ObservabilityStore;
use crate::state::{AppStateStore, PaneStatus};

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
    /// as `OutputSubscriber::generation` on the PTY path.
    generation: u64,
}

/// Registry of per-thread frontend event channels (Tauri-managed
/// state).
///
/// Analogous to `SessionRuntime.output_subscribers` in `terminal/mod.rs`:
/// when a chat pane mounts with a bound thread it invokes
/// [`attach_agent_chat_output`] with a fresh `Channel`; the event
/// bridge ([`forward_event`]) then fans that thread's live events —
/// crucially the high-frequency `content_delta` token stream — out to
/// every attached channel instead of broadcasting them app-wide.
///
/// Each thread maps to a **list** of subscribers, not a single one, so
/// the same thread can be mirrored to several consumers at once (the
/// desktop window plus any browser client rendering the same thread).
/// Every subscriber receives an identical event stream.
///
/// Replay split (decided in issue #75): the channel carries **live
/// events only**. Transcript-affecting events are persisted to SQLite
/// by `forward_event` regardless of whether a channel is attached, and
/// a late-attaching / resumed pane rebuilds history through the
/// existing DB hydrate (`agent_chat_list_messages` +
/// `hydrateThread`) — so no replay-on-attach machinery is needed here
/// even for a fresh mirror client. Events that fire while no channel is
/// attached and that are not persisted (lifecycle notices like
/// `session_state_changed`) are dropped — exactly the behaviour the
/// event-bus path had for unmounted panes, where no listener was
/// registered to hear them.
#[derive(Default)]
pub struct AgentChatChannelRegistry {
    channels: Mutex<HashMap<String, Vec<AgentChatChannelEntry>>>,
    next_generation: AtomicU64,
}

impl AgentChatChannelRegistry {
    /// Add `channel` as a live subscriber for `thread_id`, alongside any
    /// existing subscribers (fan-out — every attached consumer receives
    /// the thread's events). Returns the generation token the caller
    /// must hand back to [`detach`](Self::detach).
    pub fn attach(
        &self,
        thread_id: &str,
        channel: Channel<AgentChatEventPayload>,
    ) -> u64 {
        let generation = self.next_generation.fetch_add(1, Ordering::Relaxed) + 1;
        let mut channels = self.channels.lock().expect("channel registry poisoned");
        channels
            .entry(thread_id.to_string())
            .or_default()
            .push(AgentChatChannelEntry {
                channel,
                generation,
            });
        generation
    }

    /// Remove the subscriber for `thread_id` whose generation matches.
    /// Returns whether an entry was actually removed. Idempotent:
    /// detaching an unknown thread or a superseded generation is a
    /// no-op, and other subscribers on the same thread are untouched.
    pub fn detach(&self, thread_id: &str, generation: u64) -> bool {
        let mut channels = self.channels.lock().expect("channel registry poisoned");
        let Some(entries) = channels.get_mut(thread_id) else {
            return false;
        };
        let before = entries.len();
        entries.retain(|entry| entry.generation != generation);
        let removed = entries.len() != before;
        // Drop the now-empty bucket so the map doesn't accumulate keys
        // for threads with no subscribers.
        if entries.is_empty() {
            channels.remove(thread_id);
        }
        removed
    }

    /// Clone every live channel for `thread_id`. Cloning is cheap (the
    /// `Channel` is internally ref-counted) and keeps the lock scope
    /// tight on the hot streaming path — the caller sends outside the
    /// lock. Returns an empty vec when no consumer is attached.
    pub fn channels_for(&self, thread_id: &str) -> Vec<Channel<AgentChatEventPayload>> {
        let channels = self.channels.lock().expect("channel registry poisoned");
        channels
            .get(thread_id)
            .map(|entries| entries.iter().map(|entry| entry.channel.clone()).collect())
            .unwrap_or_default()
    }
}

/// Register a per-thread `Channel` that will receive every live
/// runtime event for `thread_id` (see [`AgentChatChannelRegistry`]).
/// Multiple consumers can attach to the same thread at once (mirror
/// mode); each gets an identical event stream. Returns the attach
/// generation; the frontend passes it back to
/// [`detach_agent_chat_output`] on unmount so a stale detach only ever
/// removes its own subscriber, never a sibling's.
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
/// (the subscriber already went away, or belongs to a sibling consumer)
/// is a silent no-op that leaves other subscribers on the thread intact.
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
    // Drop any lingering status dot for this pane. The session tear-down
    // below emits SessionStateChanged::Closed → Idle, but that races the
    // tree mutation: once close_pane removes the node, the thread→pane
    // walk can no longer resolve it, so clear by pane id here while the
    // key is still meaningful.
    state.set_pane_status(&pane_id, PaneStatus::Idle);
    // Forget any running-subagent tracking for this thread. The
    // SessionStateChanged::Closed event below also clears it, but that
    // races the tear-down (and may be dropped if no channel is attached),
    // so clear eagerly by thread id while it is still resolvable —
    // otherwise a subagent whose terminal status is never observed could
    // pin a stuck "working" spinner forever.
    if let Some((_, thread_id)) = &chat_thread {
        let tracker: State<'_, SubagentTracker> = app.state();
        tracker.clear_thread(thread_id);
        let activity: State<'_, RunActivityTracker> = app.state();
        activity.clear_thread(thread_id);
    }
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
    // Snapshot the per-thread chat configuration BEFORE the provider
    // consumes `input`, so it can be persisted onto the session row for
    // restart-resume (re-seeding the pickers) and read back by
    // `ensure_live_session` when it silently restarts a dead session.
    // Session start writes the exact launch selection but never CLEARS a
    // column it simply doesn't know about, so map each plain
    // `Option<String>` through `keep_or_set` (`Some` → set, `None` →
    // leave untouched) rather than treating a `None` as an explicit
    // clear.
    let config_for_persist = AgentChatSessionConfig {
        model: AgentChatSessionConfig::keep_or_set(input.model.clone()),
        effort: AgentChatSessionConfig::keep_or_set(input.effort.clone()),
        context_window: AgentChatSessionConfig::keep_or_set(input.context_window.clone()),
        permission_mode: AgentChatSessionConfig::keep_or_set(input.permission_mode.clone()),
    };
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
        // Persist a provider-returned resume cursor NOW, after the row
        // exists. The async `ResumeCursorUpdated` persist path is a plain
        // UPDATE racing this upsert across a spawned bridge task — when the
        // event wins it updates 0 rows and the cursor is silently lost,
        // leaving the FIRST dead-run rebuild with no conversation context
        // (OpenCode returns its cursor at start; Claude's SDK id arrives
        // later by event and Codex's start-time cursor carries no
        // extractable id, so this is a no-op for them). Best-effort like the
        // neighboring persists.
        if let Some(sdk_session_id) = session
            .resume_cursor
            .as_ref()
            .and_then(extract_sdk_session_id)
        {
            if let Err(error) =
                db.set_agent_chat_sdk_session_id(&session.thread_id.0, &sdk_session_id)
            {
                eprintln!(
                    "[codemux::agent_chat] failed to persist start-time resume cursor: {error}"
                );
            }
        }
        // Persist the per-thread chat config the session started with so
        // a restart re-seeds the pickers and auto-resume rebuilds the SDK
        // session with the same settings. Best-effort — a failed write
        // just means the pane falls back to defaults after a restart.
        if let Err(error) =
            db.update_agent_chat_session_config(&session.thread_id.0, &config_for_persist)
        {
            eprintln!(
                "[codemux::agent_chat] failed to persist session config: {error}"
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

// ── Backend auto-resume ──────────────────────────────────────────────

/// Per-`thread_id` async locks that serialize [`ensure_live_session`]'s
/// check→rebuild sequence.
///
/// Every provider `start_session` is TOCTOU: it checks the session map
/// under a read lock, releases it, spawns the sidecar, and only THEN
/// takes the write lock to insert. So two concurrent auto-resuming
/// commands on the same dead thread (e.g. an `agent_chat_send_turn`
/// racing an `agent_chat_respond_to_request` on a replayed pending
/// request after a restart) can both observe `has_session() == false`,
/// both pass the `contains_key` guard, and both spawn a sidecar — the
/// second insert then overwrites and drops the first session, orphaning
/// one sidecar and resuming the same server-side session id twice.
/// Holding a per-thread lock across the whole check→rebuild makes the
/// resume idempotent.
fn resume_locks() -> &'static Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>> {
    static LOCKS: OnceLock<Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>> = OnceLock::new();
    LOCKS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Fetch (or create) the resume lock for a single thread. The returned
/// `Arc` is cloned out from under the map's std mutex so the short
/// synchronous section never overlaps the `.await` on the async lock.
fn resume_lock_for(thread_id: &str) -> Arc<tokio::sync::Mutex<()>> {
    let mut map = resume_locks().lock().unwrap();
    map.entry(thread_id.to_string())
        .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
        .clone()
}

/// Ensure a live provider session is bound to `thread_id`, silently
/// rebuilding it from the persisted `agent_chat_sessions` row when the
/// provider's in-memory session map has no entry (the state after an app
/// restart, where NO startup rehydration exists and the map starts
/// empty).
///
/// This is the single choke point for backend auto-resume: every command
/// that needs a live session (`agent_chat_send_turn`,
/// `agent_chat_respond_to_request`) calls it first, so a user who reopens
/// the app and immediately interacts with an existing chat pane gets
/// their conversation transparently resumed instead of a
/// `session_not_found` error.
///
/// Reuses the SAME `thread_id` on the rebuilt session — the map is empty
/// after a restart so there is no collision, and keeping the id means the
/// attached frontend `Channel`, the pane snapshot, and the store slice
/// all stay valid with zero migration.
///
/// Resume strategy: pass `resume_cursor: {"resume": <sdk_session_id>}`
/// when the row carries one (Claude reads `resume`/`sessionId`; codex /
/// opencode consume their own cursor shapes best-effort). If the
/// resume-start fails (e.g. the SDK rejects a stale session id), retry
/// once as a FRESH session — the visible transcript already hydrates from
/// the DB, so the user keeps their history even though the provider
/// forgets its server-side context.
///
/// A no-op (returns `Ok`) when a session is already live, or when there
/// is no persisted row to rebuild from — in the latter case the caller's
/// own provider call surfaces the normal `SessionNotFound`.
///
/// Generic over the Tauri runtime so the integration tests can drive it
/// on `tauri::test::mock_app`, mirroring `forward_event`.
pub async fn ensure_live_session<R: Runtime>(
    app: &AppHandle<R>,
    provider_kind: ProviderKind,
    thread_id: &ThreadId,
) -> Result<(), String> {
    let registry: State<'_, ProviderRegistry> = app.state();
    let impl_ = lookup_provider(&registry, provider_kind).await?;
    // Fast path: a live session already exists, so no need to serialize
    // — this is the common case (every send after the first).
    if impl_.has_session(thread_id).await {
        return Ok(());
    }

    // Serialize the check→rebuild across concurrent callers on the same
    // thread so two auto-resumes can't each spawn a sidecar (see
    // `resume_locks`). Held for the whole rebuild below.
    let resume_lock = resume_lock_for(&thread_id.0);
    let _resume_guard = resume_lock.lock().await;
    // Re-check under the lock: another task may have rebuilt the session
    // while we were waiting to acquire it.
    if impl_.has_session(thread_id).await {
        return Ok(());
    }

    // No live session — try to rebuild from the persisted row.
    let record = {
        let db: State<'_, DatabaseStore> = app.state();
        db.get_agent_chat_session(&thread_id.0)
    };
    let Some(record) = record else {
        // Nothing persisted; let the caller's provider call surface the
        // normal SessionNotFound rather than inventing a fresh session
        // with no history.
        return Ok(());
    };

    let cwd = record
        .cwd
        .as_deref()
        .filter(|s| !s.is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());

    // Rebuild the workspace env overlay so the resumed agent's
    // browser / CLI subprocesses route to its OWN workspace, exactly
    // like `agent_chat_start_session`. Resolve the pane from the thread;
    // an orphaned thread (no pane) injects nothing.
    let env = {
        let state: State<'_, AppStateStore> = app.state();
        match state.agent_chat_pane_id_for_thread(&thread_id.0) {
            Some(pane_id) => {
                let workspace_id = state.workspace_id_for_pane(&pane_id);
                let snapshot = state.snapshot();
                let ws = workspace_id
                    .as_ref()
                    .and_then(|id| snapshot.workspaces.iter().find(|w| &w.workspace_id.0 == id));
                workspace_env_overlay(ws, &pane_id, None)
            }
            None => None,
        }
    };

    let resume_cursor = record
        .sdk_session_id
        .as_ref()
        .map(|id| serde_json::json!({ "resume": id }));

    let build_input = |resume_cursor: Option<serde_json::Value>| StartSessionInput {
        thread_id: thread_id.clone(),
        cwd: cwd.clone(),
        model: record.model.clone(),
        resume_cursor,
        permission_mode: record.permission_mode.clone(),
        effort: record.effort.clone(),
        context_window: record.context_window.clone(),
        additional_directories: vec![],
        env: env.clone(),
        extra: serde_json::Value::Null,
    };

    eprintln!(
        "[codemux::agent_chat] auto-resuming dead session thread={} provider={provider_kind:?} \
         resume={} model={:?}",
        thread_id.0,
        resume_cursor.is_some(),
        record.model,
    );

    match impl_.start_session(build_input(resume_cursor.clone())).await {
        Ok(_) => Ok(()),
        Err(err) if resume_cursor.is_some() => {
            // Resume-start failed — retry once as a fresh session. The
            // transcript already hydrates from the DB, so the user keeps
            // their visible history.
            eprintln!(
                "[codemux::agent_chat] resume-start failed for thread={} ({err}); \
                 retrying as a fresh session",
                thread_id.0,
            );
            impl_
                .start_session(build_input(None))
                .await
                .map_err(provider_err)?;
            Ok(())
        }
        Err(err) => Err(provider_err(err)),
    }
}

/// Queue a user turn on an existing session.
///
/// Returns a [`TurnStartResult`]: when the session was busy the turn is
/// **queued** (`queued_id` set) rather than rejected, and dispatches
/// automatically when the current turn finishes. The user-message
/// envelope is persisted here only for an immediate start; a queued
/// turn's envelope is written when it actually DISPATCHES (see
/// [`forward_event`]'s `QueuedTurnDispatched` handling) so chat history
/// reflects real turn order.
#[tauri::command]
pub async fn agent_chat_send_turn(
    app: AppHandle,
    provider: ProviderKind,
    input: SendTurnInput,
) -> Result<crate::agent_provider::TurnStartResult, String> {
    let observability: State<'_, ObservabilityStore> = app.state();
    feature_flag_on(&observability)?;
    // Auto-resume: if the provider's session map has no live session for
    // this thread (e.g. the app was restarted), rebuild it from the
    // persisted row before the turn so the user never sees a
    // `session_not_found`. No-op when a session is already live.
    ensure_live_session(&app, provider, &input.thread_id).await?;
    let registry: State<'_, ProviderRegistry> = app.state();
    let impl_ = lookup_provider(&registry, provider).await?;
    // Capture the inputs we need for persistence before the provider
    // consumes `input`.
    let thread_id_for_persist = input.thread_id.0.clone();
    let user_text_for_persist = input.text.clone();
    let first_line = first_line_title(&input.text);
    // Persist any attachments to disk NOW, before the provider consumes
    // `input`. This runs for both immediate and queued sends: the bytes
    // are in hand here and nowhere else (the deferred, dispatch-time
    // envelope write in `forward_event` only sees `text`). Best-effort —
    // a failed write is logged and skipped inside the helper and never
    // blocks the turn.
    let saved_images = save_chat_images(&thread_id_for_persist, &input.images);
    // A genuine new user turn is the authoritative, provider-agnostic
    // anchor for resetting subagent tracking. Clear the thread's tracker
    // entry here so a subagent left non-terminal by the previous turn
    // (e.g. Claude's background `async_launched` task that never emits a
    // terminal `task_notification`) cannot pin `review_pending`/`running`
    // and suppress `Review` for this turn and every one after (Finding 1).
    // A genuinely-still-alive background subagent re-inserts itself via its
    // next `Running` snapshot (Claude re-emits these through `task_progress`
    // ticks). Unlike `SessionStateChanged::Running`, which Claude does not
    // fire per user turn, this fires on every turn for all three providers.
    //
    // Follow-up queueing note: for a send that gets QUEUED behind an
    // active turn this clear runs at enqueue time (not at dispatch) —
    // live subagents of the in-flight turn re-insert on their next
    // Running snapshot, and the stale non-terminal entries this exists
    // to purge are gone by the time the queued turn dispatches.
    {
        let tracker: State<'_, SubagentTracker> = app.state();
        tracker.clear_thread(&thread_id_for_persist);
    }
    let result = impl_.send_turn(input).await.map_err(provider_err)?;

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
    // Persist the user message envelope — but ONLY for an immediate
    // start. A queued turn is persisted when it dispatches, so the
    // transcript records real turn order rather than type order. User
    // messages never come back through the provider event stream, so
    // this is the one chance to record them. Best-effort: a failed write
    // means resume will skip this user turn but the live conversation is
    // unaffected.
    match &result.queued_id {
        None => persist_user_message(
            &db,
            &thread_id_for_persist,
            &user_text_for_persist,
            &saved_images,
        ),
        // Queued: stash the (already-on-disk) image records keyed by the
        // provider's queued_id so the deferred envelope write in
        // `forward_event`'s `QueuedTurnDispatched` arm can attach them at
        // real turn order. Skip the map entirely when there are no images
        // (nothing to carry — the dispatch persist writes text only).
        Some(queued_id) if !saved_images.is_empty() => {
            pending_queued_images()
                .lock()
                .unwrap()
                .insert(queued_id.clone(), saved_images);
        }
        Some(_) => {}
    }
    Ok(result)
}

/// Cancel a queued (not-yet-dispatched) follow-up turn. Idempotent — an
/// unknown or already-dispatched id is a silent success. On success the
/// provider emits a `QueuedTurnCancelled` event so the UI removes the
/// greyed bubble.
#[tauri::command]
pub async fn agent_chat_cancel_queued_turn(
    app: AppHandle,
    provider: ProviderKind,
    thread_id: ThreadId,
    queued_id: String,
) -> Result<(), String> {
    let observability: State<'_, ObservabilityStore> = app.state();
    feature_flag_on(&observability)?;
    let registry: State<'_, ProviderRegistry> = app.state();
    let impl_ = lookup_provider(&registry, provider).await?;
    impl_
        .cancel_queued_turn(thread_id, queued_id)
        .await
        .map_err(provider_err)
}

/// Send a queued (not-yet-dispatched) follow-up turn **now**: promote it
/// to the front of the queue and dispatch it immediately, soft-interrupting
/// the active turn if one is running. The interrupt preserves the session,
/// the full transcript, and all on-disk work — nothing is discarded — and
/// the message then runs as a normal follow-up turn so the agent re-plans
/// with the steer. Idempotent — an unknown or already-dispatched id is a
/// silent success. No persistence work here: the interrupt's `Ready` state
/// events settle the composer and the provider's `QueuedTurnDispatched`
/// event (handled in [`forward_event`]) promotes the bubble and writes the
/// deferred user-message envelope, attaching any `pending_queued_images`.
#[tauri::command]
pub async fn agent_chat_send_queued_turn_now(
    app: AppHandle,
    provider: ProviderKind,
    thread_id: ThreadId,
    queued_id: String,
) -> Result<(), String> {
    let observability: State<'_, ObservabilityStore> = app.state();
    feature_flag_on(&observability)?;
    let registry: State<'_, ProviderRegistry> = app.state();
    let impl_ = lookup_provider(&registry, provider).await?;
    impl_
        .send_queued_turn_now(thread_id, queued_id)
        .await
        .map_err(provider_err)
}

/// A chat image attachment that has been written to disk for the
/// transcript. User messages (and their images) never come back through
/// the provider event stream, so the only record of an attachment is the
/// path we mint here plus its original MIME type — both go onto the
/// `user_message` envelope so a resumed conversation can re-render the
/// image the same way the live send did.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PersistedChatImage {
    /// Absolute path to the on-disk copy of the image bytes.
    path: String,
    /// Original media type (e.g. `"image/png"`), echoed verbatim so the
    /// frontend can label / re-encode the attachment on hydrate.
    media_type: String,
}

/// Map a media type to a file extension for the on-disk copy. Deliberately
/// narrow — only the types Agent Chat actually accepts — with a `bin`
/// fallback so an unknown/garbage MIME still round-trips to disk rather
/// than being dropped. Case-insensitive because clients are inconsistent
/// about MIME casing (mirrors `files::clipboard_image_extension`).
fn chat_image_extension(media_type: &str) -> &'static str {
    match media_type.to_ascii_lowercase().as_str() {
        "image/png" => "png",
        "image/jpeg" | "image/jpg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        _ => "bin",
    }
}

/// Resolve a thread's chat-image directory:
/// `<config>/<APP_DIR_NAME>/agent-chat/images/<thread_id>/`.
///
/// Mirrors how [`crate::database`] resolves its store path (config dir +
/// app name) so images live beside the transcript DB — a durable location
/// a resumed conversation can rely on — rather than in a temp dir the OS
/// may reap out from under it (unlike the ephemeral clipboard-paste files
/// in `commands::files`).
fn chat_images_dir(thread_id: &str) -> Option<std::path::PathBuf> {
    Some(
        dirs::config_dir()?
            .join(crate::APP_DIR_NAME)
            .join("agent-chat")
            .join("images")
            .join(thread_id),
    )
}

/// Write each attachment's bytes to disk under the thread's image dir and
/// return the records to record on the transcript envelope.
///
/// Best-effort by contract: a failed directory-create or write logs and
/// SKIPS that image — it must never propagate an error that would block
/// the turn (the live send still carries the bytes to the provider). An
/// empty input is a cheap no-op returning an empty vec, so callers can
/// invoke it unconditionally.
fn save_chat_images(
    thread_id: &str,
    images: &[crate::agent_provider::ImageInput],
) -> Vec<PersistedChatImage> {
    if images.is_empty() {
        return Vec::new();
    }
    let Some(dir) = chat_images_dir(thread_id) else {
        eprintln!(
            "[codemux::agent_chat] could not resolve config dir; \
             skipping {} chat image(s)",
            images.len()
        );
        return Vec::new();
    };
    if let Err(error) = std::fs::create_dir_all(&dir) {
        eprintln!(
            "[codemux::agent_chat] failed to create chat image dir {}: {error}",
            dir.display()
        );
        return Vec::new();
    }
    let mut saved = Vec::with_capacity(images.len());
    for image in images {
        let ext = chat_image_extension(&image.media_type);
        let path = dir.join(format!("{}.{ext}", uuid::Uuid::new_v4()));
        match std::fs::write(&path, &image.data) {
            Ok(()) => saved.push(PersistedChatImage {
                path: path.to_string_lossy().into_owned(),
                media_type: image.media_type.clone(),
            }),
            Err(error) => eprintln!(
                "[codemux::agent_chat] failed to write chat image {}: {error}",
                path.display()
            ),
        }
    }
    saved
}

/// Process-wide bridge carrying a QUEUED send's on-disk image records
/// through to the deferred user-message envelope write.
///
/// The provider's follow-up queue carries a queued turn's *text* to
/// dispatch, but it has no notion of the transcript paths the command
/// layer minted — those live only here, keyed by the same `queued_id` the
/// provider echoes on [`ProviderRuntimeEvent::QueuedTurnDispatched`]. An
/// immediate (non-queued) send never touches this map: its envelope is
/// written inline in [`agent_chat_send_turn`]. Follows the same
/// module-static idiom as [`resume_locks`] to avoid threading a new
/// managed state through every `forward_event` test harness.
fn pending_queued_images() -> &'static Mutex<HashMap<String, Vec<PersistedChatImage>>> {
    static PENDING: OnceLock<Mutex<HashMap<String, Vec<PersistedChatImage>>>> =
        OnceLock::new();
    PENDING.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Append the synthetic `{"type":"user_message"}` transcript envelope.
/// Best-effort — a failed write only means resume skips this user turn.
///
/// When `images` is non-empty the envelope gains an `"images"` array of
/// `{"path","media_type"}` records; it is OMITTED entirely otherwise so
/// older transcripts and image-less turns stay byte-identical (backward
/// compatible — the frontend hydrate treats a missing field as "no
/// attachments").
fn persist_user_message(
    db: &DatabaseStore,
    thread_id: &str,
    text: &str,
    images: &[PersistedChatImage],
) {
    let mut user_msg = serde_json::json!({
        "type": "user_message",
        "thread_id": thread_id,
        "text": text,
    });
    if !images.is_empty() {
        let images_json: Vec<serde_json::Value> = images
            .iter()
            .map(|image| {
                serde_json::json!({
                    "path": image.path,
                    "media_type": image.media_type,
                })
            })
            .collect();
        user_msg["images"] = serde_json::Value::Array(images_json);
    }
    if let Ok(payload) = serde_json::to_string(&user_msg) {
        let _ = db.append_agent_chat_message(thread_id, &payload);
    }
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
///
/// A `SessionNotFound` is swallowed into `Ok`: there is nothing to
/// interrupt on a dead session (e.g. after a restart, before any turn
/// has re-hydrated it), so a stale stop-click must not surface an error.
/// Interrupt deliberately does NOT auto-resume — restarting a session
/// just to immediately interrupt it would be pointless.
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
    match impl_.interrupt_turn(thread_id, turn_id).await {
        Ok(()) => Ok(()),
        Err(ProviderError::SessionNotFound { .. }) => Ok(()),
        Err(err) => Err(provider_err(err)),
    }
}

/// Whether a live turn is currently in flight on a thread.
///
/// Cheap in-memory probe used by the frontend hydrate-on-remount path to
/// tell "run still in flight" apart from "run died mid-turn": a healthy
/// mid-flight run must not be labeled "Run interrupted" after a workspace
/// switch remount. Deliberately does NOT auto-resume — this is a read-only
/// check against the provider's live session registry, and a thread with no
/// live session (e.g. after a restart) is correctly `false`. The frontend
/// treats any error as `false`, so gating/lookup failures degrade safely.
#[tauri::command]
pub async fn agent_chat_turn_active(
    app: AppHandle,
    provider: ProviderKind,
    thread_id: ThreadId,
) -> Result<bool, String> {
    let observability: State<'_, ObservabilityStore> = app.state();
    feature_flag_on(&observability)?;
    let registry: State<'_, ProviderRegistry> = app.state();
    let impl_ = lookup_provider(&registry, provider).await?;
    Ok(impl_.turn_active(&thread_id).await)
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
    // Auto-resume before responding: a pending approval a user acts on
    // after a restart must land on a live session, not a
    // `session_not_found`. No-op when a session is already live.
    ensure_live_session(&app, provider, &thread_id).await?;
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
///
/// Persist-always, apply-if-live: the new model is written to the
/// session's DB config unconditionally (so it survives a restart and is
/// picked up by the next `ensure_live_session` auto-resume) and only
/// THEN applied to the live session. A `SessionNotFound` from the live
/// apply is swallowed into `Ok` — the value already took effect for the
/// next resume.
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
    // Persist first so a restart / next auto-resume uses the new model
    // even if no live session exists to apply it to right now.
    {
        let db: State<'_, DatabaseStore> = app.state();
        let config = AgentChatSessionConfig {
            model: AgentChatSessionConfig::set(model.clone()),
            ..AgentChatSessionConfig::default()
        };
        if let Err(error) = db.update_agent_chat_session_config(&thread_id.0, &config) {
            eprintln!("[codemux::agent_chat] failed to persist model config: {error}");
        }
    }
    match impl_.set_model(thread_id, model).await {
        Ok(()) => Ok(()),
        // A dead session is fine: the persisted value is applied on the
        // next auto-resume.
        Err(ProviderError::SessionNotFound { .. }) => Ok(()),
        Err(err) => Err(provider_err(err)),
    }
}

/// Change a session's permission mode (accept-edits, bypass, plan,
/// etc.). The mode is passed through as a string; providers reject
/// unknown values via `ProviderError::ValidationError`.
///
/// Persist-always, apply-if-live (same contract as
/// [`agent_chat_set_model`]): the mode is written to the DB config
/// before the live apply, and a `SessionNotFound` from the apply is
/// swallowed into `Ok` so a restart / next auto-resume adopts the mode.
/// Non-`SessionNotFound` errors (e.g. a provider that rejects
/// mid-session permission changes) still surface.
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
    // Persist first so the value survives a restart / next auto-resume.
    {
        let db: State<'_, DatabaseStore> = app.state();
        let config = AgentChatSessionConfig {
            permission_mode: AgentChatSessionConfig::set(mode.clone()),
            ..AgentChatSessionConfig::default()
        };
        if let Err(error) = db.update_agent_chat_session_config(&thread_id.0, &config) {
            eprintln!(
                "[codemux::agent_chat] failed to persist permission_mode config: {error}"
            );
        }
    }
    match impl_.set_permission_mode(thread_id, mode).await {
        Ok(()) => Ok(()),
        Err(ProviderError::SessionNotFound { .. }) => Ok(()),
        Err(err) => Err(provider_err(err)),
    }
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

/// Fetch a single persisted chat session row by `thread_id`, including
/// the per-thread config columns (`model`, `effort`, `context_window`,
/// `permission_mode`).
///
/// Unlike [`agent_chat_list_sessions`], this returns the row EVEN when
/// its `sdk_session_id` is still `NULL` — the frontend calls it on pane
/// mount to re-seed the pickers (and the resume cursor) for a thread
/// that may not yet have produced its first SDK message. Returns `None`
/// when no row exists for the thread.
///
/// Not gated on the feature flag, consistent with the other DB-only
/// history commands (`agent_chat_list_sessions`,
/// `agent_chat_get_checkpoint`): the pane should fall back to defaults
/// rather than surface a feature-flag error string when the flag is off.
#[tauri::command]
pub async fn agent_chat_get_session(
    db: State<'_, DatabaseStore>,
    thread_id: String,
) -> Result<Option<AgentChatSessionRecord>, String> {
    Ok(db.get_agent_chat_session(&thread_id))
}

/// Persist a per-thread chat config change (model / effort /
/// context-window / permission-mode) WITHOUT requiring a live session.
///
/// DB-only: the picker handlers call this fire-and-forget on every
/// change so the selection survives a restart and is re-applied by the
/// next `ensure_live_session` auto-resume. Each field is tri-state: an
/// absent field leaves the stored column untouched, an explicit JSON
/// `null` CLEARS it to NULL (used by the model-change compat reset), and
/// a value overwrites it (see
/// [`DatabaseStore::update_agent_chat_session_config`]).
///
/// Not gated on the feature flag, same rationale as
/// [`agent_chat_get_session`].
#[tauri::command]
pub async fn agent_chat_update_session_config(
    db: State<'_, DatabaseStore>,
    thread_id: String,
    config: AgentChatSessionConfig,
) -> Result<(), String> {
    db.update_agent_chat_session_config(&thread_id, &config)
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
    // Best-effort: drop the thread's on-disk image directory alongside the
    // DB rows. The messages cascade via FK, but the image files live
    // outside SQLite, so nothing else would ever reclaim them. A failure
    // here (missing dir is the common case) must not fail the delete.
    if let Some(dir) = chat_images_dir(&thread_id) {
        let _ = std::fs::remove_dir_all(&dir);
    }
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
    // A queued follow-up turn just dispatched — NOW persist its
    // user-message envelope, at real turn order (it was intentionally
    // skipped at enqueue time in `agent_chat_send_turn`). The queue lives
    // in provider memory, so this event is the one moment the command
    // layer can record the turn.
    if let ProviderRuntimeEvent::QueuedTurnDispatched {
        thread_id,
        queued_id,
        text,
        ..
    } = &event
    {
        if !thread_id.0.is_empty() {
            let db: State<'_, DatabaseStore> = app.state();
            // Reclaim any image records stashed at enqueue time (see
            // `pending_queued_images`) so the deferred envelope carries the
            // same attachments an immediate send would have. Absent entry →
            // a text-only queued turn, persisted with no images.
            let images = pending_queued_images()
                .lock()
                .unwrap()
                .remove(queued_id)
                .unwrap_or_default();
            persist_user_message(&db, &thread_id.0, text, &images);
        }
    }
    // A queued turn was cancelled before it dispatched, so its envelope
    // will never be written. Drop the stashed image records and best-effort
    // delete the orphaned on-disk copies so a cancelled attachment does not
    // leak a file forever.
    if let ProviderRuntimeEvent::QueuedTurnCancelled { queued_id, .. } = &event {
        let orphaned = pending_queued_images().lock().unwrap().remove(queued_id);
        if let Some(images) = orphaned {
            for image in images {
                let _ = std::fs::remove_file(&image.path);
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
    // Publish the chat session's lifecycle into the shared pane_statuses
    // store so the sidebar rows, collapsed-rail aggregate, hover flyout,
    // and PaneNode borders reflect chat agents exactly like terminal
    // agents (whose status flows in through hooks.rs). This is the whole
    // reason chat workspaces previously showed no status dot.
    let tracker: State<'_, SubagentTracker> = app.state();
    publish_pane_status(app, &tracker, &event);

    // Record last-activity for the stall watchdog. Keyed on the event's
    // thread; thread-less events (global warnings) and the transient
    // `RunStalled` itself are no-ops inside the tracker.
    if let Some(activity_thread) = thread_id_for_event(&event) {
        let activity: State<'_, RunActivityTracker> = app.state();
        activity.record(&activity_thread.0, &event, SystemTime::now());
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
    // over the per-thread Channels only — never the global bus. An empty
    // subscriber list means no pane is currently attached to this thread;
    // transcript events were already persisted above, so the DB hydrate
    // replays them on (re)attach. When several consumers are attached
    // (mirror mode), each receives an identical copy.
    let channels: State<'_, AgentChatChannelRegistry> = app.state();
    for channel in channels.channels_for(&thread_id.0) {
        if let Err(error) = channel.send(payload.clone()) {
            eprintln!(
                "[codemux::agent_chat] Failed to send event on thread channel {}: {error}",
                thread_id.0
            );
        }
    }
}

// ── Per-thread running-subagent tracking ─────────────────────────────

/// Per-thread bookkeeping that keeps the sidebar "working" spinner alive
/// while subagents outlive the parent chat turn.
///
/// The primary chat turn can finish (`TurnCompleted`) while subagents it
/// spawned are still running. The old stateless mapper published `Review`
/// at that point, dropping the spinner even though work continued in the
/// child transcripts. This struct remembers which subagents are still
/// running/pending per thread, plus whether a `Review` transition is
/// *owed* — i.e. the turn completed while subagents were still live and
/// the sidebar should flip to `Review` only once the last one finishes.
///
/// All decision logic lives in [`map_event_to_pane_status`], a pure
/// function over `(event, &mut ThreadSubagentState)` with no `AppHandle`,
/// so it is directly unit-testable.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
struct ThreadSubagentState {
    /// `subagent_id`s currently `Pending` or `Running`.
    running: std::collections::BTreeSet<String>,
    /// A turn finished while `running` was non-empty, so the `Review`
    /// transition is owed once the last tracked subagent goes terminal.
    review_pending: bool,
}

impl ThreadSubagentState {
    /// Whether this thread tracks no live subagents and owes no `Review`
    /// — the entry can be dropped from [`SubagentTracker`] so the map
    /// never grows unbounded.
    fn is_clear(&self) -> bool {
        self.running.is_empty() && !self.review_pending
    }
}

/// Tauri-managed, per-thread running-subagent tracker.
///
/// Shared across the single event-forwarding path ([`forward_event`] →
/// [`publish_pane_status`]). Analogous to [`AgentChatChannelRegistry`]:
/// a `Mutex<HashMap<thread_id, _>>` guarded by a single lock, held only
/// for the duration of one event's decision. Only *live* provider events
/// reach it — transcript hydration/resume replays persisted events on the
/// frontend (`agent_chat_list_messages` + `hydrateThread`), never through
/// `forward_event` — so no replay guard is needed here (see module notes
/// on `should_persist_event`).
#[derive(Default)]
pub struct SubagentTracker {
    threads: Mutex<HashMap<String, ThreadSubagentState>>,
}

impl SubagentTracker {
    /// Run the pane-status decision for `event` against `thread_id`'s
    /// tracked subagent state, mutating it in place. Drops the entry once
    /// it returns to the clear state so the map stays bounded.
    fn decide(&self, thread_id: &str, event: &ProviderRuntimeEvent) -> Option<PaneStatus> {
        let mut threads = self.threads.lock().expect("subagent tracker poisoned");
        let state = threads.entry(thread_id.to_string()).or_default();
        let status = map_event_to_pane_status(event, state);
        if state.is_clear() {
            threads.remove(thread_id);
        }
        status
    }

    /// Forget all subagent tracking for `thread_id`. Called on explicit
    /// pane close so a subagent whose terminal status is never observed
    /// cannot pin the spinner after the session is gone. Idempotent.
    pub fn clear_thread(&self, thread_id: &str) {
        let mut threads = self.threads.lock().expect("subagent tracker poisoned");
        threads.remove(thread_id);
    }
}

/// Map a provider runtime event to the sidebar pane status it should
/// drive, or `None` to leave the current status untouched, updating the
/// thread's [`ThreadSubagentState`] as a side effect.
///
/// The chat-side analogue of `hooks.rs::map_event_type` for terminal
/// agents, so chat sessions publish the same working / needs-input /
/// review vocabulary the sidebar already renders:
///
/// - streaming deltas / committed items / running session / a resolved
///   request (the turn resumes) → `Working`
/// - a user-facing request opened (tool approval, plan, AskUserQuestion)
///   or the session parked on an approval → `Permission`
/// - the turn finished → `Review` (the caller downgrades to `Idle` when
///   the pane's workspace is already active, matching
///   `handle_lifecycle_event`) — **but** if subagents are still running,
///   hold `Working` and defer `Review` until the last one finishes.
/// - the session closed or errored → `Idle` (clear any stuck indicator),
///   and forget this thread's subagent tracking.
///
/// Subagent semantics (the reason this is stateful):
/// - `SubagentUpdated` with `Running`/`Pending` records the subagent as
///   live. It only *restores* `Working` when a `Review` was already owed
///   (a running snapshot arriving after the parent turn finished); during
///   an in-flight turn the parent already owns `Working`, so it returns
///   `None` rather than resurrecting the spinner out of nowhere.
/// - `SubagentUpdated` with a terminal status (`Completed`/`Failed`/
///   `Stopped`) drops the subagent; when the last one clears and a
///   `Review` was owed, it finally publishes `Review`.
///
/// A genuine **new user turn** resets the thread's tracker (both
/// `running` and `review_pending`). The authoritative, provider-agnostic
/// anchor is the send-message command path
/// ([`agent_chat_send_turn`] calls [`SubagentTracker::clear_thread`]);
/// `SessionStateChanged::Running` also resets here (reliably per-turn for
/// Codex/OpenCode). `review_pending` is therefore cleared only by:
/// publishing the owed/normal `Review`, a new-turn reset, session
/// `Closed`/`Error`, or `agent_chat_close_pane` — never by a stray
/// streaming delta.
///
/// Everything else leaves the indicator alone. In particular
/// `SessionConfigured` and the `Starting` / `Ready` lifecycle phases map
/// to `None`: a freshly-started session with no turn is idle, exactly
/// like a freshly-spawned terminal shows no status until its first hook.
fn map_event_to_pane_status(
    event: &ProviderRuntimeEvent,
    state: &mut ThreadSubagentState,
) -> Option<PaneStatus> {
    use crate::agent_provider::{SessionStatus, SubagentStatus};
    match event {
        // Streaming output keeps the spinner alive but must NOT touch
        // `review_pending`. Parent-scoped output (`subagent_id == None`)
        // can still trickle in *after* a deferred `TurnCompleted` (e.g. a
        // final result item) while subagents keep running; clearing the
        // owed `Review` here would silently drop it and strand the pane at
        // `Working`. A genuine new turn is what resets `review_pending` —
        // via the send-message command clear (authoritative,
        // provider-agnostic) or a `SessionStateChanged::Running` snapshot
        // — never a stray delta.
        ProviderRuntimeEvent::ContentDelta { .. }
        | ProviderRuntimeEvent::ItemCompleted { .. }
        | ProviderRuntimeEvent::RequestResolved { .. } => Some(PaneStatus::Working),
        ProviderRuntimeEvent::RequestOpened { .. } => Some(PaneStatus::Permission),
        ProviderRuntimeEvent::SubagentUpdated { subagent, .. } => match subagent.status {
            SubagentStatus::Pending | SubagentStatus::Running => {
                if !subagent.subagent_id.is_empty() {
                    state.running.insert(subagent.subagent_id.clone());
                }
                // Keep/restore the spinner only when a `Review` was owed;
                // otherwise the parent turn owns `Working` and a snapshot
                // (metadata: activity / tokens) must not move the dot.
                if state.review_pending {
                    Some(PaneStatus::Working)
                } else {
                    None
                }
            }
            SubagentStatus::Completed | SubagentStatus::Failed | SubagentStatus::Stopped => {
                state.running.remove(&subagent.subagent_id);
                if state.running.is_empty() && state.review_pending {
                    state.review_pending = false;
                    Some(PaneStatus::Review)
                } else {
                    None
                }
            }
        },
        ProviderRuntimeEvent::TurnCompleted { .. } => {
            if state.running.is_empty() {
                Some(PaneStatus::Review)
            } else {
                // Subagents outlive the parent turn: hold the spinner and
                // owe a `Review` for when the last of them finishes.
                state.review_pending = true;
                Some(PaneStatus::Working)
            }
        }
        ProviderRuntimeEvent::SessionStateChanged { status, .. } => match status {
            SessionStatus::Running { .. } => {
                // A fresh turn started: reset the whole tracker. Both the
                // owed `Review` *and* any still-`running` set from the
                // previous turn are stale — a leftover id would otherwise
                // hold `Working` and suppress `Review` for every future
                // turn until session close (Finding 1). A genuinely-alive
                // background subagent re-inserts itself on its next
                // `Running` snapshot (Claude re-emits these via
                // `task_progress` ticks), so clearing here is safe.
                // Reliable per-turn for Codex (`turn/started`) and OpenCode
                // (`session.status = busy`); Claude's authoritative reset
                // is the send-message command clear (see
                // `agent_chat_send_turn`).
                state.running.clear();
                state.review_pending = false;
                Some(PaneStatus::Working)
            }
            SessionStatus::WaitingApproval { .. } => Some(PaneStatus::Permission),
            SessionStatus::Closed | SessionStatus::Error { .. } => {
                // Tear-down: drop all tracking so a subagent whose
                // terminal status is never observed (e.g. Claude's
                // background `async_launched` task that never emits a
                // later `task_notification`) cannot pin the spinner.
                state.running.clear();
                state.review_pending = false;
                Some(PaneStatus::Idle)
            }
            SessionStatus::Starting | SessionStatus::Ready => None,
        },
        ProviderRuntimeEvent::SessionConfigured { .. }
        | ProviderRuntimeEvent::ResumeCursorUpdated { .. }
        | ProviderRuntimeEvent::RuntimeWarning { .. }
        // Follow-up queue transitions don't drive the sidebar dot on
        // their own: a dispatched turn's own `SessionStateChanged {
        // Running }` sets Working, and enqueue / cancel leave the current
        // indicator untouched.
        | ProviderRuntimeEvent::TurnQueued { .. }
        | ProviderRuntimeEvent::QueuedTurnDispatched { .. }
        | ProviderRuntimeEvent::QueuedTurnCancelled { .. }
        // A stall is advisory only — the run may well be fine (a long quiet
        // tool call). The sidebar dot must NOT change; the amber transcript
        // notice is the entire signal.
        | ProviderRuntimeEvent::RunStalled { .. } => None,
        // A workflow launching/completing doesn't itself imply a pane
        // status transition — the subagents it spawns and the turn it
        // runs within already drive `Working`/`Review` via their own
        // events above.
        ProviderRuntimeEvent::WorkflowUpdated { .. } => None,
    }
}

/// Publish the pane status implied by `event` into the shared
/// `pane_statuses` store and, when it actually changes, push a fresh
/// app-state snapshot to the frontend so the sidebar re-renders.
///
/// The change-guard in
/// [`set_pane_status_by_thread`](AppStateStore::set_pane_status_by_thread)
/// means the `content_delta` token stream collapses to a single
/// `Working` write per turn — so the direct emit here fires only on real
/// transitions (~2–3 per turn), never per token. A direct
/// `Emitter::emit` is used rather than the Wry-only
/// `schedule_emit_app_state` debouncer because `forward_event` is generic
/// over the Tauri runtime (the mock runtime drives the unit tests) and
/// the transitions are already rate-limited by the guard.
fn publish_pane_status<R: Runtime>(
    app: &AppHandle<R>,
    tracker: &SubagentTracker,
    event: &ProviderRuntimeEvent,
) {
    let Some(thread_id) = thread_id_for_event(event) else {
        return;
    };
    if thread_id.0.is_empty() {
        return;
    }
    // The status decision is per-thread and stateful (subagents can
    // outlive the parent turn), so resolve the thread first and route the
    // event through that thread's tracker.
    let Some(mut status) = tracker.decide(&thread_id.0, event) else {
        return;
    };
    let state: State<'_, AppStateStore> = app.state();
    // Mirror the terminal path: a turn that finishes in the workspace the
    // user is already looking at clears to Idle instead of nagging with a
    // review dot for output they can already see.
    if status == PaneStatus::Review && state.is_thread_pane_in_active_workspace(&thread_id.0) {
        status = PaneStatus::Idle;
    }
    let status_changed = state.set_pane_status_by_thread(&thread_id.0, status.clone());
    // A finished run (turn complete and settled to `Review`/`Idle`, or the
    // session closed/errored into `Idle`) releases the workspace's
    // background agent browser session, if any, so the GUI-mode chip /
    // context-bar indicator stop showing "LIVE" once the agent has nothing
    // left to do. `Working`/`Permission` must NOT release — the tracker
    // already defers `Review` while subagents are still running, so by the
    // time we see a terminal status here the run really is over.
    let released = run_finished(status)
        && state
            .agent_chat_pane_id_for_thread(&thread_id.0)
            .and_then(|pane_id| state.workspace_id_for_pane(&pane_id))
            .map(|workspace_id| state.release_detached_agent_browser(&workspace_id))
            .unwrap_or(false);
    // Compute both booleans above (not short-circuited via `||`) so the
    // release call always runs even when the pane status itself didn't
    // change (e.g. the thread was already Idle when the session closed).
    if status_changed || released {
        let snapshot = state.snapshot();
        if let Err(error) = app.emit("app-state-changed", &snapshot) {
            eprintln!("[codemux::agent_chat] failed to emit app state: {error}");
        }
    }
}

/// Whether `status` represents the agent-chat run being over — a turn
/// settled to `Review`/`Idle`, or the underlying session closing/erroring
/// into `Idle`. Extracted from [`publish_pane_status`] so the "should we
/// release the background browser session" policy is unit-testable
/// without an `AppHandle`/`AppStateStore`. `Working` and `Permission` are
/// mid-run and must return `false`.
fn run_finished(status: PaneStatus) -> bool {
    matches!(status, PaneStatus::Review | PaneStatus::Idle)
}

// ── Per-thread run-activity tracking (stall watchdog) ─────────────────

/// How long a mid-turn thread may go silent (zero runtime events) before
/// the stall watchdog flags it. 10 minutes on purpose: long enough that a
/// legitimately quiet stretch — a slow `cargo build`, a big network fetch,
/// a model just thinking — never trips it, short enough that a silently
/// dead run (laptop slept mid-turn, a provider usage-limit cutoff that
/// emits no terminal event) surfaces to the user in a reasonable time.
const STALL_THRESHOLD: Duration = Duration::from_secs(600);

/// How often [`spawn_stall_watchdog`] sweeps the activity tracker. 30s
/// keeps the surfaced "no activity for Nm" duration reasonably fresh
/// without waking the runtime more than twice a minute.
const STALL_SWEEP_INTERVAL: Duration = Duration::from_secs(30);

/// Wall-vs-monotonic gap beyond which the sweep logs a wake-from-sleep
/// breadcrumb. The wall-clock elapsed already handles correctness (the
/// sleep window counts as silence, which is the point); this is only a
/// diagnostic.
const WAKE_JUMP_LOG_THRESHOLD: Duration = Duration::from_secs(60);

/// Per-thread record of the last observed runtime activity, used to
/// detect a mid-turn run that has gone silent.
///
/// Wall clock ([`SystemTime`]) on purpose: after a suspend/resume the
/// elapsed silence includes the sleep window, which is exactly the
/// wake-from-sleep detection issue #154 asks for (a monotonic clock would
/// "pause" across sleep and never notice the dead run).
#[derive(Debug, Clone, PartialEq, Eq)]
struct ThreadActivity {
    /// When the last activity-bearing event was observed.
    last_event: SystemTime,
    /// Whether a turn is in flight. Only mid-turn threads can stall.
    mid_turn: bool,
    /// Whether the thread is parked on a user approval. Waiting on the
    /// USER is expected silence, so the stall clock pauses.
    waiting_approval: bool,
}

/// Outcome of feeding one event to the activity tracker. Kept as a pure
/// value so [`activity_update`] is unit-testable without any shared state.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ActivityUpdate {
    /// Replace the thread's activity record with this value.
    Set(ThreadActivity),
    /// Remove the thread entry (turn settled / session gone). Keeps the
    /// map bounded — entries only exist while a thread is mid-turn.
    Remove,
    /// Leave the current record untouched (event is not activity).
    Ignore,
}

/// Pure decision: how `event` affects a thread's run-activity record,
/// given the current entry (if any) and the current wall-clock time.
///
/// Every mid-turn signal stamps `now` and (re)arms `mid_turn`; an
/// approval prompt additionally pauses the stall clock (`waiting_approval`)
/// until it resolves. Turn/session settlement removes the entry.
/// [`ProviderRuntimeEvent::RunStalled`] is explicitly ignored so a
/// re-emitted stall notice can never count as activity and reset its own
/// clock.
fn activity_update(
    current: Option<&ThreadActivity>,
    event: &ProviderRuntimeEvent,
    now: SystemTime,
) -> ActivityUpdate {
    use crate::agent_provider::SessionStatus;
    let mid_turn = |waiting_approval: bool| {
        ActivityUpdate::Set(ThreadActivity {
            last_event: now,
            mid_turn: true,
            waiting_approval,
        })
    };
    match event {
        // Real agent progress — the turn is alive and not waiting on us.
        ProviderRuntimeEvent::ContentDelta { .. }
        | ProviderRuntimeEvent::ItemCompleted { .. }
        | ProviderRuntimeEvent::SubagentUpdated { .. }
        | ProviderRuntimeEvent::WorkflowUpdated { .. }
        | ProviderRuntimeEvent::QueuedTurnDispatched { .. }
        | ProviderRuntimeEvent::RequestResolved { .. } => mid_turn(false),
        // A turn started (Codex/OpenCode emit this reliably per turn).
        ProviderRuntimeEvent::SessionStateChanged {
            status: SessionStatus::Running { .. },
            ..
        } => mid_turn(false),
        // Waiting on the user — pause the stall clock.
        ProviderRuntimeEvent::RequestOpened { .. }
        | ProviderRuntimeEvent::SessionStateChanged {
            status: SessionStatus::WaitingApproval { .. },
            ..
        } => mid_turn(true),
        // Turn/session settled — the thread is no longer mid-turn.
        ProviderRuntimeEvent::TurnCompleted { .. }
        | ProviderRuntimeEvent::SessionStateChanged {
            status: SessionStatus::Ready
                | SessionStatus::Closed
                | SessionStatus::Error { .. },
            ..
        } => ActivityUpdate::Remove,
        // Everything else (Starting/Ready-lifecycle noise, warnings,
        // resume cursors, queue enqueue/cancel, config) is not activity;
        // crucially `RunStalled` itself must not reset the clock.
        _ => {
            // Preserve the current record if one exists; otherwise there is
            // nothing to track.
            let _ = current;
            ActivityUpdate::Ignore
        }
    }
}

/// Pure sweep predicate: `Some(silent_secs)` when a thread is mid-turn,
/// not waiting on the user, and silent for at least `threshold`.
fn select_stalled(a: &ThreadActivity, now: SystemTime, threshold: Duration) -> Option<u64> {
    if !a.mid_turn || a.waiting_approval {
        return None;
    }
    let elapsed = now.duration_since(a.last_event).unwrap_or_default();
    if elapsed >= threshold {
        Some(elapsed.as_secs())
    } else {
        None
    }
}

/// Tauri-managed, per-thread run-activity tracker feeding the stall
/// watchdog. Mirrors [`SubagentTracker`]: a single `Mutex<HashMap>` held
/// only for the duration of one event's decision. Only *live* provider
/// events reach it (transcript hydration replays on the frontend, never
/// through `forward_event`), so no replay guard is needed.
#[derive(Default)]
pub struct RunActivityTracker {
    threads: Mutex<HashMap<String, ThreadActivity>>,
}

impl RunActivityTracker {
    /// Fold one event into `thread_id`'s activity record.
    fn record(&self, thread_id: &str, event: &ProviderRuntimeEvent, now: SystemTime) {
        if thread_id.is_empty() {
            return;
        }
        let mut threads = self.threads.lock().expect("activity tracker poisoned");
        match activity_update(threads.get(thread_id), event, now) {
            ActivityUpdate::Set(activity) => {
                threads.insert(thread_id.to_string(), activity);
            }
            ActivityUpdate::Remove => {
                threads.remove(thread_id);
            }
            ActivityUpdate::Ignore => {}
        }
    }

    /// Snapshot the threads that have been mid-turn and silent for at
    /// least `threshold`, with how long (seconds) each has been silent.
    fn stalled(&self, now: SystemTime, threshold: Duration) -> Vec<(String, u64)> {
        let threads = self.threads.lock().expect("activity tracker poisoned");
        threads
            .iter()
            .filter_map(|(id, activity)| {
                select_stalled(activity, now, threshold).map(|secs| (id.clone(), secs))
            })
            .collect()
    }

    /// Forget all activity tracking for `thread_id`. Called on explicit
    /// pane close so a thread whose terminal event is never observed can't
    /// keep tripping the watchdog. Idempotent.
    pub fn clear_thread(&self, thread_id: &str) {
        let mut threads = self.threads.lock().expect("activity tracker poisoned");
        threads.remove(thread_id);
    }
}

/// Background sweep: every [`STALL_SWEEP_INTERVAL`] flag any thread that
/// has been mid-turn and silent for at least [`STALL_THRESHOLD`] by
/// forwarding a [`ProviderRuntimeEvent::RunStalled`]. Advisory only — the
/// session is never touched. Re-emitting on subsequent ticks while still
/// stalled is intentional: it keeps the UI's "no activity for Nm" duration
/// fresh, and the event is transient (never persisted, cleared by the
/// frontend on the next real activity).
///
/// Spawned once from `lib.rs` right after `spawn_event_bridge`, inside the
/// same `agent_chat_enabled` gate. Generic over the Tauri runtime so the
/// unit tests can drive the pure helpers on the mock runtime.
pub async fn spawn_stall_watchdog<R: Runtime>(app: AppHandle<R>) {
    tauri::async_runtime::spawn(async move {
        let mut interval = tokio::time::interval(STALL_SWEEP_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // Previous tick's (monotonic, wall) pair, for the wake-from-sleep
        // breadcrumb. Correctness rides on wall-clock elapsed; this is
        // diagnostics only.
        let mut prev: Option<(std::time::Instant, SystemTime)> = None;
        loop {
            interval.tick().await;
            let now_mono = std::time::Instant::now();
            let now_wall = SystemTime::now();
            if let Some((prev_mono, prev_wall)) = prev {
                let mono_delta = now_mono.duration_since(prev_mono);
                let wall_delta = now_wall
                    .duration_since(prev_wall)
                    .unwrap_or_default();
                if wall_delta > mono_delta + WAKE_JUMP_LOG_THRESHOLD {
                    eprintln!(
                        "[codemux::agent_chat] stall watchdog observed a wake-from-sleep jump (wall {wall_delta:?} vs monotonic {mono_delta:?})"
                    );
                }
            }
            prev = Some((now_mono, now_wall));

            let stalled = {
                let tracker: State<'_, RunActivityTracker> = app.state();
                tracker.stalled(now_wall, STALL_THRESHOLD)
            };
            for (thread_id, silent_for_secs) in stalled {
                forward_event(
                    &app,
                    ProviderRuntimeEvent::RunStalled {
                        thread_id: ThreadId(thread_id),
                        silent_for_secs,
                    },
                );
            }
        }
    });
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
            // Subagent snapshots persist so the orchestration card and
            // child transcripts survive restart: DB hydrate replays them
            // through the same reducer with zero schema migration.
            | ProviderRuntimeEvent::SubagentUpdated { .. }
            // Workflow snapshots persist for the same reason — the
            // workflow run card and its phase attribution must survive a
            // restart via hydrate-replay, not a bespoke schema.
            | ProviderRuntimeEvent::WorkflowUpdated { .. }
    )
    // NOTE: `RunStalled` is deliberately NOT persisted. It is a transient
    // advisory recomputed live by the stall watchdog; the durable record of
    // a dead run is the settled/`child_exited` `TurnCompleted`, which drives
    // the "Run interrupted" divider on hydrate.
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
        | ProviderRuntimeEvent::SubagentUpdated { thread_id, .. }
        | ProviderRuntimeEvent::WorkflowUpdated { thread_id, .. }
        | ProviderRuntimeEvent::ResumeCursorUpdated { thread_id, .. }
        | ProviderRuntimeEvent::TurnQueued { thread_id, .. }
        | ProviderRuntimeEvent::QueuedTurnDispatched { thread_id, .. }
        | ProviderRuntimeEvent::QueuedTurnCancelled { thread_id, .. }
        | ProviderRuntimeEvent::RunStalled { thread_id, .. } => Some(thread_id.clone()),
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

    // ── chat image persistence ──

    #[test]
    fn chat_image_extension_maps_known_types_and_falls_back() {
        assert_eq!(chat_image_extension("image/png"), "png");
        assert_eq!(chat_image_extension("image/jpeg"), "jpg");
        assert_eq!(chat_image_extension("image/jpg"), "jpg");
        assert_eq!(chat_image_extension("image/gif"), "gif");
        assert_eq!(chat_image_extension("image/webp"), "webp");
        // Case-insensitive.
        assert_eq!(chat_image_extension("IMAGE/PNG"), "png");
        // Unknown / non-image types fall back to `bin` rather than drop.
        assert_eq!(chat_image_extension("image/heic"), "bin");
        assert_eq!(chat_image_extension("application/pdf"), "bin");
    }

    /// Build the user_message envelope the same way `persist_user_message`
    /// does, so the shape assertions below don't need a DB.
    fn user_message_envelope(
        thread_id: &str,
        text: &str,
        images: &[PersistedChatImage],
    ) -> serde_json::Value {
        let mut user_msg = json!({
            "type": "user_message",
            "thread_id": thread_id,
            "text": text,
        });
        if !images.is_empty() {
            let images_json: Vec<serde_json::Value> = images
                .iter()
                .map(|image| json!({"path": image.path, "media_type": image.media_type}))
                .collect();
            user_msg["images"] = serde_json::Value::Array(images_json);
        }
        user_msg
    }

    #[test]
    fn user_message_envelope_omits_images_when_none() {
        let envelope = user_message_envelope("t1", "hello", &[]);
        assert_eq!(
            envelope,
            json!({"type": "user_message", "thread_id": "t1", "text": "hello"})
        );
        // The field is ABSENT, not `null` — backward compatible with the
        // pre-images transcript shape.
        assert!(envelope.get("images").is_none());
    }

    #[test]
    fn user_message_envelope_includes_images_when_present() {
        let images = vec![
            PersistedChatImage {
                path: "/tmp/a.png".into(),
                media_type: "image/png".into(),
            },
            PersistedChatImage {
                path: "/tmp/b.jpg".into(),
                media_type: "image/jpeg".into(),
            },
        ];
        let envelope = user_message_envelope("t1", "look", &images);
        assert_eq!(
            envelope["images"],
            json!([
                {"path": "/tmp/a.png", "media_type": "image/png"},
                {"path": "/tmp/b.jpg", "media_type": "image/jpeg"},
            ])
        );
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
            is_git: true,
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
        CompletedItem, ContentDelta, SubagentSnapshot, SubagentStatus, TurnStatus, TurnUsage,
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
            subagent_id: None,
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
            subagent_id: None,
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
            subagent_id: None,
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
            subagent_id: None,
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
            subagent_id: None,
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
    fn should_not_persist_run_stalled() {
        // Transient advisory — recomputed live, never replayed.
        let e = ProviderRuntimeEvent::RunStalled {
            thread_id: tid(),
            silent_for_secs: 700,
        };
        assert!(!should_persist_event(&e));
    }

    // ── Run-activity tracker + stall selection (pure) ──

    fn running_event() -> ProviderRuntimeEvent {
        ProviderRuntimeEvent::SessionStateChanged {
            thread_id: tid(),
            status: SessionStatus::Running { active_turn: turn() },
        }
    }

    #[test]
    fn activity_running_arms_mid_turn_and_stamps() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
        match activity_update(None, &running_event(), now) {
            ActivityUpdate::Set(a) => {
                assert!(a.mid_turn);
                assert!(!a.waiting_approval);
                assert_eq!(a.last_event, now);
            }
            other => panic!("expected Set, got {other:?}"),
        }
    }

    #[test]
    fn activity_content_delta_stamps_mid_turn() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(50);
        let e = ProviderRuntimeEvent::ContentDelta {
            thread_id: tid(),
            turn_id: turn(),
            delta: ContentDelta::Text { text: "hi".into() },
            subagent_id: None,
        };
        assert!(matches!(
            activity_update(None, &e, now),
            ActivityUpdate::Set(ThreadActivity { mid_turn: true, waiting_approval: false, .. })
        ));
    }

    #[test]
    fn activity_approval_pauses_the_stall_clock() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(10);
        let opened = ProviderRuntimeEvent::RequestOpened {
            thread_id: tid(),
            turn_id: turn(),
            request_id: req(),
            request_kind: "tool".into(),
            payload: json!({}),
            tool_use_id: None,
            subagent_id: None,
        };
        assert!(matches!(
            activity_update(None, &opened, now),
            ActivityUpdate::Set(ThreadActivity { waiting_approval: true, .. })
        ));
        let waiting = ProviderRuntimeEvent::SessionStateChanged {
            thread_id: tid(),
            status: SessionStatus::WaitingApproval { request_id: req() },
        };
        assert!(matches!(
            activity_update(None, &waiting, now),
            ActivityUpdate::Set(ThreadActivity { waiting_approval: true, .. })
        ));
        // Resolving the approval un-pauses the clock.
        let resolved = ProviderRuntimeEvent::RequestResolved {
            thread_id: tid(),
            request_id: req(),
            decision: ApprovalDecision::AllowForSession,
        };
        assert!(matches!(
            activity_update(None, &resolved, now),
            ActivityUpdate::Set(ThreadActivity { waiting_approval: false, mid_turn: true, .. })
        ));
    }

    #[test]
    fn activity_turn_completed_and_terminal_states_remove() {
        let now = SystemTime::UNIX_EPOCH;
        let completed = ProviderRuntimeEvent::TurnCompleted {
            thread_id: tid(),
            turn_id: turn(),
            status: TurnStatus::Success,
            usage: None,
        };
        assert_eq!(activity_update(None, &completed, now), ActivityUpdate::Remove);
        for status in [
            SessionStatus::Ready,
            SessionStatus::Closed,
            SessionStatus::Error { message: "x".into() },
        ] {
            let e = ProviderRuntimeEvent::SessionStateChanged {
                thread_id: tid(),
                status,
            };
            assert_eq!(activity_update(None, &e, now), ActivityUpdate::Remove);
        }
    }

    #[test]
    fn activity_run_stalled_is_ignored_not_activity() {
        // RunStalled itself must never reset the clock, or the notice would
        // keep resurrecting its own freshness.
        let now = SystemTime::UNIX_EPOCH;
        let e = ProviderRuntimeEvent::RunStalled {
            thread_id: tid(),
            silent_for_secs: 700,
        };
        assert_eq!(activity_update(None, &e, now), ActivityUpdate::Ignore);
    }

    #[test]
    fn select_stalled_respects_threshold_boundary() {
        let base = SystemTime::UNIX_EPOCH + Duration::from_secs(10_000);
        let a = ThreadActivity {
            last_event: base,
            mid_turn: true,
            waiting_approval: false,
        };
        // Just under threshold → not stalled.
        assert_eq!(
            select_stalled(&a, base + STALL_THRESHOLD - Duration::from_secs(1), STALL_THRESHOLD),
            None
        );
        // Exactly at threshold → stalled, reports elapsed seconds.
        assert_eq!(
            select_stalled(&a, base + STALL_THRESHOLD, STALL_THRESHOLD),
            Some(STALL_THRESHOLD.as_secs())
        );
    }

    #[test]
    fn select_stalled_excludes_waiting_and_idle() {
        let base = SystemTime::UNIX_EPOCH + Duration::from_secs(10_000);
        let late = base + STALL_THRESHOLD + Duration::from_secs(60);
        // Waiting on the user is expected silence — never stalled.
        let waiting = ThreadActivity {
            last_event: base,
            mid_turn: true,
            waiting_approval: true,
        };
        assert_eq!(select_stalled(&waiting, late, STALL_THRESHOLD), None);
        // Not mid-turn → nothing to stall.
        let idle = ThreadActivity {
            last_event: base,
            mid_turn: false,
            waiting_approval: false,
        };
        assert_eq!(select_stalled(&idle, late, STALL_THRESHOLD), None);
    }

    #[test]
    fn activity_tracker_records_and_sweeps() {
        let tracker = RunActivityTracker::default();
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(100);
        tracker.record("thread-a", &running_event(), t0);
        // Below threshold: not yet stalled.
        assert!(tracker
            .stalled(t0 + Duration::from_secs(300), STALL_THRESHOLD)
            .is_empty());
        // Past threshold: surfaced.
        let stalled = tracker.stalled(t0 + STALL_THRESHOLD, STALL_THRESHOLD);
        assert_eq!(stalled, vec![("thread-a".to_string(), STALL_THRESHOLD.as_secs())]);
        // A settling turn removes the entry — the map stays bounded.
        let completed = ProviderRuntimeEvent::TurnCompleted {
            thread_id: ThreadId("thread-a".into()),
            turn_id: turn(),
            status: TurnStatus::Success,
            usage: None,
        };
        tracker.record("thread-a", &completed, t0 + STALL_THRESHOLD);
        assert!(tracker
            .stalled(t0 + STALL_THRESHOLD * 2, STALL_THRESHOLD)
            .is_empty());
    }

    #[test]
    fn run_stalled_leaves_pane_status_untouched() {
        // Advisory only — the sidebar dot must not move.
        let mut st = ThreadSubagentState::default();
        let e = ProviderRuntimeEvent::RunStalled {
            thread_id: tid(),
            silent_for_secs: 700,
        };
        assert_eq!(map_event_to_pane_status(&e, &mut st), None);
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
                subagent_id: None,
            },
        }
    }

    /// Fan a payload out to every channel the registry holds for a thread,
    /// exactly as `forward_event` does for thread-scoped events.
    fn fan_out(
        registry: &AgentChatChannelRegistry,
        thread: &str,
        payload: AgentChatEventPayload,
    ) {
        for channel in registry.channels_for(thread) {
            channel.send(payload.clone()).expect("send ok");
        }
    }

    #[test]
    fn registry_attach_routes_sends_through_channel() {
        let registry = AgentChatChannelRegistry::default();
        let (channel, captured) = capture_channel();
        registry.attach("t1", channel);

        fan_out(&registry, "t1", delta_payload("t1", "hello"));

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
    fn registry_channels_for_unknown_thread_is_empty() {
        let registry = AgentChatChannelRegistry::default();
        assert!(registry.channels_for("nope").is_empty());
    }

    /// Two panes attached to the SAME thread (mirror mode) BOTH receive every
    /// event — the multi-client fan-out guarantee.
    #[test]
    fn registry_fans_out_to_all_subscribers() {
        let registry = AgentChatChannelRegistry::default();
        let (first, first_captured) = capture_channel();
        let (second, second_captured) = capture_channel();
        let g1 = registry.attach("t1", first);
        let g2 = registry.attach("t1", second);
        assert_ne!(g1, g2, "each attach mints a fresh generation");

        fan_out(&registry, "t1", delta_payload("t1", "x"));

        assert_eq!(
            first_captured.lock().unwrap().len(),
            1,
            "first subscriber receives its copy"
        );
        assert_eq!(
            second_captured.lock().unwrap().len(),
            1,
            "second subscriber receives its copy"
        );
    }

    /// Detaching one subscriber removes exactly that one and leaves the other
    /// streaming (the per-generation teardown).
    #[test]
    fn registry_detach_removes_only_matching_subscriber() {
        let registry = AgentChatChannelRegistry::default();
        let (first, first_captured) = capture_channel();
        let (second, second_captured) = capture_channel();
        let g1 = registry.attach("t1", first);
        let _g2 = registry.attach("t1", second);

        assert!(registry.detach("t1", g1), "matching detach removes the entry");

        fan_out(&registry, "t1", delta_payload("t1", "after"));
        assert!(
            first_captured.lock().unwrap().is_empty(),
            "detached subscriber must receive nothing further"
        );
        assert_eq!(
            second_captured.lock().unwrap().len(),
            1,
            "the surviving subscriber keeps streaming"
        );
    }

    #[test]
    fn registry_detach_with_stale_generation_is_noop() {
        let registry = AgentChatChannelRegistry::default();
        let (channel, captured) = capture_channel();
        let generation = registry.attach("t1", channel);

        // A late detach carrying a generation that was never issued (or already
        // removed) must not tear down the live subscriber.
        assert!(!registry.detach("t1", generation + 999));
        fan_out(&registry, "t1", delta_payload("t1", "still-live"));
        assert_eq!(
            captured.lock().unwrap().len(),
            1,
            "live subscriber survives a stale detach"
        );
    }

    #[test]
    fn registry_detach_with_current_generation_removes() {
        let registry = AgentChatChannelRegistry::default();
        let (channel, _) = capture_channel();
        let generation = registry.attach("t1", channel);
        assert!(registry.detach("t1", generation));
        assert!(registry.channels_for("t1").is_empty());
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

        fan_out(&registry, "thread-a", delta_payload("thread-a", "for-a"));

        assert_eq!(a_captured.lock().unwrap().len(), 1);
        assert!(
            b_captured.lock().unwrap().is_empty(),
            "no cross-thread leakage"
        );
    }

    /// Build a `SubagentUpdated` event carrying just an id + status.
    fn subagent_event(id: &str, status: SubagentStatus) -> ProviderRuntimeEvent {
        ProviderRuntimeEvent::SubagentUpdated {
            thread_id: tid(),
            subagent: SubagentSnapshot {
                subagent_id: id.into(),
                status,
                ..Default::default()
            },
        }
    }

    fn turn_completed() -> ProviderRuntimeEvent {
        ProviderRuntimeEvent::TurnCompleted {
            thread_id: tid(),
            turn_id: turn(),
            status: TurnStatus::Success,
            usage: None,
        }
    }

    #[test]
    fn map_event_to_pane_status_covers_the_lifecycle() {
        use crate::state::PaneStatus;

        // The mapper is now stateful; each stand-alone assertion below
        // uses a fresh, empty tracker state so it exercises the base
        // vocabulary in isolation (no subagents in flight).
        let mut st = ThreadSubagentState::default();

        // Working: a turn is producing output or resuming after approval.
        assert_eq!(
            map_event_to_pane_status(
                &ProviderRuntimeEvent::ContentDelta {
                    thread_id: tid(),
                    turn_id: turn(),
                    delta: ContentDelta::Text { text: "hi".into() },
                    subagent_id: None,
                },
                &mut st
            ),
            Some(PaneStatus::Working)
        );
        assert_eq!(
            map_event_to_pane_status(
                &ProviderRuntimeEvent::ItemCompleted {
                    thread_id: tid(),
                    turn_id: turn(),
                    item: CompletedItem::AssistantText { text: "hi".into() },
                    subagent_id: None,
                },
                &mut st
            ),
            Some(PaneStatus::Working)
        );
        assert_eq!(
            map_event_to_pane_status(
                &ProviderRuntimeEvent::RequestResolved {
                    thread_id: tid(),
                    request_id: req(),
                    decision: ApprovalDecision::AllowForSession,
                },
                &mut st
            ),
            Some(PaneStatus::Working)
        );
        assert_eq!(
            map_event_to_pane_status(
                &ProviderRuntimeEvent::SessionStateChanged {
                    thread_id: tid(),
                    status: SessionStatus::Running { active_turn: turn() },
                },
                &mut st
            ),
            Some(PaneStatus::Working)
        );

        // Permission: a user-facing request is open / session parked on one.
        assert_eq!(
            map_event_to_pane_status(
                &ProviderRuntimeEvent::RequestOpened {
                    thread_id: tid(),
                    turn_id: turn(),
                    request_id: req(),
                    request_kind: "tool".into(),
                    payload: json!({}),
                    tool_use_id: None,
                    subagent_id: None,
                },
                &mut st
            ),
            Some(PaneStatus::Permission)
        );
        assert_eq!(
            map_event_to_pane_status(
                &ProviderRuntimeEvent::SessionStateChanged {
                    thread_id: tid(),
                    status: SessionStatus::WaitingApproval { request_id: req() },
                },
                &mut st
            ),
            Some(PaneStatus::Permission)
        );

        // Review: the turn finished with no subagents in flight (caller
        // may downgrade to Idle if focused).
        assert_eq!(
            map_event_to_pane_status(&turn_completed(), &mut st),
            Some(PaneStatus::Review)
        );

        // Idle: session torn down or broken — clear the indicator.
        assert_eq!(
            map_event_to_pane_status(
                &ProviderRuntimeEvent::SessionStateChanged {
                    thread_id: tid(),
                    status: SessionStatus::Closed,
                },
                &mut st
            ),
            Some(PaneStatus::Idle)
        );
        assert_eq!(
            map_event_to_pane_status(
                &ProviderRuntimeEvent::SessionStateChanged {
                    thread_id: tid(),
                    status: SessionStatus::Error { message: "boom".into() },
                },
                &mut st
            ),
            Some(PaneStatus::Idle)
        );

        // None: a fresh/ready session and configuration notices don't move
        // the dot — parity with a freshly-spawned terminal showing nothing.
        assert_eq!(
            map_event_to_pane_status(
                &ProviderRuntimeEvent::SessionStateChanged {
                    thread_id: tid(),
                    status: SessionStatus::Ready,
                },
                &mut st
            ),
            None
        );
        assert_eq!(
            map_event_to_pane_status(
                &ProviderRuntimeEvent::SessionStateChanged {
                    thread_id: tid(),
                    status: SessionStatus::Starting,
                },
                &mut st
            ),
            None
        );
        assert_eq!(
            map_event_to_pane_status(
                &ProviderRuntimeEvent::SessionConfigured {
                    thread_id: tid(),
                    provider_session_id: ProviderSessionId("s".into()),
                },
                &mut st
            ),
            None
        );
        assert_eq!(
            map_event_to_pane_status(
                &ProviderRuntimeEvent::RuntimeWarning {
                    thread_id: Some(tid()),
                    message: "warn".into(),
                    original_payload: None,
                },
                &mut st
            ),
            None
        );
    }

    #[test]
    fn run_finished_only_for_review_and_idle() {
        // Backs the background-agent-browser release wiring in
        // `publish_pane_status`: a run is only "over" once the tracker
        // settles to Review (turn done, nothing left in flight) or Idle
        // (turn done and already seen, or session closed/errored). A
        // mid-run Working or an approval prompt must never release the
        // chip out from under a still-live agent.
        assert!(run_finished(PaneStatus::Review));
        assert!(run_finished(PaneStatus::Idle));
        assert!(!run_finished(PaneStatus::Working));
        assert!(!run_finished(PaneStatus::Permission));
    }

    // (a) TurnCompleted while a subagent is still running holds Working
    //     instead of flipping to Review.
    #[test]
    fn turn_completed_with_running_subagent_stays_working() {
        let mut st = ThreadSubagentState::default();
        // Subagent spawns mid-turn.
        assert_eq!(
            map_event_to_pane_status(&subagent_event("s1", SubagentStatus::Running), &mut st),
            None,
            "a running snapshot during an in-flight turn doesn't move the dot"
        );
        // Parent turn finishes but the subagent is still running.
        assert_eq!(
            map_event_to_pane_status(&turn_completed(), &mut st),
            Some(PaneStatus::Working),
            "spinner holds while the subagent runs"
        );
        assert!(st.review_pending, "a Review is now owed");
        assert!(st.running.contains("s1"));
    }

    // (b) The last subagent going terminal after TurnCompleted publishes
    //     the deferred Review.
    #[test]
    fn last_subagent_terminal_after_turn_completed_publishes_review() {
        let mut st = ThreadSubagentState::default();
        map_event_to_pane_status(&subagent_event("s1", SubagentStatus::Running), &mut st);
        map_event_to_pane_status(&subagent_event("s2", SubagentStatus::Running), &mut st);
        map_event_to_pane_status(&turn_completed(), &mut st);

        // First subagent finishes — still one running, no Review yet.
        assert_eq!(
            map_event_to_pane_status(&subagent_event("s1", SubagentStatus::Completed), &mut st),
            None
        );
        assert!(st.review_pending);
        // Last subagent finishes — the deferred Review fires.
        assert_eq!(
            map_event_to_pane_status(&subagent_event("s2", SubagentStatus::Failed), &mut st),
            Some(PaneStatus::Review)
        );
        assert!(st.is_clear(), "tracker returns to the clear state");
    }

    // (c) No subagents: TurnCompleted → Review, unchanged from before.
    #[test]
    fn turn_completed_without_subagents_reviews() {
        let mut st = ThreadSubagentState::default();
        assert_eq!(
            map_event_to_pane_status(&turn_completed(), &mut st),
            Some(PaneStatus::Review)
        );
        assert!(st.is_clear());
    }

    // (d) Session Closed / Error clears tracking so a never-terminating
    //     subagent can't pin the spinner.
    #[test]
    fn session_close_clears_subagent_tracking() {
        use crate::state::PaneStatus;
        let mut st = ThreadSubagentState::default();
        map_event_to_pane_status(&subagent_event("s1", SubagentStatus::Running), &mut st);
        map_event_to_pane_status(&turn_completed(), &mut st);
        assert!(!st.is_clear());

        assert_eq!(
            map_event_to_pane_status(
                &ProviderRuntimeEvent::SessionStateChanged {
                    thread_id: tid(),
                    status: SessionStatus::Closed,
                },
                &mut st
            ),
            Some(PaneStatus::Idle)
        );
        assert!(st.is_clear(), "close forgets the stuck subagent");

        // Same for Error.
        let mut st = ThreadSubagentState::default();
        map_event_to_pane_status(&subagent_event("s1", SubagentStatus::Running), &mut st);
        map_event_to_pane_status(&turn_completed(), &mut st);
        assert_eq!(
            map_event_to_pane_status(
                &ProviderRuntimeEvent::SessionStateChanged {
                    thread_id: tid(),
                    status: SessionStatus::Error { message: "boom".into() },
                },
                &mut st
            ),
            Some(PaneStatus::Idle)
        );
        assert!(st.is_clear());
    }

    // (e) Subagent completes BEFORE the parent turn finishes → normal
    //     Review, no deferral.
    #[test]
    fn subagent_completes_before_turn_completed_reviews() {
        let mut st = ThreadSubagentState::default();
        map_event_to_pane_status(&subagent_event("s1", SubagentStatus::Running), &mut st);
        // Subagent finishes while the parent turn is still going.
        assert_eq!(
            map_event_to_pane_status(&subagent_event("s1", SubagentStatus::Completed), &mut st),
            None,
            "no Review owed yet, so a terminal subagent is silent"
        );
        assert!(st.is_clear());
        // Parent turn then completes with nothing outstanding.
        assert_eq!(
            map_event_to_pane_status(&turn_completed(), &mut st),
            Some(PaneStatus::Review)
        );
    }

    // A running snapshot arriving AFTER a Review is owed restores Working
    // (the resurrection guard only blocks snapshots during a live turn).
    #[test]
    fn running_snapshot_after_review_owed_restores_working() {
        let mut st = ThreadSubagentState::default();
        map_event_to_pane_status(&subagent_event("s1", SubagentStatus::Running), &mut st);
        map_event_to_pane_status(&turn_completed(), &mut st);
        assert_eq!(
            map_event_to_pane_status(&subagent_event("s1", SubagentStatus::Running), &mut st),
            Some(PaneStatus::Working),
            "a live running snapshot keeps the spinner while Review is owed"
        );
    }

    // (f) The tracker drops the thread entry once it returns to clear, so
    //     a later replayed/stray terminal snapshot can't underflow or
    //     resurrect anything — it maps to None against a fresh state.
    #[test]
    fn tracker_drops_cleared_threads_and_ignores_stray_terminals() {
        let tracker = SubagentTracker::default();
        // Live turn with a subagent that outlives it.
        assert_eq!(
            tracker.decide("t", &subagent_event("s1", SubagentStatus::Running)),
            None
        );
        assert_eq!(
            tracker.decide("t", &turn_completed()),
            Some(PaneStatus::Working)
        );
        assert_eq!(
            tracker.decide("t", &subagent_event("s1", SubagentStatus::Completed)),
            Some(PaneStatus::Review)
        );
        // Entry is dropped; a stray duplicate terminal is a harmless None.
        assert_eq!(
            tracker.decide("t", &subagent_event("s1", SubagentStatus::Completed)),
            None
        );
        // clear_thread is idempotent on an already-gone thread.
        tracker.clear_thread("t");
        tracker.clear_thread("does-not-exist");
    }

    fn session_running() -> ProviderRuntimeEvent {
        ProviderRuntimeEvent::SessionStateChanged {
            thread_id: tid(),
            status: SessionStatus::Running { active_turn: turn() },
        }
    }

    // Finding 1 (regression): a subagent left non-terminal by a prior turn
    // must NOT suppress `Review` on every later turn. The new-turn reset
    // (here modelled by `SessionStateChanged::Running`, the provider-level
    // per-turn anchor) drains the stale `running` id so the next
    // `TurnCompleted` flips to `Review`. Would fail before the fix, when
    // `Running` cleared only `review_pending` and left `running` intact.
    #[test]
    fn stale_running_subagent_does_not_suppress_review_across_turns() {
        let mut st = ThreadSubagentState::default();
        // Turn 1: a background subagent spawns and never goes terminal.
        map_event_to_pane_status(&subagent_event("bg", SubagentStatus::Running), &mut st);
        assert_eq!(
            map_event_to_pane_status(&turn_completed(), &mut st),
            Some(PaneStatus::Working),
            "turn 1 defers Review while the background subagent runs"
        );
        assert!(st.review_pending);
        assert!(st.running.contains("bg"));

        // Turn 2 starts: the new-turn reset drains the stale tracking.
        assert_eq!(
            map_event_to_pane_status(&session_running(), &mut st),
            Some(PaneStatus::Working)
        );
        assert!(st.is_clear(), "new turn resets both running and review_pending");

        // Turn 2 finishes with nothing tracked → Review fires normally.
        assert_eq!(
            map_event_to_pane_status(&turn_completed(), &mut st),
            Some(PaneStatus::Review),
            "no stale id suppresses the second turn's Review"
        );
    }

    // Same regression via the authoritative anchor: the send-message
    // command path clears the tracker entry. Model it with
    // `SubagentTracker::clear_thread` between the two turns.
    #[test]
    fn send_command_clear_resets_tracking_across_turns() {
        let tracker = SubagentTracker::default();
        // Turn 1: subagent outlives the turn, Review deferred.
        tracker.decide("t", &subagent_event("bg", SubagentStatus::Running));
        assert_eq!(tracker.decide("t", &turn_completed()), Some(PaneStatus::Working));

        // agent_chat_send_turn clears the thread's tracker before turn 2.
        tracker.clear_thread("t");

        // Turn 2 completes clean → Review, not a stuck Working.
        assert_eq!(tracker.decide("t", &turn_completed()), Some(PaneStatus::Review));
    }

    // Finding 2: a deferred Review must survive parent-scoped output that
    // trickles in after `TurnCompleted` while subagents still run. The
    // parent-scoped `ItemCompleted` maps `Working` but must NOT clear the
    // owed `review_pending`, so the later terminal subagent still fires
    // `Review`. Would fail before the fix, when parent-scoped output with
    // `subagent_id == None` cleared `review_pending`.
    #[test]
    fn deferred_review_survives_parent_scoped_output() {
        let mut st = ThreadSubagentState::default();
        map_event_to_pane_status(&subagent_event("s1", SubagentStatus::Running), &mut st);
        assert_eq!(
            map_event_to_pane_status(&turn_completed(), &mut st),
            Some(PaneStatus::Working),
            "Review deferred while the subagent runs"
        );
        assert!(st.review_pending);

        // A late parent-scoped item (subagent_id == None) arrives after the
        // deferred TurnCompleted. It keeps Working but leaves the owed
        // Review intact.
        assert_eq!(
            map_event_to_pane_status(
                &ProviderRuntimeEvent::ItemCompleted {
                    thread_id: tid(),
                    turn_id: turn(),
                    item: CompletedItem::AssistantText { text: "final".into() },
                    subagent_id: None,
                },
                &mut st
            ),
            Some(PaneStatus::Working)
        );
        assert!(st.review_pending, "parent-scoped output must not drop the owed Review");

        // The last subagent goes terminal → the owed Review finally fires.
        assert_eq!(
            map_event_to_pane_status(&subagent_event("s1", SubagentStatus::Completed), &mut st),
            Some(PaneStatus::Review)
        );
        assert!(st.is_clear());
    }

    // After a new-turn reset drained the stale tracking, a late terminal
    // snapshot for the forgotten subagent is a harmless no-op — it doesn't
    // underflow `running`, resurrect a Review, or move the dot.
    #[test]
    fn stale_terminal_after_new_turn_reset_is_noop() {
        let mut st = ThreadSubagentState::default();
        map_event_to_pane_status(&subagent_event("bg", SubagentStatus::Running), &mut st);
        map_event_to_pane_status(&turn_completed(), &mut st);
        // New turn resets tracking.
        map_event_to_pane_status(&session_running(), &mut st);
        assert!(st.is_clear());

        // The old subagent's terminal snapshot finally arrives — no-op.
        assert_eq!(
            map_event_to_pane_status(&subagent_event("bg", SubagentStatus::Completed), &mut st),
            None,
            "a terminal for an already-forgotten subagent moves nothing"
        );
        assert!(st.is_clear());
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
