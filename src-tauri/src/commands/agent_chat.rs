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
use tauri::ipc::{Channel, InvokeBody, Request};
use tauri::{AppHandle, Emitter, Manager, Runtime, State};

use crate::agent_provider::{
    AgentProvider, ApprovalDecision, ProviderChatCapabilities, ProviderError, ProviderKind,
    ProviderRuntimeEvent, RequestId, RequestResponseFailureReason, SendTurnInput,
    SerializableProviderError, StartSessionInput, ThreadId, TurnId,
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
    /// `agent_chat_messages.id` of the row this event was persisted as,
    /// when it was persisted at all (ephemeral kinds — `content_delta`,
    /// lifecycle notices — carry `None`).
    ///
    /// This is the dedup key for cursor resume: a live event whose id is
    /// at or below the thread's `lastPersistedEventId` is history the
    /// frontend already applied through a tail read, so it drops it. The
    /// `QueuedTurnDispatched` arm carries the id of the user-message row
    /// it just wrote, because that row is the durable record of the
    /// bubble this event materializes.
    #[serde(default)]
    pub persisted_id: Option<i64>,
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
pub fn agent_chat_create_pane<R: Runtime>(
    app: AppHandle<R>,
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
pub fn agent_chat_close_pane<R: Runtime>(
    app: AppHandle<R>,
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
pub fn dev_agent_chat_spawn_test_pane<R: Runtime>(
    app: AppHandle<R>,
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
pub async fn agent_chat_start_session<R: Runtime>(
    app: AppHandle<R>,
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
    // Heal a `None` permission_mode to the provider default BEFORE anything
    // else reads `input`: the provider launch below, the `config_for_persist`
    // snapshot, and the record the frontend re-seeds its picker from all draw
    // from `input.permission_mode`, so mutating it here is the single point
    // that keeps them in lockstep. A frontend caller can pass `None` while its
    // UI already shows "Full access" (the picker seeds to the provider default
    // for a NULL row — see `fallback_permission_mode`), which would otherwise
    // launch the CLI in `default` mode and prompt for every tool. This mirrors
    // the resume-path heal in `ensure_live_session` (search
    // `healed_permission_mode`) so BOTH session-minting entry points are immune
    // to a null-passing caller. OpenCode has no permission modes, so its
    // fallback is `None` and this is a no-op there.
    let requested_permission_mode = input.permission_mode.take();
    let resolved_permission_mode =
        resolve_start_permission_mode(provider, requested_permission_mode.clone());
    if resolved_permission_mode != requested_permission_mode {
        eprintln!(
            "[codemux::agent_chat] healing permission_mode at start pane={pane_id} \
             provider={provider:?} requested={requested_permission_mode:?} \
             resolved={resolved_permission_mode:?}"
        );
    }
    input.permission_mode = resolved_permission_mode;
    // Snapshot the per-thread chat configuration BEFORE the provider
    // consumes `input`, so it can be persisted onto the session row for
    // restart-resume (re-seeding the pickers) and read back by
    // `ensure_live_session` when it silently restarts a dead session.
    // Session start carries the complete launch selection. Persist its
    // nullable fields exactly so an in-place provider handoff does not
    // retain configuration that only made sense to the previous adapter.
    let config_for_persist = AgentChatSessionConfig {
        // A session start carries the complete launch configuration, so
        // NULL means "use this provider/model's default" and must clear a
        // value left by the previous provider. `keep_or_set` was unsafe for
        // an in-place Claude -> Codex/OpenCode handoff because it preserved
        // Claude-only effort/context/permission columns on the new session.
        model: Some(input.model.clone()),
        effort: Some(input.effort.clone()),
        context_window: Some(input.context_window.clone()),
        permission_mode: Some(input.permission_mode.clone()),
        fast_mode: Some(input.fast_mode),
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
    // Resume preflight (Claude only): the frontend may hand us a resume
    // cursor built from a persisted `sdk_session_id` (history-dropdown
    // pick / restart). If the CLI's on-disk session JSONL is confirmed
    // gone, drop the cursor so the session starts fresh instead of wedging
    // on a dead id, and best-effort clear the persisted column. Codex /
    // OpenCode carry their own cursor shapes and are untouched.
    if provider == ProviderKind::Claude {
        if let Some(stale) = input
            .resume_cursor
            .as_ref()
            .and_then(extract_sdk_session_id)
            .filter(|id| claude_session_file_missing(id))
        {
            eprintln!(
                "[codemux::agent_chat] dropping stale resume cursor at start pane={pane_id} \
                 (Claude session file for {stale} is gone); starting fresh"
            );
            input.resume_cursor = None;
            let db: State<'_, DatabaseStore> = app.state();
            if let Err(error) = db.clear_agent_chat_sdk_session_id(&input.thread_id.0) {
                eprintln!(
                    "[codemux::agent_chat] failed to clear stale sdk_session_id at start: {error}"
                );
            }
        }
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
    // Session startup is authoritative for BOTH halves of the pane binding.
    // This matters when the model picker hands an existing pane from Claude
    // to Codex/OpenCode: persisting only `thread_id` would leave the pane's
    // provider stale and route the next app-restart resume through Claude.
    state.set_agent_chat_binding(&pane_id, provider, session.thread_id.0.clone());
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
        // A CodeMux thread can keep its transcript while changing provider,
        // but provider-native resume cursors are not portable. The upsert
        // atomically clears the old cursor when `provider` changes, then the
        // start-time persist below records any cursor returned by the new
        // adapter. Keeping this inside one SQL statement avoids a second
        // clear racing a new provider's cursor event.
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
fn spawn_run_checkpoint<R: Runtime>(
    app: &AppHandle<R>,
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
pub async fn agent_chat_restore_checkpoint<R: Runtime>(
    app: AppHandle<R>,
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

/// The provider default permission mode used to heal a session row whose
/// persisted `permission_mode` is NULL.
///
/// WHY: the `permission_mode` column was added to `agent_chat_sessions`
/// after chat had already shipped (see the
/// `ALTER TABLE agent_chat_sessions ADD COLUMN permission_mode` migration
/// in `database.rs`), so rows created before it read back `None`. The
/// frontend seeds its permission picker to the provider default ("Full
/// access" for Claude) for such rows, so if [`ensure_live_session`]
/// rebuilds the live session passing `None` the SDK launches in `default`
/// mode and prompts for every tool — the UI says Full access while the
/// live session desyncs and asks for approvals on every Edit/Bash.
/// Substituting the provider default here keeps the rebuilt session in the
/// same mode the UI already displays.
///
/// The returned strings MUST match the `default_permission_mode` each
/// provider declares in its capabilities module
/// (`agent_provider/{claude,codex,opencode}/capabilities.rs`); they are
/// duplicated here as a cheap `&'static str` rather than building the full
/// capabilities bundle (which allocates the model list) just to read one
/// field. OpenCode has no permission modes, so it stays `None`.
fn fallback_permission_mode(provider: ProviderKind) -> Option<&'static str> {
    match provider {
        ProviderKind::Claude => Some("bypassPermissions"),
        ProviderKind::Codex => Some("danger-full-access"),
        ProviderKind::OpenCode => None,
    }
}

/// Resolve the permission mode a *starting* session should launch with,
/// healing a `None` request to the provider default.
///
/// WHY: `agent_chat_start_session` is the PRIMARY path that mints a live
/// session, yet a frontend caller can hand us `permission_mode: None`
/// even though its UI is showing "Full access" — because the picker seeds
/// itself to the provider default for a NULL persisted row (see
/// `fallback_permission_mode`'s doc comment). If we forwarded that `None`
/// straight to the provider the CLI would boot in `default` mode and
/// prompt for every tool while the UI insists on Full access — the exact
/// desync [`ensure_live_session`] already heals on the resume path
/// (search `healed_permission_mode`). This mirrors that heal at the start
/// path so BOTH entry points are immune to a null-passing caller: the
/// provider launch, the persisted `agent_chat_sessions` row, and the
/// frontend's record-seeding all agree on one mode.
///
/// A `Some` request is authoritative and passes through untouched (the
/// user explicitly picked a mode). OpenCode's fallback is `None`, so a
/// `None` request stays `None` — it has no permission modes and there is
/// nothing to heal.
fn resolve_start_permission_mode(
    provider: ProviderKind,
    requested: Option<String>,
) -> Option<String> {
    match requested {
        // v0.14.2 could persist the other provider's Full-access protocol
        // value after a frontend provider switch. Both strings represent the
        // same explicit UI choice, so canonicalize that known cross-provider
        // mismatch instead of forwarding an unsupported value that Codex
        // silently omits from thread/start.
        Some(mode) if provider == ProviderKind::Codex && mode == "bypassPermissions" => {
            Some("danger-full-access".to_string())
        }
        Some(mode) if provider == ProviderKind::Claude && mode == "danger-full-access" => {
            Some("bypassPermissions".to_string())
        }
        Some(mode) => Some(mode),
        None => fallback_permission_mode(provider).map(str::to_string),
    }
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

    // Build the resume cursor from the persisted id, but for Claude run a
    // preflight: if the CLI's on-disk session JSONL is confirmed gone,
    // skip the cursor and start fresh rather than wedging the rebuild on a
    // dead id. Best-effort clear the column so no later rebuild reuses it.
    // Codex / OpenCode carry their own cursor shapes and are untouched.
    let resume_cursor = match record.sdk_session_id.as_ref() {
        Some(id)
            if provider_kind == ProviderKind::Claude && claude_session_file_missing(id) =>
        {
            eprintln!(
                "[codemux::agent_chat] dropping stale resume cursor for thread={} \
                 (Claude session file for {id} is gone); starting fresh",
                thread_id.0,
            );
            let db: State<'_, DatabaseStore> = app.state();
            if let Err(error) = db.clear_agent_chat_sdk_session_id(&thread_id.0) {
                eprintln!(
                    "[codemux::agent_chat] failed to clear stale sdk_session_id for thread={}: {error}",
                    thread_id.0,
                );
            }
            None
        }
        Some(id) => Some(serde_json::json!({ "resume": id })),
        None => None,
    };

    // Resolve both legacy NULL rows and the v0.14.2 cross-provider
    // Full-access values through the same canonical start-path helper. The
    // best-effort persist below runs only when the stored value changed.
    let stored_permission_mode = record.permission_mode.clone();
    let permission_mode =
        resolve_start_permission_mode(provider_kind, stored_permission_mode.clone());
    let healed_permission_mode = (permission_mode != stored_permission_mode)
        .then(|| permission_mode.clone())
        .flatten();

    if let Some(mode) = healed_permission_mode {
        // Best-effort heal the row so future rebuilds and the frontend's
        // picker seed agree on the mode. Never fail the resume over a DB
        // write — the in-memory session is already resolving to the right
        // mode via `permission_mode` above.
        let db: State<'_, DatabaseStore> = app.state();
        let config = AgentChatSessionConfig {
            permission_mode: AgentChatSessionConfig::set(mode),
            ..AgentChatSessionConfig::default()
        };
        if let Err(error) = db.update_agent_chat_session_config(&thread_id.0, &config) {
            eprintln!(
                "[codemux::agent_chat] failed to heal permission_mode for thread={}: {error}",
                thread_id.0,
            );
        }
    }

    let build_input = |resume_cursor: Option<serde_json::Value>| StartSessionInput {
        thread_id: thread_id.clone(),
        cwd: cwd.clone(),
        model: record.model.clone(),
        resume_cursor,
        permission_mode: permission_mode.clone(),
        effort: record.effort.clone(),
        context_window: record.context_window.clone(),
        fast_mode: record.fast_mode,
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

/// Command-layer input for [`agent_chat_send_turn`].
///
/// Mirrors the provider's [`SendTurnInput`] field-for-field EXCEPT `images`:
/// the frontend now sends staged-file references
/// (`[{ path, media_type }]`) instead of raw bytes (`{ data: number[],
/// media_type }`). The command finalizes those refs, reads the real bytes,
/// and builds the provider's byte-carrying `SendTurnInput` internally — the
/// provider trait's contract is unchanged (adapters still receive bytes).
#[derive(Debug, Clone, Deserialize)]
pub struct SendTurnCommandInput {
    /// Thread the turn belongs to.
    pub thread_id: ThreadId,
    /// Plain-text content of the user message.
    pub text: String,
    /// Staged image references, in order.
    #[serde(default)]
    pub images: Vec<ChatImageRef>,
    /// Optional per-turn model override.
    pub model_override: Option<String>,
    /// Optional per-turn effort override.
    #[serde(default)]
    pub effort_override: Option<String>,
    /// Optional per-turn permission-mode override.
    #[serde(default)]
    pub permission_mode_override: Option<String>,
    /// Optional client-generated correlation token for the follow-up queue.
    #[serde(default)]
    pub client_nonce: Option<String>,
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
pub async fn agent_chat_send_turn<R: Runtime>(
    app: AppHandle<R>,
    provider: ProviderKind,
    input: SendTurnCommandInput,
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
    // Capture the inputs we need for persistence before dispatching.
    let thread_id_for_persist = input.thread_id.0.clone();
    let user_text_for_persist = input.text.clone();
    let first_line = first_line_title(&input.text);
    // Finalize the staged image refs NOW: promote each staged file into the
    // thread's dir and read its bytes back for the provider. This replaces
    // the old bytes-writing `save_chat_images` — the transcript records use
    // the FINAL paths and the provider gets real bytes. A security
    // rejection (a ref outside the permitted dirs) fails the send; a per-
    // image fs failure is logged and skipped inside the helper.
    let (saved_images, image_inputs) =
        finalize_chat_images(&thread_id_for_persist, &input.images).await?;
    // Build the provider's byte-carrying input from the command DTO. The
    // provider trait is unchanged — adapters still receive `ImageInput`s.
    let provider_input = SendTurnInput {
        thread_id: input.thread_id.clone(),
        text: input.text.clone(),
        images: image_inputs,
        model_override: input.model_override.clone(),
        effort_override: input.effort_override.clone(),
        permission_mode_override: input.permission_mode_override.clone(),
        client_nonce: input.client_nonce.clone(),
    };
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
    let result = impl_.send_turn(provider_input).await.map_err(provider_err)?;

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
        None => {
            persist_user_message(
                &db,
                &thread_id_for_persist,
                &user_text_for_persist,
                &saved_images,
                input.client_nonce.as_deref(),
            );
        }
        // Queued: stash the (already-on-disk) image records and the
        // client nonce keyed by the provider's queued_id so the deferred
        // envelope write in `forward_event`'s `QueuedTurnDispatched` arm
        // can attach them at real turn order. Skip the map entirely when
        // there is nothing to carry (the dispatch persist writes text
        // only).
        Some(queued_id) if !saved_images.is_empty() || input.client_nonce.is_some() => {
            pending_queued_images().lock().unwrap().insert(
                queued_id.clone(),
                PendingQueuedTurn {
                    images: saved_images,
                    client_nonce: input.client_nonce.clone(),
                },
            );
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
pub async fn agent_chat_cancel_queued_turn<R: Runtime>(
    app: AppHandle<R>,
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
pub async fn agent_chat_send_queued_turn_now<R: Runtime>(
    app: AppHandle<R>,
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

/// Resolve the root that holds every chat-image directory:
/// `<config>/<APP_DIR_NAME>/agent-chat/images/`.
///
/// Mirrors how [`crate::database`] resolves its store path (config dir +
/// app name) so images live beside the transcript DB — a durable location
/// a resumed conversation can rely on — rather than in a temp dir the OS
/// may reap out from under it (unlike the ephemeral clipboard-paste files
/// in `commands::files`). Both the per-thread dirs and the upload
/// [`chat_images_staging_dir`] hang off this single root so the containment
/// checks in the stage / discard / read / finalize paths all reason about
/// one canonical prefix.
fn chat_images_root() -> Option<std::path::PathBuf> {
    Some(
        dirs::config_dir()?
            .join(crate::APP_DIR_NAME)
            .join("agent-chat")
            .join("images"),
    )
}

/// Resolve a thread's chat-image directory:
/// `<config>/<APP_DIR_NAME>/agent-chat/images/<thread_id>/`. Finalized
/// attachments for a turn land here.
fn chat_images_dir(thread_id: &str) -> Option<std::path::PathBuf> {
    Some(chat_images_root()?.join(thread_id))
}

/// Resolve the staging directory freshly-uploaded (not-yet-sent) images are
/// written to: `<config>/<APP_DIR_NAME>/agent-chat/images/staging/`.
///
/// A raw-body upload ([`agent_chat_stage_image`]) writes here first; the
/// image is only promoted into a thread's dir when its turn actually sends
/// ([`finalize_chat_images`]). No real thread id is the literal `"staging"`
/// (provider-minted ids are uuids / opaque tokens), so this never collides
/// with a per-thread dir under the same root.
fn chat_images_staging_dir() -> Option<std::path::PathBuf> {
    Some(chat_images_root()?.join("staging"))
}

// ── Chat-image staging / finalize (raw-body upload path) ─────────────
//
// Images no longer travel through the `agent_chat_send_turn` JSON payload
// as a `number[]` per byte (a multi-MB screenshot ballooned to tens of MB
// of JSON through WebKit IPC and stalled the send for minutes). Instead the
// composer uploads each image once via [`agent_chat_stage_image`] (raw
// request body — no per-byte JSON), which writes it to the shared staging
// dir and hands back an absolute path; the send then references staged
// files by path and [`finalize_chat_images`] promotes them into the
// thread's dir. All fs work is async (`tokio::fs`) — nothing blocks the
// runtime.

/// The MIME types Agent Chat accepts for an attachment. Everything else is
/// rejected at the staging boundary so a garbage payload never reaches the
/// provider or the disk.
const ACCEPTED_CHAT_IMAGE_MIME: [&str; 4] =
    ["image/png", "image/jpeg", "image/gif", "image/webp"];

/// A freshly-staged image the composer can reference on its next send.
/// Serde field names match [`PersistedChatImage`]'s (`path` / `media_type`)
/// so the frontend threads one shape through stage → send.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StagedChatImage {
    /// Absolute path to the staged bytes under the staging dir.
    pub path: String,
    /// The validated media type (echoed back verbatim).
    pub media_type: String,
}

/// A staged-file reference the frontend sends in place of raw bytes on a
/// user turn. The command layer finalizes these into the thread's image dir
/// and reads the bytes back for the provider (see [`finalize_chat_images`]).
#[derive(Debug, Clone, Deserialize)]
pub struct ChatImageRef {
    /// Absolute path returned by [`agent_chat_stage_image`] (staging) or an
    /// already-finalized path inside this thread's dir.
    pub path: String,
    /// Original media type (e.g. `"image/png"`).
    pub media_type: String,
}

/// The bytes + inferred media type returned by [`agent_chat_read_image`].
#[derive(Debug, Clone, Serialize)]
pub struct ChatImageBytes {
    pub bytes: Vec<u8>,
    pub media_type: String,
}

/// Validate a client-supplied MIME string against the accepted set.
/// Case-insensitive (clients are inconsistent about MIME casing) and
/// returns the canonical lowercase form on success.
fn validate_chat_image_mime(media_type: &str) -> Result<String, String> {
    let lower = media_type.to_ascii_lowercase();
    // Fold the jpg alias so a client sending `image/jpg` still validates.
    let canonical = if lower == "image/jpg" {
        "image/jpeg".to_string()
    } else {
        lower
    };
    if ACCEPTED_CHAT_IMAGE_MIME.contains(&canonical.as_str()) {
        Ok(canonical)
    } else {
        Err(format!(
            "validation_error: unsupported image media type: {media_type}"
        ))
    }
}

/// Reverse of [`chat_image_extension`]: infer a media type from a stored
/// file's extension. Used by [`agent_chat_read_image`] so the fallback
/// loader can label the bytes it returns. Unknown extensions fall back to
/// the generic binary type rather than guessing.
fn media_type_from_extension(path: &std::path::Path) -> String {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("png") => "image/png",
        Some("jpg") | Some("jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        _ => "application/octet-stream",
    }
    .to_string()
}

/// Verify `candidate` resolves to a path whose parent is inside `root`
/// (both canonicalized) and return the canonicalized absolute path.
///
/// This is the security boundary for the discard / read commands: it
/// defeats `../` traversal and symlinked parents by canonicalizing the
/// candidate's PARENT directory (which must exist) rather than the file
/// itself (which may already be gone, or is about to be read). The file
/// name is re-joined onto the canonical parent. Returns an error when the
/// candidate has no parent/file component, when the parent cannot be
/// resolved, or when it escapes `root`.
fn resolve_within(
    root: &std::path::Path,
    candidate: &str,
) -> Result<std::path::PathBuf, String> {
    let candidate = std::path::Path::new(candidate);
    let file_name = candidate
        .file_name()
        .ok_or_else(|| "validation_error: path has no file component".to_string())?;
    let parent = candidate
        .parent()
        .ok_or_else(|| "validation_error: path has no parent component".to_string())?;
    let canon_root = root
        .canonicalize()
        .map_err(|error| format!("validation_error: cannot resolve root dir: {error}"))?;
    let canon_parent = parent
        .canonicalize()
        .map_err(|error| format!("validation_error: cannot resolve path parent: {error}"))?;
    if !canon_parent.starts_with(&canon_root) {
        return Err(
            "security_error: path escapes the permitted chat-image directory".to_string(),
        );
    }
    Ok(canon_parent.join(file_name))
}

/// Where a [`ChatImageRef`] lives relative to the chat-image root.
enum ChatImageRefLocation {
    /// Under the staging dir — must be promoted into the thread's dir.
    Staged(std::path::PathBuf),
    /// Already inside this thread's dir — passes through unchanged.
    ThreadLocal(std::path::PathBuf),
}

/// Classify a ref's on-disk location for [`finalize_chat_images`].
///
/// Security boundary: a ref pointing anywhere other than this thread's dir
/// or the shared staging dir (a different thread's dir, or entirely outside
/// the chat-image root) is rejected with an error so a compromised /
/// buggy frontend cannot smuggle an arbitrary file into a turn. The
/// candidate's canonicalized parent is compared against the canonicalized
/// staging / thread dirs; dirs that don't exist yet simply can't match.
fn classify_chat_image_ref(
    staging: &std::path::Path,
    thread_dir: &std::path::Path,
    candidate: &str,
) -> Result<ChatImageRefLocation, String> {
    let candidate_path = std::path::Path::new(candidate);
    let file_name = candidate_path
        .file_name()
        .ok_or_else(|| "validation_error: image path has no file component".to_string())?;
    let parent = candidate_path
        .parent()
        .ok_or_else(|| "validation_error: image path has no parent component".to_string())?;
    let canon_parent = parent
        .canonicalize()
        .map_err(|error| format!("validation_error: cannot resolve image parent: {error}"))?;

    if staging
        .canonicalize()
        .map(|s| canon_parent == s)
        .unwrap_or(false)
    {
        return Ok(ChatImageRefLocation::Staged(canon_parent.join(file_name)));
    }
    if thread_dir
        .canonicalize()
        .map(|t| canon_parent == t)
        .unwrap_or(false)
    {
        return Ok(ChatImageRefLocation::ThreadLocal(
            canon_parent.join(file_name),
        ));
    }
    Err("security_error: image path is outside the permitted chat-image directories".to_string())
}

/// Move a staged file into its final home, falling back to copy+remove when
/// a plain rename fails across devices (staging and the thread dir share a
/// root today, but a symlinked config dir could straddle a mount).
async fn move_staged_file(
    src: &std::path::Path,
    dst: &std::path::Path,
) -> Result<(), String> {
    if tokio::fs::rename(src, dst).await.is_ok() {
        return Ok(());
    }
    tokio::fs::copy(src, dst)
        .await
        .map_err(|error| format!("failed to finalize staged image: {error}"))?;
    // Best-effort cleanup of the staging copy; a leftover file is harmless.
    let _ = tokio::fs::remove_file(src).await;
    Ok(())
}

/// Finalize a turn's staged image refs: promote staged files into the
/// thread's dir, verify already-thread-local refs, and read every finalized
/// file's bytes for the provider.
///
/// Returns `(records, image_inputs)`: `records` are the [`PersistedChatImage`]
/// entries (FINAL absolute paths) to stamp on the transcript envelope, and
/// `image_inputs` are the real-byte [`ImageInput`](crate::agent_provider::ImageInput)s
/// the provider needs. A ref that fails the location classification is a
/// SECURITY rejection and fails the whole call; a ref that classifies but
/// then fails an fs op (missing file, unreadable) is logged and SKIPPED —
/// mirroring the old best-effort contract so one bad attachment never blocks
/// the turn. Empty input is a cheap no-op.
async fn finalize_chat_images(
    thread_id: &str,
    refs: &[ChatImageRef],
) -> Result<(Vec<PersistedChatImage>, Vec<crate::agent_provider::ImageInput>), String> {
    if refs.is_empty() {
        return Ok((Vec::new(), Vec::new()));
    }
    let staging = chat_images_staging_dir()
        .ok_or_else(|| "config_dir_unavailable".to_string())?;
    let thread_dir =
        chat_images_dir(thread_id).ok_or_else(|| "config_dir_unavailable".to_string())?;
    // Create the thread dir up front so a rename into it always has a target.
    tokio::fs::create_dir_all(&thread_dir)
        .await
        .map_err(|error| format!("failed to create chat image dir: {error}"))?;

    let mut records = Vec::with_capacity(refs.len());
    let mut inputs = Vec::with_capacity(refs.len());
    for image in refs {
        // Classification is the security gate — propagate its error.
        let final_path = match classify_chat_image_ref(&staging, &thread_dir, &image.path)? {
            ChatImageRefLocation::Staged(src) => {
                let file_name = src
                    .file_name()
                    .map(|n| n.to_os_string())
                    .unwrap_or_else(|| {
                        let ext = chat_image_extension(&image.media_type);
                        std::ffi::OsString::from(format!("{}.{ext}", uuid::Uuid::new_v4()))
                    });
                let dst = thread_dir.join(&file_name);
                if let Err(error) = move_staged_file(&src, &dst).await {
                    eprintln!(
                        "[codemux::agent_chat] failed to finalize staged image {}: {error}",
                        src.display()
                    );
                    continue;
                }
                dst
            }
            ChatImageRefLocation::ThreadLocal(path) => path,
        };
        // Read the finalized bytes for the provider. A missing/unreadable
        // file skips this attachment (best-effort) rather than failing the
        // turn.
        match tokio::fs::read(&final_path).await {
            Ok(bytes) => {
                inputs.push(crate::agent_provider::ImageInput {
                    data: bytes,
                    media_type: image.media_type.clone(),
                });
                records.push(PersistedChatImage {
                    path: final_path.to_string_lossy().into_owned(),
                    media_type: image.media_type.clone(),
                });
            }
            Err(error) => eprintln!(
                "[codemux::agent_chat] failed to read finalized image {}: {error}",
                final_path.display()
            ),
        }
    }
    Ok((records, inputs))
}

/// JSON fallback body for [`agent_chat_stage_image`] on transports that
/// cannot carry a raw invoke payload. The web-remote bridge serializes
/// every invoke as JSON (`dispatch_invoke` synthesizes `InvokeBody::Json`)
/// and drops invoke options, so a remote browser client ships the bytes
/// base64-encoded in the body instead — ~1.33x the raw size, still far
/// cheaper than the per-byte `number[]` JSON this feature replaced.
#[derive(Debug, Deserialize)]
struct StageChatImageJsonBody {
    bytes_base64: String,
    media_type: String,
}

/// Stage a single chat image via a RAW request body (no per-byte JSON).
///
/// The composer invokes this as
/// `invoke("agent_chat_stage_image", bytesUint8Array, { headers: { "x-media-type": mime } })`,
/// so the payload arrives as [`InvokeBody::Raw`] and the MIME rides the
/// `x-media-type` header — the same mechanism the official fs plugin's
/// `write_file` uses. Transports that can't carry a raw body (web-remote)
/// send [`StageChatImageJsonBody`] instead. The bytes are validated
/// (accepted MIME + size cap) and written to the staging dir with
/// `tokio::fs`; the returned absolute path is what the next
/// `agent_chat_send_turn` references.
#[tauri::command]
pub async fn agent_chat_stage_image<R: Runtime>(
    app: AppHandle<R>,
    request: Request<'_>,
) -> Result<StagedChatImage, String> {
    let observability: State<'_, ObservabilityStore> = app.state();
    feature_flag_on(&observability)?;

    // Copy the bytes out of the request so no borrow of `request` is held
    // across the awaits below. The MIME comes from the `x-media-type`
    // header on the raw path, or from the JSON body on the fallback path
    // (the web-remote bridge drops invoke options, headers included).
    let (bytes, media_type) = match request.body() {
        InvokeBody::Raw(bytes) => {
            let media_type = request
                .headers()
                .get("x-media-type")
                .and_then(|value| value.to_str().ok())
                .map(|s| s.to_string())
                .ok_or_else(|| {
                    "validation_error: missing x-media-type header".to_string()
                })?;
            (bytes.clone(), media_type)
        }
        InvokeBody::Json(value) => {
            let body: StageChatImageJsonBody = serde_json::from_value(value.clone())
                .map_err(|_| {
                    "validation_error: expected a raw image body or \
                     { bytes_base64, media_type }"
                        .to_string()
                })?;
            use base64::Engine as _;
            let decoded = base64::engine::general_purpose::STANDARD
                .decode(body.bytes_base64.as_bytes())
                .map_err(|error| {
                    format!("validation_error: invalid base64 image body: {error}")
                })?;
            (decoded, body.media_type)
        }
    };
    let media_type = validate_chat_image_mime(&media_type)?;
    if bytes.is_empty() {
        return Err("validation_error: empty image body".to_string());
    }
    if bytes.len() > crate::commands::files::MAX_CLIPBOARD_IMAGE_BYTES {
        return Err(format!(
            "validation_error: image too large ({:.1} MB, limit is {} MB)",
            bytes.len() as f64 / (1024.0 * 1024.0),
            crate::commands::files::MAX_CLIPBOARD_IMAGE_BYTES / (1024 * 1024),
        ));
    }

    let staging =
        chat_images_staging_dir().ok_or_else(|| "config_dir_unavailable".to_string())?;
    tokio::fs::create_dir_all(&staging)
        .await
        .map_err(|error| format!("failed to create staging dir: {error}"))?;
    let ext = chat_image_extension(&media_type);
    let path = staging.join(format!("{}.{ext}", uuid::Uuid::new_v4()));
    tokio::fs::write(&path, &bytes)
        .await
        .map_err(|error| format!("failed to write staged image: {error}"))?;

    Ok(StagedChatImage {
        path: path.to_string_lossy().into_owned(),
        media_type,
    })
}

/// Reap leaked chat-image staging files at startup.
///
/// A staged upload is normally either promoted into its thread's dir on
/// send ([`finalize_chat_images`]) or deleted when the user removes the
/// chip ([`agent_chat_discard_staged_image`]) — but a crash or an
/// abandoned draft leaks the file. Staging paths never survive a restart
/// of the composer that minted them (attachment chips hold their bytes in
/// memory only), so anything past a generous grace window is garbage.
/// The 24h grace exists for belt-and-braces coexistence with any other
/// process sharing the config dir (e.g. a headless `codemux-remote`
/// daemon on the same host) rather than for the desktop app itself.
/// Best-effort: every failure is ignored — a leftover file is harmless.
pub async fn sweep_stale_staged_images() {
    const STAGING_GRACE: Duration = Duration::from_secs(24 * 60 * 60);
    let Some(staging) = chat_images_staging_dir() else {
        return;
    };
    let Ok(mut entries) = tokio::fs::read_dir(&staging).await else {
        return;
    };
    let now = SystemTime::now();
    while let Ok(Some(entry)) = entries.next_entry().await {
        let Ok(metadata) = entry.metadata().await else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        let age = metadata
            .modified()
            .ok()
            .and_then(|modified| now.duration_since(modified).ok());
        if age.is_some_and(|age| age > STAGING_GRACE) {
            let _ = tokio::fs::remove_file(entry.path()).await;
        }
    }
}

/// Discard a staged image the user removed before sending. Idempotent: a
/// path already gone is a best-effort success. Rejects any path outside the
/// staging dir (security) so this can never be turned into an arbitrary
/// file delete.
#[tauri::command]
pub async fn agent_chat_discard_staged_image<R: Runtime>(
    app: AppHandle<R>,
    path: String,
) -> Result<(), String> {
    let observability: State<'_, ObservabilityStore> = app.state();
    feature_flag_on(&observability)?;
    let staging =
        chat_images_staging_dir().ok_or_else(|| "config_dir_unavailable".to_string())?;
    // Ensure the staging root exists so its canonicalization (inside
    // `resolve_within`) succeeds even before the first upload of a session.
    tokio::fs::create_dir_all(&staging)
        .await
        .map_err(|error| format!("failed to resolve staging dir: {error}"))?;
    let resolved = resolve_within(&staging, &path)?;
    match tokio::fs::remove_file(&resolved).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("failed to discard staged image: {error}")),
    }
}

/// Read a finalized (or staged) chat image's bytes back for the frontend.
///
/// An on-error fallback loader for contexts where the asset protocol can't
/// serve a local path (the dev browser mock, a web-remote client). The path
/// must resolve inside the chat-image root (any thread dir or staging);
/// anything else is rejected. `tokio::fs::read`, media type inferred from
/// the extension.
#[tauri::command]
pub async fn agent_chat_read_image<R: Runtime>(
    app: AppHandle<R>,
    path: String,
) -> Result<ChatImageBytes, String> {
    let observability: State<'_, ObservabilityStore> = app.state();
    feature_flag_on(&observability)?;
    let root = chat_images_root().ok_or_else(|| "config_dir_unavailable".to_string())?;
    let resolved = resolve_within(&root, &path)?;
    let media_type = media_type_from_extension(&resolved);
    let bytes = tokio::fs::read(&resolved)
        .await
        .map_err(|error| format!("failed to read chat image: {error}"))?;
    Ok(ChatImageBytes { bytes, media_type })
}

/// Warm MCP servers eagerly, before the first turn.
///
/// The frontend fires this as soon as a draft / new-thread surface mounts so
/// the `npx`-launched MCP handshakes (which can take seconds on first run)
/// overlap the user typing their first prompt instead of blocking the turn.
/// Returns immediately: the actual `prime_for_chat` work runs on a
/// background task. The prime the correctness backstop in
/// [`agent_chat_start_session`] still performs becomes a cheap no-op because
/// the registry's `prime_lock` serializes the two so they can't double-spawn
/// (see [`crate::mcp::registry::McpRegistry`]).
#[tauri::command]
pub async fn agent_chat_prime_mcp<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    let observability: State<'_, ObservabilityStore> = app.state();
    feature_flag_on(&observability)?;
    // Prime against the active workspace's root when there is one so
    // project-scoped `.mcp.json` servers warm too; a draft with no workspace
    // primes the global/user set (project_root = None).
    let project_root = {
        let state: State<'_, AppStateStore> = app.state();
        let snapshot = state.snapshot();
        snapshot
            .workspaces
            .iter()
            .find(|w| w.workspace_id == snapshot.active_workspace_id)
            .map(|w| std::path::PathBuf::from(&w.cwd))
            .filter(|p| !p.as_os_str().is_empty())
    };
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        use crate::mcp::registry::McpRegistry;
        let mcp_registry: State<'_, McpRegistry> = app.state();
        let registry = mcp_registry.inner().clone_handle();
        registry
            .prime_for_chat(Some(&app), project_root.as_deref())
            .await;
    });
    Ok(())
}

/// What a QUEUED send stashes for its deferred envelope write.
#[derive(Debug, Default)]
struct PendingQueuedTurn {
    /// On-disk image records minted at enqueue time.
    images: Vec<PersistedChatImage>,
    /// The composer's correlation token for the optimistic bubble, so
    /// the deferred envelope is dedupable against it on a cursor tail
    /// read (see [`persist_user_message`]).
    client_nonce: Option<String>,
}

/// Process-wide bridge carrying a QUEUED send's on-disk image records
/// and client nonce through to the deferred user-message envelope write.
///
/// The provider's follow-up queue carries a queued turn's *text* to
/// dispatch, but it has no notion of the transcript paths the command
/// layer minted — those live only here, keyed by the same `queued_id` the
/// provider echoes on [`ProviderRuntimeEvent::QueuedTurnDispatched`]. An
/// immediate (non-queued) send never touches this map: its envelope is
/// written inline in [`agent_chat_send_turn`]. Follows the same
/// module-static idiom as [`resume_locks`] to avoid threading a new
/// managed state through every `forward_event` test harness.
fn pending_queued_images() -> &'static Mutex<HashMap<String, PendingQueuedTurn>> {
    static PENDING: OnceLock<Mutex<HashMap<String, PendingQueuedTurn>>> =
        OnceLock::new();
    PENDING.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Append the synthetic `{"type":"user_message"}` transcript envelope.
/// Best-effort — a failed write only means resume skips this user turn.
/// Returns the row id so the caller can hand it to the frontend as a
/// cursor position.
///
/// When `images` is non-empty the envelope gains an `"images"` array of
/// `{"path","media_type"}` records; it is OMITTED entirely otherwise so
/// older transcripts and image-less turns stay byte-identical (backward
/// compatible — the frontend hydrate treats a missing field as "no
/// attachments").
///
/// `client_nonce` records the token the composer stamped on its
/// optimistic bubble. User turns never come back through the provider
/// stream, so this row is the only place a cursor tail read can learn
/// about them — and without the nonce a tail read that straddles a send
/// would render the same bubble twice (the optimistic one plus the
/// replayed row). Omitted when absent, keeping older rows byte-identical.
fn persist_user_message(
    db: &DatabaseStore,
    thread_id: &str,
    text: &str,
    images: &[PersistedChatImage],
    client_nonce: Option<&str>,
) -> Option<i64> {
    let mut user_msg = serde_json::json!({
        "type": "user_message",
        "thread_id": thread_id,
        "text": text,
    });
    if let Some(nonce) = client_nonce.filter(|n| !n.is_empty()) {
        user_msg["client_nonce"] = serde_json::Value::String(nonce.to_string());
    }
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
    let payload = serde_json::to_string(&user_msg).ok()?;
    db.append_agent_chat_message(thread_id, &payload).ok().flatten()
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
pub async fn agent_chat_interrupt_turn<R: Runtime>(
    app: AppHandle<R>,
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
pub async fn agent_chat_turn_active<R: Runtime>(
    app: AppHandle<R>,
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
pub async fn agent_chat_respond_to_request<R: Runtime>(
    app: AppHandle<R>,
    provider: ProviderKind,
    thread_id: ThreadId,
    request_id: RequestId,
    decision: ApprovalDecision,
) -> Result<(), String> {
    let observability: State<'_, ObservabilityStore> = app.state();
    feature_flag_on(&observability)?;
    let registry: State<'_, ProviderRegistry> = app.state();
    let impl_ = lookup_provider(&registry, provider).await?;

    // Conversation history is resumable; provider callbacks usually are
    // not. Rebuilding a Claude/Codex session here creates a healthy process
    // with an empty pending-request map, then guarantees the confusing
    // "request not found or already resolved" failure. Terminalize the
    // orphan instead. OpenCode opts into resume because its permission
    // request lives in the external HTTP server rather than this process.
    if !impl_.has_session(&thread_id).await {
        if !impl_.pending_requests_survive_session_restart() {
            emit_stale_request_failure(&app, thread_id, request_id);
            return Ok(());
        }
        ensure_live_session(&app, provider, &thread_id).await?;
    }

    match impl_
        .respond_to_request(thread_id.clone(), request_id.clone(), decision)
        .await
    {
        Ok(()) => Ok(()),
        // A provider can lose the callback between the liveness check and
        // the response, or a recovered external session may no longer know
        // the request. Persist one terminal failure so remount/hydration
        // cannot make the same dead control actionable again.
        Err(ProviderError::RequestNotPending { .. })
        | Err(ProviderError::SessionNotFound { .. })
        | Err(ProviderError::SessionClosed { .. }) => {
            emit_stale_request_failure(&app, thread_id, request_id);
            Ok(())
        }
        Err(err) => Err(provider_err(err)),
    }
}

const STALE_REQUEST_MESSAGE: &str =
    "This request expired when the provider session restarted. Continue to have the agent repeat it.";

fn emit_stale_request_failure<R: Runtime>(
    app: &AppHandle<R>,
    thread_id: ThreadId,
    request_id: RequestId,
) {
    forward_event(
        app,
        ProviderRuntimeEvent::RequestResponseFailed {
            thread_id,
            request_id,
            reason: RequestResponseFailureReason::StaleProviderCallback,
            message: STALE_REQUEST_MESSAGE.to_string(),
        },
    );
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
pub async fn agent_chat_set_model<R: Runtime>(
    app: AppHandle<R>,
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
pub async fn agent_chat_set_permission_mode<R: Runtime>(
    app: AppHandle<R>,
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
pub async fn list_chat_provider_capabilities<R: Runtime>(
    app: AppHandle<R>,
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

/// List the provider-native slash commands available to a chat thread
/// anchored at `cwd`. Claude is the only provider that reports a
/// command vocabulary today (via the Agent SDK's `supportedCommands()`
/// probe — built-ins like `/compact` / `/init` / `/review` plus custom
/// `~/.claude/commands` and `<cwd>/.claude/commands` entries). Codex
/// and OpenCode expose no discovery surface, so they resolve to an
/// empty list — the composer then shows only Codemux's own built-ins,
/// matching how reference multi-provider clients behave.
///
/// Selecting one of these in the UI inserts the literal `/name ` text
/// into the draft; the text is forwarded verbatim to the provider,
/// which interprets the leading slash itself. Codemux never executes
/// provider commands locally.
#[tauri::command]
pub async fn list_chat_slash_commands(
    provider: ProviderKind,
    cwd: String,
    slash_cache: tauri::State<
        '_,
        std::sync::Arc<crate::agent_provider::claude::slash_commands::ClaudeSlashCommandCache>,
    >,
) -> Result<Vec<crate::agent_provider::claude::slash_commands::ProviderSlashCommand>, String> {
    match provider {
        ProviderKind::Claude => slash_cache.get_or_harvest(&cwd).await,
        // No discovery surface on these providers (yet) — empty list,
        // not an error, so the popup renders without a failure footer.
        ProviderKind::Codex | ProviderKind::OpenCode => Ok(Vec::new()),
    }
}

/// Gracefully terminate a session. Idempotent on the provider side.
#[tauri::command]
pub async fn agent_chat_stop_session<R: Runtime>(
    app: AppHandle<R>,
    provider: ProviderKind,
    thread_id: ThreadId,
) -> Result<(), String> {
    let observability: State<'_, ObservabilityStore> = app.state();
    feature_flag_on(&observability)?;
    let registry: State<'_, ProviderRegistry> = app.state();
    let impl_ = lookup_provider(&registry, provider).await?;
    match impl_.stop_session(thread_id).await {
        Ok(()) | Err(ProviderError::SessionNotFound { .. }) => Ok(()),
        Err(err) => Err(provider_err(err)),
    }
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
pub fn shutdown_agent_chat_threads<R: Runtime>(
    app: &AppHandle<R>,
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

/// One persisted row, tagged with its durable `agent_chat_messages.id`.
///
/// The id is the frontend's resume cursor: it records the highest id it
/// has applied and asks for everything after it on the next mount, so a
/// warm revisit never replays history it already holds.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AgentChatMessageRow {
    pub id: i64,
    pub payload: String,
}

/// Cursor read of a thread's transcript: every row with `id > after_id`
/// (all rows when `after_id` is null), ascending.
///
/// Unlike [`agent_chat_list_messages`] this read SHAPES payloads: a
/// `tool_result` whose serialized `content` exceeds
/// [`LAZY_TOOL_RESULT_THRESHOLD_BYTES`] ships a metadata stub with a
/// preview instead of the body (see [`shape_persisted_payload`]). Stored
/// rows are untouched; the frontend fetches a full body on demand via
/// [`agent_chat_get_tool_result`]. The older command keeps returning
/// verbatim payloads so clients pinned to that surface (mirror /
/// web-remote) are unaffected.
#[tauri::command]
pub async fn agent_chat_list_messages_after(
    db: State<'_, DatabaseStore>,
    thread_id: String,
    after_id: Option<i64>,
) -> Result<Vec<AgentChatMessageRow>, String> {
    Ok(db
        .list_agent_chat_messages_after(&thread_id, after_id)
        .into_iter()
        .map(|(id, payload)| AgentChatMessageRow {
            payload: shape_persisted_payload(id, &payload),
            id,
        })
        .collect())
}

/// Highest persisted row id for a thread (`null` when it has none).
///
/// The frontend probes this alongside a warm tail read to catch a
/// cursor that is AHEAD of the thread's own history — a cursor carried
/// over from a merged or deleted thread, whose rows were re-homed under
/// a different `thread_id`. Such a cursor would make the tail read look
/// permanently empty; seeing head < cursor, the pane falls back to a
/// cold hydrate.
#[tauri::command]
pub async fn agent_chat_thread_head_id(
    db: State<'_, DatabaseStore>,
    thread_id: String,
) -> Result<Option<i64>, String> {
    Ok(db.max_agent_chat_message_id(&thread_id))
}

/// Return one persisted row verbatim, by rowid — the full payload the
/// list read replaced with a lazy stub. The frontend pulls the
/// `item.content` out of it when the user expands or copies the tool
/// result.
#[tauri::command]
pub async fn agent_chat_get_tool_result(
    db: State<'_, DatabaseStore>,
    row_id: i64,
) -> Result<String, String> {
    db.get_agent_chat_message(row_id)
        .ok_or_else(|| format!("not_found: agent_chat_messages row {row_id}"))
}

// ── Lazy tool-result shaping (read path only) ─────────────────────────

/// Serialized `tool_result` content above this size is replaced by a
/// stub on the cursor read path. A 13 MB thread is mostly collapsed
/// tool output the user never expands; shipping it costs IPC, two JSON
/// parses and permanent renderer memory for nothing.
pub const LAZY_TOOL_RESULT_THRESHOLD_BYTES: usize = 32 * 1024;

/// How much of the stringified body travels with the stub. The collapsed
/// card renders 12 lines, so this is comfortably more than it can show.
pub const LAZY_TOOL_RESULT_PREVIEW_BYTES: usize = 2 * 1024;

/// Key the stub object hangs off, so a stub is unambiguously
/// distinguishable from a genuine tool-result body.
pub const LAZY_TOOL_RESULT_KEY: &str = "__codemux_lazy_tool_result";

/// Replace an oversized `tool_result` body with a metadata stub.
///
/// Returns the payload unchanged unless ALL of the following hold: the
/// envelope is an `item_completed` carrying a `tool_result`, its
/// `content` serializes to more than [`LAZY_TOOL_RESULT_THRESHOLD_BYTES`],
/// and the content carries no image block. Image-bearing results are
/// never stubbed so the thumbnail grid and the auto-expand-on-image
/// behaviour keep working off the real content.
fn shape_persisted_payload(row_id: i64, payload: &str) -> String {
    // Cheap gate: the stub only ever shrinks a payload that is already
    // bigger than the threshold, so smaller rows skip the parse entirely.
    if payload.len() <= LAZY_TOOL_RESULT_THRESHOLD_BYTES {
        return payload.to_string();
    }
    let Ok(mut value) = serde_json::from_str::<serde_json::Value>(payload) else {
        return payload.to_string();
    };
    if value.get("type").and_then(|v| v.as_str()) != Some("item_completed") {
        return payload.to_string();
    }
    let Some(item) = value.get_mut("item").and_then(|i| i.as_object_mut()) else {
        return payload.to_string();
    };
    if item.get("kind").and_then(|v| v.as_str()) != Some("tool_result") {
        return payload.to_string();
    }
    let Some(content) = item.get("content") else {
        return payload.to_string();
    };
    if content_has_image_block(content) {
        return payload.to_string();
    }
    let Ok(serialized) = serde_json::to_string(content) else {
        return payload.to_string();
    };
    if serialized.len() <= LAZY_TOOL_RESULT_THRESHOLD_BYTES {
        return payload.to_string();
    }
    let text = stringify_tool_content(content);
    let stub = serde_json::json!({
        LAZY_TOOL_RESULT_KEY: {
            "row_id": row_id,
            "bytes": serialized.len(),
            "preview": truncate_on_char_boundary(&text, LAZY_TOOL_RESULT_PREVIEW_BYTES),
            // `split('\n')` (not `lines()`) so the count matches the
            // frontend's `split("\n").length` on the same text.
            "line_count": text.split('\n').count(),
            "has_images": false,
        }
    });
    item.insert("content".to_string(), stub);
    serde_json::to_string(&value).unwrap_or_else(|_| payload.to_string())
}

/// Mirror of the frontend's `hasToolResultImages`
/// (`src/lib/agent-chat/tool-result-images.ts`): an entry counts only if
/// the renderer would actually draw it.
///
/// The accept-set has to match, not merely overlap. Too narrow and we stub
/// a body the thumbnail grid needs; too wide — the old "any `type` starting
/// with image" test — and a base64 PDF or a sourceless entry bypasses the
/// stub, which is exactly backwards, since those are the biggest payloads
/// and nothing renders them.
fn content_has_image_block(content: &serde_json::Value) -> bool {
    let Some(entries) = content.as_array() else {
        return false;
    };
    entries.iter().any(is_renderable_image_entry)
}

fn is_renderable_image_entry(entry: &serde_json::Value) -> bool {
    let Some(entry) = entry.as_object() else {
        return false;
    };
    match entry.get("type").and_then(|t| t.as_str()) {
        // `{ image_url: { url } }` or a bare string url.
        Some("image_url") => entry
            .get("image_url")
            .and_then(|iu| match iu.as_object() {
                Some(obj) => obj.get("url").and_then(|u| u.as_str()),
                None => iu.as_str(),
            })
            .is_some_and(is_safe_image_src),
        Some("image") => {
            let Some(source) = entry.get("source").and_then(|s| s.as_object()) else {
                return false;
            };
            match source.get("type").and_then(|t| t.as_str()) {
                Some("base64") => {
                    let has_data = source
                        .get("data")
                        .and_then(|d| d.as_str())
                        .is_some_and(|d| !d.is_empty());
                    // An absent media_type renders as image/png frontend-side.
                    let media_type = source
                        .get("media_type")
                        .and_then(|m| m.as_str())
                        .unwrap_or("image/png");
                    has_data && media_type.to_ascii_lowercase().starts_with("image/")
                }
                Some("url") => source
                    .get("url")
                    .and_then(|u| u.as_str())
                    .is_some_and(is_safe_image_src),
                _ => false,
            }
        }
        _ => false,
    }
}

/// Same accept-set as the frontend's `safeImageSrc`: only sources the
/// webview will load as an image.
fn is_safe_image_src(raw: &str) -> bool {
    let url = raw.trim().to_ascii_lowercase();
    url.starts_with("data:image/")
        || url.starts_with("http://")
        || url.starts_with("https://")
}

/// Flatten tool-result content to the text the collapsed card would
/// render. Mirrors `contentToString` in `ToolCallBlock.tsx` closely
/// enough for a preview: strings pass through, array entries contribute
/// their `text` field (or pretty JSON), objects contribute `text` or
/// pretty JSON.
fn stringify_tool_content(content: &serde_json::Value) -> String {
    match content {
        serde_json::Value::Null => String::new(),
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(entries) => entries
            .iter()
            .map(|entry| match entry {
                serde_json::Value::Null => String::new(),
                serde_json::Value::String(s) => s.clone(),
                other => match other.get("text").and_then(|t| t.as_str()) {
                    Some(text) => text.to_string(),
                    None => serde_json::to_string_pretty(other).unwrap_or_default(),
                },
            })
            .collect::<Vec<_>>()
            .join("\n"),
        other => match other.get("text").and_then(|t| t.as_str()) {
            Some(text) => text.to_string(),
            None => serde_json::to_string_pretty(other).unwrap_or_default(),
        },
    }
}

fn truncate_on_char_boundary(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }
    let mut end = max_bytes;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
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
    // Row id of whatever this event wrote to `agent_chat_messages`, if
    // anything. Persistence happens BEFORE fan-out (below), so the live
    // payload can carry the durable position of its own row — that is
    // what lets a resuming pane tell "already replayed from the tail"
    // apart from "new".
    let mut persisted_id: Option<i64> = None;
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
        } else if resume_cursor.is_null() {
            // A JSON-null cursor is the stale-session recovery signal (the
            // sidecar rebuilt a fresh query because the persisted id's
            // on-disk JSONL was gone). Clear the dead id so no later
            // rebuild hands it back as `resume`. Only EXACT null clears —
            // a malformed / unrecognized cursor keeps today's ignore
            // behavior. The rebuilt query's fresh `sdk-session-id` arrives
            // shortly and re-persists via the branch above.
            let db: State<'_, DatabaseStore> = app.state();
            if let Err(error) = db.clear_agent_chat_sdk_session_id(&thread_id.0) {
                eprintln!(
                    "[codemux::agent_chat] failed to clear stale sdk_session_id: {error}"
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
            let pending = pending_queued_images()
                .lock()
                .unwrap()
                .remove(queued_id)
                .unwrap_or_default();
            // The dispatch event itself is not persisted; the bubble it
            // promotes is. The row is deliberately NOT reported as this
            // event's `persisted_id`: a pane that never saw the enqueue
            // has no queued bubble to promote, so the reducer no-ops and
            // the row must still arrive through a later tail read. The
            // nonce carried on the envelope is what keeps that tail read
            // from duplicating a bubble that WAS promoted.
            persist_user_message(
                &db,
                &thread_id.0,
                text,
                &pending.images,
                pending.client_nonce.as_deref(),
            );
        }
    }
    // A queued turn was cancelled before it dispatched, so its envelope
    // will never be written. Drop the stashed image records and best-effort
    // delete the orphaned on-disk copies so a cancelled attachment does not
    // leak a file forever.
    if let ProviderRuntimeEvent::QueuedTurnCancelled { queued_id, .. } = &event {
        let orphaned = pending_queued_images().lock().unwrap().remove(queued_id);
        if let Some(pending) = orphaned {
            for image in pending.images {
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
                    match db.append_agent_chat_message(&thread_id.0, &payload) {
                        Ok(row_id) => persisted_id = row_id,
                        Err(error) => eprintln!(
                            "[codemux::agent_chat] failed to persist message: {error}"
                        ),
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
        persisted_id,
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
        ProviderRuntimeEvent::RequestResponseFailed { .. } => Some(PaneStatus::Review),
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
        ProviderRuntimeEvent::WorkflowUpdated { .. }
        | ProviderRuntimeEvent::TasksUpdated { .. } => None,
        // Context-usage snapshots are pure metadata riding alongside the
        // turn's real progress events — they must never move the dot.
        ProviderRuntimeEvent::ContextUsageUpdated { .. } => None,
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
        // Stamped like every other emitted snapshot: an unrevisioned payload
        // would be applied unconditionally by the renderer and could overwrite
        // a newer delta-applied domain with pre-delta values.
        let snapshot = state.stamped_snapshot();
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
        | ProviderRuntimeEvent::TasksUpdated { .. }
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
        | ProviderRuntimeEvent::RequestResponseFailed { .. }
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
            | ProviderRuntimeEvent::RequestResponseFailed { .. }
            // Subagent snapshots persist so the orchestration card and
            // child transcripts survive restart: DB hydrate replays them
            // through the same reducer with zero schema migration.
            | ProviderRuntimeEvent::SubagentUpdated { .. }
            // Workflow snapshots persist for the same reason — the
            // workflow run card and its phase attribution must survive a
            // restart via hydrate-replay, not a bespoke schema.
            | ProviderRuntimeEvent::WorkflowUpdated { .. }
            // Context-usage snapshots persist so the meter survives a
            // restart: hydrate replays the latest snapshot through the
            // reducer, no bespoke column needed. Adapters only emit on
            // meaningful change, so the volume tracks assistant
            // messages, not the token stream.
            | ProviderRuntimeEvent::ContextUsageUpdated { .. }
            // Task plans are durable conversation state. Persist the full
            // replacement event so hydrate uses the same reducer path.
            | ProviderRuntimeEvent::TasksUpdated { .. }
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

/// Whether `session_id` looks like a value we are willing to probe for on
/// disk. Purely a path-traversal guard for the scan in
/// [`claude_session_file_missing_in`]: the id is interpolated into a
/// filename, so reject anything empty or carrying path separators / parent
/// components / NULs. We deliberately do NOT enforce a strict uuid shape —
/// the goal is only to refuse dangerous inputs, never to reject an
/// otherwise-valid cursor over formatting.
fn is_plausible_session_id(session_id: &str) -> bool {
    !session_id.is_empty()
        && !session_id.contains('/')
        && !session_id.contains('\\')
        && !session_id.contains("..")
        && !session_id.contains('\0')
}

/// Resolve the Claude CLI config dir: `$CLAUDE_CONFIG_DIR` when set, else
/// `~/.claude`. `None` only when neither can be resolved (no HOME).
fn claude_config_dir() -> Option<std::path::PathBuf> {
    if let Ok(dir) = std::env::var("CLAUDE_CONFIG_DIR") {
        if !dir.is_empty() {
            return Some(std::path::PathBuf::from(dir));
        }
    }
    dirs::home_dir().map(|home| home.join(".claude"))
}

/// POSITIVE-CONFIRMATION check against a given `config_dir`: is the Claude
/// CLI's on-disk conversation JSONL (`<config>/projects/<slug>/<id>.jsonl`)
/// DEFINITIVELY absent?
///
/// The contract is asymmetric on purpose — we only ever return `true` when
/// we are certain the file is gone, so a stale resume cursor is dropped
/// *only* on confirmed absence and never on IO doubt:
///
/// - `session_id` not plausible (traversal guard) → `false`.
/// - `<config>/projects` missing or unreadable → `false` (uncertain).
/// - any subdir listing errors mid-scan → `false` (uncertain).
/// - `<id>.jsonl` found in any project subdir → `false` (present).
/// - every subdir listed cleanly and the file was nowhere → `true`.
///
/// Split from the env-reading wrapper so the core logic is unit-testable
/// with a temp dir and no env mutation.
fn claude_session_file_missing_in(config_dir: &std::path::Path, session_id: &str) -> bool {
    if !is_plausible_session_id(session_id) {
        return false;
    }
    let projects = config_dir.join("projects");
    let Ok(entries) = std::fs::read_dir(&projects) else {
        // projects/ absent or unreadable — uncertain, keep resume.
        return false;
    };
    let target = format!("{session_id}.jsonl");
    for entry in entries {
        let Ok(entry) = entry else {
            // Couldn't stat an entry — can't be sure the file is absent.
            return false;
        };
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        match std::fs::read_dir(&path) {
            Ok(files) => {
                for file in files {
                    let Ok(file) = file else {
                        return false;
                    };
                    if file.file_name() == std::ffi::OsStr::new(&target) {
                        return false;
                    }
                }
            }
            Err(_) => {
                // Couldn't read this project subdir — uncertain.
                return false;
            }
        }
    }
    true
}

/// Env-reading wrapper over [`claude_session_file_missing_in`]. Resolves
/// the CLI config dir from `$CLAUDE_CONFIG_DIR` / `~/.claude`; returns
/// `false` (keep the resume cursor) when the dir can't be resolved.
///
/// Used as a resume preflight: when a persisted Claude `sdk_session_id`'s
/// JSONL is confirmed gone (CLI cleanup / update), we skip the resume
/// cursor so the rebuilt session starts fresh instead of wedging on a
/// dead id.
pub fn claude_session_file_missing(session_id: &str) -> bool {
    match claude_config_dir() {
        Some(dir) => claude_session_file_missing_in(&dir, session_id),
        None => false,
    }
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
        | ProviderRuntimeEvent::RequestResponseFailed { thread_id, .. }
        | ProviderRuntimeEvent::SessionStateChanged { thread_id, .. }
        | ProviderRuntimeEvent::SubagentUpdated { thread_id, .. }
        | ProviderRuntimeEvent::WorkflowUpdated { thread_id, .. }
        | ProviderRuntimeEvent::TasksUpdated { thread_id, .. }
        | ProviderRuntimeEvent::ResumeCursorUpdated { thread_id, .. }
        | ProviderRuntimeEvent::TurnQueued { thread_id, .. }
        | ProviderRuntimeEvent::QueuedTurnDispatched { thread_id, .. }
        | ProviderRuntimeEvent::QueuedTurnCancelled { thread_id, .. }
        | ProviderRuntimeEvent::ContextUsageUpdated { thread_id, .. }
        | ProviderRuntimeEvent::RunStalled { thread_id, .. } => Some(thread_id.clone()),
        ProviderRuntimeEvent::RuntimeWarning { thread_id, .. } => thread_id.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ── lazy tool-result shaping (read path) ──

    fn tool_result_payload(content: serde_json::Value) -> String {
        serde_json::to_string(&json!({
            "type": "item_completed",
            "thread_id": "t",
            "turn_id": "turn-1",
            "item": {
                "kind": "tool_result",
                "tool_use_id": "tu-1",
                "content": content,
                "is_error": false,
            }
        }))
        .unwrap()
    }

    fn big_text(bytes: usize) -> String {
        // Multi-line so `line_count` is meaningful.
        "0123456789abcdefghij\n".repeat(bytes / 21 + 1)
    }

    fn stub_of(payload: &str) -> Option<serde_json::Value> {
        let value: serde_json::Value = serde_json::from_str(payload).unwrap();
        value
            .get("item")?
            .get("content")?
            .get(LAZY_TOOL_RESULT_KEY)
            .cloned()
    }

    #[test]
    fn shape_leaves_small_tool_results_untouched() {
        let payload = tool_result_payload(json!("just a little output"));
        assert_eq!(shape_persisted_payload(7, &payload), payload);
    }

    #[test]
    fn shape_stubs_oversized_tool_result_content() {
        let text = big_text(LAZY_TOOL_RESULT_THRESHOLD_BYTES + 5_000);
        let payload = tool_result_payload(json!(text));
        let shaped = shape_persisted_payload(42, &payload);
        assert!(shaped.len() < payload.len() / 4);

        let stub = stub_of(&shaped).expect("content replaced by a stub");
        assert_eq!(stub["row_id"], json!(42));
        assert_eq!(stub["has_images"], json!(false));
        assert!(stub["bytes"].as_u64().unwrap() > LAZY_TOOL_RESULT_THRESHOLD_BYTES as u64);
        assert_eq!(
            stub["line_count"].as_u64().unwrap() as usize,
            text.split('\n').count()
        );
        let preview = stub["preview"].as_str().unwrap();
        assert!(preview.len() <= LAZY_TOOL_RESULT_PREVIEW_BYTES);
        assert!(text.starts_with(preview));
        // Everything outside `content` survives verbatim.
        let shaped_value: serde_json::Value = serde_json::from_str(&shaped).unwrap();
        assert_eq!(shaped_value["item"]["tool_use_id"], json!("tu-1"));
        assert_eq!(shaped_value["turn_id"], json!("turn-1"));
    }

    #[test]
    fn shape_previews_block_arrays_as_flattened_text() {
        let text = big_text(LAZY_TOOL_RESULT_THRESHOLD_BYTES + 5_000);
        let payload = tool_result_payload(json!([{ "type": "text", "text": text }]));
        let stub = stub_of(&shape_persisted_payload(3, &payload)).expect("stubbed");
        assert!(text.starts_with(stub["preview"].as_str().unwrap()));
    }

    #[test]
    fn shape_never_stubs_image_bearing_results() {
        // Image blocks stay verbatim so the thumbnail grid and the
        // auto-expand-on-image behaviour keep reading real content.
        let data = "A".repeat(LAZY_TOOL_RESULT_THRESHOLD_BYTES + 5_000);
        let payload = tool_result_payload(json!([
            { "type": "text", "text": "screenshot:" },
            { "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": data } },
        ]));
        assert_eq!(shape_persisted_payload(9, &payload), payload);

        let url_payload = tool_result_payload(json!([
            { "type": "image_url", "image_url": { "url": format!("https://example.test/{data}") } },
        ]));
        assert_eq!(shape_persisted_payload(9, &url_payload), url_payload);

        // A bare-string `image_url` and a `source: { type: "url" }` are the
        // frontend's other two accepted shapes.
        let bare = tool_result_payload(json!([
            { "type": "image_url", "image_url": format!("https://example.test/{data}") },
        ]));
        assert_eq!(shape_persisted_payload(9, &bare), bare);
        let sourced = tool_result_payload(json!([
            { "type": "image", "source": { "type": "url", "url": format!("https://example.test/{data}") } },
        ]));
        assert_eq!(shape_persisted_payload(9, &sourced), sourced);
        // Media type absent → the frontend renders it as image/png.
        let defaulted = tool_result_payload(json!([
            { "type": "image", "source": { "type": "base64", "data": data } },
        ]));
        assert_eq!(shape_persisted_payload(9, &defaulted), defaulted);
    }

    #[test]
    fn shape_stubs_entries_the_frontend_would_never_render_as_images() {
        // The heaviest payloads used to ride through unstubbed on nothing
        // but a `type` prefix. None of these draw anything.
        let data = "A".repeat(LAZY_TOOL_RESULT_THRESHOLD_BYTES + 5_000);
        let cases = vec![
            // Base64 document, not an image.
            json!([{ "type": "image", "source": { "type": "base64", "media_type": "application/pdf", "data": data } }]),
            // No source at all.
            json!([{ "type": "image", "data": data }]),
            // Source present but of an unknown kind.
            json!([{ "type": "image", "source": { "type": "file", "path": data } }]),
            // Empty base64 data.
            json!([{ "type": "image", "source": { "type": "base64", "media_type": "image/png", "data": "" }, "note": data }]),
            // `image_url` carrying no url string.
            json!([{ "type": "image_url", "image_url": { "detail": data } }]),
            // A scheme the webview will not load as an image.
            json!([{ "type": "image_url", "image_url": { "url": format!("file:///{data}.png") } }]),
            // Prefix-matching junk type.
            json!([{ "type": "imagemagick", "text": data }]),
        ];
        for content in cases {
            let payload = tool_result_payload(content.clone());
            assert!(
                stub_of(&shape_persisted_payload(9, &payload)).is_some(),
                "expected a stub for {content}",
            );
        }
    }

    #[test]
    fn shape_ignores_non_tool_result_envelopes() {
        let text = big_text(LAZY_TOOL_RESULT_THRESHOLD_BYTES + 5_000);
        let assistant = serde_json::to_string(&json!({
            "type": "item_completed",
            "thread_id": "t",
            "turn_id": "turn-1",
            "item": { "kind": "assistant_message", "text": text },
        }))
        .unwrap();
        assert_eq!(shape_persisted_payload(1, &assistant), assistant);

        let user = serde_json::to_string(&json!({
            "type": "user_message", "thread_id": "t", "text": text,
        }))
        .unwrap();
        assert_eq!(shape_persisted_payload(1, &user), user);

        // Unparseable rows pass through rather than wedging the read.
        let garbage = format!("not json {}", "x".repeat(LAZY_TOOL_RESULT_THRESHOLD_BYTES));
        assert_eq!(shape_persisted_payload(1, &garbage), garbage);
    }

    #[test]
    fn shape_skips_payloads_whose_bulk_is_not_the_content() {
        // A large envelope whose `content` is small keeps its body: the
        // stub only pays off when the content itself is the weight.
        let payload = tool_result_payload(json!("small"));
        let padded = {
            let mut value: serde_json::Value = serde_json::from_str(&payload).unwrap();
            value["item"]["tool_name"] =
                json!("N".repeat(LAZY_TOOL_RESULT_THRESHOLD_BYTES + 100));
            serde_json::to_string(&value).unwrap()
        };
        assert_eq!(shape_persisted_payload(1, &padded), padded);
    }

    // ── start-path permission_mode heal ──

    #[test]
    fn resolve_start_permission_mode_heals_null_to_provider_default() {
        // A null-passing caller (UI shows "Full access") gets the provider
        // default substituted so the launch matches the display.
        assert_eq!(
            resolve_start_permission_mode(ProviderKind::Claude, None),
            Some("bypassPermissions".to_string())
        );
        assert_eq!(
            resolve_start_permission_mode(ProviderKind::Codex, None),
            Some("danger-full-access".to_string())
        );
        // OpenCode has no permission modes, so None stays None.
        assert_eq!(
            resolve_start_permission_mode(ProviderKind::OpenCode, None),
            None
        );
    }

    #[test]
    fn resolve_start_permission_mode_passes_through_explicit_choice() {
        // An explicit mode is authoritative and is never overwritten.
        assert_eq!(
            resolve_start_permission_mode(ProviderKind::Claude, Some("plan".to_string())),
            Some("plan".to_string())
        );
        assert_eq!(
            resolve_start_permission_mode(ProviderKind::Claude, Some("default".to_string())),
            Some("default".to_string())
        );
    }

    #[test]
    fn resolve_start_permission_mode_canonicalizes_cross_provider_full_access() {
        assert_eq!(
            resolve_start_permission_mode(
                ProviderKind::Codex,
                Some("bypassPermissions".to_string())
            ),
            Some("danger-full-access".to_string())
        );
        assert_eq!(
            resolve_start_permission_mode(
                ProviderKind::Claude,
                Some("danger-full-access".to_string())
            ),
            Some("bypassPermissions".to_string())
        );
    }

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

    // ── chat image staging: MIME + extension mapping ──

    #[test]
    fn validate_chat_image_mime_accepts_supported_types() {
        assert_eq!(
            validate_chat_image_mime("image/png").unwrap(),
            "image/png"
        );
        assert_eq!(
            validate_chat_image_mime("image/jpeg").unwrap(),
            "image/jpeg"
        );
        assert_eq!(
            validate_chat_image_mime("image/gif").unwrap(),
            "image/gif"
        );
        assert_eq!(
            validate_chat_image_mime("image/webp").unwrap(),
            "image/webp"
        );
    }

    #[test]
    fn validate_chat_image_mime_folds_jpg_alias_and_case() {
        // `image/jpg` normalizes to the canonical `image/jpeg`.
        assert_eq!(validate_chat_image_mime("image/jpg").unwrap(), "image/jpeg");
        // Casing is folded.
        assert_eq!(validate_chat_image_mime("IMAGE/PNG").unwrap(), "image/png");
        assert_eq!(validate_chat_image_mime("Image/WebP").unwrap(), "image/webp");
    }

    #[test]
    fn validate_chat_image_mime_rejects_unsupported() {
        assert!(validate_chat_image_mime("image/heic").is_err());
        assert!(validate_chat_image_mime("image/svg+xml").is_err());
        assert!(validate_chat_image_mime("application/pdf").is_err());
        assert!(validate_chat_image_mime("text/plain").is_err());
    }

    #[test]
    fn media_type_from_extension_round_trips_known_types() {
        use std::path::Path;
        assert_eq!(
            media_type_from_extension(Path::new("/x/a.png")),
            "image/png"
        );
        assert_eq!(
            media_type_from_extension(Path::new("/x/a.jpg")),
            "image/jpeg"
        );
        assert_eq!(
            media_type_from_extension(Path::new("/x/a.jpeg")),
            "image/jpeg"
        );
        assert_eq!(
            media_type_from_extension(Path::new("/x/a.gif")),
            "image/gif"
        );
        assert_eq!(
            media_type_from_extension(Path::new("/x/a.webp")),
            "image/webp"
        );
        // Case-insensitive on the extension.
        assert_eq!(
            media_type_from_extension(Path::new("/x/a.PNG")),
            "image/png"
        );
        // Unknown / missing extension falls back to the generic type.
        assert_eq!(
            media_type_from_extension(Path::new("/x/a.bin")),
            "application/octet-stream"
        );
        assert_eq!(
            media_type_from_extension(Path::new("/x/noext")),
            "application/octet-stream"
        );
    }

    // ── chat image staging: path containment (security boundary) ──

    #[test]
    fn resolve_within_accepts_a_path_inside_root() {
        let root = tempfile::tempdir().unwrap();
        let file = root.path().join("img.png");
        std::fs::write(&file, b"x").unwrap();
        let resolved =
            resolve_within(root.path(), file.to_str().unwrap()).unwrap();
        assert_eq!(resolved, file.canonicalize().unwrap());
    }

    #[test]
    fn resolve_within_accepts_a_path_in_a_subdir_of_root() {
        let root = tempfile::tempdir().unwrap();
        let sub = root.path().join("thread-1");
        std::fs::create_dir_all(&sub).unwrap();
        let file = sub.join("img.png");
        std::fs::write(&file, b"x").unwrap();
        let resolved =
            resolve_within(root.path(), file.to_str().unwrap()).unwrap();
        assert_eq!(resolved, file.canonicalize().unwrap());
    }

    #[test]
    fn resolve_within_rejects_parent_traversal() {
        let root = tempfile::tempdir().unwrap();
        let inside = root.path().join("staging");
        std::fs::create_dir_all(&inside).unwrap();
        // A `../` escape that lands outside the root is rejected even though
        // the components textually start under it.
        let escape = format!("{}/../../etc/passwd", inside.display());
        let err = resolve_within(&inside, &escape).unwrap_err();
        assert!(err.contains("security_error") || err.contains("validation_error"));
    }

    #[test]
    fn resolve_within_rejects_a_sibling_of_root() {
        let base = tempfile::tempdir().unwrap();
        let root = base.path().join("images");
        let sibling = base.path().join("other");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&sibling).unwrap();
        let outside = sibling.join("evil.png");
        std::fs::write(&outside, b"x").unwrap();
        let err = resolve_within(&root, outside.to_str().unwrap()).unwrap_err();
        assert!(err.contains("security_error"));
    }

    #[test]
    fn classify_chat_image_ref_tags_staged_and_thread_local() {
        let base = tempfile::tempdir().unwrap();
        let staging = base.path().join("staging");
        let thread_dir = base.path().join("thread-1");
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::create_dir_all(&thread_dir).unwrap();

        let staged = staging.join("s.png");
        std::fs::write(&staged, b"x").unwrap();
        let local = thread_dir.join("l.png");
        std::fs::write(&local, b"x").unwrap();

        match classify_chat_image_ref(&staging, &thread_dir, staged.to_str().unwrap())
            .unwrap()
        {
            ChatImageRefLocation::Staged(p) => {
                assert_eq!(p, staged.canonicalize().unwrap())
            }
            ChatImageRefLocation::ThreadLocal(_) => panic!("expected Staged"),
        }
        match classify_chat_image_ref(&staging, &thread_dir, local.to_str().unwrap())
            .unwrap()
        {
            ChatImageRefLocation::ThreadLocal(p) => {
                assert_eq!(p, local.canonicalize().unwrap())
            }
            ChatImageRefLocation::Staged(_) => panic!("expected ThreadLocal"),
        }
    }

    #[test]
    fn classify_chat_image_ref_rejects_other_thread_and_outside() {
        let base = tempfile::tempdir().unwrap();
        let staging = base.path().join("staging");
        let thread_dir = base.path().join("thread-1");
        let other_thread = base.path().join("thread-2");
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::create_dir_all(&thread_dir).unwrap();
        std::fs::create_dir_all(&other_thread).unwrap();

        // A file in a DIFFERENT thread's dir is not smuggleable into this
        // turn — even though it lives under the same chat-image root.
        let foreign = other_thread.join("f.png");
        std::fs::write(&foreign, b"x").unwrap();
        assert!(
            classify_chat_image_ref(&staging, &thread_dir, foreign.to_str().unwrap())
                .is_err()
        );

        // A file entirely outside the tree is rejected.
        let outside_dir = tempfile::tempdir().unwrap();
        let outside = outside_dir.path().join("o.png");
        std::fs::write(&outside, b"x").unwrap();
        assert!(
            classify_chat_image_ref(&staging, &thread_dir, outside.to_str().unwrap())
                .is_err()
        );
    }

    #[tokio::test]
    async fn move_staged_file_relocates_bytes() {
        let base = tempfile::tempdir().unwrap();
        let src = base.path().join("staged.png");
        let dst = base.path().join("thread-1").join("final.png");
        std::fs::create_dir_all(dst.parent().unwrap()).unwrap();
        std::fs::write(&src, b"pixels").unwrap();

        move_staged_file(&src, &dst).await.unwrap();
        assert!(!src.exists(), "source should be gone after finalize");
        assert_eq!(std::fs::read(&dst).unwrap(), b"pixels");
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

    // ── claude_session_file_missing preflight ──
    //
    // Positive-confirmation semantics: only return `true` when the JSONL
    // is DEFINITIVELY absent. Any IO doubt keeps the resume cursor.

    #[test]
    fn session_file_missing_true_when_projects_dir_has_no_match() {
        let cfg = tempfile::tempdir().unwrap();
        // A projects/ tree that exists and lists cleanly, but holds no
        // `<id>.jsonl` — the definitively-absent case.
        let proj = cfg.path().join("projects").join("-home-user-repo");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(proj.join("some-other-session.jsonl"), b"{}").unwrap();
        assert!(claude_session_file_missing_in(cfg.path(), "dead-uuid"));
    }

    #[test]
    fn session_file_present_returns_false() {
        let cfg = tempfile::tempdir().unwrap();
        let proj = cfg.path().join("projects").join("-home-user-repo");
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(proj.join("live-uuid.jsonl"), b"{}").unwrap();
        assert!(!claude_session_file_missing_in(cfg.path(), "live-uuid"));
    }

    #[test]
    fn session_file_present_in_second_project_dir_returns_false() {
        let cfg = tempfile::tempdir().unwrap();
        let a = cfg.path().join("projects").join("-a");
        let b = cfg.path().join("projects").join("-b");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        std::fs::write(b.join("live-uuid.jsonl"), b"{}").unwrap();
        assert!(!claude_session_file_missing_in(cfg.path(), "live-uuid"));
    }

    #[test]
    fn session_file_missing_false_when_projects_dir_absent() {
        // Uncertain (no projects/ to scan) ⇒ keep the resume cursor.
        let cfg = tempfile::tempdir().unwrap();
        assert!(!claude_session_file_missing_in(cfg.path(), "any-uuid"));
    }

    #[test]
    fn session_file_missing_rejects_implausible_ids() {
        let cfg = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(cfg.path().join("projects")).unwrap();
        // Traversal-guard: never probe on empty / separator / parent ids.
        assert!(!claude_session_file_missing_in(cfg.path(), ""));
        assert!(!claude_session_file_missing_in(cfg.path(), "../escape"));
        assert!(!claude_session_file_missing_in(cfg.path(), "a/b"));
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
            last_active_at: None,
            last_visited_at: None,
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
        CompletedItem, ContentDelta, SubagentSnapshot, SubagentStatus, TaskSnapshotItem,
        TaskStatus, TasksSnapshot, TurnStatus, TurnUsage,
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
    fn should_persist_tasks_snapshot_for_hydration() {
        let event = ProviderRuntimeEvent::TasksUpdated {
            thread_id: tid(),
            tasks: TasksSnapshot {
                explanation: Some("Ship the feature".into()),
                tasks: vec![TaskSnapshotItem {
                    task_id: "implement".into(),
                    title: "Implement the panel".into(),
                    status: TaskStatus::InProgress,
                    detail: None,
                    blocked_by: Vec::new(),
                }],
            },
        };

        assert!(should_persist_event(&event));
        assert_eq!(thread_id_for_event(&event), Some(tid()));
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
            persisted_id: None,
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
